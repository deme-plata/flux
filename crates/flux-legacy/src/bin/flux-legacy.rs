//! flux-legacy CLI — analyze (P1) + actuate (P2) a legacy cargo workspace.
//!
//!   flux-legacy analyze <root> [--json]        # measured per-crate metrics + god-files
//!   flux-legacy plan    <root> [--json]        # ranked refactor plan
//!   flux-legacy report  <root>                 # analyze + plan, human-readable
//!   flux-legacy execute <root> [--top N]       # P2: turn the top-N plan into agent BRIEFs
//!   flux-legacy split   <file> [--max N] [--stage]   # P2: dry-run split a god-file into modules
//!
//! `split` is DRY-RUN: it prints the proposed module partition and (with --stage) writes preview
//! files to `<file-dir>/.flux-legacy-preview/`. It NEVER mutates the original source.

use flux_legacy::{ai_analyze, analyze_workspace_legacy, ask, corpus, execute, pipeline, plan, precheck, pulse, render, split, stabilize};

fn arg_val(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

/// Risk tier (0–4) of a treatment → which gate-path it must pass before mainnet.
fn release_tier(crate_name: &str, kind: &str) -> u8 {
    const CONSENSUS_CRITICAL: &[&str] = &["q-types", "q-dag-knight", "q-consensus-guard", "q-narwhal-core"];
    const STATE_CRITICAL: &[&str] = &["q-storage", "q-network"];
    if CONSENSUS_CRITICAL.contains(&crate_name) {
        4
    } else if STATE_CRITICAL.contains(&crate_name) {
        3
    } else if kind.contains("decouple") {
        2
    } else if kind.contains("add-tests") {
        1
    } else {
        2
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("report");
    let target = args.get(2).map(|s| s.as_str()).unwrap_or(".");
    let json = args.iter().any(|a| a == "--json");

    match cmd {
        "analyze" => {
            let report = flux_legacy::project::analyze_auto(target);
            println!("{}", if json { render::render_json(&report) } else { render::render_text(&report) });
        }
        "stabilize" => {
            let unit = args.get(2).filter(|a| !a.starts_with("--")).cloned().unwrap_or_else(|| "q-api-server.service".to_string());
            let since = arg_val(&args, "--since").unwrap_or_else(|| "30 min ago".to_string());
            match pulse::read_journal(&unit, &since) {
                Ok(log) => {
                    let report = pulse::mine(&log, &format!("{unit} / {since}"));
                    let pl = stabilize::plan(&report);
                    if json { println!("{}", serde_json::to_string_pretty(&pl).unwrap_or_default()); }
                    else { println!("{}", stabilize::render(&pl)); }
                }
                Err(e) => { eprintln!("flux-legacy stabilize: cannot read journal for {unit}: {e}"); std::process::exit(2); }
            }
        }
        "plan" => {
            let report = flux_legacy::project::analyze_auto(target);
            let tasks = plan::refactor_plan(&report);
            if json {
                println!("{}", serde_json::to_string_pretty(&tasks).unwrap_or_default());
            } else {
                println!("{}", render::render_plan(&tasks));
            }
        }
        // P2 — turn the ranked plan into executable agent briefs
        "execute" => {
            let report = flux_legacy::project::analyze_auto(target);
            let tasks = plan::refactor_plan(&report);
            let top_n = arg_val(&args, "--top").and_then(|s| s.parse().ok()).unwrap_or(10);
            let briefs = execute::plan_execution(&tasks, top_n);
            if json {
                println!("{}", serde_json::to_string_pretty(&briefs).unwrap_or_default());
            } else {
                let sum = execute::summarize(&briefs);
                println!("⚙  EXECUTE — top {} refactor briefs (est {} min · ~${:.2} total)", sum.briefs, sum.est_minutes_total, sum.budget_usd_total);
                for b in &briefs {
                    println!("\n  #{:<2} {} · {} → {}", b.rank, b.crate_name, b.kind, b.target);
                    println!("     ~{}min · accept: {}", b.est_minutes, b.acceptance);
                    println!("     prompt: {}", b.prompt);
                }
            }
        }
        // P2 — dry-run split one god-file into themed modules
        "split" => {
            let file = target;
            let src = match std::fs::read_to_string(file) {
                Ok(s) => s,
                Err(e) => { eprintln!("flux-legacy split: cannot read {file}: {e}"); std::process::exit(2); }
            };
            let max = arg_val(&args, "--max").and_then(|s| s.parse().ok()).unwrap_or(8);
            let patch = split::plan_split(file, &src, max);
            if json {
                println!("{}", serde_json::to_string_pretty(&patch).unwrap_or_default());
            } else {
                println!("{}", split::render_patch(&patch));
            }
            if args.iter().any(|a| a == "--stage") {
                let dir = std::path::Path::new(file).parent().map(|p| p.to_path_buf()).unwrap_or_default();
                let staging = dir.join(".flux-legacy-preview");
                match split::stage_patch(staging.to_str().unwrap_or(".flux-legacy-preview"), &patch) {
                    Ok(written) => {
                        println!("\n📂 staged {} preview file(s) under {} (original untouched):", written.len(), staging.display());
                        for w in written { println!("   {w}"); }
                    }
                    Err(e) => eprintln!("stage failed: {e}"),
                }
            } else {
                println!("\n(dry-run — pass --stage to write preview module files; the original is never modified)");
            }
        }
        // flux-context bridge — pack the highest-value code into a token window (ANY repo, BETA 2)
        "corpus" => {
            let window = arg_val(&args, "--window")
                .and_then(|s| s.parse().ok())
                .unwrap_or(flux_context::DEFAULT_WINDOW_TOKENS);
            if flux_legacy::project::is_rust_workspace(target) {
                // Rust workspace → the rich ranked corpus (fan-in/role aware)
                let pack = corpus::build_corpus(&analyze_workspace_legacy(target), window);
                if json { println!("{}", serde_json::to_string_pretty(&pack).unwrap_or_default()); }
                else { println!("{}", corpus::render_corpus(&pack)); }
                if let Some(out) = arg_val(&args, "--out") {
                    match corpus::write_bundle(&pack, &out) {
                        Ok(tok) => println!("\n📝 wrote {} ({} tok) — feed to DeepSeek / Claude", out, tok),
                        Err(e) => eprintln!("write bundle failed: {e}"),
                    }
                } else { println!("\n(pass --out <file> to write the bundle; --window N for the budget)"); }
            } else {
                // any other language → the generic project bundle
                let (bundle, nfiles, toks) = flux_legacy::project::project_bundle(target, window);
                println!("📚 project bundle — {nfiles} files · ~{toks} tok / {window} window");
                if let Some(out) = arg_val(&args, "--out") {
                    match std::fs::write(&out, &bundle) {
                        Ok(_) => println!("📝 wrote {out} (~{toks} tok) — feed to DeepSeek / Claude"),
                        Err(e) => eprintln!("write failed: {e}"),
                    }
                } else { println!("\n(pass --out <file> to write the bundle)"); }
            }
        }
        // legacy × context → DeepSeek v4 1M: analyze the whole node in one shot
        "ask" => {
            let question = args.get(3).filter(|q| !q.starts_with("--")).cloned()
                .unwrap_or_else(|| "Analyze this node's architecture: the biggest structural risks, the worst god-files, the most dangerous coupling, and the top 5 refactors you'd do first. Be concrete with crate/file names.".into());
            let window = arg_val(&args, "--window").and_then(|s| s.parse().ok()).unwrap_or(900_000u32);
            let model = match arg_val(&args, "--model").as_deref() {
                Some("pro") => ask::MODEL_PRO,
                _ => ask::MODEL_FLASH,
            };
            let timeout = arg_val(&args, "--timeout").and_then(|s| s.parse().ok()).unwrap_or(600u64);
            // BETA 2: bundle_auto works on ANY repo (Rust workspace → ranked corpus; else → generic)
            let bundle = flux_legacy::project::bundle_auto(target, window);
            let toks = flux_context::est_tokens(&bundle);
            let nfiles = bundle.matches("// ==== ").count();
            eprintln!("📚 packed ~{toks} tok / {window} window · {nfiles} files → asking {model}…");
            let user = format!("{question}\n\n{bundle}");
            match ask::ask_deepseek(model, ask::DEFAULT_SYSTEM, &user, timeout) {
                Ok(r) => {
                    println!("\n══════════ {} · {} in / {} out tok ══════════\n", r.model, r.prompt_tokens, r.completion_tokens);
                    println!("{}", r.answer);
                    // surface any concrete diff the model proposed, ready for P5 precheck → P4 verify
                    if let Some(patch) = ai_analyze::extract_patch(&r.answer) {
                        println!("\n──────── extracted patch ({} lines) — verify via P4 before applying ────────\n{patch}",
                            patch.lines().count());
                    }
                }
                Err(e) => { eprintln!("ask failed: {e}"); std::process::exit(1); }
            }
        }
        // 🏥 LEGACY HEALTH — ER triage: each crate a patient, sickest first
        "triage" => {
            use flux_legacy::triage;
            let ward = triage::triage(&flux_legacy::project::analyze_auto(target));
            if json {
                println!("{}", serde_json::to_string_pretty(&ward).unwrap_or_default());
            } else {
                println!("{}", triage::render_ward(&ward));
            }
        }
        // 🌐 BETA 2 — detect language(s) + survey ANY repo (not just Rust)
        "detect" => {
            use flux_legacy::lang;
            let s = lang::survey(target);
            if json { println!("{}", serde_json::to_string_pretty(&s).unwrap_or_default()); }
            else { println!("{}", lang::render_survey(&s)); }
        }
        // 🚀 BETA 2 — import a project off GitHub, then survey it (the "billion projects → Flux" door)
        "import" => {
            use flux_legacy::{import, lang};
            let reference = match args.get(2) {
                Some(r) if !r.starts_with("--") => r.clone(),
                _ => { eprintln!("usage: flux-legacy import <owner/repo|url> [--dest DIR] [--analyze]"); std::process::exit(2); }
            };
            let dest = arg_val(&args, "--dest").unwrap_or_else(|| "/home/orobit/flux-imports".into());
            // idempotent: if already imported, survey the existing checkout instead of failing
            let existing = import::parse_github_url(&reference).ok().map(|(o, r, _)| {
                std::path::PathBuf::from(&dest).join(format!("{o}__{r}"))
            }).filter(|p| p.exists());
            let (root, reused) = if let Some(p) = existing {
                eprintln!("↻ already imported → surveying existing {}", p.display());
                (p.to_string_lossy().to_string(), true)
            } else {
                let spec = import::ImportSpec::new(reference.clone(), std::path::PathBuf::from(&dest));
                eprintln!("🚀 importing {reference} → {dest} (shallow · sandboxed · capped · no hooks)…");
                match import::import(&spec) {
                    import::ImportOutcome::Imported { path, .. } => (path.to_string_lossy().to_string(), false),
                    import::ImportOutcome::Rejected(why) => {
                        if json { println!("{}", serde_json::json!({"ok": false, "error": why})); }
                        else { eprintln!("import rejected (fail-closed gate): {why}"); }
                        std::process::exit(1);
                    }
                }
            };
            let s = lang::survey(&root);
            if json {
                // the control-panel Import tab consumes this: survey + ER triage in one shot
                let ward = flux_legacy::triage::triage(&flux_legacy::project::analyze_auto(&root));
                let out = serde_json::json!({
                    "ok": true, "root": root, "reused": reused,
                    "primary": s.primary, "total_loc": s.total_loc, "total_files": s.total_files,
                    "tallies": s.tallies, "god_files": s.god_files, "patients": ward.patients,
                });
                println!("{}", serde_json::to_string(&out).unwrap_or_default());
            } else {
                println!("✅ {} → {}", if reused { "reused" } else { "cloned" }, root);
                println!("\n{}", lang::render_survey(&s));
                if args.iter().any(|a| a == "--analyze") {
                    println!("→ next: flux-legacy triage {root}  ·  flux-legacy ask {root} \"...\"  ·  flux-legacy corpus {root} --out bundle.txt");
                }
            }
        }
        // 📦 BETA-1 RELEASE — the consultancy-grade assessment over the whole node
        "release" => {
            use flux_legacy::{release, stability, triage};
            let report = flux_legacy::project::analyze_auto(target);
            let ward = triage::triage(&report);
            let sickest: Vec<String> = ward.patients.iter()
                .filter(|p| p.acuity != triage::Acuity::Healthy)
                .take(8)
                .map(|p| format!("{} {} `{}` — {}", p.acuity.icon(), p.acuity.label(), p.crate_name, p.diagnosis))
                .collect();
            let tasks = plan::refactor_plan(&report);
            let ranked: Vec<(usize, String, String, String, u8)> = tasks.iter().take(12)
                .map(|t| (t.rank, t.crate_name.clone(), t.target.clone(), t.kind.clone(), release_tier(&t.crate_name, &t.kind)))
                .collect();
            // best-effort live health (graceful off-Epsilon)
            let sig = stability::probe("q-api-server", "q-api-server-v10.11", "data-mainnet-genesis", "https://quillon.xyz/api/v1/status");
            let sr = stability::assess(&sig);
            let watch = sr.findings.iter().filter(|f| f.health == stability::Health::Watch).count();
            let danger = sr.findings.iter().filter(|f| f.health == stability::Health::Danger).count();
            let health = format!("{:?} — {} watch · {} danger ({})", sr.verdict, watch, danger,
                if sig.process_alive { "node up" } else { "node not found here — static assessment only" });
            // tests_green: we don't run the suite from here; the operator asserts it (default true,
            // override with --tests-red). The CI gate (flux_combo) is the real proof.
            let tests_green = !args.iter().any(|a| a == "--tests-red");
            let assessment = release::build_assessment(
                target, report.crate_count, report.total_loc, report.god_files.len(),
                health, sickest, &ranked, tests_green,
            );
            let out_text = if json {
                serde_json::to_string_pretty(&assessment).unwrap_or_default()
            } else {
                release::render_assessment(&assessment)
            };
            println!("{out_text}");
            if let Some(path) = arg_val(&args, "--out") {
                match std::fs::write(&path, &out_text) {
                    Ok(_) => eprintln!("\n📦 wrote {} ({})", path, release::BETA1_VERSION),
                    Err(e) => eprintln!("write failed: {e}"),
                }
            }
        }
        // 🏥 ADMIT — walk ONE patient through the whole hospital (triage→psych→[consult]→surgery→recovery→discharge)
        "admit" => {
            use flux_legacy::admit;
            let crate_name = match args.get(3) {
                Some(c) if !c.starts_with("--") => c.clone(),
                _ => { eprintln!("usage: flux-legacy admit <root> <crate> [--consult] [--model pro]"); std::process::exit(2); }
            };
            let mut opts = admit::AdmitOpts::default();
            opts.consult = args.iter().any(|a| a == "--consult");
            if let Some("pro") = arg_val(&args, "--model").as_deref() { opts.model = ask::MODEL_PRO.to_string(); }
            if opts.consult { eprintln!("🩺 calling the independent doctor (DeepSeek) into the admission…"); }
            let rec = admit::admit(target, &crate_name, &opts);
            if json {
                println!("{}", serde_json::to_string_pretty(&rec).unwrap_or_default());
            } else {
                println!("{}", admit::render_admission(&rec));
            }
            std::process::exit(if rec.discharged { 0 } else { 1 });
        }
        // 🩺👨‍⚕️ CONSULT — DeepSeek as the independent doctor; second opinion vs in-house triage/psych
        "consult" => {
            use flux_legacy::consult;
            let crate_name = match args.get(3) {
                Some(c) if !c.starts_with("--") => c.clone(),
                _ => { eprintln!("usage: flux-legacy consult <root> <crate> [--model pro] [--window N]"); std::process::exit(2); }
            };
            let window = arg_val(&args, "--window").and_then(|s| s.parse().ok()).unwrap_or(consult::DEFAULT_CONSULT_WINDOW);
            let model = match arg_val(&args, "--model").as_deref() {
                Some("pro") => ask::MODEL_PRO,
                _ => ask::MODEL_FLASH,
            };
            let timeout = arg_val(&args, "--timeout").and_then(|s| s.parse().ok()).unwrap_or(600u64);
            let in_house = consult::in_house_read(target, &crate_name);
            eprintln!("🩺 consulting {model} on `{crate_name}` (independent, cold read)…");
            match consult::consult_crate(target, &crate_name, model, window, timeout) {
                Ok(note) => println!("{}", consult::second_opinion(&note, &in_house)),
                Err(e) => { eprintln!("consult failed: {e}"); std::process::exit(1); }
            }
        }
        // 🧠 PSYCHIATRY — behavioral pathology in wicked code (unsafe/swallowed-errors/todo!/panic)
        "psych" => {
            use flux_legacy::psych;
            let report = psych::evaluate_workspace(target);
            if json {
                println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
            } else {
                println!("{}", psych::render_psych(&report));
            }
        }
        // P10 — audit a RUNNING node's stability (tailored for the live-node job)
        "health" => {
            use flux_legacy::stability;
            let service = arg_val(&args, "--service").unwrap_or_else(|| "q-api-server".into());
            let proc_match = arg_val(&args, "--proc").unwrap_or_else(|| "q-api-server-v10.11".into());
            let db = arg_val(&args, "--db").unwrap_or_else(|| "data-mainnet-genesis".into());
            let endpoint = arg_val(&args, "--endpoint").unwrap_or_else(|| "https://quillon.xyz/api/v1/status".into());
            let sig = stability::probe(&service, &proc_match, &db, &endpoint);
            let report = stability::assess(&sig);
            if json {
                println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
            } else {
                println!("{}", stability::render(&report));
            }
            // exit code reflects severity so a babysitter loop can branch on it
            std::process::exit(match report.verdict {
                stability::Verdict::InterventionNeeded => 2,
                stability::Verdict::WatchClosely => 1,
                stability::Verdict::StableForNow => 0,
            });
        }
        // P6 LAND — split a god-file, precheck, and (with --confirm) branch+commit+push to the hub
        "sync" => {
            let repo = target;
            let rel = match args.get(3) {
                Some(r) if !r.starts_with("--") => r.clone(),
                _ => { eprintln!("usage: flux-legacy sync <repo_root> <god-file-rel> [--confirm] [--no-push] [--remote R]"); std::process::exit(2); }
            };
            let abs = std::path::Path::new(repo).join(&rel);
            let src = match std::fs::read_to_string(&abs) {
                Ok(s) => s,
                Err(e) => { eprintln!("cannot read {}: {e}", abs.display()); std::process::exit(2); }
            };
            // crate name = path segment after "crates/"; crate src dir = the god-file's parent
            let parts: Vec<&str> = rel.split('/').collect();
            let crate_name = parts.iter().position(|p| *p == "crates").and_then(|i| parts.get(i + 1)).copied().unwrap_or("crate");
            let crate_src_rel = std::path::Path::new(&rel).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
            let file = std::path::Path::new(&rel).file_name().and_then(|s| s.to_str()).unwrap_or(&rel);

            let max = arg_val(&args, "--max").and_then(|s| s.parse().ok()).unwrap_or(8);
            let patch = split::plan_split(abs.to_str().unwrap_or(&rel), &src, max);

            // P5 gate — refuse to land an Unsafe structural split
            let pre = precheck::precheck_split(&patch);
            println!("{}", precheck::render_precheck(&pre));
            if matches!(pre.verdict, precheck::Verdict::Unsafe) {
                println!("\n⛔ precheck UNSAFE — not landing. (fix the split or lower --max)");
                std::process::exit(1);
            }

            let edits = pipeline::split_to_edits(&patch, &crate_src_rel);
            let remote = arg_val(&args, "--remote").unwrap_or_else(|| "origin".into());
            let mut opts = pipeline::SyncOpts::for_split(crate_name, file, &remote);
            opts.confirm = args.iter().any(|a| a == "--confirm");
            opts.push = !args.iter().any(|a| a == "--no-push");
            if opts.confirm {
                eprintln!("⚠  --confirm set: ensure P4 verify / shadow gate is GREEN for this split before landing.");
            }
            let report = pipeline::sync_apply(repo, &edits, &opts);
            println!("{}", pipeline::render_sync(&report));
        }
        // "report" (default)
        _ => {
            let report = flux_legacy::project::analyze_auto(target);
            println!("{}", render::render_text(&report));
            println!();
            let tasks = plan::refactor_plan(&report);
            println!("{}", render::render_plan(&tasks));
        }
    }
}
