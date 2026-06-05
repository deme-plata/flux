//! flux-biosim CLI — run the headless bioreactor / lab-meat sim on CPU, emit a JSON timeseries.
//!   flux-biosim [--steps N] [--sample K] [--o2 0..1] [--feed 0..1] [--perfusion 0..1] [--lactate-yield Y]
//!   → {params, final:{cells,lactate,grams,…}, net_yield, series:[BioReport]}
//! This is the real CPU backend a /capi endpoint streams into the browser bioreactor viz.
use flux_biosim::{net_yield, Params, Reactor};

fn argf(a: &[String], f: &str, d: f64) -> f64 {
    a.iter().position(|x| x == f).and_then(|i| a.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(d)
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let steps = argf(&a, "--steps", 4000.0) as u32;
    let sample = argf(&a, "--sample", 100.0) as u32;
    let mut p = Params::default();
    p.o2 = argf(&a, "--o2", p.o2);
    p.feed = argf(&a, "--feed", p.feed);
    p.perfusion = argf(&a, "--perfusion", p.perfusion);
    p.lactate_yield = argf(&a, "--lactate-yield", p.lactate_yield);

    let t0 = std::time::Instant::now();
    let mut r = Reactor::new(p, 2.0e4);
    let series = r.run(steps, sample);
    let ms = t0.elapsed().as_secs_f64() * 1000.0;

    let out = serde_json::json!({
        "ok": true,
        "engine": "flux-biosim (lactate-inhibited Monod chemostat + perfusion, headless CPU)",
        "host": "epsilon-cpu",
        "params": p,
        "steps": steps,
        "wall_ms": (ms * 1000.0).round() / 1000.0,
        "final": r.report(),
        "net_yield": net_yield(p, steps),
        "series": series,
    });
    println!("{}", serde_json::to_string(&out).unwrap_or_default());
}
