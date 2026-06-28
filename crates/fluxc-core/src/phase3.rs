// fluxc-core/phase3.rs — Phase 3 CLI commands
// ── Code Search (replaces find + grep) ──

/// Search the workspace with content-aware BLAKE3 deduplication.
/// Replaces `find . -name '*.rs' | xargs grep pattern`.
/// Uses flux-rev's search_live for deduplicated, glob-filtered full-text search.
pub fn search_code(pattern: &str, glob: Option<&str>, json: bool) {
    use flux_rev::search::search_live;
    let root = std::env::current_dir().unwrap_or_default();

    println!("⚡ Flux Search — \"{}\"", pattern);
    if let Some(g) = glob {
        println!("  Glob: {}", g);
    }

    let start = std::time::Instant::now();
    match search_live(&root, pattern, glob) {
        Ok(matches) => {
            let elapsed = start.elapsed().as_secs_f64();
            if json {
                let output = serde_json::json!({
                    "pattern": pattern,
                    "glob": glob,
                    "matches": matches.iter().map(|m| serde_json::json!({
                        "path": m.path,
                        "line": m.line_number,
                        "content": m.line,
                        "hash": m.hash,
                    })).collect::<Vec<_>>(),
                    "count": matches.len(),
                    "elapsed_secs": elapsed,
                });
                println!("{}", serde_json::to_string_pretty(&output).unwrap_or_default());
            } else {
                let mut by_file: std::collections::BTreeMap<String, Vec<&flux_rev::search::LiveMatch>> = std::collections::BTreeMap::new();
                for m in &matches {
                    by_file.entry(m.path.clone()).or_default().push(m);
                }
                for (path, ms) in &by_file {
                    // Color the path cyan (ANSI)
                    println!("\x1b[36m{}\x1b[0m", path);
                    for m in ms {
                        println!("  {:>4}: {}", m.line_number, m.line);
                    }
                }
                println!("\n  {} match(es) in {} file(s) · {:.3}s",
                    matches.len(), by_file.len(), elapsed);
            }
        }
        Err(e) => eprintln!("  Search error: {}", e),
    }
}

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

// ── JIT Execution ──
/// Compile a standalone .rs source file → native executable → run it.
/// Returns the process exit code. This is the `fluxc run test.rs` path.
pub fn compile_run(path: &str, args: &[String]) -> i32 {
    use std::process::Command;
    println!("⚡ Flux JIT — compile + run {}", path);

    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => { eprintln!("  Cannot read {}: {}", path, e); return 1; }
    };

    let hash = blake3::hash(src.as_bytes());
    let cache_key = hash.to_hex().to_string();

    let tmp_dir = std::env::temp_dir().join("flux_jit");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let obj_path = tmp_dir.join(format!("{}.o", &cache_key[..16]));
    let exe_path = tmp_dir.join(format!("{}.exe", &cache_key[..16]));

    // Check object cache first (BLAKE3 of source)
    // Check object cache on disk (BLAKE3 of source)
    let mir_cache_path = tmp_dir.join(format!("mir_{}.bin", &cache_key[..16]));
    let cached = mir_cache_path.exists();
    if !cached {
        // Compile: rustc --emit=mir → parse → lower → object
        let tmp_src = tmp_dir.join("_src.rs");
        let _ = std::fs::write(&tmp_src, &src);

        let tmp_mir = tmp_dir.join("_mir.txt");
        let _ = Command::new("rustc")
            .args(["--crate-type", "lib", "--emit=mir", "-o"])
            .arg(&tmp_mir)
            .arg(&tmp_src)
            .status();

        let mir_text = match std::fs::read_to_string(&tmp_mir) {
            Ok(t) => t,
            Err(e) => { eprintln!("  rustc MIR failed: {}", e); return 1; }
        };

        if mir_text.contains("error") && !mir_text.contains("fn ") {
            eprintln!("  rustc error in source:\n{}", mir_text);
            return 1;
        }

        let funcs = match flux_frontend::mir::parse_mir(&mir_text) {
            Ok(f) => f,
            Err(e) => { eprintln!("  MIR parse error: {}", e); return 1; }
        };

        println!("  MIR: {} functions", funcs.len());
        let ir_funcs: Vec<flux_frontend::FunctionDef> = funcs.iter()
            .map(flux_frontend::mir::lower_mir_to_ir)
            .collect();

        let unit = flux_frontend::TranslationUnit {
            file_path: path.to_string(),
            functions: ir_funcs,
            // Pull struct definitions from the source (the MIR-only unit carries none) so the
            // backend can resolve named-struct field layouts for scalar replacement.
            structs: flux_frontend::parse_source(&src, path).map(|u| u.structs).unwrap_or_default(),
            enums: flux_frontend::parse_source(&src, path).map(|u| u.enums).unwrap_or_default(),
            imports: vec![],
        };

        let mir_overrides = build_mir_overrides(&funcs);
        match flux_backend::compile_unit_to_object_with_mir(&unit, &mir_overrides, &obj_path) {
            Ok(()) => println!("  Object: ✓ {}", obj_path.display()),
            Err(e) => { eprintln!("  Object emit: {}", e); return 1; }
        }

        // Touch cache file to mark this source as compiled
        let _ = std::fs::write(&mir_cache_path, &cache_key);
    } else {
        println!("  Cache: ✓ hit (skipping rustc)");
    }

    // Link: cc obj_path → exe_path
    println!("  Link: cc → {}", exe_path.display());
    let link = Command::new("cc")
        .arg(&obj_path)
        .args(["-o", &exe_path.to_string_lossy()])
        .status();
    match link {
        Ok(s) if s.success() => {}
        Ok(s) => { eprintln!("  Link failed: exit {}", s.code().unwrap_or(1)); return 1; }
        Err(e) => { eprintln!("  cc not found: {} (install gcc/clang)", e); return 1; }
    }

    // Run
    println!("  Run: {}", exe_path.display());
    let status = Command::new(&exe_path).args(args).status();
    match status {
        Ok(s) => {
            let code = s.code().unwrap_or(0);
            println!("  Exit: {}", code);
            code
        }
        Err(e) => { eprintln!("  Run failed: {}", e); 1 }
    }
}

