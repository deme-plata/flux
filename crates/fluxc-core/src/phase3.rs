// fluxc-core/phase3.rs — Phase 3 CLI commands

pub fn api_generate(title: &str, format: &str) {
    println!("⚡ Flux API Builder — discovering endpoints...");
    let root = std::env::current_dir().unwrap_or_default();
    let ws = match flux_graph::resolve_workspace(&root) {
        Ok(w) => w, Err(e) => { eprintln!("flux-graph: {}", e); return; }
    };
    let eps = flux_api::discover_endpoints(&ws);
    println!("  {} endpoints across {} crates", eps.len(), ws.crates.len());
    if format == "openapi" || format == "all" {
        let spec = flux_api::generate_openapi(title, "1.0", &eps);
        println!("  OpenAPI 3.1: {} paths", spec["paths"].as_object().map(|p| p.len()).unwrap_or(0));
    }
    if format == "typescript" || format == "all" {
        println!("  TypeScript SDK: {} chars", flux_api::generate_typescript_sdk(&eps, "http://localhost:8080").len());
    }
    if format == "python" || format == "all" {
        println!("  Python SDK: {} chars", flux_api::generate_python_sdk(&eps, "http://localhost:8080").len());
    }
}

pub fn optimize_analyze(preset: &str) {
    println!("⚡ Flux Optimizer — analyzing workspace...");
    let root = std::env::current_dir().unwrap_or_default();
    let ws = match flux_graph::resolve_workspace(&root) {
        Ok(w) => w, Err(e) => { eprintln!("flux-graph: {}", e); return; }
    };
    let p = match preset {
        "POWER_SAVER" => flux_optimize::OptimizationPreset::PowerSaver,
        "MAX_PERF" => flux_optimize::OptimizationPreset::MaxPerf,
        _ => flux_optimize::OptimizationPreset::Balanced,
    };
    let r = flux_optimize::apply(&ws, p);
    println!("  SIMD: {} | io_uring: {} | Cache: {} | Est gain: {:.0}% | Watt: {:?}",
        r.simd_opportunities.len(), r.iouring_opportunities.len(),
        r.cache_line_fixes.len(), r.estimated_perf_gain_pct, r.estimated_watt_impact);
    let pw = flux_optimize::estimate_perf_watt(&ws);
    println!("  {:.2} MFLOPS/W | {:.0}K B/J | {:.3}g CO2/build",
        pw.mflops_per_watt, pw.bytes_per_joule / 1000.0, pw.estimated_carbon_kg_per_build * 1000.0);
}

pub fn ai_audit() {
    println!("⚡ Flux AI Audit — 6 AI-mode rules...");
    let root = std::env::current_dir().unwrap_or_default();
    let ws = match flux_graph::resolve_workspace(&root) {
        Ok(w) => w, Err(e) => { eprintln!("flux-graph: {}", e); return; }
    };
    let r = flux_ai::full_ai_audit(&ws);
    println!("  Score: {:.0}% | Lifetime hints: {} | Send/Sync: {} | Races: {}",
        r.overall_score * 100.0, r.lifetime_suggestions.len(), r.send_sync_suggestions.len(),
        r.race_detection_findings.len());
    println!("  Unsafe: {}/{} verified | Ownership wrappers: {} | Deadlocks: {}",
        r.unsafe_verification.iter().map(|u| u.verified_count).sum::<usize>(),
        r.unsafe_verification.iter().map(|u| u.unsafe_block_count).sum::<usize>(),
        r.ownership_wrappers.len(), r.deadlock_findings.len());
}

pub fn agility_audit() {
    println!("⚡ Flux Agility Audit — crypto analysis...");
    let root = std::env::current_dir().unwrap_or_default();
    let ws = match flux_graph::resolve_workspace(&root) {
        Ok(w) => w, Err(e) => { eprintln!("flux-graph: {}", e); return; }
    };
    let a = flux_graph::agility::audit_agility(&ws);
    println!("  Score: {:.0}% | PQ crates: {} | Classical: {} | Migrations: {}",
        a.agility_score * 100.0, a.pq_crates, a.classical_crates, a.migration_needed.len());
    for m in &a.migration_needed {
        println!("    {}: {:?} → {}", m.crate_name, m.current, m.recommended);
    }
}

pub fn compile_file(path: &str) {
    compile_impl(path, false);
}

