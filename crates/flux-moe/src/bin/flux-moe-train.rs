//! flux-moe-train — build the chronos fine-tune corpus + PROPOSE the train command.
//!
//! Propose-only: it writes the JSONL corpus and PRINTS the HF trl/peft command —
//! it never launches training (that needs a Vast GPU + HF deps; run the printed
//! command there yourself). The chronos records below are REAL runs captured
//! 2026-05-31 via flux_chronos_run (seed 42, reproducible).
//!
//!   flux-moe-train [out.jsonl]      # default: ./chronos-corpus.jsonl

use flux_moe::dataset::{parse_run, to_jsonl, ChronosRecord};
use flux_moe::toolcorpus;
use flux_moe::trainer::{plan, train_command};

// The exact reports flux_chronos_run emitted (read-from-output, not invented).
const REPORTS: &[&str] = &[
    "🕰 flux_chronos_run — star-flood\n  4 nodes (3 sinks) · 50 msgs each · 20ms latency · 0.0% loss · redundancy x1\n  Unique delivered: 150/150 (100.0%)\n  Seed: 42 (reproducible) · sim wall: 0ms",
    "🕰 flux_chronos_run — star-flood\n  8 nodes (7 sinks) · 50 msgs each · 40ms latency · 0.0% loss · redundancy x1\n  Unique delivered: 350/350 (100.0%)\n  Seed: 42 (reproducible) · sim wall: 6ms",
    "🕰 flux_chronos_run — star-flood\n  12 nodes (11 sinks) · 50 msgs each · 80ms latency · 0.0% loss · redundancy x1\n  Unique delivered: 550/550 (100.0%)\n  Seed: 42 (reproducible) · sim wall: 0ms",
    "🕰 flux_chronos_run — star-flood\n  12 nodes (11 sinks) · 50 msgs each · 80ms latency · 0.0% loss · redundancy x3\n  Unique delivered: 550/550 (100.0%)\n  Seed: 42 (reproducible) · sim wall: 2ms",
    "🕰 flux_chronos_run — star-flood\n  16 nodes (15 sinks) · 50 msgs each · 120ms latency · 0.0% loss · redundancy x2\n  Unique delivered: 750/750 (100.0%)\n  Seed: 42 (reproducible) · sim wall: 1ms",
];

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "chronos-corpus.jsonl".into());

    let records: Vec<ChronosRecord> = REPORTS.iter().filter_map(|r| parse_run(r)).collect();
    eprintln!("parsed {}/{} chronos reports into records", records.len(), REPORTS.len());

    let jsonl = to_jsonl(&records);
    let n_examples = jsonl.lines().count();
    if let Err(e) = std::fs::write(&out, &jsonl) {
        eprintln!("write {out}: {e}");
        std::process::exit(1);
    }
    println!("✓ wrote {n_examples} chronos examples ({} records × 2 framings) → {out}", records.len());

    // The agentic-money / flux TOOL-CALL corpus — the "execution on par with
    // Claude Code" differentiator. Validate every seed call, then emit alongside.
    match toolcorpus::validate_seed() {
        Ok(n) => {
            let tool_path = out.replace("chronos-corpus", "toolcall-corpus");
            let tool_path = if tool_path == out { "toolcall-corpus.jsonl".to_string() } else { tool_path };
            let tj = toolcorpus::to_jsonl();
            if let Err(e) = std::fs::write(&tool_path, &tj) {
                eprintln!("write {tool_path}: {e}");
            } else {
                println!("✓ wrote {n} tool-call examples (agentic-money + flux surfaces) → {tool_path}");
            }
        }
        Err(e) => eprintln!("⚠ tool-call seed INVALID — not written: {e}"),
    }

    // The trading/general expert is the target; pick a base + propose the command.
    // (≤3B so CPU-LoRA is feasible on the owned swarm; classifier_only=false.)
    let p = plan("Qwen/Qwen3-1.5B", 1.5, /*gpu_available=*/ false, /*classifier_only=*/ false);
    println!("\n── proposed trainer plan ──");
    println!("base:    {}", p.base_model);
    println!("backend: {:?} · method {:?}", p.backend, p.method);
    println!("est:     {:.1}h · ${:.2}", p.est_hours, p.est_usd);
    println!("note:    {}", p.note);

    let cmd = train_command(&p, &out, "./flux-moe-chronos-lora");
    println!("\n── PROPOSED command (run on the training box; NOT executed here) ──");
    println!("{}", cmd.join(" "));
    println!("\n(needs: pip install trl peft transformers + the HF base model pulled.");
    println!(" For the GPU/QLoRA path, rent a Vast box and re-run `plan(.., gpu_available=true)`.)");
}