// ── MIR Cache ──
/// Try to load cached MIR parse result for source content.
/// Returns Some(MirFunction vec) on cache hit, None on miss.
pub fn mir_cache_lookup(source: &str) -> Option<Vec<flux_frontend::mir::MirFunction>> {
    let hash = blake3::hash(source.as_bytes());
    let key = hash.to_hex().to_string();
    let cache_dir = std::path::PathBuf::from("target/flux-cache");
    let _ = std::fs::create_dir_all(&cache_dir);
    let path = cache_dir.join(format!("mir_{}.bin", &key[..32]));
    if path.exists() {
        match std::fs::read(&path) {
            Ok(bytes) => {
                match bincode::deserialize::<Vec<flux_frontend::mir::MirFunction>>(&bytes) {
                    Ok(funcs) => return Some(funcs),
                    Err(_) => { /* stale format */ }
                }
            }
            Err(_) => {}
        }
    }
    None
}

/// Store MIR parse result in cache.
pub fn mir_cache_store(source: &str, funcs: &[flux_frontend::mir::MirFunction]) {
    let hash = blake3::hash(source.as_bytes());
    let key = hash.to_hex().to_string();
    let cache_dir = std::path::PathBuf::from("target/flux-cache");
    let _ = std::fs::create_dir_all(&cache_dir);
    let path = cache_dir.join(format!("mir_{}.bin", &key[..32]));
    if let Ok(encoded) = bincode::serialize(funcs) {
        let _ = std::fs::write(&path, &encoded);
    }
}