/// Workspace-aware Phase 2: resolve a crate via flux-graph, find its entry source,
/// drive the MIR → flux-frontend IR → flux-backend pipeline against it. Reports a
/// per-function coverage summary so progress is measurable across iterations.
pub fn compile_package(name: &str) {
    println!("⚡ Flux Phase 2 — workspace MIR (crate '{}')", name);
    let root = std::env::current_dir().unwrap_or_default();
    let ws = match flux_graph::resolve_workspace(&root) {
        Ok(w) => w,
        Err(e) => { eprintln!("  flux-graph error: {}", e); return; }
    };
    let ci = match ws.crates.iter().find(|c| c.name == name) {
        Some(c) => c,
        None => {
            eprintln!("  crate '{}' not found in workspace", name);
            eprintln!("  available: {}", ws.crates.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", "));
            return;
        }
    };
    let entry = match ci.crate_type {
        flux_graph::CrateType::Lib => ci.path.join("src/lib.rs"),
        flux_graph::CrateType::Bin => ci.path.join("src/main.rs"),
        flux_graph::CrateType::ProcMacro => {
            eprintln!("  proc-macro crates can't compile to native via MIR pipeline");
            return;
        }
    };
    if !entry.exists() {
        eprintln!("  entry file not found: {}", entry.display());
        eprintln!("  (TODO: parse [[bin]]/[[lib]].path from Cargo.toml)");
        return;
    }
    println!("  Entry: {}", entry.display());
    println!("  Deps:  {} ({} path, {} crates.io)",
        ci.dependencies.len(),
        ci.dependencies.iter().filter(|d| matches!(d.kind, flux_graph::DepKind::Path)).count(),
        ci.dependencies.iter().filter(|d| matches!(d.kind, flux_graph::DepKind::CratesIo)).count());

    // Phase 2c: resolve each dep to an rlib in target/debug/deps and thread as --extern.
    // Reuses cargo's previously-built artifacts; no need to rebuild deps from scratch.
    let deps_dir = ws.root.join("target/debug/deps");
    let mut extern_args: Vec<String> = Vec::new();
    let mut resolved: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for dep in &ci.dependencies {
        let rlib_prefix = format!("lib{}-", dep.name.replace('-', "_"));
        let newest = std::fs::read_dir(&deps_dir).ok().into_iter().flatten().flatten()
            .filter(|e| {
                let fname = e.file_name();
                let s = fname.to_string_lossy();
                s.starts_with(&rlib_prefix) && s.ends_with(".rlib")
            })
            .filter_map(|e| e.metadata().ok().and_then(|m| m.modified().ok()).map(|t| (t, e.path())))
            .max_by_key(|(t, _)| *t)
            .map(|(_, p)| p);
        match newest {
            Some(p) => {
                extern_args.push(format!("--extern={}={}", dep.name.replace('-', "_"), p.display()));
                resolved.push((dep.name.clone(), p));
            }
            None => missing.push(dep.name.clone()),
        }
    }
    println!("  Externs:");
    for (n, p) in &resolved { println!("    ✓ {} → {}", n, p.file_name().unwrap_or_default().to_string_lossy()); }
    for n in &missing { println!("    ✗ {} (no rlib in {})", n, deps_dir.display()); }
    if !missing.is_empty() {
        println!("    hint: run `fluxc self` or `cargo build --workspace` first to populate deps");
    }

    // Drive rustc --emit=mir on the entry file with resolved externs threaded in.
    let tmp_mir = std::env::temp_dir().join(format!("flux_pkg_{}.mir", name));
    let mut cmd = std::process::Command::new("rustc");
    cmd.args(["--crate-type", "lib", "--edition", &ci.edition, "--emit=mir", "-o"])
        .arg(&tmp_mir)
        .args(["-L", &format!("dependency={}", deps_dir.display())]);
    for ea in &extern_args { cmd.arg(ea); }
    cmd.arg(&entry);
    let mir_status = cmd.output();
    let mir_text = match mir_status {
        Ok(out) if out.status.success() => std::fs::read_to_string(&tmp_mir).unwrap_or_default(),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            eprintln!("  rustc --emit=mir failed (next gap: --extern threading for deps).");
            eprintln!("  first error: {}", stderr.lines().find(|l| l.contains("error")).unwrap_or("(no error line)"));
            return;
        }
        Err(e) => { eprintln!("  could not spawn rustc: {}", e); return; }
    };

    let funcs = match flux_frontend::mir::parse_mir(&mir_text) {
        Ok(f) => f,
        Err(e) => { eprintln!("  MIR parse error: {}", e); return; }
    };
    println!("  Parsed: {} MIR functions", funcs.len());

    let ir_funcs: Vec<flux_frontend::FunctionDef> = funcs.iter()
        .map(flux_frontend::mir::lower_mir_to_ir)
        .collect();
    let unit = flux_frontend::TranslationUnit {
        file_path: entry.to_string_lossy().to_string(),
        functions: ir_funcs,
        structs: vec![],
        imports: vec![],
    };
    let clif_results = flux_backend::compile_unit(&unit);
    let clif_ok = clif_results.as_ref().map(|v| v.len()).unwrap_or(0);

    println!();
    println!("  Coverage:");
    println!("    MIR parse  : {} / {} functions", funcs.len(), funcs.len());
    println!("    IR lower   : {} / {} functions", unit.functions.len(), funcs.len());
    println!("    CLIF gen   : {} / {} functions", clif_ok, funcs.len());

    if funcs.is_empty() {
        println!("    Object     : skipped (no functions to emit)");
        return;
    }
    // Per-function path selection: cyclic functions (those with loops) take the
    // MIR-direct codegen path; the rest use the Expr-based lowering.
    let mir_overrides = build_mir_overrides(&funcs);
    if !mir_overrides.is_empty() {
        println!("    MIR-direct : {}  (loops/match)",
            mir_overrides.keys().cloned().collect::<Vec<_>>().join(", "));
    }
    let obj_path = ci.path.join("target").join(format!("{}.flux.o", name));
    let _ = std::fs::create_dir_all(obj_path.parent().unwrap());
    match flux_backend::compile_unit_to_object_with_mir(&unit, &mir_overrides, &obj_path) {
        Ok(()) => println!("    Object     : ✓ {}", obj_path.display()),
        Err(e) => println!("    Object     : ✗ {}", e),
    }
}

