//! flux-moe — route a prompt across the Qwen/Gemma expert pool (the Flux hybrid).
//!
//!   flux-moe "<prompt>"              # route to the best expert
//!   flux-moe --ensemble "<prompt>"   # best + Gemma cross-check
//!   FLUX_MOE_OLLAMA=http://host:11434 (default Delta)

use flux_moe::builder::{assist, BuildTask};
use flux_moe::{classify, flux_llm, generate, generate_stream, registry, route, Mode};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // `flux-moe stream "<prompt>"` — like the forced-model path, but emits tokens to stdout AS
    // they generate (flux_moe::generate_stream). The SSE proxy spawns this and pipes stdout live.
    if args.first().map(|s| s.as_str()) == Some("stream") {
        use std::io::Write;
        let endpoint = std::env::var("FLUX_MOE_OLLAMA").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
        let model = std::env::var("FLUX_MOE_MODEL").unwrap_or_else(|_| "qwen3.6:latest".into());
        let prompt = args[1..].join(" ");
        if prompt.trim().is_empty() { eprintln!("usage: flux-moe stream \"<prompt>\""); return; }
        let mut out = std::io::stdout();
        match generate_stream(&endpoint, &model, &prompt, |t| { let _ = out.write_all(t.as_bytes()); let _ = out.flush(); }) {
            Ok(_) => {}
            Err(e) => { eprintln!("stream failed: {e}"); std::process::exit(1); }
        }
        return;
    }

    // `flux-moe pipeline [--budget $] "<task>"` — the Verified Build Pipeline:
    // qwen3.6 (local) drafts → free rustc compile-gate → deepseek-v4-flash (cloud) judges
    // → ships only on 2-of-2 (compile + approve), priced + budgeted.
    if args.first().map(|s| s.as_str()) == Some("pipeline") {
        let endpoint = std::env::var("FLUX_MOE_OLLAMA").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
        let proposer = std::env::var("FLUX_MOE_PROPOSER").unwrap_or_else(|_| "qwen3.6:latest".into());
        let judge = std::env::var("FLUX_MOE_JUDGE").unwrap_or_else(|_| flux_moe::cloud::DEEPSEEK_FLASH.to_string());
        let rest = &args[1..];
        let mut budget = 0.05_f64;
        let mut land: Option<String> = None;
        let mut mode = flux_moe::pipeline::LandMode::Append;
        let mut goal_parts = Vec::new();
        let mut it = rest.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--budget" => { if let Some(b) = it.next() { budget = b.parse().unwrap_or(budget); } }
                "--land" => { land = it.next().cloned(); }
                "--mode" => { if let Some(m) = it.next() { mode = match m.as_str() {
                    "new" => flux_moe::pipeline::LandMode::New,
                    "overwrite" => flux_moe::pipeline::LandMode::Overwrite,
                    _ => flux_moe::pipeline::LandMode::Append,
                }; } }
                _ => goal_parts.push(a.clone()),
            }
        }
        let task = goal_parts.join(" ");
        if task.trim().is_empty() {
            eprintln!("usage: flux-moe pipeline [--budget <usd>] [--land <file> [--mode new|overwrite|append]] \"<task>\"");
            return;
        }
        let ops = flux_moe::pipeline::LiveOps { local_endpoint: endpoint, proposer: proposer.clone(), judge_model: judge.clone() };
        eprintln!("→ pipeline · propose {proposer} (local) → rustc gate → judge {judge} (cloud) · budget ${budget:.4}");
        match flux_moe::pipeline::run(&ops, &task, budget) {
            Ok(r) => {
                println!("── code ──\n{}\n", r.code.trim());
                println!("compile: {}", if r.compiled { "✅ passed" } else { "❌ FAILED" });
                if !r.compiled && !r.compile_log.trim().is_empty() {
                    println!("  {}", r.compile_log.lines().find(|l| l.contains("error")).unwrap_or("").trim());
                }
                if r.judged { println!("judge:   {}\n  {}", if r.approved { "✅ APPROVE" } else { "❌ REJECT" }, r.verdict.lines().nth(1).unwrap_or("").trim()); }
                println!("\n{}  · spent ${:.6} / ${:.4} budget", r.stage.label(), r.spent_usd, r.budget_usd);
                // ── auto-integration: only a SHIPPED result lands, and only if it survives verify ──
                if let Some(path) = land {
                    if r.shipped() {
                        // verify = FLUX_MOE_VERIFY_CMD (e.g. whole-crate build) if set, else standalone rustc on the file.
                        let vcmd = std::env::var("FLUX_MOE_VERIFY_CMD").ok();
                        let pclone = path.clone();
                        let verify = move || -> (bool, String) {
                            if let Some(cmd) = &vcmd {
                                match std::process::Command::new("bash").arg("-c").arg(cmd).output() {
                                    Ok(o) => (o.status.success(), String::from_utf8_lossy(&o.stderr).chars().take(800).collect()),
                                    Err(e) => (false, format!("verify cmd failed to run: {e}")),
                                }
                            } else {
                                match std::fs::read_to_string(&pclone) {
                                    Ok(src) => flux_moe::pipeline::compile_rust_lib(&src),
                                    Err(e) => (false, format!("read back failed: {e}")),
                                }
                            }
                        };
                        match flux_moe::pipeline::integrate(&r, &path, mode, verify) {
                            Ok(l) if l.landed() => println!("\n🟢 AUTO-INTEGRATED → {} (verified, kept)", l.path),
                            Ok(l) if l.rolled_back => println!("\n🔴 land rolled back ({}): {}", l.path, l.log.lines().find(|x| x.contains("error")).unwrap_or("verify failed").trim()),
                            Ok(l) => println!("\n⚪ land state: wrote={} verified={}", l.wrote, l.verified),
                            Err(e) => println!("\n⚪ not landed: {e}"),
                        }
                    } else {
                        println!("⚪ not landed: only a 2-of-2 SHIP auto-integrates (this was '{}')", r.stage.label());
                    }
                }
            }
            Err(e) => eprintln!("pipeline failed: {e}"),
        }
        return;
    }

    // `flux-moe gate "<task>"` — the cross-transport 2-of-2: a LOCAL ollama expert
    // (qwen3.6 on Epsilon, free) proposes; the DeepSeek CLOUD judge (deepseek-v4-flash)
    // approves/rejects, with the dollar cost of the veto attached.
    if args.first().map(|s| s.as_str()) == Some("gate") {
        let endpoint = std::env::var("FLUX_MOE_OLLAMA").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
        let proposer = std::env::var("FLUX_MOE_PROPOSER").unwrap_or_else(|_| "qwen3.6:latest".into());
        let judge = std::env::var("FLUX_MOE_JUDGE").unwrap_or_else(|_| flux_moe::cloud::DEEPSEEK_FLASH.to_string());
        let task = args[1..].join(" ");
        if task.trim().is_empty() {
            eprintln!("usage: flux-moe gate \"<task>\"   (proposer {proposer} @ {endpoint} · judge {judge} via DeepSeek cloud)");
            return;
        }
        eprintln!("→ gate · propose {proposer} (local) · judge {judge} (cloud)");
        match flux_moe::cloud::propose_then_cloud_judge(&endpoint, &proposer, &judge, &task) {
            Ok(v) => {
                println!("── proposal ({proposer}) ──\n{}\n", v.proposal.trim());
                println!("── verdict ({judge}) ──\n{}", v.verdict.trim());
                println!("\n{}  · judge cost ${:.6}",
                    if v.approved { "✅ APPROVED" } else { "❌ REJECTED" }, v.judge_cost_usd);
            }
            Err(e) => eprintln!("gate failed: {e}"),
        }
        return;
    }

    // `flux-moe build [--pkg P] "<goal>"` — qwen3.6 (or FLUX_MOE_MODEL) writes compile-ready Rust
    // for the goal, THROUGH flux-moe's router/generate path. This is "the model helps build".
    if args.first().map(|s| s.as_str()) == Some("build") {
        let endpoint = flux_moe::ollama_endpoint().unwrap_or_else(|e| { eprintln!("flux-moe: {e}"); std::process::exit(2) });
        let model = std::env::var("FLUX_MOE_MODEL").unwrap_or_else(|_| "qwen3.6:latest".into());
        let rest = &args[1..];
        let mut pkg = "flux".to_string();
        let mut goal_parts = Vec::new();
        let mut it = rest.iter();
        while let Some(a) = it.next() {
            if a == "--pkg" { if let Some(p) = it.next() { pkg = p.clone(); } }
            else { goal_parts.push(a.clone()); }
        }
        let goal = goal_parts.join(" ");
        if goal.trim().is_empty() {
            eprintln!("usage: flux-moe build [--pkg <package>] \"<goal>\"   (model: {model}, ollama: {endpoint})");
            return;
        }
        eprintln!("→ build assist · pkg {pkg} · expert {model}");
        match assist(&endpoint, &model, &BuildTask { package: pkg, goal }) {
            Ok(p) => println!("{}", p.code),
            Err(e) => eprintln!("build assist failed: {e}"),
        }
        return;
    }
    let ensemble = args.iter().any(|a| a == "--ensemble");
    let prompt: String = args.iter().filter(|a| !a.starts_with("--")).cloned().collect::<Vec<_>>().join(" ");
    let endpoint = flux_moe::ollama_endpoint().unwrap_or_else(|e| { eprintln!("flux-moe: {e}"); std::process::exit(2) });

    if prompt.trim().is_empty() {
        println!("flux-moe — Flux hybrid LLM (mixture-of-models router over Qwen + Gemma)\n");
        println!("experts:");
        for e in registry() {
            println!("  {:<14} {:<22} handles {:?}", e.name, e.model, e.handles);
        }
        println!("\nusage: flux-moe [--ensemble] \"<prompt>\"   (Ollama: {endpoint})");
        return;
    }

    // FLUX_MOE_MODEL forces a specific expert (e.g. deepseek-r1:70b) past the router — proves a
    // reasoning expert works end-to-end via the (think-aware, keep-alive) generate() path.
    if let Ok(forced) = std::env::var("FLUX_MOE_MODEL") {
        let forced = forced.trim().to_string();
        eprintln!("→ forced model {forced} (router bypassed)");
        match generate(&endpoint, &forced, &prompt) {
            Ok(out) => println!("{out}"),
            Err(e) => eprintln!("generate failed: {e}\n(ollama at {endpoint} with `ollama pull {forced}`?)"),
        }
        return;
    }

    let task = classify(&prompt);
    let experts = registry();
    let pick = route(&experts, task);
    eprintln!("→ task {task:?} → expert {} ({})", pick.name, pick.model);
    match flux_llm(&endpoint, &prompt, if ensemble { Mode::Ensemble } else { Mode::Route }) {
        Ok(out) => println!("{out}"),
        Err(e) => {
            eprintln!("generate failed: {e}");
            eprintln!("(routing works; needs Ollama at {endpoint} with the model pulled — `ollama pull {}`)", pick.model);
        }
    }
}