// ── Integration Tests ──
/// Run the comprehensive native-compile test suite.
/// Each test: compile .rs → object → link → run → verify expected output.
/// Returns number of failures (0 = all passed).
pub fn run_integration_tests() -> usize {
    use std::process::Command;
    println!("⚡ Flux Native Integration Tests");
    println!("═══════════════════════════════");

    let tests: Vec<(&str, &str, &str)> = vec![
        ("add",   "fn main() -> i64 { let a:i64=40; let b:i64=2; a+b }",         "42"),
        ("sub",   "fn main() -> i64 { let a:i64=50; let b:i64=8; a-b }",         "42"),
        ("mul",   "fn main() -> i64 { let a:i64=6; let b:i64=7; a*b }",          "42"),
        ("div",   "fn main() -> i64 { let a:i64=84; let b:i64=2; a/b }",         "42"),
        ("eq_t",  "fn main() -> bool { 42 == 42 }",                               "true"),
        ("eq_f",  "fn main() -> bool { 42 == 0 }",                                "false"),
        ("neq_t", "fn main() -> bool { 42 != 0 }",                                "true"),
        ("neq_f", "fn main() -> bool { 42 != 42 }",                               "false"),
        ("lt_t",  "fn main() -> bool { 1 < 42 }",                                 "true"),
        ("lt_f",  "fn main() -> bool { 42 < 1 }",                                 "false"),
        ("gt_t",  "fn main() -> bool { 42 > 1 }",                                 "true"),
        ("gt_f",  "fn main() -> bool { 1 > 42 }",                                 "false"),
        ("le_t",  "fn main() -> bool { 42 <= 42 }",                               "true"),
        ("le_f",  "fn main() -> bool { 43 <= 42 }",                               "false"),
        ("ge_t",  "fn main() -> bool { 42 >= 42 }",                               "true"),
        ("ge_f",  "fn main() -> bool { 41 >= 42 }",                               "false"),
        ("and_t", "fn main() -> bool { true && true }",                           "true"),
        ("and_f", "fn main() -> bool { true && false }",                          "false"),
        ("or_t",  "fn main() -> bool { true || false }",                          "true"),
        ("or_f",  "fn main() -> bool { false || false }",                         "false"),
        ("if_then",   "fn main() -> i64 { if 42 > 0 { 1 } else { 0 } }",          "1"),
        ("if_else",   "fn main() -> i64 { if 0 > 42 { 1 } else { 2 } }",          "2"),
        ("while_sum", "fn main() -> i64 { let mut i:i64=0; let mut s:i64=0; while i<10 { s=s+i; i=i+1; } s }", "45"),
        ("match_3way","fn main() -> i64 { let x:i64=2; match x { 1=>10, 2=>20, _=>30 } }", "20"),
        ("fib", "fn main() -> i64 { fib(10) } fn fib(n:i64) -> i64 { if n<=1 { n } else { fib(n-1)+fib(n-2) } }", "55"),
        ("call", "fn main() -> i64 { add(20,22) } fn add(a:i64,b:i64)->i64 { a+b }", "42"),
        ("tup",  "fn main() -> i64 { let t:(i64,i64)=(40,2); t.0+t.1 }",      "42"),
    ];

    let mut passed = 0usize;
    let mut failed = 0usize;
    let tmp = std::env::temp_dir().join("flux_native_tests");
    let _ = std::fs::create_dir_all(&tmp);

    for (name, src, expected) in &tests {
        print!("  {:20} ... ", name);
        let src_path = tmp.join(format!("test_{}.rs", name));
        let obj_path = tmp.join(format!("test_{}.o", name));
        let exe_path = tmp.join(format!("test_{}.exe", name));

        let full_src = format!("{}\n", src);
        let _ = std::fs::write(&src_path, &full_src);

        let rustc_out = Command::new("rustc")
            .args(["--crate-type", "lib", "--emit=mir", "-o"])
            .arg(tmp.join(format!("test_{}.mir", name)))
            .arg(&src_path)
            .output();

        match rustc_out {
            Ok(out) if out.status.success() => {
                let mir_text = std::fs::read_to_string(tmp.join(format!("test_{}.mir", name)))
                    .unwrap_or_default();
                if mir_text.is_empty() || (mir_text.contains("error") && !mir_text.contains("fn ")) {
                    eprintln!("RUSTC_ERR");
                    failed += 1;
                    continue;
                }
                match flux_frontend::mir::parse_mir(&mir_text) {
                    Ok(funcs) => {
                        let ir_funcs: Vec<flux_frontend::FunctionDef> = funcs.iter()
                            .map(flux_frontend::mir::lower_mir_to_ir)
                            .collect();
                        let unit = flux_frontend::TranslationUnit {
                            file_path: src_path.to_string_lossy().to_string(),
                            functions: ir_funcs, structs: vec![], enums: vec![], imports: vec![],
                        };
                        let overrides = build_mir_overrides(&funcs);
                        match flux_backend::compile_unit_to_object_with_mir(&unit, &overrides, &obj_path) {
                            Ok(()) => {
                                match Command::new("cc").arg(&obj_path).args(["-o", &exe_path.to_string_lossy()]).status() {
                                    Ok(s) if s.success() => {
                                        match Command::new(&exe_path).output() {
                                            Ok(out) => {
                                                let got = String::from_utf8_lossy(&out.stdout).trim().to_string();
                                                if got == *expected {
                                                    println!("OK ({})", got);
                                                    passed += 1;
                                                } else {
                                                    eprintln!("MISMATCH expected={} got={}", expected, got);
                                                    failed += 1;
                                                }
                                            }
                                            Err(e) => { eprintln!("RUN_ERR({})", e); failed += 1; }
                                        }
                                    }
                                    Ok(s) => { eprintln!("LINK({})", s.code().unwrap_or(1)); failed += 1; }
                                    Err(e) => { eprintln!("CC({})", e); failed += 1; }
                                }
                            }
                            Err(e) => { eprintln!("OBJ({})", e); failed += 1; }
                        }
                    }
                    Err(e) => { eprintln!("PARSE({})", e); failed += 1; }
                }
            }
            Ok(_) => { eprintln!("RUSTC_FAIL"); failed += 1; }
            Err(e) => { eprintln!("RUSTC({})", e); failed += 1; }
        }
    }

    println!("═══════════════════════════════");
    println!("  Passed: {}  Failed: {}  Total: {}", passed, failed, passed + failed);
    failed
}