/// Build a map of function-name → MirFunction for every function with a control-flow
/// cycle (back-edge). These go through the MIR-direct backend path (Cranelift Variables).
fn build_mir_overrides(funcs: &[flux_frontend::mir::MirFunction])
    -> std::collections::HashMap<String, flux_frontend::mir::MirFunction>
{
    funcs.iter()
        .filter(|f| has_cycles(f) || has_complex_switch(f) || has_tuples(f))
        .map(|f| (f.name.clone(), f.clone()))
        .collect()
}

/// True if any local or param has a tuple type. Tuple support lives only in the
/// MIR-direct path (each field becomes its own Cranelift Variable).
fn has_tuples(mir: &flux_frontend::mir::MirFunction) -> bool {
    mir.locals.iter().any(|l| l.ty.trim().starts_with('('))
        || mir.params.iter().any(|p| p.ty.trim().starts_with('('))
}

/// True if any block ends with a SwitchInt that has more than one explicit target
/// (i.e. a real `match` rather than a boolean dispatch). The Expr-based path's
/// pattern detector only handles 2-way switches; everything else routes to MIR-direct.
fn has_complex_switch(mir: &flux_frontend::mir::MirFunction) -> bool {
    mir.blocks.iter().any(|b| matches!(
        &b.terminator,
        Some(flux_frontend::mir::MirTerminator::SwitchInt { targets, .. }) if targets.len() > 1
    ))
}

/// Emit a `<artifact>.proof` file next to the compiled object using the unsigned
/// scaffold mode of `crate::provenance`. SQIsign signing TBD (needs key management);
/// for now the bundle binds the artifact hash + source hash + agent identity.
fn emit_provenance_proof(artifact_path: &std::path::Path, source_path: &std::path::Path) {
    let artifact_bytes = match std::fs::read(artifact_path) {
        Ok(b) => b,
        Err(e) => { eprintln!("  Provenance: cannot read artifact: {}", e); return; }
    };
    let source_bytes = std::fs::read(source_path).unwrap_or_default();
    let agent_wallet = read_agent_wallet();
    let swarm_task_id = read_active_swarm_task();
    let ctx = crate::provenance::ProvenanceContext {
        artifact_bytes,
        source_bytes,
        agent_wallet,
        swarm_task_id,
        settle_tx: None,
        fluxc_git: [0u8; 20],
        fluxc_version: 0x0a01_0001,
    };
    let keys = crate::provenance::load_agent_keys();
    let result = match &keys {
        Some((sk, pk)) => crate::provenance::emit_signed(&ctx, sk, pk),
        None => crate::provenance::emit(&ctx),
    };
    match result {
        Ok(proof) => {
            let proof_path = {
                let mut p = artifact_path.as_os_str().to_owned();
                p.push(".proof");
                std::path::PathBuf::from(p)
            };
            match crate::provenance::to_json_bytes(&proof) {
                Ok(bytes) => match std::fs::write(&proof_path, bytes) {
                    Ok(()) => {
                        let mode = if proof.sqisign_sig.is_empty() { "unsigned scaffold" } else { "SQIsign L5 signed" };
                        println!("  Provenance: ✓ {} ({})", proof_path.display(), mode);
                    }
                    Err(e) => eprintln!("  Provenance write error: {}", e),
                },
                Err(e) => eprintln!("  Provenance serialize error: {}", e),
            }
        }
        Err(e) => eprintln!("  Provenance emit error: {}", e),
    }
}

