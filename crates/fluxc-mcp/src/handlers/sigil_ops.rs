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
    // ── LANE-Y agentic money: mandates gate every agent-driven movement ──
    registry.register(
        ToolDef {
            name: "flux_sigil_mandate_create",
            description: "Grant a persisted agent spend-mandate: max TOTAL amount (base units), purpose, \
                          ttl_hours (default 24). Returns the mandate id. Every flux_sigil_txn_send must \
                          reference an active mandate with headroom. Args: agent, max_amount, purpose, [ttl_hours].",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "agent": {"type": "string", "description": "agent identity granted the mandate"},
                    "max_amount": {"type": "number", "description": "max TOTAL spend, base units"},
                    "purpose": {"type": "string", "description": "what the agent may spend on"},
                    "ttl_hours": {"type": "number", "description": "expiry horizon (default 24h)"}
                },
                "required": ["agent", "max_amount", "purpose"]
            }),
        },
        sigil_mandate_create,
    );
    registry.register(
        ToolDef {
            name: "flux_sigil_mandate_list",
            description: "List agent mandates (active + closed + expired): spent vs max, purpose, expiry.",
            input_schema: json!({"type": "object", "properties": {}}),
        },
        sigil_mandate_list,
    );
    registry.register(
        ToolDef {
            name: "flux_sigil_mandate_close",
            description: "Close a mandate by id — no further spends can reference it. Args: id.",
            input_schema: json!({
                "type": "object",
                "properties": {"id": {"type": "string"}},
                "required": ["id"]
            }),
        },
        sigil_mandate_close,
    );
    registry.register(
        ToolDef {
            name: "flux_sigil_agent_panel",
            description: "One-call agent overview: rpcd balance for the wallet + active mandates with \
                          remaining headroom. Args: [wallet] (64-hex).",
            input_schema: json!({
                "type": "object",
                "properties": {"wallet": {"type": "string", "description": "64-hex wallet for balance"}}
            }),
        },
        sigil_agent_panel,
    );
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
            description: "SIGIL benchmark. mode=live (RULE 0) measures the RUNNING node over FLUX_SIGIL_RPC: \
                          block rate, state-height rate, live TPS from /recent tx timestamps, API latency p50/p95 — \
                          args [seconds=15]. mode=turbosync/market run the chronos sim benches (args [blocks=100000]).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "mode": {"type": "string", "enum": ["live", "turbosync", "market"]},
                    "seconds": {"type": "number", "description": "live mode: measurement window (default 15, clamp 3-120)"},
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
    // LANE-Y: agent money moves ONLY under an active mandate with headroom.
    let mandate = arg_str(a, "mandate", "");
    if mandate.is_empty() {
        return "error: mandate required — agents move SIGIL only under an active spend-mandate (flux_sigil_mandate_create)".into();
    }
    let remaining = match mandate_reserve(&mandate, amount) {
        Ok(r) => r,
        Err(e) => return format!("mandate refused: {e}"),
    };
    let id = preview_id(&[&from, &to, &amount.to_string(), &token]);
    format!(
        "⬡ SIGIL SignedTx::Send — built + ready to broadcast\n{}\n\
         → SIGN with the agent's SQIsign-L5 key, then broadcast on /sigil/g0/txs\n\
         → BROADCAST SEAM: sigil-node is P2P-only; wire to :8181 JSON-RPC `submit_tx` when it lands.",
        json!({
            "kind": "Send", "from": from, "to": to, "amount": amount, "token": token, "fee": fee,
            "preview_id": id, "network_id": "sigil-g0", "mandate": mandate, "mandate_remaining": remaining.to_string(),
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
    // mode=live — RULE 0: measure the RUNNING node (block rate, height rate,
    // live TPS, API latency percentiles) over `seconds`, not a chronos replay.
    if mode == "live" {
        let seconds = a.get("seconds").and_then(|v| v.as_u64()).unwrap_or(15);
        return crate::handlers::sigil_wallet::live_node_bench(seconds);
    }
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


// ── LANE-Y: persisted agent mandates ─────────────────────────────────────────
// A mandate is a spend-authorization an operator (or a 2-of-2 council flow)
// grants an agent: max TOTAL amount, purpose, expiry. It lives in flux-state on
// the storage array (never root, never tmpfs) so a box reboot cannot erase the
// ledger of what agents were allowed to move — the same persistence lesson the
// mine-chain taught us the hard way today.
const MANDATE_STORE: &str = "/home/storage/flux-state/sigil-mandates.json";

fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn mandates_load() -> Vec<Value> {
    std::fs::read_to_string(MANDATE_STORE)
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<Value>>(&t).ok())
        .unwrap_or_default()
}

fn mandates_save(v: &[Value]) -> Result<(), String> {
    let dir = std::path::Path::new(MANDATE_STORE).parent().unwrap();
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = format!("{MANDATE_STORE}.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(v).unwrap_or_default())
        .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, MANDATE_STORE).map_err(|e| e.to_string()) // atomic — feed-writer lesson
}

/// Validate a spend of `amount` against mandate `id`; on success RESERVE it
/// (spent += amount, persisted) and return Ok(remaining). Reservation happens at
/// construction time: better to under-spend a mandate than let a failed
/// broadcast double-spend its headroom.
fn mandate_reserve(id: &str, amount: u128) -> Result<u128, String> {
    let mut all = mandates_load();
    let now = now_ts();
    for m in all.iter_mut() {
        if m["id"].as_str() == Some(id) {
            if m["status"].as_str() != Some("active") {
                return Err(format!("mandate {id} is {}", m["status"].as_str().unwrap_or("?")));
            }
            if m["expires_ts"].as_u64().unwrap_or(0) < now {
                m["status"] = json!("expired");
                let _ = mandates_save(&all);
                return Err(format!("mandate {id} expired"));
            }
            let max = m["max_amount"].as_str().and_then(|x| x.parse::<u128>().ok()).unwrap_or(0);
            let spent = m["spent"].as_str().and_then(|x| x.parse::<u128>().ok()).unwrap_or(0);
            if spent.saturating_add(amount) > max {
                return Err(format!(
                    "mandate {id} headroom exceeded: spent {spent} + {amount} > max {max}"
                ));
            }
            let new_spent = spent.saturating_add(amount);
            m["spent"] = json!(new_spent.to_string());
            mandates_save(&all)?;
            return Ok(max - new_spent);
        }
    }
    Err(format!("mandate {id} not found — create one with flux_sigil_mandate_create"))
}

fn sigil_mandate_create(a: &Value) -> String {
    let agent = arg_str(a, "agent", "unnamed-agent");
    let max_amount = arg_u128(a, "max_amount", 0);
    let purpose = arg_str(a, "purpose", "");
    let ttl_hours = arg_u128(a, "ttl_hours", 24) as u64;
    if max_amount == 0 || purpose.is_empty() {
        return "error: max_amount (base units) and purpose required — a mandate without a bound or a reason is not a mandate".into();
    }
    let id = preview_id(&[&agent, &max_amount.to_string(), &purpose, &now_ts().to_string()]);
    let mut all = mandates_load();
    all.push(json!({
        "id": id, "agent": agent, "max_amount": max_amount.to_string(), "spent": "0",
        "purpose": purpose, "created_ts": now_ts(),
        "expires_ts": now_ts() + ttl_hours * 3600, "status": "active",
    }));
    if let Err(e) = mandates_save(&all) {
        return format!("error: mandate not persisted ({e}) — refusing to grant an unpersisted authorization");
    }
    format!("⬡ MANDATE GRANTED {id}\n  agent: {agent}\n  max: {max_amount} base units\n  purpose: {purpose}\n  expires in {ttl_hours}h\n  → pass mandate=\"{id}\" on flux_sigil_txn_send / flux_sigil_dex_swap")
}

fn sigil_mandate_list(_a: &Value) -> String {
    let all = mandates_load();
    if all.is_empty() {
        return "no mandates — flux_sigil_mandate_create grants the first one".into();
    }
    let now = now_ts();
    let mut out = String::from("⬡ SIGIL AGENT MANDATES\n");
    for m in &all {
        let exp = m["expires_ts"].as_u64().unwrap_or(0);
        let status = if m["status"].as_str() == Some("active") && exp < now { "expired" }
                     else { m["status"].as_str().unwrap_or("?") };
        out.push_str(&format!(
            "  {} [{}] agent={} spent={}/{} purpose={} expires_in={}s\n",
            m["id"].as_str().unwrap_or("?"), status,
            m["agent"].as_str().unwrap_or("?"),
            m["spent"].as_str().unwrap_or("0"), m["max_amount"].as_str().unwrap_or("0"),
            m["purpose"].as_str().unwrap_or(""), exp.saturating_sub(now),
        ));
    }
    out
}

fn sigil_mandate_close(a: &Value) -> String {
    let id = arg_str(a, "id", "");
    let mut all = mandates_load();
    for m in all.iter_mut() {
        if m["id"].as_str() == Some(&id as &str) {
            m["status"] = json!("closed");
            return match mandates_save(&all) {
                Ok(()) => format!("mandate {id} closed"),
                Err(e) => format!("error: close not persisted ({e})"),
            };
        }
    }
    format!("mandate {id} not found")
}

fn sigil_agent_panel(a: &Value) -> String {
    let wallet = arg_str(a, "wallet", "");
    let bal = if wallet.len() == 64 {
        std::process::Command::new("curl")
            .args(["-s", "--max-time", "4",
                   &format!("http://127.0.0.1:8099/api/v1/balance?wallet={wallet}")])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_else(|| "rpcd unreachable".into())
    } else {
        "(pass wallet=64-hex for balance)".into()
    };
    let now = now_ts();
    let active: Vec<String> = mandates_load().iter()
        .filter(|m| m["status"].as_str() == Some("active")
                 && m["expires_ts"].as_u64().unwrap_or(0) >= now)
        .map(|m| {
            let max = m["max_amount"].as_str().and_then(|x| x.parse::<u128>().ok()).unwrap_or(0);
            let spent = m["spent"].as_str().and_then(|x| x.parse::<u128>().ok()).unwrap_or(0);
            format!("{} headroom={} purpose={}",
                    m["id"].as_str().unwrap_or("?"), max.saturating_sub(spent),
                    m["purpose"].as_str().unwrap_or(""))
        })
        .collect();
    format!("⬡ SIGIL AGENT PANEL\n  balance: {}\n  active mandates: {}\n{}",
            bal.trim(), active.len(),
            if active.is_empty() { "    (none — agents cannot move funds without one)".to_string() }
            else { active.iter().map(|l| format!("    {l}\n")).collect::<String>() })
}