// ── AI Lifetime Suggestion ──
/// Submit source code to the two-mind gate for lifetime analysis.
/// Proposer: qwen3.6 (local Ollama). Vetoer: DeepSeek-V4 API.
/// Returns the suggestion text, or None if vetoed.
pub fn suggest_lifetime(path: &str) -> Option<String> {
    use std::process::Command;
    println!("⚡ Flux AI Lifetime Suggestion — {}", path);

    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => { eprintln!("  Cannot read {}: {}", path, e); return None; }
    };

    let prompt = format!(
        "Analyze this Rust code for lifetime issues. \
        Identify any missing lifetime annotations, dangling references, \
        or borrow-checker problems. Return a concise JSON with:\n\
        - \"issues\": array of {{ \"line\": N, \"problem\": \"...\", \"fix\": \"...\" }}\n\
        - \"summary\": one-line summary\n\n\
        Code:\n```rust\n{}\n```",
        source
    );

    // Phase 1: Proposer (local qwen3.6 via Ollama)
    println!("  Proposer: qwen3.6 (local) ...");
    let proposal = match Command::new("ollama")
        .args(["run", "qwen3.6:latest"])
        .arg(&prompt)
        .output()
    {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            if text.is_empty() {
                eprintln!("  qwen3.6 returned empty — skipping veto gate");
                return None;
            }
            println!("  Proposer: {} chars", text.len());
            text
        }
        Err(e) => {
            eprintln!("  qwen3.6 unavailable (ollama not running?): {}", e);
            // Fallback: try DeepSeek directly
            println!("  Fallback: DeepSeek-V4 API ...");
            send_to_deepseek(&prompt)
        }
    };

    // Phase 2: Vetoer (DeepSeek-V4 API)
    if !proposal.is_empty() && std::env::var("DEEPSEEK_API_KEY").is_ok() {
        println!("  Vetoer: DeepSeek-V4 ...");
        let veto_prompt = format!(
            "You are a Rust compiler safety auditor. Review this lifetime analysis proposal. \
            If it is correct and safe, reply 'APPROVE'. If any suggestion is wrong or \
            dangerous, reply 'VETO: <reason>'. Be strict.\n\n\
            Proposal:\n{}", proposal
        );
        match send_to_deepseek(&veto_prompt) {
            verdict if verdict.to_lowercase().contains("approve") => {
                println!("  Verdict: APPROVED ✓");
                return Some(proposal);
            }
            verdict => {
                println!("  Verdict: VETOED — {}", verdict);
                return None;
            }
        }
    }

    // No vetoer configured — trust the proposer
    println!("  No veto gate configured — accepting proposal");
    Some(proposal)
}

