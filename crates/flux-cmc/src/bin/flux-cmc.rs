//! flux-cmc CLI — drive the CoinMarketCap DECIDE engine.
//!   flux-cmc endpoints                  list the flux-api spec
//!   flux-cmc openapi                    emit the OpenAPI 3.1 doc
//!   flux-cmc quote  <SYM[,SYM...]>      latest price + % changes
//!   flux-cmc signal <SYM>               boiled verdict (bullish/bearish/neutral + why)
//!   flux-cmc global                     total cap / BTC dominance / volume
//!   flux-cmc movers [limit] [sort]      top movers
//!   flux-cmc health                     key plan + credits (liveness)
use flux_cmc::{cmc_spec, Cmc};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let g = |i: usize| a.get(i).cloned().unwrap_or_default();
    let pj = |r: Result<serde_json::Value, String>| match r {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
        Err(e) => { eprintln!("✗ {e}"); std::process::exit(1); }
    };
    match a.first().map(|s| s.as_str()) {
        Some("endpoints") => for e in cmc_spec() { println!("{:<6?} {:<40} {}", e.method, e.path, e.summary); },
        Some("openapi") => println!("{}", flux_api::generate_openapi("CMC Flux Engine", "1.0.0", &cmc_spec())),
        Some("quote") => pj(Cmc::from_env().and_then(|c| c.quote(&g(1)))),
        Some("signal") => pj(Cmc::from_env().and_then(|c| c.signal(&g(1)))),
        Some("global") => pj(Cmc::from_env().and_then(|c| c.global())),
        Some("movers") => pj(Cmc::from_env().and_then(|c| c.movers(g(1).parse().unwrap_or(10), &g(2)))),
        Some("health") => pj(Cmc::from_env().and_then(|c| c.health())),
        _ => eprintln!("flux-cmc — CMC decide engine · endpoints|openapi|quote|signal|global|movers|health"),
    }
}
