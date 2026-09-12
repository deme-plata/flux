//! `flux_sigil_court` — read + cross-node health surface for the SIGIL Nation Supreme Court.
//!
//! The court lives inside `sigil-node` (`sigil_api::court::CourtBridge`) and exposes an
//! HTTP surface under `/v1/court/*`. This handler is the MCP window onto it: an agent can
//! read the constitution, the docket, the bench and precedents without hand-rolling curl,
//! and — crucially — can ask whether TWO nodes' courts actually AGREE.
//!
//! Why the diff tool matters: the court's block-evidence layer is deterministic, but its
//! mutable docket (rulings, honours, contempt, disclosure orders) is, as of 2026-09-12,
//! **node-local and un-gossiped**. So two nodes diverge the moment a ruling is filed against
//! only one of them. `flux_sigil_court_diff` measures that divergence directly; it is the
//! probe that pairs with the `/sigil/g2/court` gossip topic that makes them converge.
//!
//! All three tools are READ-ONLY. Base node = `FLUX_SIGIL_RPC` (default on-box
//! `http://127.0.0.1:18181`); the peer for the diff = `FLUX_SIGIL_PEER_RPC` or the `peer`
//! arg (default `http://10.77.0.5:18181`, happysrv).

use crate::handlers::{ToolDef, ToolRegistry};
use serde_json::{json, Value};
use std::time::Duration;

fn get_from(base: &str, path: &str) -> Result<Value, String> {
    let url = format!("{}{}", base.trim_end_matches('/'), path);
    let resp = ureq::get(&url).timeout(Duration::from_secs(12)).call();
    let body = match resp {
        Ok(r) => r.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(c, r)) => {
            return Err(format!("HTTP {c}: {}", r.into_string().unwrap_or_default().chars().take(160).collect::<String>()))
        }
        Err(e) => return Err(format!("{e}")),
    };
    serde_json::from_str(&body).map_err(|e| format!("not JSON ({e}): {}", body.chars().take(120).collect::<String>()))
}

/// Unwrap the `{ok,data}` envelope sigil-api wraps every response in.
fn data(v: Value) -> Value {
    v.get("data").cloned().unwrap_or(v)
}

fn base() -> String {
    crate::handlers::sigil_wallet::rpc_base()
}

fn peer_base(args: &Value) -> String {
    args.get("peer").and_then(|v| v.as_str()).map(|s| s.to_string())
        .or_else(|| std::env::var("FLUX_SIGIL_PEER_RPC").ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "http://10.77.0.5:18181".to_string())
}

/// Full court snapshot from one node: constitution, bench, docket summary, precedents.
fn flux_sigil_court(_args: &Value) -> String {
    let b = base();
    let status = get_from(&b, "/v1/court/status");
    if let Err(e) = &status {
        return json!({"ok": false, "error": e, "node": b,
            "hint": "set FLUX_SIGIL_RPC to a reachable sigil-api (on-box http://127.0.0.1:18181)"}).to_string();
    }
    let status = data(status.unwrap());
    let bench = get_from(&b, "/v1/court/bench").map(data).unwrap_or(Value::Null);
    let precedents = get_from(&b, "/v1/court/precedents").map(data).unwrap_or(Value::Null);
    let docket = get_from(&b, "/v1/court/docket").map(data).unwrap_or(Value::Null);
    json!({
        "ok": true,
        "node": b,
        "constitution": {
            "version": status.get("constitution_version"),
            "hash": status.get("constitution_hash"),
            "articles": status.get("articles"),
        },
        "court_root": status.get("court_root"),
        "docket_head": status.get("docket_head"),
        "docket_entries": status.get("docket_entries"),
        "bench": bench,
        "precedents": precedents,
        "docket": docket,
    }).to_string()
}

/// The full docket (every recorded act of the court) from one node.
fn flux_sigil_court_docket(_args: &Value) -> String {
    let b = base();
    match get_from(&b, "/v1/court/docket") {
        Ok(v) => json!({"ok": true, "node": b, "docket": data(v)}).to_string(),
        Err(e) => json!({"ok": false, "error": e, "node": b}).to_string(),
    }
}

