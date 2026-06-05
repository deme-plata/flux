//! flux-cowsim CLI — run the headless virtual-fence / cow-haptic ABM on CPU, emit a JSON timeseries.
//!   flux-cowsim [--n N] [--steps N] [--sample K] [--warn W] [--sound S] [--haptic H] [--seed K]
//!   → {params, summary:{shocks_total, mean_containment, welfare_score, …}, series:[HerdReport]}
//! This is the real CPU backend a /capi endpoint streams into the browser cowherd viz.
use flux_cowsim::{CowParams, Herd};

fn argf(a: &[String], f: &str, d: f64) -> f64 {
    a.iter().position(|x| x == f).and_then(|i| a.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(d)
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let steps = argf(&a, "--steps", 3000.0) as u32;
    let sample = argf(&a, "--sample", 100.0) as u32;
    let mut p = CowParams::default();
    p.n = argf(&a, "--n", p.n as f64) as usize;
    p.warn_band = argf(&a, "--warn", p.warn_band);
    p.sound_push = argf(&a, "--sound", p.sound_push);
    p.haptic_push = argf(&a, "--haptic", p.haptic_push);
    p.seed = argf(&a, "--seed", p.seed as f64) as u64;

    let t0 = std::time::Instant::now();
    let mut h = Herd::new(p);
    let (series, summary) = h.run(steps, sample);
    let ms = t0.elapsed().as_secs_f64() * 1000.0;

    let out = serde_json::json!({
        "ok": true,
        "engine": "flux-cowsim (Boids + virtual-fence collar state machine, headless CPU)",
        "host": "epsilon-cpu",
        "params": p,
        "wall_ms": (ms * 1000.0).round() / 1000.0,
        "summary": summary,
        "series": series,
    });
    println!("{}", serde_json::to_string(&out).unwrap_or_default());
}
