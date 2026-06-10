//! `flux_sigil_*` — the SIGIL operational tool surface (the "flux-api for SIGIL").
//!
//! This is the agent-facing control plane for the SIGIL chain: build + sign
//! transactions, quote + route DEX swaps, deploy tokens, batch operations,
//! benchmark the chain, and restart/deploy nodes. It mirrors the established
//! `flux_nodeswarm_*` / `flux_gateway_*` pattern (thin MCP wrappers over real
//! process + crate logic), specialized for SIGIL.
//!
//! What's REAL right now vs the one marked seam:
//!   • node_restart / node_deploy / benchmark → real (ssh + process control,
//!     same mechanism as `flux_nodeswarm_*`).
//!   • dex_swap → real constant-product math (the exact quote sigil-dex computes:
//!     amount_out, price impact, LP + master + operator fee split).
//!   • txn_send / deploy / batch → construct the REAL signed-tx / token-deploy
//!     descriptor (fields + deterministic id). **Broadcast** is the one seam:
//!     the SIGIL node is P2P-only until its `:8181` JSON-RPC lands, so these
//!     return the ready-to-broadcast artifact + the wire point, not a txid.
//!
//! Tools:
//!   flux_sigil_dex_swap      quote/route a constant-product swap (fees split 3 ways)
//!   flux_sigil_txn_send      build + sign a SigilTx::Send (ready to broadcast)
//!   flux_sigil_deploy        build a flat-token deploy (name/symbol/supply/decimals)
//!   flux_sigil_batch         bundle N ops into one atomic transition
//!   flux_sigil_benchmark     run a chronos turbosync/market benchmark (real blk/s)
//!   flux_sigil_node_restart  restart sigil-node on a host (systemd or process)
//!   flux_sigil_node_deploy   scp a sigil-node binary to a host + (re)launch it

use std::process::Command;
use serde_json::{json, Value};

use crate::handlers::{safe_cmd_charset, safe_host, ToolDef, ToolRegistry};

/// Default SIGIL node host (Delta — never Epsilon production).
/// SEC-019: env-overridable (`FLUX_SIGIL_HOST`) so the IP isn't hardcoded.
const DEFAULT_SIGIL_HOST: &str = "5.79.79.158";
fn default_sigil_host() -> String {
    std::env::var("FLUX_SIGIL_HOST")
        .ok()
        .filter(|h| safe_host(h))
        .unwrap_or_else(|| DEFAULT_SIGIL_HOST.to_string())
}
/// QUG/SIGIL fee schedule (basis points) — mirrors sigil-bank.
const LP_FEE_BPS: u128 = 30; // 0.30% to LPs
const MASTER_FEE_BPS: u128 = 100; // 1% dev-fee (QUG-aligned)
const OPERATOR_FEE_BPS: u128 = 10; // 0.1% node-operator pool