/// Send a prompt to the DeepSeek-V4 API via curl. Returns the response content.
fn send_to_deepseek(prompt: &str) -> String {
    use std::process::Command;
    let api_key = std::env::var("DEEPSEEK_API_KEY").unwrap_or_default();
    if api_key.is_empty() { return String::new(); }
    let body = serde_json::json!({
        "model": "deepseek-v4-flash",
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": 1024,
        "temperature": 0.3
    });
    let body_str = serde_json::to_string(&body).unwrap_or_default();
    // SEC-004: the Authorization header goes in via a stdin-fed curl config
    // (`-K -`), NEVER as an argv `-H` — argv is world-readable in /proc.
    let spawned = Command::new("curl")
        .args(["-s", "-X", "POST", "https://api.deepseek.com/chat/completions"])
        .args(["-K", "-"])
        .args(["-H", "Content-Type: application/json"])
        .args(["-d", &body_str])
        .args(["--max-time", "60"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    let out = spawned.and_then(|mut child| {
        use std::io::Write;
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(format!("header = \"Authorization: Bearer {}\"\n", api_key).as_bytes());
        }
        drop(child.stdin.take());
        child.wait_with_output()
    });
    match out {
        Ok(out) => {
            let resp: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
            resp["choices"][0]["message"]["content"]
                .as_str().unwrap_or("").to_string()
        }
        Err(e) => {
            eprintln!("  DeepSeek API error: {}", e);
            String::new()
        }
    }
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
        enums: vec![],
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
        .filter(|f| has_cycles(f) || has_complex_switch(f) || has_tuples(f) || has_structs(f))
        .map(|f| (f.name.clone(), f.clone()))
        .collect()
}

/// True if any local or param has a tuple type. Tuple support lives only in the
/// MIR-direct path (each field becomes its own Cranelift Variable).
fn has_tuples(mir: &flux_frontend::mir::MirFunction) -> bool {
    mir.locals.iter().any(|l| l.ty.trim().starts_with('('))
        || mir.params.iter().any(|p| p.ty.trim().starts_with('('))
}

/// True if any local/param is a named struct (a capitalized type name that isn't a tuple,
/// ref, or primitive). Struct support — like tuples — lives only in the MIR-direct path.
fn has_structs(mir: &flux_frontend::mir::MirFunction) -> bool {
    let is_struct = |t: &str| {
        let t = t.trim();
        !t.is_empty() && !t.starts_with('(') && !t.starts_with('&') && !t.starts_with('*')
            && !matches!(t, "i8"|"i16"|"i32"|"i64"|"u8"|"u16"|"u32"|"u64"|"usize"|"isize"|"bool"|"f32"|"f64"|"()"|"!")
            && t.chars().next().map_or(false, |c| c.is_ascii_uppercase())
    };
    mir.locals.iter().chain(mir.params.iter()).any(|l| is_struct(&l.ty))
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
    // Prefer the require-both SQIsign+Ed25519 hybrid; fall back to single-leg
    // SQIsign if the agent has no Ed25519 leg yet; fall back to unsigned scaffold
    // if no agent key is configured at all.
    let result = if let Some((ssk, spk, esk, epk)) = crate::provenance::load_agent_keys_hybrid() {
        crate::provenance::emit_signed_hybrid(&ctx, &ssk, &spk, &esk, &epk)
    } else if let Some((sk, pk)) = crate::provenance::load_agent_keys() {
        crate::provenance::emit_signed(&ctx, &sk, &pk)
    } else {
        crate::provenance::emit(&ctx)
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
                        let mode = if !proof.ed25519_sig.is_empty() {
                            "SQIsign-L5 + Ed25519 hybrid signed"
                        } else if !proof.sqisign_sig.is_empty() {
                            "SQIsign L5 signed"
                        } else {
                            "unsigned scaffold"
                        };
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
                        
                        // Carry struct + enum defs from the syn parse: the backend needs struct layouts
                        // (aggregate scalar-replacement) and enum variant tables + data-carrying layouts
                        // (ladder rung 4). Without these, a data-carrying enum's constructor functions
                        // aren't recognised/skipped → duplicate-name panic, and its match payload reads 0.
                        let (src_structs, src_enums) = flux_frontend::parse_source(&source, path)
                            .map(|u| (u.structs, u.enums))
                            .unwrap_or_default();
                        let unit = flux_frontend::TranslationUnit {
                            file_path: path.to_string(),
                            functions: ir_funcs,
                            structs: src_structs,
                            enums: src_enums,
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

// ── Self-Healing Compiler Loop ──
/// Agentic closed loop: compile → test → fail → AI diagnose → fix → recompile → retest.
/// Uses all Flux subsystems: MIR cache, JIT, two-mind AI gate, webhooks, provenance.
///
/// On success: provenance-signs the fix, fires webhook, updates cache.
/// On exhaustion: fires webhook, escalates to swarm bus.
pub fn heal(path: &str, max_attempts: usize, auto_commit: bool) -> i32 {
    use std::process::Command;
    println!("⚡ Flux Self-Healing Compiler");
    println!("  Target: {}", path);
    println!("  Max attempts: {}", max_attempts);
    println!("  Auto-commit: {}", auto_commit);

    let original = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => { eprintln!("  Cannot read {}: {}", path, e); return 1; }
    };

    // Save backup
    let backup_path = format!("{}.heal.bak", path);
    let _ = std::fs::write(&backup_path, &original);

    let mut current_source = original.clone();
    let mut attempt = 0usize;

    loop {
        attempt += 1;
        println!("\n── Attempt {} / {} ──", attempt, max_attempts);

        // Step 1: Try to compile + run via JIT
        let tmp_dir = std::env::temp_dir().join("flux_heal");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let tmp_src = tmp_dir.join(format!("heal_{}.rs", attempt));
        let _ = std::fs::write(&tmp_src, &current_source);

        // Compile via MIR bridge
        let tmp_mir = tmp_dir.join(format!("heal_{}.mir", attempt));
        let rustc = Command::new("rustc")
            .args(["--crate-type", "lib", "--emit=mir", "-o"])
            .arg(&tmp_mir)
            .arg(&tmp_src)
            .output();

        let compile_ok = match &rustc {
            Ok(out) if out.status.success() => {
                let mir_text = std::fs::read_to_string(&tmp_mir).unwrap_or_default();
                if mir_text.contains("error") && !mir_text.contains("fn ") {
                    // Compile error — capture it
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let error_msg = if !stderr.is_empty() { stderr.to_string() } else { mir_text.clone() };
                    println!("  ✗ Compile error (attempt {})", attempt);
                    println!("  ── Error ──\n{}", error_msg.lines().take(15).collect::<Vec<_>>().join("\n"));

                    if attempt >= max_attempts {
                        println!("\n  ⚠ Max attempts exhausted. Escalating to swarm...");
                        fire_heal_webhook("heal_exhausted", path, attempt, &error_msg);
                        return 1;
                    }

                    // AI diagnose + fix
                    match ai_diagnose_and_fix(path, &current_source, &error_msg, "compile") {
                        Some(fixed) => {
                            current_source = fixed;
                            let _ = std::fs::write(path, &current_source);
                            println!("  ✓ Fix applied, retrying...");
                            continue;
                        }
                        None => {
                            println!("  ✗ AI could not produce a fix");
                            fire_heal_webhook("heal_exhausted", path, attempt, &error_msg);
                            return 1;
                        }
                    }
                } else {
                    // No compile error — proceed to object emission
                    true
                }
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let error_msg = if !stderr.is_empty() { stderr.to_string() } else { format!("Exit: {}", out.status) };
                println!("  ✗ rustc failed (attempt {})", attempt);
                println!("  ── Error ──\n{}", error_msg.lines().take(15).collect::<Vec<_>>().join("\n"));

                if attempt >= max_attempts {
                    fire_heal_webhook("heal_exhausted", path, attempt, &error_msg);
                    return 1;
                }
                match ai_diagnose_and_fix(path, &current_source, &error_msg, "compile") {
                    Some(fixed) => { current_source = fixed; let _ = std::fs::write(path, &current_source); continue; }
                    None => { fire_heal_webhook("heal_exhausted", path, attempt, &error_msg); return 1; }
                }
            }
            Err(e) => {
                eprintln!("  ✗ rustc not found: {}", e);
                return 1;
            }
        };

        if !compile_ok {
            continue;
        }

        // Step 2: Try to build the object
        let mir_text = std::fs::read_to_string(&tmp_mir).unwrap_or_default();
        let funcs = match flux_frontend::mir::parse_mir(&mir_text) {
            Ok(f) => f,
            Err(e) => {
                let error_msg = format!("MIR parse: {}", e);
                if attempt >= max_attempts {
                    fire_heal_webhook("heal_exhausted", path, attempt, &error_msg);
                    return 1;
                }
                match ai_diagnose_and_fix(path, &current_source, &error_msg, "parse") {
                    Some(fixed) => { current_source = fixed; let _ = std::fs::write(path, &current_source); continue; }
                    None => { fire_heal_webhook("heal_exhausted", path, attempt, &error_msg); return 1; }
                }
            }
        };

        let ir_funcs: Vec<flux_frontend::FunctionDef> = funcs.iter()
            .map(flux_frontend::mir::lower_mir_to_ir)
            .collect();

        let unit = flux_frontend::TranslationUnit {
            file_path: path.to_string(),
            functions: ir_funcs,
            structs: vec![],
            enums: vec![],
            imports: vec![],
        };

        let obj_path = tmp_dir.join(format!("heal_{}.o", attempt));
        let mir_overrides = build_mir_overrides(&funcs);

        match flux_backend::compile_unit_to_object_with_mir(&unit, &mir_overrides, &obj_path) {
            Ok(()) => {}
            Err(e) => {
                let error_msg = format!("Object emit: {}", e);
                if attempt >= max_attempts {
                    fire_heal_webhook("heal_exhausted", path, attempt, &error_msg);
                    return 1;
                }
                match ai_diagnose_and_fix(path, &current_source, &error_msg, "codegen") {
                    Some(fixed) => { current_source = fixed; let _ = std::fs::write(path, &current_source); continue; }
                    None => { fire_heal_webhook("heal_exhausted", path, attempt, &error_msg); return 1; }
                }
            }
        }

        // Step 3: Link and run
        let exe_path = tmp_dir.join(format!("heal_{}.exe", attempt));
        let link = Command::new("cc")
            .arg(&obj_path)
            .args(["-o", &exe_path.to_string_lossy()])
            .status();

        match link {
            Ok(s) if s.success() => {}
            Ok(s) => {
                let error_msg = format!("Link failed: exit {}", s.code().unwrap_or(1));
                if attempt >= max_attempts { fire_heal_webhook("heal_exhausted", path, attempt, &error_msg); return 1; }
                match ai_diagnose_and_fix(path, &current_source, &error_msg, "link") {
                    Some(fixed) => { current_source = fixed; let _ = std::fs::write(path, &current_source); continue; }
                    None => { fire_heal_webhook("heal_exhausted", path, attempt, &error_msg); return 1; }
                }
            }
            Err(e) => {
                eprintln!("  ✗ cc not found: {}", e);
                return 1;
            }
        }

        // Run the executable
        let run = Command::new(&exe_path).output();
        match run {
            Ok(out) if out.status.success() => {
                // ✓ SUCCESS — heal complete
                println!("\n  ✓ Heal successful on attempt {}!", attempt);
                println!("  ── Output ──\n{}", String::from_utf8_lossy(&out.stdout).trim());

                // Provenance-sign the fix
                if auto_commit {
                    println!("  Provenance: signing fix...");
                    let _ = std::process::Command::new("fluxc")
                        .args(["compile-native", "--provenance", path])
                        .status();
                }

                // Fire success webhook
                fire_heal_webhook("heal_success", path, attempt,
                    &format!("Fixed in {} attempt(s). Output: {}",
                        attempt, String::from_utf8_lossy(&out.stdout).trim()));

                // Update MIR cache
                let hash = blake3::hash(current_source.as_bytes());
                let key = hash.to_hex().to_string();
                let cache_dir = std::path::PathBuf::from("target/flux-cache");
                let _ = std::fs::create_dir_all(&cache_dir);
                let _ = std::fs::write(
                    cache_dir.join(format!("mir_{}.bin", &key[..32])),
                    &key,
                );

                println!("  ✓ Heal complete. Backup saved at {}", backup_path);
                return 0;
            }
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                let error_msg = format!("Runtime error (exit {}):\nstdout: {}\nstderr: {}",
                    out.status.code().unwrap_or(-1), stdout.trim(), stderr.trim());
                println!("  ✗ Runtime error (attempt {})", attempt);
                println!("  ── Error ──\n{}", error_msg.lines().take(10).collect::<Vec<_>>().join("\n"));

                if attempt >= max_attempts {
                    fire_heal_webhook("heal_exhausted", path, attempt, &error_msg);
                    return 1;
                }
                match ai_diagnose_and_fix(path, &current_source, &error_msg, "runtime") {
                    Some(fixed) => { current_source = fixed; let _ = std::fs::write(path, &current_source); continue; }
                    None => { fire_heal_webhook("heal_exhausted", path, attempt, &error_msg); return 1; }
                }
            }
            Err(e) => {
                let error_msg = format!("Execution error: {}", e);
                if attempt >= max_attempts { fire_heal_webhook("heal_exhausted", path, attempt, &error_msg); return 1; }
                match ai_diagnose_and_fix(path, &current_source, &error_msg, "exec") {
                    Some(fixed) => { current_source = fixed; let _ = std::fs::write(path, &current_source); continue; }
                    None => { fire_heal_webhook("heal_exhausted", path, attempt, &error_msg); return 1; }
                }
            }
        }
    }
}

/// AI diagnose-and-fix: sends source + error to two-mind gate, returns fixed source.
fn ai_diagnose_and_fix(
    path: &str,
    source: &str,
    error: &str,
    phase: &str,
) -> Option<String> {
    println!("  🧠 AI diagnosing ({})...", phase);

    let prompt = format!(
        "You are a Rust compiler expert. A program failed during the '{}' phase.\n\n\
         === SOURCE CODE ({}) ===\n```rust\n{}\n```\n\n\
         === ERROR ===\n{}\n\n\
         Analyze the error and produce a CORRECTED version of the FULL source code.\n\
         Return ONLY the corrected Rust code inside a ```rust code block.\n\
         Do NOT explain — just output the fixed code.\n\
         If you cannot fix it, output: CANNOT_FIX\n",
        phase, path, source, error
    );

    let proposal = match std::process::Command::new("ollama")
        .args(["run", "qwen3.6:latest"])
        .arg(&prompt)
        .output()
    {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            if text.is_empty() || text.contains("CANNOT_FIX") {
                // Fallback to DeepSeek
                send_to_deepseek(&prompt)
            } else {
                text
            }
        }
        Err(_) => send_to_deepseek(&prompt),
    };

    if proposal.is_empty() || proposal.contains("CANNOT_FIX") {
        return None;
    }

    // Extract code from markdown block
    let fixed = if let Some(start) = proposal.find("```rust") {
        let rest = &proposal[start + 7..];
        if let Some(end) = rest.find("```") {
            rest[..end].trim().to_string()
        } else {
            proposal.trim().to_string()
        }
    } else if let Some(start) = proposal.find("```") {
        let rest = &proposal[start + 3..];
        if let Some(end) = rest.find("```") {
            rest[..end].trim().to_string()
        } else {
            proposal.trim().to_string()
        }
    } else {
        proposal.trim().to_string()
    };

    if fixed.is_empty() || fixed == source {
        None
    } else {
        // Two-mind veto gate
        if std::env::var("DEEPSEEK_API_KEY").is_ok() {
            let veto_prompt = format!(
                "You are a Rust safety auditor. Review this AI-generated fix. \
                If it is correct and safe, reply 'APPROVE'. If it introduces bugs \
                or is dangerous, reply 'VETO: <reason>'. Be strict.\n\n\
                Original error: {}\n\n\
                Proposed fix:\n```rust\n{}\n```",
                error, fixed
            );
            let verdict = send_to_deepseek(&veto_prompt);
            if !verdict.to_lowercase().contains("approve") {
                println!("  Vetoed: {}", verdict.lines().next().unwrap_or("unknown"));
                return None;
            }
            println!("  Veto gate: APPROVED ✓");
        }

        Some(fixed)
    }
}

/// Fire a webhook for heal events.
fn fire_heal_webhook(event: &str, path: &str, attempt: usize, error: &str) {
    let data = serde_json::json!({
        "event": event,
        "path": path,
        "attempt": attempt,
        "error": error.lines().take(5).collect::<Vec<_>>().join("\n"),
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    });
    crate::webhook::auto_dispatch(event, data);
    crate::serve_events::push_feed_event(
        "Heal",
        &format!("{} (attempt {}) → {}", path, attempt, event),
        "heal",
    );
}

// (legacy local lower_mir_to_ir + mir_type_to_ir removed — now using
//  flux_frontend::mir::lower_mir_to_ir which dispatches across 10 BinOp variants
//  and reuses the already-lowered _0 value in the Return path.)

pub fn architect_plan() { let root=std::env::current_dir().unwrap_or_default(); let ws=match flux_graph::resolve_workspace(&root) { Ok(w)=>w, Err(e)=>{eprintln!("flux-graph: {}",e);return;} }; println!("Reading {} crates...",ws.crates.len()); let plan=flux_architect::analyze_workspace(&ws); println!(" {} crates, {} files, {} LOC",plan.crates_analyzed,plan.files_analyzed,plan.loc_analyzed); println!(" {} findings across {} dimensions",plan.findings.len(),plan.dimensions_covered.len()); for f in &plan.findings { println!("
  #{} [{:?}] {:.0}% gain | {}:{} | conf: {:.0}%",f.rank,f.dimension,f.estimated_impact_pct,f.crate_name,f.file,f.confidence*100.0); println!("  {}",f.summary); println!("  → {}",f.suggestion); } println!("
  Total estimated gain: {:.0}%",plan.total_estimated_gain_pct); }
