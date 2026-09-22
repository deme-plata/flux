// groot-daemon — run the reference GR00T control daemon.
//
//   groot-daemon --port 9470 --operator qnk<64hex> [--operator qnk<64hex> ...]
//
// Operators may also come from GROOT_OPERATORS (comma-separated qnk addresses).
// A wallet on the operator list can move the robot by signing QRBT1 envelopes
// (e.g. via the quillon-wallet MCP's seed-derived key). Anyone can estop.

use flux_groot::{envelope::wallet_bytes, Daemon, DaemonConfig};
use std::collections::HashSet;

fn main() {
    let mut cfg = DaemonConfig { port: 9470, ..Default::default() };
    let mut operators: HashSet<[u8; 32]> = HashSet::new();

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                i += 1;
                cfg.port = args.get(i).and_then(|v| v.parse().ok()).unwrap_or_else(|| {
                    eprintln!("--port needs a number");
                    std::process::exit(2);
                });
            }
            "--operator" => {
                i += 1;
                let w = args.get(i).cloned().unwrap_or_default();
                match wallet_bytes(&w) {
                    Ok(b) => {
                        operators.insert(b);
                    }
                    Err(e) => {
                        eprintln!("bad --operator {w}: {e}");
                        std::process::exit(2);
                    }
                }
            }
            other => {
                eprintln!("unknown arg {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    if let Ok(env_ops) = std::env::var("GROOT_OPERATORS") {
        for w in env_ops.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match wallet_bytes(w) {
                Ok(b) => {
                    operators.insert(b);
                }
                Err(e) => eprintln!("skipping bad GROOT_OPERATORS entry {w}: {e}"),
            }
        }
    }

    if operators.is_empty() {
        eprintln!(
            "⚠ no operators configured — the robot will accept estop but refuse ALL motion.\n\
             Add --operator qnk<64hex> or set GROOT_OPERATORS."
        );
    }
    cfg.operators = operators;

    let d = Daemon::spawn(cfg).unwrap_or_else(|e| {
        eprintln!("bind failed: {e}");
        std::process::exit(1);
    });
    println!("groot-daemon listening on {}", d.base_url());
    println!("  POST /v1/robot/command   (QRBT1 wallet-signed)");
    println!("  POST /v1/robot/estop     (unsigned — safety is never gated)");
    println!("  GET  /v1/robot/state | /v1/robot/receipts | /v1/robot/telemetry (SSE)");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