fn read_agent_wallet() -> [u8; 32] {
    if let Ok(hex) = std::env::var("FLUX_AGENT_WALLET") {
        let trimmed = hex.trim_start_matches("qnk").trim_start_matches("0x");
        let mut out = [0u8; 32];
        for (i, chunk) in trimmed.as_bytes().chunks(2).take(32).enumerate() {
            if let Ok(s) = std::str::from_utf8(chunk) {
                if let Ok(b) = u8::from_str_radix(s, 16) { out[i] = b; }
            }
        }
        return out;
    }
    [0u8; 32]
}

fn read_active_swarm_task() -> [u8; 16] {
    let raw = std::fs::read_to_string("/tmp/flux-swarm.json").unwrap_or_default();
    if let Ok(d) = serde_json::from_str::<serde_json::Value>(&raw) {
        if let Some(claims) = d.get("claims").and_then(|v| v.as_array()) {
            for c in claims {
                if let (Some(agent), Some(task)) =
                    (c.get("agent").and_then(|v| v.as_str()),
                     c.get("task_id").and_then(|v| v.as_str()))
                {
                    if std::env::var("FLUX_AGENT_NAME").as_deref().unwrap_or("rocky") == agent {
                        let mut out = [0u8; 16];
                        let bytes = task.as_bytes();
                        for (i, &b) in bytes.iter().take(16).enumerate() { out[i] = b; }
                        return out;
                    }
                }
            }
        }
    }
    [0u8; 16]
}

/// DFS the MIR CFG looking for a back-edge to a block currently on the recursion stack.
/// Returns true iff there's a cycle (loop).
fn has_cycles(mir: &flux_frontend::mir::MirFunction) -> bool {
    use flux_frontend::mir::MirTerminator;
    use std::collections::HashMap;
    fn successors<'a>(t: &'a Option<MirTerminator>) -> Vec<&'a str> {
        match t {
            Some(MirTerminator::Goto(s)) => vec![s.as_str()],
            Some(MirTerminator::Assert { target, .. }) => vec![target.as_str()],
            Some(MirTerminator::Call { target, .. }) => vec![target.as_str()],
            Some(MirTerminator::SwitchInt { targets, otherwise, .. }) => {
                let mut v: Vec<&str> = targets.iter().map(|(_, t)| t.as_str()).collect();
                v.push(otherwise.as_str());
                v
            }
            _ => vec![],
        }
    }
    fn dfs<'a>(
        label: &'a str,
        mir: &'a flux_frontend::mir::MirFunction,
        color: &mut HashMap<&'a str, u8>,
    ) -> bool {
        match color.get(label) {
            Some(2) => return false,
            Some(1) => return true,
            _ => {}
        }
        color.insert(label, 1);
        if let Some(b) = mir.blocks.iter().find(|b| b.label == label) {
            for succ in successors(&b.terminator) {
                if let Some(succ_label) = mir.blocks.iter()
                    .find(|b| b.label == succ).map(|b| b.label.as_str()) {
                    if dfs(succ_label, mir, color) { return true; }
                }
            }
        }
        color.insert(label, 2);
        false
    }
    let mut color: HashMap<&str, u8> = HashMap::new();
    mir.blocks.first().map_or(false, |b| dfs(&b.label, mir, &mut color))
}

pub fn compile_impl(path: &str, use_mir: bool) {
    compile_impl_with_provenance(path, use_mir, false);
}

