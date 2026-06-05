//! flux-moe-autotune - sweep the throughput-affecting ollama options for a model, print each
//! config's tok/s, cache the fastest, and report whether the target is met.
//!   FLUX_MOE_OLLAMA=http://host:11434  FLUX_MOE_TARGET_TPS=6  flux-moe-autotune [model]
use flux_moe::autotune;

fn main() {
    let endpoint = std::env::var("FLUX_MOE_OLLAMA").unwrap_or_else(|_| "http://localhost:11434".to_string());
    let model = std::env::args().nth(1).unwrap_or_else(|| "qwen3.6:latest".to_string());
    let cores = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(8);
    let target: f64 = std::env::var("FLUX_MOE_TARGET_TPS").ok().and_then(|s| s.parse().ok()).unwrap_or(6.0);
    let tmo: u64 = std::env::var("FLUX_MOE_TUNE_TIMEOUT_S").ok().and_then(|s| s.parse().ok()).unwrap_or(240);
    eprintln!("autotuning {model} @ {endpoint}  (cores={cores}, target={target} tok/s, per-config timeout={tmo}s)");
    match autotune::autotune(&endpoint, &model, cores, 48, tmo) {
        Ok(tr) => {
            for (p, tps) in &tr.trials {
                let thr = if p.num_thread == 0 { "auto".to_string() } else { p.num_thread.to_string() };
                println!("  ctx={:<4} batch={:<3} mlock={:<5} thr={:<4} -> {:.2} tok/s", p.num_ctx, p.num_batch, p.use_mlock, thr, tps);
            }
            let thr = if tr.best.num_thread == 0 { "auto".to_string() } else { tr.best.num_thread.to_string() };
            println!("BEST: ctx={} batch={} mlock={} thr={} = {:.2} tok/s (target {:.1})",
                     tr.best.num_ctx, tr.best.num_batch, tr.best.use_mlock, thr, tr.best_tok_s, target);
            autotune::save_best(&model, &tr.best);
            println!("cached -> {:?}", autotune::cache_path(&model));
            println!("{}", if tr.best_tok_s >= target { "TARGET MET" } else { "below target (best available on this box)" });
        }
        Err(e) => {
            eprintln!("autotune failed: {e}");
            std::process::exit(1);
        }
    }
}
