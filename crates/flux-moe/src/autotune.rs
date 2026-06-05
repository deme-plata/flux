//! autotune.rs - flux-moe DYNAMIC inference auto-tuner.
//!
//! When a model generates too slowly (tok/s below target), flux automatically sweeps the
//! THROUGHPUT-affecting ollama options and keeps the fastest. The levers, in order of impact on a
//! memory-tight CPU box running a big MoE:
//!   * `use_mlock` - pin the model in RAM so a 36B MoE stops SWAP-THRASHING (the killer here).
//!   * `num_ctx`   - shrink the KV cache (less memory -> less swap).
//!   * `num_thread`- more threads can THRASH memory bandwidth on an MoE; fewer can be faster.
//!   * `num_batch` - prompt-eval batch size.
//! Quality is untouched (temperature stays 0). The winning config is cached per model so production
//! starts fast, and `generate_dynamic` re-tunes on the fly when a call drops below target.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// The ollama generation options that affect THROUGHPUT (not output quality).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TuneParams {
    /// 0 = let ollama auto-pick; else an explicit CPU-thread count.
    pub num_thread: u32,
    pub num_ctx: u32,
    pub num_batch: u32,
    pub use_mlock: bool,
    /// 0 = force CPU (no wasted GPU-offload probing on a CPU box).
    pub num_gpu: u32,
}

impl TuneParams {
    pub fn cpu_default() -> Self {
        TuneParams { num_thread: 0, num_ctx: 4096, num_batch: 512, use_mlock: false, num_gpu: 0 }
    }
    fn to_options(self) -> serde_json::Value {
        let mut o = serde_json::json!({
            "temperature": 0,
            "num_ctx": self.num_ctx,
            "num_batch": self.num_batch,
            "use_mlock": self.use_mlock,
            "num_gpu": self.num_gpu,
        });
        if self.num_thread > 0 {
            o["num_thread"] = self.num_thread.into();
        }
        o
    }
}

/// Throughput stats from one generation (ollama reports eval_count + eval_duration).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GenStats {
    pub eval_count: u64,
    pub eval_ns: u64,
    pub wall_s: f64,
}
impl GenStats {
    /// Pure generation tokens/second (excludes prompt eval), from ollama's own counters.
    pub fn tok_s(&self) -> f64 {
        if self.eval_ns == 0 { 0.0 } else { self.eval_count as f64 / (self.eval_ns as f64 / 1e9) }
    }
}

#[derive(Deserialize)]
struct TimedResp {
    response: String,
    #[serde(default)]
    eval_count: u64,
    #[serde(default)]
    eval_duration: u64,
}

/// One timed generation with explicit tuning options. Returns (text, throughput stats).
pub fn generate_timed(
    endpoint: &str,
    model: &str,
    prompt: &str,
    p: TuneParams,
    num_predict: u32,
    timeout_s: u64,
) -> Result<(String, GenStats), String> {
    let mut opts = p.to_options();
    opts["num_predict"] = num_predict.into();
    let body = serde_json::json!({
        "model": model, "prompt": prompt, "stream": false, "keep_alive": "30m", "options": opts
    });
    let t = std::time::Instant::now();
    let r: TimedResp = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(timeout_s))
        .build()
        .map_err(|e| e.to_string())?
        .post(format!("{endpoint}/api/generate"))
        .json(&body)
        .send()
        .map_err(|e| format!("connect {endpoint}: {e}"))?
        .error_for_status()
        .map_err(|e| format!("http: {e}"))?
        .json()
        .map_err(|e| format!("decode: {e}"))?;
    let wall = t.elapsed().as_secs_f64();
    Ok((r.response, GenStats { eval_count: r.eval_count, eval_ns: r.eval_duration, wall_s: wall }))
}

#[derive(Debug, Clone)]
pub struct TuneResult {
    pub best: TuneParams,
    pub best_tok_s: f64,
    pub trials: Vec<(TuneParams, f64)>,
}

