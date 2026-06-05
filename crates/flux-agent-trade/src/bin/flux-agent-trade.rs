//! flux-agent-trade CLI — the flagship dry-run combo (decide → gate → 0x → settle-plan).
//!   flux-agent-trade <SYMBOL> [usd_amount] [interval]   e.g. ETHUSDT 250 1h
//! Read-only end-to-end. Nothing is signed or broadcast.
use flux_agent_trade::{propose, Gate};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let symbol = a.first().cloned().unwrap_or_else(|| "ETHUSDT".into());
    let usd: f64 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(250.0);
    let interval = a.get(2).cloned().unwrap_or_else(|| "1h".into());
    let out = propose(&symbol, usd, &interval, &Gate::default());
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
}
