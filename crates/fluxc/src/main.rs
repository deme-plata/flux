// fluxc — CLI entry point for the Flux build orchestrator

use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();

    // rustc-impersonation passthrough — cargo's host-triple probe calls
    // `$RUSTC -vV` (and sometimes `-V` / `--version`) directly, OUTSIDE the
    // wrapper convention. If fluxc is in the rustc slot (e.g. cargo running
    // under `fluxc self` with RUSTC_WRAPPER=fluxc, or a workspace config that
    // misroutes RUSTC), we must look like rustc here. Forward to real rustc.
    //
    // Guard: only intercept when there's NO .rs source file in the args — a
    // real wrapper invocation always has one. Without this fluxc -vV would
    // shadow `fluxc version` and other future direct-CLI subcommands.
    if matches!(args.get(1).map(|s| s.as_str()), Some("-vV") | Some("-V") | Some("--version"))
        && !args.iter().any(|a| a.ends_with(".rs"))
    {
        let real_rustc = env::var("REAL_RUSTC").unwrap_or_else(|_| "rustc".to_string());
        let status = std::process::Command::new(&real_rustc)
            .args(&args[1..])
            .status()
            .expect("real rustc on PATH for -vV passthrough");
        std::process::exit(status.code().unwrap_or(1));
    }

    let is_wrapper = env::var("FLUXC_WRAPPING").map(|v| v == "1").unwrap_or(false);

    if is_wrapper {
        fluxc_core::wrapper_mode(&args);
        return;
    }

    let (config, subcommand_args) = fluxc_core::parse_args(&args[1..]);
    let subcommand = subcommand_args.first().map(|s| s.as_str());

    match subcommand {
        Some("build") | Some("b") => fluxc_core::detect_and_build(&config, &subcommand_args[1..]),
        Some("check") | Some("c") => fluxc_core::run_cargo("check", &config, &subcommand_args[1..]),
        Some("test") | Some("t") => fluxc_core::run_tests(subcommand_args.get(1).map(|s|s.as_str())),
        Some("quick") => fluxc_core::quick_build_run(subcommand_args.get(1).map(|s|s.as_str()).unwrap_or("fluxc"), config.release),
        Some("run") | Some("r") => {
            // JIT path: if first arg is a .rs file, compile + run natively
            if let Some(file) = subcommand_args.get(1) {
                if file.ends_with(".rs") {
                    let exit = fluxc_core::phase3::compile_run(file, &subcommand_args[2..]);
                    std::process::exit(exit);
                }
            }
            fluxc_core::detect_and_run(&config, &subcommand_args[1..])
        }
        Some("test-native") => {
            let failed = fluxc_core::phase3::run_integration_tests();
            if failed > 0 {
                std::process::exit(1);
            }
        }
        Some("suggest") => {
            let path = subcommand_args.get(1).map(|s| s.as_str()).unwrap_or("main.rs");
            match fluxc_core::phase3::suggest_lifetime(path) {
                Some(suggestion) => println!("{}", suggestion),
                None => eprintln!("  No suggestion produced (vetoed or unavailable)"),
            }
        }
        Some("heal") => {
            let path = subcommand_args.get(1).map(|s| s.as_str()).unwrap_or("main.rs");
            let mut max_attempts = 5usize;
            let mut auto_commit = false;
            let mut i = 2;
            while i < subcommand_args.len() {
                match subcommand_args[i].as_str() {
                    "--max-attempts" | "-n" => {
                        max_attempts = subcommand_args.get(i + 1)
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(5);
                        i += 2;
                    }
                    "--auto-commit" | "-y" => { auto_commit = true; i += 1; }
                    _ => { i += 1; }
                }
            }
            let exit = fluxc_core::phase3::heal(path, max_attempts, auto_commit);
            std::process::exit(exit);
        }
        Some("webhook-gen") => {
            let input = subcommand_args.get(1)
                .map(|s| std::path::PathBuf::from(s))
                .unwrap_or_else(|| std::path::PathBuf::from(".flux-webhook.toml"));
            let output = subcommand_args.get(2)
                .map(|s| std::path::PathBuf::from(s))
                .unwrap_or_else(|| std::path::PathBuf::from("src/generated"));
            match fluxc_core::webhook_inbound::process_webhook_contracts(&input, &output) {
                Ok(report) => println!("{}", report),
                Err(e) => eprintln!("  webhook-gen error: {}", e),
            }
        }
        Some("search") | Some("grep") => {
            let pattern = subcommand_args.get(1).map(|s| s.as_str()).unwrap_or("");
            let glob = subcommand_args.get(2).map(|s| s.as_str());
            let json = subcommand_args.iter().any(|a| a == "--json");
            if pattern.is_empty() {
                eprintln!("Usage: fluxc search <pattern> [glob] [--json]");
                eprintln!("  Replaces: find . -name '*.rs' | xargs grep pattern");
                eprintln!("  BLAKE3-deduplicated: identical files searched once");
            } else {
                fluxc_core::phase3::search_code(pattern, glob, json);
            }
        }
        Some("suggest-webhook") => {
            let path = subcommand_args.get(1)
                .map(|s| std::path::PathBuf::from(s))
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            let suggestions = fluxc_core::webhook_inbound::suggest_webhooks(&path);
            if suggestions.is_empty() {
                println!("  No webhook suggestions for this project.");
            } else {
                println!("⚡ Flux Webhook Suggestions ({}):", suggestions.len());
                for s in &suggestions {
                    println!("  • {} — {} [{}] → {:?}",
                        s.id, s.description, s.route, s.action);
                }
                println!("\n  Run 'fluxc webhook-gen' to generate handlers from .flux-webhook.toml");
            }
        }
        Some("watch") | Some("w") => fluxc_core::watch_mode(&config, &subcommand_args[1..]),
        Some("dev") | Some("d") => fluxc_core::dev_mode(&config),
        Some("clean") => fluxc_core::clean(),
        Some("serve") => {
            let stats = fluxc_core::serve::init_live_stats();
            fluxc_core::serve::start_server(stats, 8084);
            loop { std::thread::sleep(std::time::Duration::from_secs(60)); }
        }
        Some("latex") | Some("tex") => fluxc_core::run_latex_build(&config, &subcommand_args[1..]),
        Some("stats") | Some("s") => fluxc_core::print_stats(),
        Some("supercluster") | Some("sc") => fluxc_core::supercluster_mode(&subcommand_args[1..]),
        Some("self") => fluxc_core::self_build(config),
        Some("architect") | Some("arch") => fluxc_core::phase3::architect_plan(),
        Some("sigil-plan") => {
            let json = subcommand_args.iter().any(|a| a == "--json");
            let plan = flux_sigil_releases::plan::generate_release_plan();
            if json {
                println!("{}", serde_json::to_string_pretty(&plan).unwrap_or_default());
            } else {
                println!("⚡ SIGIL v{} Release Plan — Cortex-Optimized", plan.version);
                println!("   {} findings | {} SIGIL-specific | {}% top SIMD gain",
                    plan.cortex_insights.total_findings,
                    plan.cortex_insights.sigil_specific_findings,
                    plan.cortex_insights.top_gain_pct);
                for phase in &plan.phases {
                    println!("\n  Phase {}: {} ({} days, {:.0}% Cortex gain)",
                        phase.phase, phase.name, phase.estimated_days, phase.cortex_gain_pct);
                    for rel in &phase.releases {
                        println!("    {} — {} ({:.0}h)", rel.version, rel.title, rel.estimated_hours);
                    }
                }
                println!("\n  Priority Actions:");
                for a in &plan.priority_actions {
                    println!("    #{}. [{}] {} — {}", a.rank, a.dimension, a.action, a.impact);
                }
            }
        }
        Some("cortex") => {
            let preset = subcommand_args.get(1).map(|s| s.as_str()).unwrap_or("MaxPerf");
            let iterations: usize = subcommand_args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);
            let json = subcommand_args.iter().any(|a| a == "--json");
            fluxc_core::cortex::run_cortex_loop(preset, iterations, json);
        }
        Some("cortex-ai") => {
            let target = subcommand_args.get(1).map(|s| s.as_str()).unwrap_or("main.rs");
            let mode = subcommand_args.get(2).map(|s| s.as_str()).unwrap_or("full");
            let iterations: usize = subcommand_args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
            let json = subcommand_args.iter().any(|a| a == "--json");
            fluxc_core::cortex::run_ai_cortex(target, mode, iterations, json);
        }
        Some("cortex-summary") => {
            let json = subcommand_args.iter().any(|a| a == "--json");
            fluxc_core::cortex::run_cortex_summary(json);
        }
        Some("self-heal") => {
            let poll_ms: u64 = subcommand_args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5000);
            let auto = subcommand_args.iter().any(|a| a == "--auto" || a == "-y");
            fluxc_core::self_heal::start_self_heal(poll_ms, auto);
        }
        Some("p2p-worker") => fluxc_core::p2p_worker::run_p2p_worker(),
        Some("auto-update") => {
            // Args after subcommand: --interval N, --apply (or -y).
            let mut interval: u64 = 60;
            let mut apply = false;
            let mut i = 1;
            while i < subcommand_args.len() {
                match subcommand_args[i].as_str() {
                    "--apply" | "-y" => { apply = true; i += 1; }
                    "--interval" | "-n" => {
                        interval = subcommand_args.get(i + 1)
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(60);
                        i += 2;
                    }
                    _ => { i += 1; }
                }
            }
            fluxc_core::p2p_worker::run_auto_updater_with(interval, apply)
        }
        Some("swarm") | Some("sw") => {
            let op = subcommand_args.get(1).map(|s| s.as_str()).unwrap_or("status");
            match op {
                "register" => println!("{}", fluxc_core::swarm::swarm_register_cli(
                    subcommand_args.get(2).map(|s| s.as_str()).unwrap_or("agent1"),
                    subcommand_args.get(3).map(|s| s.as_str()).unwrap_or("qnk..."),
                )),
                "status" | "st" => {
                    println!("⬡ Flux Swarm Status");
                    let s = fluxc_core::swarm::swarm_status();
                    println!("  agents: {}  claims: {}  completed: {}", s.agents, s.active_claims, s.completed_tasks);
                    println!("  QUG paid: {:.1}", s.qug_paid);
                    if !s.agents_list.is_empty() {
                        println!("  agents:");
                        for a in &s.agents_list {
                            let status = format!("{:?}", a.status).to_lowercase();
                            println!("    {}  {}  {} QUG  on {:?}", a.id, status, a.total_earned_qug, a.current_crates);
                        }
                    }
                }
                "claim" | "cl" => {
                    let (agent_id, crates): (String, Vec<String>) = if subcommand_args.len() >= 4 {
                        (
                            subcommand_args[2].clone(),
                            subcommand_args[3..].iter().map(|s| s.to_string()).collect(),
                        )
                    } else {
                        (
                            env::var("FLUX_AGENT_ID").unwrap_or_else(|_| "cli-agent".into()),
                            subcommand_args[2..].iter().map(|s| s.to_string()).collect(),
                        )
                    };
                    if crates.is_empty() {
                        eprintln!("usage: fluxc swarm claim [agent_id] <crate> [crates...]");
                    } else {
                        match fluxc_core::swarm::claim_work(&agent_id, &crates, 10) {
                            Ok(claim) => println!("⬡ Claimed: {} by {} on {:?}", claim.task_id, claim.agent, claim.crates),
                            Err(e) if e.starts_with("self-owned:") => println!("ℹ {}", &e["self-owned: ".len()..]),
                            Err(e) => eprintln!("  claim failed: {e}"),
                        }
                    }
                }
                "complete" | "co" => {
                    let task_id = subcommand_args.get(2).map(|s| s.as_str()).unwrap_or("");
                    if task_id.is_empty() {
                        eprintln!("usage: fluxc swarm complete <task_id> [--fail]");
                    } else {
                        let agent_id = env::var("FLUX_AGENT_ID").unwrap_or_else(|_| "cli-agent".into());
                        let success = !subcommand_args.iter().any(|a| a == "--fail");
                        match fluxc_core::swarm::complete_work(&agent_id, task_id, success) {
                            Some(t) => println!("⬡ Completed: {} ({}) — {} QUG", t.task_id, if success { "✓" } else { "✗" }, t.qug_earned),
                            None => eprintln!("  complete failed (no such claim or not yours)"),
                        }
                    }
                }
                "release" | "rl" => {
                    let task_id = subcommand_args.get(2).map(|s| s.as_str()).unwrap_or("");
                    if task_id.is_empty() {
                        eprintln!("usage: fluxc swarm release <task_id>");
                    } else {
                        let agent_id = env::var("FLUX_AGENT_ID").unwrap_or_else(|_| "cli-agent".into());
                        if fluxc_core::swarm::release_claim(&agent_id, task_id) {
                            println!("⬡ Released: {task_id}");
                        } else {
                            eprintln!("  release failed");
                        }
                    }
                }
                _ => {
                    println!("⬡ fluxc swarm — Swarm coordination");
                    println!("  fluxc swarm status                 Show live swarm state");
                    println!("  fluxc swarm register <agent> <qnk> Register or refresh an agent");
                    println!("  fluxc swarm claim [agent] <crate>  Claim one or more crates");
                    println!("  fluxc swarm complete <task_id>     Complete claimed work");
                    println!("  fluxc swarm release <task_id>      Release a claim");
                }
            }
        }
        Some("os-stage") => {
            // `fluxc os-stage --packages NAME1 NAME2 [--output-dir PATH]`
            let mut packages: Vec<String> = Vec::new();
            let mut output_dir = std::path::PathBuf::from(
                "/home/orobit/q-narwhalknight/dist-final/quillonos"
            );
            let mut i = 1;
            while i < subcommand_args.len() {
                match subcommand_args[i].as_str() {
                    "--packages" | "-p" => {
                        i += 1;
                        while i < subcommand_args.len() && !subcommand_args[i].starts_with("--") {
                            packages.push(subcommand_args[i].clone());
                            i += 1;
                        }
                    }
                    "--output-dir" | "-o" => {
                        if let Some(v) = subcommand_args.get(i + 1) {
                            output_dir = std::path::PathBuf::from(v);
                            i += 2;
                        } else { i += 1; }
                    }
                    _ => i += 1,
                }
            }
            if packages.is_empty() {
                eprintln!("os-stage: --packages NAME1 [NAME2 …] required");
                return;
            }
            let pkg_refs: Vec<&str> = packages.iter().map(|s| s.as_str()).collect();
            match fluxc_core::p2p_worker::os_stage_modules(&pkg_refs, &output_dir) {
                Ok(r) => println!(
                    "✓ staged {} module(s), {} preserved → {}",
                    r.modules.len(), r.preserved_entries, r.manifest_path.display()
                ),
                Err(e) => { eprintln!("os-stage: {}", e); std::process::exit(1); }
            }
        }
        Some("release") => {
            // `fluxc release [VERSION] [--product NAME] [--binary PATH]`
            // Defaults: product=fluxc, binary=current_exe, version=workspace version.
            let mut product = "fluxc".to_string();
            let mut binary: Option<std::path::PathBuf> = None;
            let mut version: Option<String> = None;
            let mut i = 1;
            while i < subcommand_args.len() {
                match subcommand_args[i].as_str() {
                    "--product" | "-p" => {
                        if let Some(v) = subcommand_args.get(i + 1) {
                            product = v.clone(); i += 2;
                        } else { i += 1; }
                    }
                    "--binary" | "-b" => {
                        if let Some(v) = subcommand_args.get(i + 1) {
                            binary = Some(std::path::PathBuf::from(v)); i += 2;
                        } else { i += 1; }
                    }
                    arg if !arg.starts_with('-') && version.is_none() => {
                        version = Some(arg.to_string()); i += 1;
                    }
                    _ => i += 1,
                }
            }
            let version = version.unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
            if let Err(e) = fluxc_core::p2p_worker::publish_release_product(&product, &version, binary) {
                eprintln!("release: {}", e);
                std::process::exit(1);
            }
        }
        Some("release-audit") | Some("rel-audit") => fluxc_core::release_audit::print_release_audit(),
        Some("mcp") => fluxc_mcp::run_mcp_server(),
        Some("version") | Some("-V") => println!("{}", fluxc_core::version()),
        Some("api") => {
            let title = subcommand_args.get(1).map(|s| s.as_str()).unwrap_or("Flux API");
            let format = subcommand_args.get(2).map(|s| s.as_str()).unwrap_or("all");
            fluxc_core::phase3::api_generate(title, format);
        }
        Some("optimize") => {
            let preset = subcommand_args.get(1).map(|s| s.as_str()).unwrap_or("BALANCED");
            fluxc_core::phase3::optimize_analyze(preset);
        }
        Some("ai") => fluxc_core::phase3::ai_audit(),
        Some("agility") => fluxc_core::phase3::agility_audit(),
        Some("verify-proof") => {
            let artifact = subcommand_args.get(1).map(|s| s.as_str()).unwrap_or("");
            let proof_path = subcommand_args.get(2).map(|s| s.as_str())
                .map(String::from)
                .unwrap_or_else(|| format!("{}.proof", artifact));
            if artifact.is_empty() {
                eprintln!("usage: fluxc verify-proof <artifact-path> [<proof-path>]");
            } else {
                match (std::fs::read(artifact), std::fs::read(&proof_path)) {
                    (Ok(art_bytes), Ok(proof_bytes)) => {
                        match fluxc_core::provenance::from_json_bytes(&proof_bytes) {
                            Ok(proof) => match fluxc_core::provenance::verify(&art_bytes, &proof) {
                                Ok(v) => {
                                    let wallet_hex: String = v.agent_wallet.iter().map(|b| format!("{:02x}", b)).collect();
                                    let art_hex: String = v.artifact_hash.iter().map(|b| format!("{:02x}", b)).collect();
                                    let src_hex: String = v.source_hash.iter().map(|b| format!("{:02x}", b)).collect();
                                    let hybrid = !proof.ed25519_sig.is_empty();
                                    let signed = !proof.sqisign_sig.is_empty();
                                    println!("✓ Verified provenance proof");
                                    println!("  artifact:        {} ({} bytes)", artifact, art_bytes.len());
                                    println!("  artifact BLAKE3: {}", art_hex);
                                    println!("  source BLAKE3:   {}", src_hex);
                                    println!("  agent wallet:    qnk{}", wallet_hex);
                                    println!("  timestamp:       {}us", v.timestamp_us);
                                    let sig_line = if hybrid {
                                        "✓ require-both: SQIsign-L5 (292B) + Ed25519 (64B)"
                                    } else if signed {
                                        "✓ SQIsign-L5 (292B sig, 129B pubkey)"
                                    } else {
                                        "✗ (scaffold mode)"
                                    };
                                    println!("  signature:       {}", sig_line);
                                    println!("  on-chain backed: {}", if v.on_chain_backed { "✓" } else { "✗ (no settle_tx)" });
                                }
                                Err(e) => { eprintln!("✗ Verification failed: {}", e); std::process::exit(1); }
                            },
                            Err(e) => { eprintln!("✗ Cannot parse proof: {}", e); std::process::exit(1); }
                        }
                    }
                    (Err(e), _) => { eprintln!("Cannot read artifact: {}", e); std::process::exit(1); }
                    (_, Err(e)) => { eprintln!("Cannot read proof: {}", e); std::process::exit(1); }
                }
            }
        }
        Some("disasm") => {
            let path = subcommand_args.get(1).map(|s| s.as_str()).unwrap_or("");
            if path.is_empty() {
                eprintln!("usage: fluxc disasm <object-path>");
            } else {
                let nm = std::process::Command::new("nm").arg(path).output();
                let od = std::process::Command::new("objdump").args(["-d", path]).output();
                let file = std::process::Command::new("file").arg(path).output();
                println!("⚡ Flux disasm — {}", path);
                if let Ok(f) = file {
                    print!("  type:    {}", String::from_utf8_lossy(&f.stdout));
                }
                println!("\nSymbols (nm):");
                if let Ok(n) = nm { print!("{}", String::from_utf8_lossy(&n.stdout)); }
                println!("\nDisassembly (objdump -d):");
                if let Ok(o) = od { print!("{}", String::from_utf8_lossy(&o.stdout)); }
            }
        }
        Some("agent-keygen") => {
            // Provision BOTH legs of the require-both hybrid (SQIsign + Ed25519).
            // Idempotent: an agent created before the hybrid upgrade keeps its
            // SQIsign identity and gains an Ed25519 leg.
            match fluxc_core::provenance::ensure_agent_keys_hybrid(None) {
                Ok((_ssk, spk, _esk, epk)) => {
                    let spk_hex: String = spk.iter().map(|b| format!("{:02x}", b)).collect();
                    let epk_hex: String = epk.iter().map(|b| format!("{:02x}", b)).collect();
                    println!("  ✓ Agent hybrid provenance keys ensured (require-both)");
                    println!("    SQIsign-L5 pubkey: {} ({} bytes)", &spk_hex[..spk_hex.len().min(64)], spk.len());
                    println!("    Ed25519    pubkey: {} ({} bytes)", &epk_hex[..epk_hex.len().min(64)], epk.len());
                    println!("    keys stored in $FLUX_AGENT_KEY_PATH (default ~/.flux-agent-key.json)");
                }
                Err(e) => eprintln!("  agent-keygen failed: {}", e),
            }
        }
        Some("xray") => {
            let want_json = subcommand_args.iter().any(|a| a == "--json");
            match fluxc_core::xray::xray() {
                Ok(report) => {
                    if want_json {
                        match serde_json::to_string_pretty(&report) {
                            Ok(s) => println!("{}", s),
                            Err(e) => eprintln!("xray serialize error: {}", e),
                        }
                    } else {
                        print!("{}", fluxc_core::xray::render_text(&report));
                    }
                }
                Err(e) => eprintln!("xray: {}", e),
            }
        }
        Some("compile-native") => {
            if let Some(pkg) = config.package.as_deref() {
                fluxc_core::phase3::compile_package(pkg);
            } else {
                let path = subcommand_args.get(1).map(|s|s.as_str()).unwrap_or("main.rs");
                fluxc_core::phase3::compile_impl_with_provenance(path, true, config.provenance);
            }
        }
        Some("compile") => {
            let path = subcommand_args.get(1).map(|s| s.as_str()).unwrap_or("main.rs");
            fluxc_core::phase3::compile_file(path);
        }
        Some("chat") => fluxc_core::chat::chat_mode(),
        Some("plan") => {
            println!("⚡ Flux Build Plan (AI-optimized)");
            let root = env::current_dir().unwrap_or_default();
            match flux_graph::resolve_workspace(&root) {
                Ok(ws) => {
                    println!("  Crates: {} | Batches: {}", ws.crates.len(), ws.batches.len());
                    for (i, b) in ws.batches.iter().enumerate() {
                        let names: Vec<&str> = b.iter().map(|&idx| ws.crates[idx].name.as_str()).collect();
                        println!("  Batch {}: {} crate(s) — {}", i+1, b.len(), names.join(", "));
                    }
                    println!("  Est. time: ~{}s (warm)", ws.crates.len() as f64 * 0.3);
                }
                Err(e) => eprintln!("  flux-graph: {}", e),
            }
        }
        Some("explain") => {
            let crate_name = subcommand_args.get(1).map(|s| s.as_str()).unwrap_or("fluxc");
            let root = env::current_dir().unwrap_or_default();
            match flux_graph::resolve_workspace(&root) {
                Ok(ws) => {
                    if let Some(ci) = ws.crates.iter().find(|c| c.name == crate_name) {
                        println!("⚡ {}", ci.name);
                        println!("  Path: {}", ci.path.display());
                        println!("  Edition: {} | Type: {:?}", ci.edition, ci.crate_type);
                        println!("  Dependencies ({}):", ci.dependencies.len());
                        for d in &ci.dependencies {
                            println!("    {} ({:?})", d.name, d.kind);
                        }
                    } else { eprintln!("  Crate '{}' not found", crate_name); }
                }
                Err(e) => eprintln!("  flux-graph: {}", e),
            }
        }
        Some("status") => {
            let json_out = subcommand_args.get(1).map(|s| s.as_str()) == Some("--json");
            let root = env::current_dir().unwrap_or_default();
            if json_out {
                match flux_graph::resolve_workspace(&root) {
                    Ok(ws) => {
                        let agility = flux_graph::agility::audit_agility(&ws);
                        println!("{{\"crates\":{},\"batches\":{},\"agility\":{:.2},\"pq_crates\":{},\"classical_crates\":{}}}",
                            ws.crates.len(), ws.batches.len(), agility.agility_score, agility.pq_crates, agility.classical_crates);
                    }
                    Err(e) => println!("{{\"error\":\"{}\"}}", e),
                }
            } else {
                println!("⚡ Flux Status");
                match flux_graph::resolve_workspace(&root) {
                    Ok(ws) => {
                        let agility = flux_graph::agility::audit_agility(&ws);
                        println!("  Crates: {} | Batches: {} | Agility: {:.0}%", ws.crates.len(), ws.batches.len(), agility.agility_score*100.0);
                    }
                    Err(e) => eprintln!("  flux-graph: {}", e),
                }
            }
        }
        // ── v0.9: Fleet + Mesh + Wallet CLI tools ──
        Some("fleet") | Some("fl") => {
            println!("⬡ Flux Fleet Status");
            match fluxc_core::flux_net::fleet_status() {
                Ok(nodes) => {
                    for n in &nodes {
                        let dot = if n.online { "●" } else { "○" };
                        println!("  {} {}  h{}  v{}  {}s up",
                            dot, n.name, n.height, n.version, n.uptime_secs);
                    }
                    let online = nodes.iter().filter(|n| n.online).count();
                    println!("  fleet: {}/{} online", online, nodes.len());
                }
                Err(e) => eprintln!("  fleet: {e}"),
            }
        }
        Some("mesh") | Some("mh") => {
            println!("⬡ Flux P2P Mesh Health");
            match fluxc_core::flux_net::mesh_health() {
                Ok(h) => {
                    println!("  peers: {}  quality: {}  fan-out: {}",
                        h.connected_peers, h.quality, h.fan_out);
                    println!("  blocks received: {}  avg latency: {:.1}ms",
                        h.blocks_received, h.avg_block_latency_ms);
                    println!("  messages processed: {}  drop rate: {:.2}",
                        h.messages_processed, h.estimated_drop_rate);
                    if !h.peer_heights.is_empty() {
                        println!("  peer heights:");
                        for (peer, height) in &h.peer_heights {
                            println!("    {} → {}", peer, height);
                        }
                    }
                }
                Err(e) => eprintln!("  mesh: {e}"),
            }
        }
        Some("wallet") | Some("wal") => {
            let url = std::env::var("FLUX_WALLET_URL")
                .unwrap_or_else(|_| "http://localhost:9800/sigil-wallet-tron.html".into());
            println!("⬡ Opening wallet → {url}");
            #[cfg(target_os = "linux")]
            { let _ = std::process::Command::new("xdg-open").arg(&url).spawn(); }
            #[cfg(target_os = "macos")]
            { let _ = std::process::Command::new("open").arg(&url).spawn(); }
            #[cfg(windows)]
            { let _ = std::process::Command::new("cmd").args(["/c", "start", &url]).spawn(); }
        }
        _ => fluxc_core::print_usage(),
    }
}