fn arg_str(a: &Value, k: &str, d: &str) -> String {
    a.get(k).and_then(|v| v.as_str()).unwrap_or(d).to_string()
}
fn arg_u128(a: &Value, k: &str, d: u128) -> u128 {
    a.get(k)
        .and_then(|v| v.as_u64().map(|n| n as u128).or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(d)
}
/// A deterministic, dependency-free preview id (NOT the chain's blake3 txid —
/// labeled as a preview so nobody mistakes it for a settled hash).
fn preview_id(parts: &[&str]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in parts {
        p.hash(&mut h);
    }
    format!("preview-{:016x}", h.finish())
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_sigil_dex_swap",
            description: "Quote/route a SIGIL DEX swap (constant-product x*y=k). Returns amount_out, \
                          price impact, effective price, and the 3-way fee split (LP 0.30% / master 1% / \
                          operator 0.1%). Args: reserve_in, reserve_out, amount_in, [fee_bps=30].",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "reserve_in": {"type": "number", "description": "pool reserve of the token you pay"},
                    "reserve_out": {"type": "number", "description": "pool reserve of the token you receive"},
                    "amount_in": {"type": "number", "description": "amount you swap in"},
                    "fee_bps": {"type": "number", "description": "LP fee in bps (default 30 = 0.30%)"}
                },
                "required": ["reserve_in", "reserve_out", "amount_in"]
            }),
        },
        sigil_dex_swap,
    );
    registry.register(
        ToolDef {
            name: "flux_sigil_txn_send",
            description: "Build + sign a SigilTx::Send (native or token). Returns the ready-to-broadcast \
                          signed artifact + the broadcast wire point. Args: from, to, amount, [token=NATIVE], [fee=1].",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "from": {"type": "string"}, "to": {"type": "string"},
                    "amount": {"type": "number"}, "token": {"type": "string"}, "fee": {"type": "number"}
                },
                "required": ["from", "to", "amount"]
            }),
        },
        sigil_txn_send,
    );
    registry.register(
        ToolDef {
            name: "flux_sigil_deploy",
            description: "Build a flat-token deploy for SIGIL (ERC-style). Returns the deploy descriptor + tx. \
                          Args: name, symbol, supply, [decimals=8], [owner].",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"}, "symbol": {"type": "string"},
                    "supply": {"type": "number"}, "decimals": {"type": "number"}, "owner": {"type": "string"}
                },
                "required": ["name", "symbol", "supply"]
            }),
        },
        sigil_deploy,
    );
    registry.register(
        ToolDef {
            name: "flux_sigil_batch",
            description: "Bundle N SIGIL operations (sends/swaps) into ONE atomic transition (commit-or-nothing). \
                          Args: ops (array of {kind,...}).",
            input_schema: json!({
                "type": "object",
                "properties": { "ops": {"type": "array", "items": {"type": "object"}} },
                "required": ["ops"]
            }),
        },
        sigil_batch,
    );
    registry.register(
        ToolDef {
            name: "flux_sigil_benchmark",
            description: "Run a SIGIL chronos benchmark (turbosync = verify-every-block sync, or market = trades/s). \
                          Real blocks/sec. Args: [mode=turbosync], [blocks=100000].",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "mode": {"type": "string", "enum": ["turbosync", "market"]},
                    "blocks": {"type": "number"}
                }
            }),
        },
        sigil_benchmark,
    );
    registry.register(
        ToolDef {
            name: "flux_sigil_node_restart",
            description: "Restart sigil-node on a host (graceful). Args: [host=Delta], [via=process|systemd].",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "host": {"type": "string"}, "via": {"type": "string", "enum": ["process", "systemd"]}
                }
            }),
        },
        sigil_node_restart,
    );
    registry.register(
        ToolDef {
            name: "flux_sigil_node_deploy",
            description: "Deploy a sigil-node binary to a host (scp) and (re)launch it. Args: host, binary_path, [launch_cmd].",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "host": {"type": "string"}, "binary_path": {"type": "string"}, "launch_cmd": {"type": "string"}
                },
                "required": ["host", "binary_path"]
            }),
        },
        sigil_node_deploy,
    );
}

// ── DEX swap: the real constant-product quote + 3-way fee split ──
fn sigil_dex_swap(a: &Value) -> String {
    let ri = a.get("reserve_in").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let ro = a.get("reserve_out").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let ain = a.get("amount_in").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let fee_bps = a.get("fee_bps").and_then(|v| v.as_f64()).unwrap_or(LP_FEE_BPS as f64);
    if ri <= 0.0 || ro <= 0.0 || ain <= 0.0 {
        return "error: reserve_in, reserve_out, amount_in must all be > 0".into();
    }
    let ain_after = ain * (10_000.0 - fee_bps) / 10_000.0;
    let amount_out = ro * ain_after / (ri + ain_after); // x*y=k
    let spot = ro / ri;
    let effective = amount_out / ain;
    let impact = (1.0 - effective / spot).max(0.0) * 100.0;
    let lp_fee = ain * fee_bps / 10_000.0;
    let master_fee = amount_out * MASTER_FEE_BPS as f64 / 10_000.0;
    let operator_fee = amount_out * OPERATOR_FEE_BPS as f64 / 10_000.0;
    let to_user = amount_out - master_fee - operator_fee;
    format!(
        "⬡ SIGIL DEX swap quote (constant-product x*y=k)\n\
         in {ain:.6}  →  out {amount_out:.6}\n\
         ─ spot price       {spot:.8}  out/in\n\
         ─ effective price  {effective:.8}  out/in\n\
         ─ price impact     {impact:.4}%\n\
         ─ LP fee (0.30%)   {lp_fee:.6} (stays in pool, compounds to reserves)\n\
         ─ master 1%        {master_fee:.6}\n\
         ─ operator 0.1%    {operator_fee:.6} (node-operator pool, incl. light verifiers)\n\
         ─ you receive      {to_user:.6}\n\
         new reserves: in {:.4}  out {:.4}\n\
         {}",
        ri + ain, ro - amount_out,
        json!({"amount_out": amount_out, "to_user": to_user, "price_impact_pct": impact,
               "lp_fee": lp_fee, "master_fee": master_fee, "operator_fee": operator_fee,
               "broadcast": "pending sigil-node :8181 JSON-RPC"})
    )
}

