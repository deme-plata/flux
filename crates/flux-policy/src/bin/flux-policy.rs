//! `flux-policy calibrate` — the operational face of the auto-calibrating
//! powerplant. Reads metrics (JSON array of `{name,value}`) from a file arg or
//! stdin, runs `Policy::standard().calibrate`, and prints:
//!   1. the audit trail (what changed + why),
//!   2. the env block the runtime SOURCES — adopt the fix WITHOUT recompiling.
//!
//! This is what the `flux_policy_calibrate` MCP tool + the webhook stream call
//! into (the MCP registration is the thin wire, in fluxc-mcp). Run:
//!   flux-policy calibrate metrics.json
//!   echo '[{"name":"blocks_applied","value":0}]' | flux-policy calibrate
//!   flux-policy calibrate --json metrics.json     # machine-readable
//!
//! Example (today's win, automatic):
//!   echo '[{"name":"blocks_applied","value":0}]' | flux-policy calibrate
//!   → changes: param mesh_n_low 4 → 1  (0 propagation → drop gossip mesh floor…)
//!   → export FLUX_GOSSIPSUB_MESH_N_LOW=1

use flux_policy::{Metric, Policy};
use std::io::Read;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let want_json = args.iter().any(|a| a == "--json");
    let file = args.iter().skip(1).find(|a| !a.starts_with("--") && *a != "calibrate");

    let raw = match file {
        Some(p) => std::fs::read_to_string(p).unwrap_or_else(|e| {
            eprintln!("flux-policy: cannot read {p}: {e}");
            std::process::exit(2);
        }),
        None => {
            let mut s = String::new();
            let _ = std::io::stdin().read_to_string(&mut s);
            s
        }
    };
    let metrics: Vec<Metric> = serde_json::from_str(raw.trim()).unwrap_or_else(|e| {
        eprintln!("flux-policy: metrics must be a JSON array of {{name,value}}: {e}");
        std::process::exit(2);
    });

    let mut policy = Policy::standard();
    let changes = policy.calibrate(&metrics);

    if want_json {
        println!(
            "{}",
            serde_json::json!({
                "changes": changes.iter().map(|c| serde_json::json!({
                    "kind": c.kind, "target": c.target, "from": c.from, "to": c.to, "why": c.why
                })).collect::<Vec<_>>(),
                "state": policy.to_json(),
                "env": policy.to_env(),
            })
        );
        return;
    }

    if changes.is_empty() {
        println!("flux-policy: no change (metrics within policy).");
    } else {
        println!("flux-policy: {} change(s) —", changes.len());
        for c in &changes {
            println!("  [{}] {} : {} → {}   ({})", c.kind, c.target, c.from, c.to, c.why);
        }
    }
    if let Some(e) = &policy.active_engine {
        println!("  engine: {e}");
    }
    println!("\n# source this to adopt WITHOUT recompiling:");
    println!("{}", policy.to_env());
}