/// Cross-node convergence probe: do two nodes' courts AGREE? Compares the constitution,
/// the docket head/root, and the entry count. A DISAGREE on court_root while the
/// constitution matches means the docket has diverged — expected today (no court gossip),
/// and the exact thing the `/sigil/g2/court` topic is meant to fix.
fn flux_sigil_court_diff(args: &Value) -> String {
    let a = base();
    let b = peer_base(args);
    let sa = get_from(&a, "/v1/court/status");
    let sb = get_from(&b, "/v1/court/status");
    let (sa, sb) = match (sa, sb) {
        (Ok(x), Ok(y)) => (data(x), data(y)),
        (ea, eb) => {
            return json!({"ok": false, "node_a": a, "node_b": b,
                "error_a": ea.err(), "error_b": eb.err(),
                "hint": "both nodes must expose /v1/court/status; set FLUX_SIGIL_PEER_RPC for the peer"}).to_string();
        }
    };
    let f = |v: &Value, k: &str| v.get(k).cloned().unwrap_or(Value::Null);
    let same = |k: &str| f(&sa, k) == f(&sb, k);
    let constitution_agrees = same("constitution_hash") && same("constitution_version");
    let root_agrees = same("court_root");
    let verdict = if root_agrees {
        "CONVERGED — both nodes agree on the court_root"
    } else if constitution_agrees {
        "DIVERGED — same constitution, different docket (expected until /sigil/g2/court gossip lands)"
    } else {
        "DIVERGED — even the constitution differs (a real config/version mismatch, investigate)"
    };
    json!({
        "ok": true,
        "verdict": verdict,
        "constitution_agrees": constitution_agrees,
        "court_root_agrees": root_agrees,
        "node_a": {"url": a, "court_root": f(&sa,"court_root"), "docket_entries": f(&sa,"docket_entries"),
                   "docket_head": f(&sa,"docket_head"), "constitution_hash": f(&sa,"constitution_hash")},
        "node_b": {"url": b, "court_root": f(&sb,"court_root"), "docket_entries": f(&sb,"docket_entries"),
                   "docket_head": f(&sb,"docket_head"), "constitution_hash": f(&sb,"constitution_hash")},
    }).to_string()
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(ToolDef {
        name: "flux_sigil_court",
        description: "READ the SIGIL Nation Supreme Court live from the node (sigil-api /v1/court/*): constitution (version, hash, article count), court_root, docket head + entry count, the sitting bench, precedents, and the full docket. Base node = FLUX_SIGIL_RPC (default on-box http://127.0.0.1:18181). Read-only. No args.",
        input_schema: json!({"type":"object","properties":{}}),
    }, flux_sigil_court);
    registry.register(ToolDef {
        name: "flux_sigil_court_docket",
        description: "The full SIGIL court docket — every recorded act (CaseFiled, RulingIssued, HonourConferred, ContemptRecorded, DisclosureOrdered, JusticeAppointed, ...) as the node holds it. Read-only. Base = FLUX_SIGIL_RPC. No args.",
        input_schema: json!({"type":"object","properties":{}}),
    }, flux_sigil_court_docket);
    registry.register(ToolDef {
        name: "flux_sigil_court_diff",
        description: "Do two SIGIL nodes' courts AGREE? Compares constitution, court_root, docket head + entry count between the base node (FLUX_SIGIL_RPC, default 127.0.0.1:18181) and a peer (arg `peer` or FLUX_SIGIL_PEER_RPC, default happysrv http://10.77.0.5:18181). Returns CONVERGED / DIVERGED with the per-node roots. Today the docket is node-local and un-gossiped, so a DIVERGE on court_root with a matching constitution is EXPECTED — this tool is the health probe that pairs with the /sigil/g2/court gossip topic. Read-only. Args: peer (optional URL).",
        input_schema: json!({"type":"object","properties":{"peer":{"type":"string"}}}),
    }, flux_sigil_court_diff);
}
