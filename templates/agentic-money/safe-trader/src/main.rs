//! safe-trader — a DEX swap loop where EVERY action passes the Verified
//! Execution Gate before it touches the chain.
//!
//! This is the canonical agentic-money skeleton: read state → propose →
//! **gate** → execute → measure. The "propose" here is a deliberately dumb
//! mean-reversion heuristic so the gate is the star of the show. Fork this and
//! replace `propose()` with your real strategy (or wire in `llm-trader`'s LLM
//! decide) — the gate stays exactly where it is.
//!
//! Run:  safe-trader [RPC_URL] [N_SWAPS]
//!   e.g. safe-trader http://127.0.0.1:8099 8

use agentic_money_kit::gate::{evaluate, Decision, GateConfig, Verdict};
use agentic_money_kit::Rpc;

// Demo wallet + pool + token ids on a local sigil-rpcd (the seeded trader).
const TRADER: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const POOL: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const USDS: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const WQUG: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn main() {
    let mut args = std::env::args().skip(1);
    let rpc_url = args.next().unwrap_or_else(|| "http://127.0.0.1:8099".into());
    let n: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(8);

    let rpc = Rpc::new(&rpc_url);
    let cfg = GateConfig::default(); // whitelist AtoB/BtoA, clamp 100..2000, 3% slippage

    println!("safe-trader → {rpc_url}  ({n} swaps, gate policy: {cfg:?})");
    let (mut executed, mut rejected, mut volume) = (0u32, 0u32, 0u128);

    for i in 1..=n {
        let (ra, rb) = match rpc.pool_reserves(0) {
            Some(r) => r,
            None => {
                eprintln!("[{i}] no pool — is sigil-rpcd up at {rpc_url}? skipping");
                continue;
            }
        };
        let usds = rpc.balance(TRADER, USDS);
        let wqug = rpc.balance(TRADER, WQUG);

        let decision = propose(ra, rb, usds, wqug);
        // input side reserves/balance depend on direction
        let (bal_in, res_in, res_out) = if decision.dir == "AtoB" {
            (usds, ra, rb)
        } else {
            (wqug, rb, ra)
        };

        match evaluate(&cfg, &decision, bal_in, res_in, res_out) {
            Verdict::Reject(reason) => {
                rejected += 1;
                println!("[{i}] 🚫 GATE reject: {reason}");
            }
            Verdict::Approve(d) => {
                match rpc.swap(TRADER, POOL, &d.dir, d.amount_in, 1) {
                    Ok(resp) if resp.contains("\"ok\":true") => {
                        executed += 1;
                        volume += d.amount_in;
                        println!("[{i}] ✅ {} {} → on-chain ok", d.dir, d.amount_in);
                    }
                    Ok(resp) => println!("[{i}] ⚠️ swap returned: {}", resp.trim()),
                    Err(e) => println!("[{i}] ⚠️ transport error: {e}"),
                }
            }
        }
    }

    println!("\n── result ──\nexecuted={executed}  gate_rejected={rejected}  volume_in={volume}");
}

/// Dumb mean-reversion: if the pool is rich in wQUG (price of wQUG low), buy
/// wQUG (AtoB = spend USDS); else sell. Size scales with imbalance, clamped by
/// the gate. REPLACE THIS with your strategy — the gate doesn't care how the
/// proposal was made, only that it's safe.
fn propose(reserve_a_usds: u128, reserve_b_wqug: u128, _usds: u128, _wqug: u128) -> Decision {
    if reserve_b_wqug >= reserve_a_usds {
        Decision { dir: "AtoB".into(), amount_in: 500, token_in: "USDS".into(), token_out: "WQUG".into() }
    } else {
        Decision { dir: "BtoA".into(), amount_in: 500, token_in: "WQUG".into(), token_out: "USDS".into() }
    }
}
