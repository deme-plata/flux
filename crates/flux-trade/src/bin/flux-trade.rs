//! flux-trade CLI — the DECIDE+SIZE brain (lifted from q-trading-bot, fed by Binance public data).
//!   flux-trade decide <SYMBOL> [interval] [limit]   live indicators → {action, reason, kelly}
//!   flux-trade price  <SYMBOL>                       Binance mark price
use flux_trade::{binance::Binance, decide};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let g = |i: usize| a.get(i).cloned().unwrap_or_default();
    match a.first().map(|s| s.as_str()) {
        Some("decide") => {
            let sym = if g(1).is_empty() { "BTCUSDT".to_string() } else { g(1) };
            let iv = if g(2).is_empty() { "1h".to_string() } else { g(2) };
            let lim: u32 = g(3).parse().unwrap_or(200);
            match decide(&sym, &iv, lim) {
                Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
                Err(e) => { eprintln!("✗ {e}"); std::process::exit(1); }
            }
        }
        Some("price") => match Binance::new().mark_price(&g(1)) {
            Ok(p) => println!("{} {}", g(1).to_uppercase(), p),
            Err(e) => { eprintln!("✗ {e}"); std::process::exit(1); }
        },
        _ => eprintln!("flux-trade — decide <SYMBOL> [interval] [limit] | price <SYMBOL>"),
    }
}