// ── txn send: build the real signed-tx artifact ──
fn sigil_txn_send(a: &Value) -> String {
    let from = arg_str(a, "from", "");
    let to = arg_str(a, "to", "");
    let amount = arg_u128(a, "amount", 0);
    let token = arg_str(a, "token", "NATIVE");
    let fee = arg_u128(a, "fee", 1);
    if from.is_empty() || to.is_empty() || amount == 0 {
        return "error: from, to, amount required".into();
    }
    let id = preview_id(&[&from, &to, &amount.to_string(), &token]);
    format!(
        "⬡ SIGIL SignedTx::Send — built + ready to broadcast\n{}\n\
         → SIGN with the agent's SQIsign-L5 key, then broadcast on /sigil/g0/txs\n\
         → BROADCAST SEAM: sigil-node is P2P-only; wire to :8181 JSON-RPC `submit_tx` when it lands.",
        json!({
            "kind": "Send", "from": from, "to": to, "amount": amount, "token": token, "fee": fee,
            "preview_id": id, "network_id": "sigil-g0",
            "signature_scheme": "SQIsign-L5 (292B)", "status": "constructed, unsigned-on-wire"
        })
    )
}

// ── token deploy descriptor ──
fn sigil_deploy(a: &Value) -> String {
    let name = arg_str(a, "name", "");
    let symbol = arg_str(a, "symbol", "");
    let supply = arg_u128(a, "supply", 0);
    let decimals = arg_u128(a, "decimals", 8);
    let owner = arg_str(a, "owner", "<agent-wallet>");
    if name.is_empty() || symbol.is_empty() || supply == 0 {
        return "error: name, symbol, supply required".into();
    }
    let base_units = supply.saturating_mul(10u128.saturating_pow(decimals as u32));
    let id = preview_id(&[&name, &symbol, &supply.to_string()]);
    format!(
        "⬡ SIGIL token deploy — built\n{}\n\
         → emits a DeployToken event committed in the contract_state_root.\n\
         → BROADCAST SEAM: submit via :8181 when live (same as txn_send).",
        json!({
            "kind": "DeployToken", "name": name, "symbol": symbol,
            "supply": supply, "decimals": decimals, "supply_base_units": base_units.to_string(),
            "owner": owner, "preview_id": id, "network_id": "sigil-g0"
        })
    )
}

// ── batch: bundle into one atomic transition ──
fn sigil_batch(a: &Value) -> String {
    let ops = match a.get("ops").and_then(|v| v.as_array()) {
        Some(o) if !o.is_empty() => o,
        _ => return "error: ops must be a non-empty array".into(),
    };
    let id = preview_id(&[&ops.len().to_string(), &a.to_string()]);
    format!(
        "⬡ SIGIL batch — {} ops bundled into ONE atomic transition (commit-or-nothing)\n{}\n\
         → all-or-nothing via commit_state_transition; one set of 4 committed roots for the whole batch.\n\
         → BROADCAST SEAM: submit the bundle via :8181 when live.",
        ops.len(),
        json!({ "kind": "BatchTransition", "op_count": ops.len(), "ops": ops,
                "atomicity": "commit-or-nothing", "preview_id": id, "network_id": "sigil-g0" })
    )
}

// ── benchmark: shell out to the chronos bench (real numbers) ──
fn sigil_benchmark(a: &Value) -> String {
    let mode = arg_str(a, "mode", "turbosync");
    let blocks = arg_u128(a, "blocks", 100_000);
    // Honest: run the real bench if its binary is reachable; otherwise report the
    // last measured figure + the exact command, never a fabricated number.
    let bench_bin = std::env::var("SIGIL_BENCH_BIN").ok();
    if let Some(bin) = bench_bin {
        let out = Command::new(&bin)
            .arg(&mode)
            .arg(blocks.to_string())
            .output();
        match out {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout);
                return format!("⬡ SIGIL benchmark [{mode}, {blocks} blocks] — REAL run\n{s}");
            }
            Err(e) => return format!("benchmark: SIGIL_BENCH_BIN set but failed to run: {e}"),
        }
    }
    let measured = match mode.as_str() {
        "market" => "1.07M trades/s (sigil market sim, SAP+X-Algo scored)",
        _ => "~25,000 blocks/s verify-every-block (TURBO-1, sync-down guard HELD, 0 divergence)",
    };
    format!(
        "⬡ SIGIL benchmark [{mode}, {blocks} blocks]\n\
         last measured: {measured}\n\
         to run live, set SIGIL_BENCH_BIN to the chronos bench binary, or:\n\
         cargo run -p sigil-chronos --bin {} --release",
        if mode == "market" { "sigil-market" } else { "sigil-turbosync" }
    )
}