/// The curated search space worth trying on a memory-tight CPU box: a baseline, then `use_mlock`
/// configs with progressively smaller KV caches and a couple of thread counts.
pub fn search_space(cores: u32) -> Vec<TuneParams> {
    let half = (cores / 2).max(1);
    let three_q = ((cores * 3) / 4).max(1);
    vec![
        TuneParams { num_thread: 0,      num_ctx: 4096, num_batch: 512, use_mlock: false, num_gpu: 0 }, // baseline (today)
        TuneParams { num_thread: 0,      num_ctx: 2048, num_batch: 512, use_mlock: true,  num_gpu: 0 }, // mlock + smaller KV
        TuneParams { num_thread: 0,      num_ctx: 1024, num_batch: 512, use_mlock: true,  num_gpu: 0 }, // mlock + tiny KV
        TuneParams { num_thread: half,   num_ctx: 2048, num_batch: 256, use_mlock: true,  num_gpu: 0 }, // fewer threads
        TuneParams { num_thread: three_q, num_ctx: 1024, num_batch: 512, use_mlock: true, num_gpu: 0 }, // more threads + tiny KV
    ]
}

/// Sweep the search space, measuring tok/s per config; return the fastest. A config that errors or
/// times out scores 0 (and is recorded). Pure orchestration over `generate_timed`.
pub fn autotune(
    endpoint: &str,
    model: &str,
    cores: u32,
    num_predict: u32,
    per_config_timeout_s: u64,
) -> Result<TuneResult, String> {
    let prompt = "List five ways to speed up CPU LLM inference. One short line each.";
    let mut trials = Vec::new();
    let mut best = TuneParams::cpu_default();
    let mut best_tps = -1.0_f64;
    for p in search_space(cores) {
        let tps = match generate_timed(endpoint, model, prompt, p, num_predict, per_config_timeout_s) {
            Ok((_, st)) => st.tok_s(),
            Err(_) => 0.0,
        };
        trials.push((p, tps));
        if tps > best_tps {
            best_tps = tps;
            best = p;
        }
    }
    if best_tps < 0.0 {
        return Err("all tuning configs failed".into());
    }
    Ok(TuneResult { best, best_tok_s: best_tps, trials })
}

/// Per-model cache so production starts with the known-fast config.
pub fn cache_path(model: &str) -> std::path::PathBuf {
    let safe: String = model.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    std::path::PathBuf::from(format!("/tmp/flux-moe-tune-{safe}.json"))
}
pub fn save_best(model: &str, p: &TuneParams) {
    if let Ok(s) = serde_json::to_string(p) {
        let _ = std::fs::write(cache_path(model), s);
    }
}
pub fn load_best(model: &str) -> Option<TuneParams> {
    std::fs::read_to_string(cache_path(model)).ok().and_then(|s| serde_json::from_str(&s).ok())
}

/// DYNAMIC entry point: generate with the cached best config; if tok/s falls below `target`,
/// auto-tune, cache the new winner, and regenerate with it. The `bool` = whether a re-tune fired.
/// This is "flux tweaks the parameters automatically when it is too slow".
pub fn generate_dynamic(
    endpoint: &str,
    model: &str,
    prompt: &str,
    target_tok_s: f64,
    cores: u32,
) -> Result<(String, GenStats, bool), String> {
    let p = load_best(model).unwrap_or_else(TuneParams::cpu_default);
    let (text, st) = generate_timed(endpoint, model, prompt, p, 128, 600)?;
    if st.tok_s() >= target_tok_s {
        return Ok((text, st, false));
    }
    let tr = autotune(endpoint, model, cores, 48, 240)?;
    save_best(model, &tr.best);
    let (text2, st2) = generate_timed(endpoint, model, prompt, tr.best, 128, 600)?;
    Ok((text2, st2, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tok_s_math() {
        let g = GenStats { eval_count: 48, eval_ns: 12_000_000_000, wall_s: 13.0 };
        assert!((g.tok_s() - 4.0).abs() < 0.01); // 48 tokens / 12s = 4 tok/s
    }

    #[test]
    fn search_space_targets_the_swap_problem() {
        let s = search_space(48);
        assert!(s.len() >= 4);
        assert!(s.iter().any(|p| p.use_mlock), "must try use_mlock to beat swap-thrashing");
        assert!(s.iter().any(|p| p.num_ctx <= 1024), "must try a small KV cache");
        assert!(s.iter().all(|p| p.num_gpu == 0), "CPU box: never probe GPU offload");
    }

    #[test]
    fn options_carry_the_speed_levers() {
        let p = TuneParams { num_thread: 24, num_ctx: 1024, num_batch: 512, use_mlock: true, num_gpu: 0 };
        let o = p.to_options();
        assert_eq!(o["use_mlock"], true);
        assert_eq!(o["num_ctx"], 1024);
        assert_eq!(o["num_thread"], 24);
        // num_thread omitted when 0 (ollama auto)
        let auto = TuneParams { num_thread: 0, ..p };
        assert!(auto.to_options().get("num_thread").is_none());
    }
}
