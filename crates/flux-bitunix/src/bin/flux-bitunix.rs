//! flux-bitunix CLI — market data, account, and GATED order placement.
//! Every write needs `--confirm`; without it you get the exact JSON that would be sent.

use flux_bitunix::{Bitunix, Gate, Verdict, qty_from_notional, bitunix_spec};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let cmd = a.first().map(|s| s.as_str()).unwrap_or("help");
    let get = |k: &str| -> Option<String> {
        a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned()
    };
    let confirm = a.iter().any(|x| x == "--confirm");
    let pp = |r: Result<serde_json::Value, String>| match r {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
        Err(e) => { eprintln!("✗ {e}"); std::process::exit(1); }
    };

    match cmd {
        "serve" => {
            use flux_bitunix::bridge::{serve_with, Bind};
            let port: u16 = get("--port").and_then(|s| s.parse().ok()).unwrap_or(8477);
            let token = get("--token").or_else(|| std::env::var("FLUX_BITUNIX_TOKEN").ok())
                .map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            let bind = match get("--bind").as_deref() {
                Some("all") | Some("0.0.0.0") | Some("public") => Bind::All,
                _ => Bind::Loopback,
            };
            if let Err(e) = serve_with(port, bind, token) { eprintln!("✗ {e}"); std::process::exit(1); }
        }
        "bybit" => {
            use flux_bitunix::bybit::Bybit;
            let sub = a.get(1).map(|s| s.as_str()).unwrap_or("balance");
            let c = get("--category").unwrap_or_else(|| "linear".into());
            let sym = get("--symbol").unwrap_or_default();
            pp(match sub {
                "time" => Bybit::public().and_then(|b| b.server_time_skew_ms())
                    .map(|s| serde_json::json!({"local_minus_bybit_ms": s,
                        "ok_for_signing": s.abs() < 5000})),
                "tickers" => Bybit::public().and_then(|b| b.tickers(&c, &sym)),
                "kline" => Bybit::public().and_then(|b| b.kline(&c, &sym,
                    &get("--interval").unwrap_or_else(|| "60".into()),
                    get("--limit").and_then(|s| s.parse().ok()).unwrap_or(50))),
                "book" => Bybit::public().and_then(|b| b.orderbook(&c, &sym, 25)),
                "positions" => Bybit::from_env().and_then(|b| b.positions(&c, &sym)),
                "orders" => Bybit::from_env().and_then(|b| b.open_orders(&c, &sym)),
                "whoami" => Bybit::from_env().map(|b| serde_json::json!({"key": b.key_fingerprint()})),
                _ => Bybit::from_env().and_then(|b| b.wallet_balance(&get("--account").unwrap_or_default())),
            })
        }
        "arb" => {
            let syms: Vec<String> = get("--symbols")
                .unwrap_or_else(|| "BTCUSDT,ETHUSDT,SOLUSDT,XRPUSDT,DOGEUSDT".into())
                .split(',').map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty()).collect();
            pp(Ok(flux_bitunix::arb::scan(&syms)))
        }
        "sigil-exit" => {
            pp(flux_bitunix::pool::read_pool(flux_bitunix::pool::PAIR_WSIGIL3).map(|p| {
                flux_bitunix::arb::sigil_sale_capacity(p.usdc(), p.wsigil(),
                    get("--max-impact").and_then(|s| s.parse().ok()).unwrap_or(1.0))
            }))
        }
        "spec" => println!("{}", serde_json::to_string_pretty(&bitunix_spec()).unwrap_or_default()),
        "whoami" => match Bitunix::from_env() {
            Ok(b) => println!("key loaded: {}", b.key_fingerprint()),
            Err(e) => { eprintln!("✗ {e}"); std::process::exit(1); }
        },
        "tickers" => pp(Bitunix::public().and_then(|b| b.tickers(&get("--symbols").unwrap_or_default()))),
        "kline" => pp(Bitunix::public().and_then(|b| b.kline(
            &get("--symbol").unwrap_or_else(|| "BTCUSDT".into()),
            &get("--interval").unwrap_or_else(|| "1h".into()),
            get("--limit").and_then(|s| s.parse().ok()).unwrap_or(100)))),
        "depth" => pp(Bitunix::public().and_then(|b| b.depth(
            &get("--symbol").unwrap_or_else(|| "BTCUSDT".into()),
            &get("--limit").unwrap_or_else(|| "50".into())))),
        "pairs" => pp(Bitunix::public().and_then(|b| b.trading_pairs(&get("--symbols").unwrap_or_default()))),
        "account" => pp(Bitunix::from_env().and_then(|b| b.account(&get("--coin").unwrap_or_else(|| "USDT".into())))),
        "positions" => pp(Bitunix::from_env().and_then(|b| b.positions(&get("--symbol").unwrap_or_default()))),
        "orders" => pp(Bitunix::from_env().and_then(|b| b.pending_orders(
            &get("--symbol").unwrap_or_default(),
            get("--limit").and_then(|s| s.parse().ok()).unwrap_or(10)))),
        "history" => pp(Bitunix::from_env().and_then(|b| b.history_orders(
            &get("--symbol").unwrap_or_default(),
            get("--limit").and_then(|s| s.parse().ok()).unwrap_or(10)))),
        "order" => {
            let symbol = get("--symbol").unwrap_or_default();
            let side = get("--side").unwrap_or_default();
            let usd: f64 = get("--usd").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let lev: u32 = get("--leverage").and_then(|s| s.parse().ok()).unwrap_or(1);
            let otype = get("--type").unwrap_or_else(|| "MARKET".into());
            let price = get("--price");
            let r = (|| -> Result<serde_json::Value, String> {
                let b = Bitunix::from_env()?;
                let mark = b.last_price(&symbol)?;
                let gate = Gate::default();
                let plimit = price.as_ref().and_then(|p| p.parse::<f64>().ok());
                match gate.check(&symbol, &side, usd, lev, plimit, Some(mark)) {
                    Verdict::Reject(why) => return Err(format!("GATE REJECTED: {why}")),
                    Verdict::Pass => {}
                }
                let qty = qty_from_notional(usd, plimit.unwrap_or(mark), 4)?;
                let order = Bitunix::build_order(&symbol, &side, &otype, &qty,
                    price.as_deref(), Some("GTC"), false, None,
                    get("--tp").as_deref(), get("--sl").as_deref());
                b.place_order(&order, confirm)
            })();
            pp(r)
        }
        "cancel" => {
            let symbol = get("--symbol").unwrap_or_default();
            let ids: Vec<String> = get("--ids").map(|s| s.split(',').map(|x| x.trim().to_string()).collect()).unwrap_or_default();
            pp(Bitunix::from_env().and_then(|b| if ids.is_empty() { b.cancel_all(&symbol, confirm) }
                                                 else { b.cancel_orders(&symbol, &ids, confirm) }))
        }
        _ => {
            eprintln!("flux-bitunix — Bitunix Futures, the flux way\n");
            eprintln!("  public : tickers --symbols BTCUSDT | kline --symbol .. --interval 1h | depth | pairs");
            eprintln!("  signed : whoami | account [--coin USDT] | positions | orders | history");
            eprintln!("  trade  : order --symbol BTCUSDT --side BUY --usd 25 --leverage 2 [--type LIMIT --price ..] [--tp ..] [--sl ..]");
            eprintln!("           cancel --symbol BTCUSDT [--ids id1,id2]");
            eprintln!("\n  Writes are PROPOSE-ONLY unless you add --confirm. The gate caps symbol, notional and leverage.");
        }
    }
}
