// flux-qwen-batch — CLI: fan a batch of prompts across qwen3.6 in parallel, report throughput.
//
//   echo -e "prompt 1\nprompt 2\n…" | flux-qwen-batch
//   BATCH_HOST=212.13.234.23 BATCH_PORT=11434 BATCH_MODEL=qwen3.6 BATCH_PAR=6 flux-qwen-batch
//
// Compares the bigger-package (parallel) sweep against a sequential one and prints the speedup.
use flux_qwen_batch::{BatchRunner, OllamaTransport};
use std::io::Read;

fn env_or(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

fn main() {
    let host = env_or("BATCH_HOST", "212.13.234.23");
    let port: u16 = env_or("BATCH_PORT", "11434").parse().unwrap_or(11434);
    let model = env_or("BATCH_MODEL", "qwen3.6");
    let par: usize = env_or("BATCH_PAR", "6").parse().unwrap_or(6);

    // prompts from stdin (one per line); fall back to a built-in agentic batch
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf).ok();
    let mut prompts: Vec<String> = buf.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    if prompts.is_empty() {
        prompts = vec![
            "In one Rust line: fn add(a:i32,b:i32)->i32. Code only.".into(),
            "One sentence: why is content-addressed dedup good for backups?".into(),
            "Name 3 cases where a fast model and a reasoning model disagree.".into(),
            "Rust: blake3 hex of bytes, one line.".into(),
            "One line: what is backpressure in a gossip network?".into(),
            "Give a 6-word release note for a parallel batch runner.".into(),
        ];
    }

    let mut t = OllamaTransport::new(&host, port, &model);
    t.num_predict = 48;

    eprintln!("▶ flux-qwen-batch  {host}:{port}  model={model}  prompts={}  parallel={par}", prompts.len());

    let seq = BatchRunner::new(1).run(&t, &prompts);
    let parr = BatchRunner::new(par).run(&t, &prompts);
    let speedup = if parr.wall_ms > 0 { seq.wall_ms as f64 / parr.wall_ms as f64 } else { 0.0 };

    println!("{{");
    println!("  \"prompts\": {}, \"parallel\": {par},", prompts.len());
    println!("  \"sequential_ms\": {}, \"sequential_tput\": {:.2},", seq.wall_ms, seq.throughput_per_s);
    println!("  \"parallel_ms\": {}, \"parallel_tput\": {:.2},", parr.wall_ms, parr.throughput_per_s);
    println!("  \"speedup\": {:.2}, \"oks\": {}, \"errs\": {}", speedup, parr.oks, parr.errs);
    println!("}}");
    for (i, r) in parr.responses.iter().enumerate() {
        match r {
            Ok(s) => eprintln!("  [{i}] {}", s.replace('\n', " ").chars().take(90).collect::<String>()),
            Err(e) => eprintln!("  [{i}] ERR {e}"),
        }
    }
}
