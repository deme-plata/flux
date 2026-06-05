//! flux-0x CLI — drive the 0x engine.
//!   flux-0x endpoints                              list the flux-api spec
//!   flux-0x openapi                                emit the OpenAPI 3.1 doc (flux-api generated)
//!   flux-0x price  <chainId> <sell> <buy> <amt>    indicative Swap price
//!   flux-0x quote  <chainId> <sell> <buy> <amt> <taker>   firm Swap quote
//!   flux-0x xquote <oChain> <dChain> <oTok> <dTok> <amt> <taker> <destAddr> [price|speed]   cross-chain quotes
//!       (destAddr = recipient on destination chain; required for Solana etc., use "" for EVM→EVM)
//!   flux-0x status <oChain> <txHash> [quoteId]     track a cross-chain trade
//!   flux-0x health                                 liveness probe (key + connectivity)
use flux_0x::{zerox_spec, Zerox};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let g = |i: usize| a.get(i).cloned().unwrap_or_default();
    let pj = |r: Result<serde_json::Value, String>| match r {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
        Err(e) => { eprintln!("✗ {e}"); std::process::exit(1); }
    };
    match a.first().map(|s| s.as_str()) {
        Some("endpoints") => for e in zerox_spec() { println!("{:<6?} {:<32} {}", e.method, e.path, e.summary); },
        Some("openapi") => println!("{}", flux_api::generate_openapi("0x Flux Engine", "2.0.0", &zerox_spec())),
        Some("health") => pj(Zerox::from_env().and_then(|z| z.health())),
        Some("price") => pj(Zerox::from_env().and_then(|z| z.swap_price(g(1).parse().unwrap_or(1), &g(2), &g(3), &g(4)))),
        Some("quote") => pj(Zerox::from_env().and_then(|z| z.swap_quote(g(1).parse().unwrap_or(1), &g(2), &g(3), &g(4), &g(5)))),
        Some("xquote") => pj(Zerox::from_env().and_then(|z| z.crosschain_quotes(g(1).parse().unwrap_or(1), g(2).parse().unwrap_or(137), &g(3), &g(4), &g(5), &g(6), &g(7), &g(8)))),
        Some("status") => pj(Zerox::from_env().and_then(|z| z.crosschain_status(g(1).parse().unwrap_or(1), &g(2), &g(3)))),
        Some("sources") => pj(Zerox::from_env().and_then(|z| z.sources(g(1).parse().unwrap_or(1)))),
        _ => eprintln!("flux-0x — 0x engine · endpoints|openapi|price|quote|xquote|status|sources"),
    }
}