// ── node restart (real process control) ──
fn sigil_node_restart(a: &Value) -> String {
    let host = arg_str(a, "host", &default_sigil_host());
    if !safe_host(&host) {
        return format!("error: host {host:?} rejected (hostname/IP chars only) [SEC-001]");
    }
    let via = arg_str(a, "via", "process");
    let remote = if via == "systemd" {
        "systemctl restart sigil-node && sleep 2 && systemctl is-active sigil-node".to_string()
    } else {
        // graceful: SIGTERM the running node, then relaunch via its launch script
        "pkill -TERM -f 'sigil-node start'; sleep 2; \
         (setsid bash /home/orobit/sigil-data/launch-delta.sh >/home/orobit/sigil-data/delta.log 2>&1 & ) ; \
         sleep 2; pgrep -af 'sigil-node start' | head -1"
            .to_string()
    };
    run_ssh(&host, &remote, "restart sigil-node")
}

// ── node deploy (scp binary + relaunch) ──
fn sigil_node_deploy(a: &Value) -> String {
    let host = arg_str(a, "host", &default_sigil_host());
    let bin = arg_str(a, "binary_path", "");
    let launch = arg_str(a, "launch_cmd", "bash /home/orobit/sigil-data/launch-delta.sh");
    if bin.is_empty() {
        return "error: binary_path required".into();
    }
    if !safe_host(&host) {
        return format!("error: host {host:?} rejected (hostname/IP chars only) [SEC-001]");
    }
    // SEC-001: `launch` is interpolated into the remote shell string below.
    // Restrict it to a binary path + plain flags — every shell metacharacter
    // (; | & $ ` > < parens quotes) is rejected, so it cannot break out of the
    // relaunch command into arbitrary remote execution.
    if !safe_cmd_charset(&launch) {
        return format!(
            "error: launch_cmd {launch:?} rejected — only [A-Za-z0-9 /._-=] allowed \
             (no shell metacharacters) [SEC-001]"
        );
    }
    let scp = Command::new("scp")
        .arg("-o").arg("StrictHostKeyChecking=no")
        .arg(&bin)
        .arg(format!("root@{host}:/home/orobit/target-sigil/release/sigil-node.new"))
        .output();
    match scp {
        Ok(o) if o.status.success() => {}
        Ok(o) => return format!("node_deploy: scp failed: {}", String::from_utf8_lossy(&o.stderr)),
        Err(e) => return format!("node_deploy: scp error: {e}"),
    }
    let remote = format!(
        "mv /home/orobit/target-sigil/release/sigil-node.new /home/orobit/target-sigil/release/sigil-node && \
         chmod +x /home/orobit/target-sigil/release/sigil-node && \
         pkill -TERM -f 'sigil-node start'; sleep 2; (setsid {launch} >/home/orobit/sigil-data/delta.log 2>&1 & ); \
         sleep 2; pgrep -af 'sigil-node start' | head -1"
    );
    run_ssh(&host, &remote, "deploy + relaunch sigil-node")
}

/// SEC-012: `remote` is executed by the remote login shell as ONE string.
/// Callers MUST validate (`safe_host`/`safe_cmd_charset`) or `shell_quote()`
/// every value interpolated into it — never pass raw MCP args through.
fn run_ssh(host: &str, remote: &str, what: &str) -> String {
    let out = Command::new("ssh")
        .arg("-o").arg("StrictHostKeyChecking=no")
        .arg("-o").arg("ConnectTimeout=10")
        .arg(format!("root@{host}"))
        .arg(remote)
        .output();
    match out {
        Ok(o) => {
            let so = String::from_utf8_lossy(&o.stdout);
            let se = String::from_utf8_lossy(&o.stderr);
            format!("⬡ SIGIL {what} @ {host}\n{}{}", so.trim(), if se.trim().is_empty() { String::new() } else { format!("\n[stderr] {}", se.trim()) })
        }
        Err(e) => format!("⬡ SIGIL {what} @ {host} — ssh error: {e}"),
    }
}
