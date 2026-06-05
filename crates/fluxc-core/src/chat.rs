// fluxc-core/chat.rs — Interactive AI pair programming mode
//
// Aider-inspired interactive CLI. Type prompts, get Flux analysis.
// Uses fluxc's own tools (agility, optimize, AI audit, plan, explain)
// as the "model" — no external LLM needed for basic operations.
// Ollama/Gemma4 integration is a future extension point.

use std::io::{self, Write};

pub fn chat_mode() {
    println!("╔══════════════════════════════════════════╗");
    println!("║        ⚡ Flux Chat — AI Pair Programming ║");
    println!("╠══════════════════════════════════════════╣");
    println!("║ Commands:                                ║");
    println!("║   /plan      — show build plan           ║");
    println!("║   /agility   — crypto audit              ║");
    println!("║   /ai        — AI-mode rule check         ║");
    println!("║   /optimize  — performance analysis       ║");
    println!("║   /explain X — explain crate X            ║");
    println!("║   /status    — workspace status           ║");
    println!("║   /build     — build workspace            ║");
    println!("║   /compile F — compile file.rs            ║");
    println!("║   /help      — this menu                  ║");
    println!("║   /quit      — exit                       ║");
    println!("║                                           ║");
    println!("║  Type anything else to chat.              ║");
    println!("╚══════════════════════════════════════════╝");

    let root = std::env::current_dir().unwrap_or_default();
    let ws = flux_graph::resolve_workspace(&root).ok();

    loop {
        print!("\n⚡> ");
        io::stdout().flush().ok();

        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() { break; }
        let input = input.trim();

        if input.is_empty() { continue; }

        match input {
            "/quit" | "/q" | "exit" => {
                println!("  Goodbye!");
                break;
            }
            "/help" | "/h" => {
                println!("  /plan /agility /ai /optimize /explain /status /build /compile /help /quit");
            }
            "/plan" => {
                if let Some(ref ws) = ws {
                    println!("  Crates: {} | Batches: {}", ws.crates.len(), ws.batches.len());
                    for (i, b) in ws.batches.iter().enumerate() {
                        let names: Vec<&str> = b.iter().map(|&idx| ws.crates[idx].name.as_str()).collect();
                        println!("    Batch {}: {} — {}", i + 1, b.len(), names.join(", "));
                    }
                }
            }
            "/agility" => {
                if let Some(ref ws) = ws {
                    let a = flux_graph::agility::audit_agility(ws);
                    println!("  Score: {:.0}% | PQ: {} | Classical: {} | Migrations: {}",
                        a.agility_score * 100.0, a.pq_crates, a.classical_crates, a.migration_needed.len());
                    for m in &a.migration_needed {
                        println!("    {}: {:?} → {}", m.crate_name, m.current, m.recommended);
                    }
                }
            }
            "/ai" => {
                if let Some(ref ws) = ws {
                    let r = flux_ai::full_ai_audit(ws);
                    println!("  AI Score: {:.0}% | Lifetime: {} | Send/Sync: {} | Races: {} | Unsafe: {}/{}",
                        r.overall_score * 100.0, r.lifetime_suggestions.len(),
                        r.send_sync_suggestions.len(), r.race_detection_findings.len(),
                        r.unsafe_verification.iter().map(|u| u.verified_count).sum::<usize>(),
                        r.unsafe_verification.iter().map(|u| u.unsafe_block_count).sum::<usize>());
                }
            }
            "/optimize" => {
                if let Some(ref ws) = ws {
                    let r = flux_optimize::apply(ws, flux_optimize::OptimizationPreset::Balanced);
                    println!("  SIMD: {} | io_uring: {} | Cache: {} | Est gain: {:.0}%",
                        r.simd_opportunities.len(), r.iouring_opportunities.len(),
                        r.cache_line_fixes.len(), r.estimated_perf_gain_pct);
                }
            }
            s if s.starts_with("/explain") => {
                let name = s.strip_prefix("/explain ").unwrap_or("fluxc");
                if let Some(ref ws) = ws {
                    if let Some(ci) = ws.crates.iter().find(|c| c.name == name) {
                        println!("  {} | {:?} | Edition: {}", ci.name, ci.crate_type, ci.edition);
                        println!("  Dependencies:");
                        for d in &ci.dependencies {
                            println!("    {} ({:?})", d.name, d.kind);
                        }
                    } else { println!("  Crate '{}' not found", name); }
                }
            }
            "/status" => {
                if let Some(ref ws) = ws {
                    let a = flux_graph::agility::audit_agility(ws);
                    println!("  Crates: {} | Batches: {} | Agility: {:.0}% | PQ: {} | Classical: {}",
                        ws.crates.len(), ws.batches.len(), a.agility_score * 100.0,
                        a.pq_crates, a.classical_crates);
                }
            }
            "/build" => {
                println!("  Building with fluxc build --no-cargo...");
                crate::no_cargo_build(&crate::BuildConfig { no_cargo: true, ..Default::default() });
            }
            s if s.starts_with("/compile") => {
                let path = s.strip_prefix("/compile ").unwrap_or("main.rs");
                crate::phase3::compile_file(path);
            }
            _ => {
                // Chat mode: respond with helpful analysis
                let lower = input.to_lowercase();
                if lower.contains("help") || lower.contains("what") {
                    println!("  Try /plan to see the build plan, /agility for crypto audit, or /ai for code quality analysis.");
                    if let Some(ref ws) = ws {
                        println!("  You have {} crates in this workspace.", ws.crates.len());
                    }
                } else if lower.contains("build") || lower.contains("compile") {
                    println!("  Use /build to trigger a build, or /plan to preview what would be built.");
                } else if lower.contains("crypto") || lower.contains("secure") || lower.contains("quantum") {
                    println!("  Use /agility to audit crypto dependencies for post-quantum readiness.");
                } else if lower.contains("slow") || lower.contains("fast") || lower.contains("optimize") {
                    println!("  Use /optimize to find SIMD, io_uring, and cache-line improvements.");
                } else if lower.contains("error") || lower.contains("bug") || lower.contains("fix") {
                    println!("  Try /ai to run AI-mode rule checks across the workspace.");
                } else {
                    println!("  I can help with: /plan /agility /ai /optimize /explain /status /build /compile");
                    println!("  Or describe what you're working on and I'll suggest the right tool.");
                }
            }
        }
    }
}