pub fn compile_impl_with_provenance(path: &str, use_mir: bool, provenance: bool) {
    if use_mir {
        println!("⚡ Flux Compiler — MIR Bridge (rustc MIR → CLIF)");
        // Run rustc --emit=mir, parse MIR, then compile each function
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => { eprintln!("  Cannot read {}: {}", path, e); return; }
        };
        // Write to temp file with proper crate structure for MIR emission
        let tmp = std::env::temp_dir().join("flux_mir_compile.rs");
        let _ = std::fs::write(&tmp, &source);
        
        let output = std::process::Command::new("rustc")
            .args(["--crate-type", "lib", "--emit=mir", "-o", "-"])
            .arg(&tmp)
            .output();
        
        match output {
            Ok(out) => {
                let tmp_mir = std::env::temp_dir().join("flux_mir_output.mir");
        let _ = std::process::Command::new("rustc").args(["--crate-type","lib","--emit=mir","-o"]).arg(&tmp_mir).arg(&tmp).status();
        let mir_text = std::fs::read_to_string(&tmp_mir).unwrap_or_default();
                if mir_text.contains("error") && !mir_text.contains("fn ") {
                    eprintln!("  rustc MIR error:\n{}", mir_text);
                    return;
                }
                match flux_frontend::mir::parse_mir(&mir_text) {
                    Ok(funcs) => {
                        println!("  MIR parsed: {} functions", funcs.len());
                        for f in &funcs {
                            println!("  fn {}({} params) → {}", f.name, f.params.len(), f.return_type);
                        }
                        // Convert MIR functions to flux-frontend IR for backend.
                        // Uses the unified file-scope lowering (previously trapped in mod tests).
                        let ir_funcs: Vec<flux_frontend::FunctionDef> = funcs.iter()
                            .map(flux_frontend::mir::lower_mir_to_ir)
                            .collect();
                        
                        let unit = flux_frontend::TranslationUnit {
                            file_path: path.to_string(),
                            functions: ir_funcs,
                            structs: vec![],
                            imports: vec![],
                        };
                        
                        match flux_backend::compile_unit(&unit) {
                            Ok(clifs) => {
                                println!("  Compiled {} functions to CLIF:", clifs.len());
                                for (i, clif) in clifs.iter().enumerate() {
                                    if i < funcs.len() {
                                        println!("  --- {} ---\n{}", funcs[i].name, clif);
                                    }
                                }
                            }
                            Err(e) => eprintln!("  Codegen error: {}", e),
                        }
                        let obj_path = std::path::Path::new(path).with_extension("o");
                        let mir_overrides = build_mir_overrides(&funcs);
                        if !mir_overrides.is_empty() {
                            println!("  MIR-direct path (loops/match): {}",
                                mir_overrides.keys().cloned().collect::<Vec<_>>().join(", "));
                        }
                        match flux_backend::compile_unit_to_object_with_mir(&unit, &mir_overrides, &obj_path) {
                            Ok(()) => {
                                println!("  Object emitted: {}", obj_path.display());
                                if provenance {
                                    emit_provenance_proof(&obj_path, path.as_ref());
                                }
                            }
                            Err(e) => eprintln!("  Object emit error: {}", e),
                        }
                    }
                    Err(e) => eprintln!("  MIR parse error: {}", e),
                }
            }
            Err(e) => eprintln!("  rustc failed: {}", e),
        }
    } else {
        println!("⚡ Flux Compiler — Phase 3d (syn parser)");
        let p = std::path::Path::new(path);
        let unit = match flux_frontend::parse_file(p) {
            Ok(u) => u,
            Err(e) => { eprintln!("  Parse error: {}", e); return; }
        };
        println!("  Parsed: {} functions, {} structs", unit.functions.len(), unit.structs.len());
        for f in &unit.functions {
            println!("  fn {}({} params) → {:?}", f.name, f.params.len(), f.return_type);
        }
        match flux_backend::compile_unit(&unit) {
            Ok(clifs) => {
                println!("  Compiled {} functions to Cranelift IR:", clifs.len());
                for (i, clif) in clifs.iter().enumerate() {
                    if i < unit.functions.len() {
                        println!("  --- {} ---\n{}", unit.functions[i].name, clif);
                    }
                }
            }
            Err(e) => eprintln!("  Codegen error: {}", e),
        }
    }
}

// (legacy local lower_mir_to_ir + mir_type_to_ir removed — now using
//  flux_frontend::mir::lower_mir_to_ir which dispatches across 10 BinOp variants
//  and reuses the already-lowered _0 value in the Return path.)

pub fn architect_plan() { let root=std::env::current_dir().unwrap_or_default(); let ws=match flux_graph::resolve_workspace(&root) { Ok(w)=>w, Err(e)=>{eprintln!("flux-graph: {}",e);return;} }; println!("Reading {} crates...",ws.crates.len()); let plan=flux_architect::analyze_workspace(&ws); println!(" {} crates, {} files, {} LOC",plan.crates_analyzed,plan.files_analyzed,plan.loc_analyzed); println!(" {} findings across {} dimensions",plan.findings.len(),plan.dimensions_covered.len()); for f in &plan.findings { println!("
  #{} [{:?}] {:.0}% gain | {}:{} | conf: {:.0}%",f.rank,f.dimension,f.estimated_impact_pct,f.crate_name,f.file,f.confidence*100.0); println!("  {}",f.summary); println!("  → {}",f.suggestion); } println!("
  Total estimated gain: {:.0}%",plan.total_estimated_gain_pct); }
