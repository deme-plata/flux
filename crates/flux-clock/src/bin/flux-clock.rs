//! flux-clock — the SIGIL datacenter clock on the command line.
//!
//!   flux-clock read   [--node URL] [--earth URL] [--energy J] [--sample SECS] [--webhook]
//!   flux-clock crt    [--periods 2,3,5,7,11] [--z 5] [--trials 1000] [--seed N]   (defaults = the paper's Fig. 2)
//!   flux-clock hand   --period P [--unit block|second] [--scale N] [--node URL] [--listen MADDR] [--peer MADDR]... [--ticks N] [--interval S] [--name ID]
//!                     --scale N reads the height in units of N blocks (unit "block/N"): at 8 blk/s the
//!                     nodes' views differ by ~1 block of propagation lag, which breaks the paper's ¼-unit
//!                     budget for unit=block but is 0.001 of a kiloblock.
//!   flux-clock collect [--listen MADDR] [--peer MADDR]... [--secs S] [--bound N] [--unit block|second] [--name ID] [--webhook]
//!
//! Webhook: `--webhook` dispatches the reading as the `flux_clock` event to every registered
//! fluxc webhook (same registry as `fluxc webhook`).

use flux_clock::{crt, face};
use serde_json::{json, Value};
use std::time::Duration;

fn arg(args: &[String], k: &str) -> Option<String> { args.iter().position(|a| a == k).and_then(|i| args.get(i + 1).cloned()) }
fn args_all(args: &[String], k: &str) -> Vec<String> { args.iter().enumerate().filter(|(_, a)| *a == k).filter_map(|(i, _)| args.get(i + 1).cloned()).collect() }
fn has(args: &[String], k: &str) -> bool { args.iter().any(|a| a == k) }

use flux_clock::live::{get_json, height, live_inputs};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().cloned().unwrap_or_else(|| "help".into());
    let node = arg(&args, "--node").unwrap_or_else(|| std::env::var("FLUX_SIGIL_RPC").unwrap_or_else(|_| "http://127.0.0.1:18181".into()));
    let earth_url = arg(&args, "--earth").unwrap_or_else(|| std::env::var("FLUX_SIGIL_EARTH").unwrap_or_else(|_| "http://127.0.0.1:8460".into()));
    match cmd.as_str() {
        "read" => {
            let energy = arg(&args, "--energy").and_then(|s| s.parse().ok()).unwrap_or(flux_clock::kristensen::E_CORE_15W_1S_J);
            let sample = arg(&args, "--sample").and_then(|s| s.parse().ok()).unwrap_or(5.0);
            match live_inputs(&node, &earth_url, energy, sample) {
                Ok(i) => { let v = face::read(&i); if has(&args, "--webhook") { fluxc_webhooks::webhook::auto_dispatch("flux_clock", v.clone()); } println!("{}", serde_json::to_string_pretty(&v).unwrap()); }
                Err(e) => { eprintln!("read failed: {e}"); std::process::exit(1); }
            }
        }
        "crt" => {
            let periods: Vec<u64> = arg(&args, "--periods").map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect()).unwrap_or_else(|| crt::FIGURE2_PERIODS.to_vec());
            let trials: u32 = arg(&args, "--trials").and_then(|s| s.parse().ok()).unwrap_or(1000);
            let seed: u64 = arg(&args, "--seed").and_then(|s| s.parse().ok()).unwrap_or(20_260_811);
            let reports: Vec<crt::SimulationReport> = match arg(&args, "--z").and_then(|s| s.parse::<u32>().ok()) {
                Some(z) => vec![crt::simulate(&periods, z, trials, seed)],
                None => crt::FIGURE2_Z.iter().map(|&z| crt::simulate(&periods, z, trials, seed ^ z as u64)).collect(),
            };
            println!("{}", serde_json::to_string_pretty(&json!({"paper": "arXiv:2608.07938 Fig. 2", "paper_p_err_lt_1": crt::FIGURE2_P, "z_min_99pct": crt::z_min(periods.len(), 0.99), "reports": reports})).unwrap());
        }
        "hand" | "collect" => {
            let name = arg(&args, "--name").unwrap_or_else(|| format!("flux-clock-{}-{}", cmd, std::process::id()));
            let listen = arg(&args, "--listen").unwrap_or_else(|| "/ip4/0.0.0.0/tcp/0".into());
            let peers = args_all(&args, "--peer");
            let unit = arg(&args, "--unit").unwrap_or_else(|| "block".into());
            let webhook = has(&args, "--webhook");
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let nm = match flux_clock::p2p::net::start(flux_clock::p2p::net::config(&name, &listen, peers)).await { Ok(n) => n, Err(e) => { eprintln!("p2p start failed: {e}"); std::process::exit(1) } };
                eprintln!("peer id {} listening {listen}", flux_clock::p2p::net::peer_id_of(&name));
                if cmd == "hand" {
                    let period: u64 = arg(&args, "--period").and_then(|s| s.parse().ok()).unwrap_or_else(|| { eprintln!("--period required"); std::process::exit(2) });
                    let ticks: u32 = arg(&args, "--ticks").and_then(|s| s.parse().ok()).unwrap_or(60);
                    let interval = Duration::from_secs_f64(arg(&args, "--interval").and_then(|s| s.parse().ok()).unwrap_or(2.0));
                    let ut1 = get_json(&format!("{earth_url}/v1/earth/latest")).ok().and_then(|e| e["now"]["ut1utc_today"].as_f64()).unwrap_or(0.0);
                    let scale: f64 = arg(&args, "--scale").and_then(|s| s.parse().ok()).filter(|s: &f64| *s >= 1.0).unwrap_or(1.0);
                    let unit_label = if unit == "block" && scale != 1.0 { format!("block/{scale}") } else { unit.clone() };
                    let node2 = node.clone();
                    let unit2 = unit.clone();
                    let read = move || -> Option<f64> { if unit2 == "block" { height(&node2).map(|h| h as f64 / scale) } else { Some(flux_clock::now_unix() + ut1) } };
                    let sent = flux_clock::p2p::net::run_hand(&nm, &name, &unit_label, period, 1, ticks, interval, read).await;
                    println!("{}", serde_json::to_string_pretty(&json!({"published": sent.len(), "period": period, "unit": unit_label, "last": sent.last()})).unwrap());
                } else {
                    let secs: f64 = arg(&args, "--secs").and_then(|s| s.parse().ok()).unwrap_or(30.0);
                    let bound: u128 = arg(&args, "--bound").and_then(|s| s.parse().ok()).unwrap_or(100_000_000);
                    let col = flux_clock::p2p::net::collect(&nm, &unit, secs).await;
                    let r = col.reading(bound);
                    let local = if unit.starts_with("block") { height(&node) } else { None };
                    let scale: f64 = unit.split('/').nth(1).and_then(|s| s.parse().ok()).unwrap_or(1.0);
                    let v = json!({"event": "flux_clock", "kind": "decentralized", "peers_connected": nm.peer_count(), "reading": r, "local_for_comparison": if unit.starts_with("block") { local.map(|h| h as f64 / scale) } else { Some(flux_clock::now_unix()) }, "local_height": local});
                    if webhook { fluxc_webhooks::webhook::auto_dispatch("flux_clock", v.clone()); }
                    println!("{}", serde_json::to_string_pretty(&v).unwrap());
                }
                let _ = nm.stop().await;
            });
        }
        _ => { eprintln!("usage: flux-clock read|crt|hand|collect (see --help in source header)"); std::process::exit(2); }
    }
}
