//! `flux_sigil_kgauge*` — the Kristensen K-parameter, MEASURED live on the SIGIL network
//! and pushed to every registered webhook, so an agent driving fluxc always knows K.
//!
//! Two numbers are reported, and they are not the same thing (never conflate them):
//!
//! * **K\*** — the dimensionless Kristensen number built from what the chain actually
//!   exposes: `K* = 2π·√(ΔH·τ·Δs/ħ)` with ħ := 1, where Δs is the PROPOSER ENTROPY of the
//!   blocks in the window (bits — the plurality of who may propose), ΔH is the energetic
//!   disagreement seen locally (mining reject ratio + peer churn; P2P byte asymmetry is
//!   unavailable on sigil-api, so ΔH is a LOWER BOUND), and τ is the sampling window.
//!   `K = 2π·√(ΔH·Δs)/τ` is the dimensional form the wallet topbar shows. K*≈1 is the
//!   Margolus–Levitin boundary. This mirrors `gui/sigil-wallet-tron-embedded.html`'s
//!   client-side gauge exactly, so the phone, the browser and the MCP agree.
//! * **reference_gauge** — the `flux-kgauge` crate's Eq. 10/25 stress score
//!   (Theoretical Minimum v4) over the same observables, with its own phase, confidence
//!   and caveats. Low is healthy. It exists so the two definitions can be compared on the
//!   same sample; it is NOT the same quantity as K*.
//!
//! Honesty is part of the payload: every reading carries `provenance.measured` and
//! `provenance.unavailable`, and a reading over a window with fewer than 2 blocks says so.
//!
//! Webhook event name: `sigil_kgauge`. Series file (append-only JSONL, one reading per
//! line): `SIGIL_KGAUGE_SERIES` or the default under /home/storage/claude-code.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{json, Value};

use crate::handlers::{ToolDef, ToolRegistry};
use flux_kgauge::observables::{BlockFact, BlockWindow, CounterSample, NetworkSize, Observables};
use flux_kgauge::{GaugeConfig, KGauge};
use fluxc_webhooks::webhook;

pub const EVENT: &str = "sigil_kgauge";
const DEFAULT_SERIES: &str = "/home/storage/claude-code/k-parameter-paper/gauge-series/sigil-kgauge.jsonl";
/// Steady-state SIGIL block rate used ONLY for the diagnostic block-rate deviation
/// (measured 2026-08-28; 6.28/s is a catch-up rate, not steady state).
const TARGET_BPS: f64 = 0.83;
/// Λ (commitment) denominator: κ·τ_confirm blocks, window-limited by construction.
const COMMIT_BLOCKS: f64 = 18.0 * 100.0;

static WATCH_RUNNING: AtomicBool = AtomicBool::new(false);

fn series_path() -> String {
    std::env::var("SIGIL_KGAUGE_SERIES").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| DEFAULT_SERIES.to_string())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn get_json(path: &str) -> Result<Value, String> {
    let url = format!("{}{}", crate::handlers::sigil_wallet::rpc_base(), path);
    let resp = ureq::get(&url).timeout(Duration::from_secs(12)).call();
    let body = match resp {
        Ok(r) => r.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(c, r)) => return Err(format!("{url}: HTTP {c}: {}", r.into_string().unwrap_or_default().chars().take(200).collect::<String>())),
        Err(e) => return Err(format!("{url}: {e}")),
    };
    serde_json::from_str(&body).map_err(|e| format!("{url}: not JSON ({e}): {}", body.chars().take(120).collect::<String>()))
}

#[derive(Clone, Copy, Debug)]
struct Sample { ts_ms: u64, height: u64, shares_accepted: u64, rejects: u64, peers: u64 }

fn sample() -> Result<Sample, String> {
    let miners = get_json("/v1/mining/miners")?;
    let topo = get_json("/v1/network/topology")?;
    let m = miners.get("data").cloned().unwrap_or(miners);
    let t = topo.get("data").cloned().unwrap_or(topo);
    let rejects = m.get("rejects").and_then(|r| r.as_array()).map(|arr| {
        arr.iter().map(|e| e.get(1).and_then(|v| v.as_u64()).unwrap_or(0)).sum::<u64>()
    }).unwrap_or(0);
    Ok(Sample {
        ts_ms: now_ms(),
        height: m.get("height").and_then(|v| v.as_u64()).unwrap_or(0),
        shares_accepted: m.get("shares_accepted").and_then(|v| v.as_u64()).unwrap_or(0),
        rejects,
        peers: t.get("peer_count").and_then(|v| v.as_u64()).unwrap_or(0),
    })
}

fn recent_blocks() -> Result<Vec<Value>, String> {
    let v = get_json("/v1/dagknight/recent")?;
    let d = v.get("data").cloned().unwrap_or(v);
    Ok(d.get("blocks").and_then(|b| b.as_array()).cloned().unwrap_or_default())
}

fn bytes32(v: Option<&Value>) -> [u8; 32] {
    let mut out = [0u8; 32];
    if let Some(arr) = v.and_then(|x| x.as_array()) {
        for (i, b) in arr.iter().take(32).enumerate() { out[i] = b.as_u64().unwrap_or(0) as u8; }
    } else if let Some(s) = v.and_then(|x| x.as_str()) {
        if let Ok(b) = hex::decode(s.trim_start_matches("0x")) { for (i, x) in b.iter().take(32).enumerate() { out[i] = *x; } }
    }
    out
}

fn shannon_bits(counts: &std::collections::HashMap<[u8; 32], u64>) -> f64 {
    let total: u64 = counts.values().sum();
    if total == 0 { return 0.0; }
    let n = total as f64;
    -counts.values().map(|&c| { let p = c as f64 / n; if p > 0.0 { p * p.log2() } else { 0.0 } }).sum::<f64>()
}

fn regime(k: f64) -> &'static str { if k < 5.0 { "stable" } else if k < 10.0 { "approaching" } else { "critical" } }

/// One complete reading: two counter samples `window_secs` apart, the blocks that landed
/// in between (or, on a quiet window, the most recent ones — flagged), K, K*, and the
/// crate's reference gauge over the identical observables.
pub(crate) fn measure(window_secs: f64) -> Result<Value, String> {
    let a = sample()?;
    std::thread::sleep(Duration::from_secs_f64(window_secs));
    let b = sample()?;
    let all_blocks = recent_blocks()?;
    let tau = ((b.ts_ms.saturating_sub(a.ts_ms)) as f64 / 1000.0).max(window_secs * 0.5);

    // Blocks produced INSIDE the window; fall back to the recent set if the feed has none.
    let in_window: Vec<&Value> = all_blocks.iter().filter(|bk| bk.get("height").and_then(|h| h.as_u64()).unwrap_or(0) > a.height).collect();
    let (blocks, window_only): (Vec<&Value>, bool) = if in_window.len() >= 2 { (in_window, true) } else { (all_blocks.iter().collect(), false) };

    let mut counts: std::collections::HashMap<[u8; 32], u64> = std::collections::HashMap::new();
    for bk in &blocks { *counts.entry(bytes32(bk.get("producer"))).or_insert(0) += 1; }
    let ds = shannon_bits(&counts).max(0.0);   // Δs: proposer entropy, bits (a lone producer is +0.0, not -0.0)
    let distinct = counts.len();
    let blue_false = blocks.iter().filter(|bk| bk.get("is_blue").and_then(|v| v.as_bool()) == Some(false)).count();

    let sub_delta = b.shares_accepted.saturating_sub(a.shares_accepted);
    // Rejects and accepts are BOTH cumulative counters on /v1/mining/miners; the ratio must
    // be delta-over-delta for the window. (The wallet topbar divides the cumulative reject
    // total by a per-window accept delta, which manufactures ΔH≈0.7 on a quiet chain —
    // measured 2026-09-06, 182 lifetime rejects vs 72 accepts in 30 s. Not copied here.)
    let rej_delta = b.rejects.saturating_sub(a.rejects);
    let rej_ratio = if sub_delta + rej_delta > 0 { rej_delta as f64 / (sub_delta + rej_delta) as f64 } else { 0.0 };
    let churn = if a.peers > 0 { (b.peers as f64 - a.peers as f64).abs() / a.peers as f64 } else { 0.0 };
    let dh = rej_ratio + churn;                // ΔH lower bound (no byte counters on sigil-api)

    let k = if tau > 0.0 { 2.0 * std::f64::consts::PI * (dh * ds).sqrt() / tau } else { 0.0 };
    let k_star = 2.0 * std::f64::consts::PI * (dh * tau * ds).sqrt(); // ħ := 1
    let d_commit = b.height.saturating_sub(a.height);
    let obs_bps = d_commit as f64 / tau;
    let br_dev = (obs_bps - TARGET_BPS).abs() / TARGET_BPS;
    let n_total = (b.peers + 2).max(8) as f64;
    let omega = 1.0 - (-(b.peers as f64) / n_total).exp();
    let lambda = 1.0 - (-(d_commit as f64) / COMMIT_BLOCKS).exp();
    let dominant = if dh == 0.0 && ds == 0.0 { "none (quiet chain)" } else if dh >= ds { "energetic disagreement (ΔH)" } else { "proposer plurality (Δs)" };

    // ── reference gauge: flux-kgauge Eq. 10/25 over the SAME observables ──
    let to_counter = |s: &Sample| CounterSample {
        mining_submitted: s.shares_accepted + s.rejects, mining_accepted: s.shares_accepted,
        p2p_bytes_in: 0, p2p_bytes_out: 0, peer_count: s.peers, local_height: s.height, network_height: 0,
    };
    let facts: Vec<BlockFact> = blocks.iter().map(|bk| BlockFact {
        height: bk.get("height").and_then(|h| h.as_u64()).unwrap_or(0),
        producer: bytes32(bk.get("producer")),
        merge_parent_count: bk.get("merge_parents").and_then(|m| m.as_array()).map(|m| m.len() as u32).unwrap_or(0),
        blue_score: bk.get("blue_score").and_then(|v| v.as_u64()),
        is_blue: bk.get("is_blue").and_then(|v| v.as_bool()),
    }).collect();
    let obs = Observables { previous: to_counter(&a), current: to_counter(&b), window_secs: tau, window: BlockWindow { blocks: facts }, network_size: NetworkSize::Estimated(n_total as u64), kappa: 0.0 };
    let report = KGauge::new(GaugeConfig::sigil()).observe(&obs);
    let reference = json!({
        "definition": "flux-kgauge (Theoretical Minimum v4) Eq.10 base / Eq.25 enhanced — a STRESS score, low = healthy; not the same quantity as K*",
        "k_base": report.base.k_base, "k_enhanced": report.k_enhanced,
        "phase": report.phase.as_str(), "confidence": format!("{:?}", report.base.confidence),
        "provenance": format!("{:?}", report.base.provenance), "actionable": report.is_actionable(),
        "caveats": report.caveats.iter().map(|c| c.detail.clone()).collect::<Vec<_>>(),
    });

    Ok(json!({
        "ok": true, "event": EVENT, "ts_ms": b.ts_ms, "node": crate::handlers::sigil_wallet::rpc_base(),
        "network": "sigil-g2",
        "definition": "K = 2π·√(ΔH·Δs)/τ ; K* = 2π·√(ΔH·τ·Δs/ħ), ħ:=1 (dimensionless Kristensen number; K*≈1 = Margolus–Levitin boundary). Δs = proposer entropy (bits), ΔH = reject ratio + peer churn (lower bound), τ = window (s).",
        "K": k, "K_star": k_star, "regime": regime(k),
        "delta_H": dh, "delta_s_bits": ds, "tau_secs": tau, "dominant": dominant,
        "omega_observer_coverage": omega, "lambda_commitment": lambda,
        "window": { "blocks": blocks.len(), "window_only": window_only, "distinct_producers": distinct,
                    "tip_start": a.height, "tip_end": b.height, "blocks_added": d_commit,
                    "block_rate_bps": obs_bps, "block_rate_dev_vs_target": br_dev, "target_bps_diagnostic": TARGET_BPS, "is_blue_false": blue_false },
        "inputs": { "reject_ratio": rej_ratio, "rejects_delta": rej_delta, "rejects_lifetime": b.rejects, "shares_accepted_delta": sub_delta,
                    "peers_start": a.peers, "peers_end": b.peers, "peer_churn": churn },
        "provenance": {
            "measured": ["proposer entropy (Δs)", "block rate", "mining reject ratio", "peer count / churn"],
            "unavailable": ["P2P byte asymmetry (no byte counters on sigil-api) → ΔH is a lower bound", "network height (not exposed) → Ω uses an estimated network size"],
            "diagnostic_only": ["block-rate deviation — shown, never substituted for Δs (that substitution makes the gauge blind to churn)"],
            "note": if window_only { "blocks counted are exactly those produced inside the window" } else { "fewer than 2 blocks landed inside the window; Δs computed over the node's recent-block feed instead" }
        },
        "reference_gauge": reference
    }))
}

fn record(reading: &Value) -> Result<String, String> {
    use std::io::Write;
    let p = series_path();
    if let Some(dir) = std::path::Path::new(&p).parent() { std::fs::create_dir_all(dir).map_err(|e| e.to_string())?; }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&p).map_err(|e| format!("{p}: {e}"))?;
    writeln!(f, "{}", reading).map_err(|e| e.to_string())?;
    Ok(p)
}

fn arg_f64(a: &Value, k: &str, d: f64) -> f64 { a.get(k).and_then(|v| v.as_f64()).unwrap_or(d) }
fn arg_bool(a: &Value, k: &str, d: bool) -> bool { a.get(k).and_then(|v| v.as_bool()).unwrap_or(d) }

fn flux_sigil_kgauge(args: &Value) -> String {
    let window = arg_f64(args, "window_secs", 60.0).clamp(10.0, 600.0);
    let do_hook = arg_bool(args, "webhook", true);
    let do_record = arg_bool(args, "record", true);
    match measure(window) {
        Ok(mut r) => {
            if do_record { match record(&r) { Ok(p) => { r["recorded_to"] = json!(p); } Err(e) => { r["record_error"] = json!(e); } } }
            if do_hook { webhook::auto_dispatch(EVENT, r.clone()); r["webhook_event"] = json!(EVENT); }
            serde_json::to_string_pretty(&r).unwrap_or_else(|_| r.to_string())
        }
        Err(e) => json!({"ok": false, "error": e, "hint": "is sigil-api reachable? FLUX_SIGIL_RPC overrides the base (default http://127.0.0.1:18181)"}).to_string(),
    }
}

fn flux_sigil_kgauge_watch(args: &Value) -> String {
    if arg_bool(args, "stop", false) {
        WATCH_RUNNING.store(false, Ordering::SeqCst);
        return json!({"ok": true, "watch": "stopping after the current window"}).to_string();
    }
    if arg_bool(args, "status", false) || (args.get("every_secs").is_none() && WATCH_RUNNING.load(Ordering::SeqCst)) {
        return json!({"ok": true, "running": WATCH_RUNNING.load(Ordering::SeqCst), "series": series_path(), "event": EVENT}).to_string();
    }
    let every = arg_f64(args, "every_secs", 300.0).max(60.0);
    let window = arg_f64(args, "window_secs", 60.0).clamp(10.0, 600.0).min(every);
    if WATCH_RUNNING.swap(true, Ordering::SeqCst) {
        return json!({"ok": true, "running": true, "note": "already watching — pass stop:true first to change the cadence", "series": series_path()}).to_string();
    }
    std::thread::spawn(move || {
        while WATCH_RUNNING.load(Ordering::SeqCst) {
            match measure(window) {
                Ok(mut r) => { let _ = record(&r).map(|p| r["recorded_to"] = json!(p)); webhook::auto_dispatch(EVENT, r); }
                Err(e) => { webhook::auto_dispatch(EVENT, json!({"ok": false, "event": EVENT, "error": e, "ts_ms": now_ms()})); }
            }
            let rest = (every - window).max(1.0);
            let mut slept = 0.0;
            while slept < rest && WATCH_RUNNING.load(Ordering::SeqCst) { std::thread::sleep(Duration::from_secs(1)); slept += 1.0; }
        }
    });
    json!({"ok": true, "running": true, "every_secs": every, "window_secs": window, "event": EVENT, "series": series_path(),
           "note": "each reading is appended to the series file and dispatched to every webhook registered for this event (or all events); lives as long as this MCP process"}).to_string()
}

fn flux_sigil_kgauge_series(args: &Value) -> String {
    let n = args.get("last").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
    let p = series_path();
    let text = match std::fs::read_to_string(&p) { Ok(t) => t, Err(e) => return json!({"ok": false, "error": format!("{p}: {e}")}).to_string() };
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail: Vec<Value> = lines.iter().rev().take(n).rev().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let ks: Vec<f64> = tail.iter().filter_map(|r| r.get("K_star").and_then(|v| v.as_f64())).collect();
    let mean = if ks.is_empty() { 0.0 } else { ks.iter().sum::<f64>() / ks.len() as f64 };
    let (min, max) = ks.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &x| (lo.min(x), hi.max(x)));
    json!({"ok": true, "series": p, "total_readings": lines.len(), "returned": tail.len(),
           "K_star": {"mean": mean, "min": if ks.is_empty() {0.0} else {min}, "max": if ks.is_empty() {0.0} else {max}},
           "readings": tail}).to_string()
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(ToolDef {
        name: "flux_sigil_kgauge",
        description: "MEASURE the Kristensen K-parameter live on SIGIL (sigil-g2): samples the node twice `window_secs` apart, takes proposer entropy Δs (bits) over the blocks produced in between, energetic disagreement ΔH (reject ratio + peer churn; byte asymmetry unavailable → lower bound), and returns K = 2π√(ΔH·Δs)/τ, the dimensionless K* = 2π√(ΔH·τ·Δs/ħ) (ħ:=1; K*≈1 = Margolus–Levitin boundary), regime, Ω, Λ, full provenance, plus the flux-kgauge crate's Eq.10/25 reference stress score over the same sample. Appends the reading to the series file and dispatches it to every registered webhook as event `sigil_kgauge`. Args: window_secs (10..600, default 60), webhook (default true), record (default true).",
        input_schema: json!({"type":"object","properties":{"window_secs":{"type":"number"},"webhook":{"type":"boolean"},"record":{"type":"boolean"}}}),
    }, flux_sigil_kgauge);
    registry.register(ToolDef {
        name: "flux_sigil_kgauge_watch",
        description: "Always know K: start a background sampler inside this MCP process that measures the SIGIL K-parameter every `every_secs` (default 300, min 60) over a `window_secs` window (default 60), appends each reading to the series file and pushes it to every registered webhook as `sigil_kgauge`. Args: every_secs, window_secs, stop (true to stop), status (true to report). Register a receiver first with flux_webhook_register(events:[\"sigil_kgauge\"]).",
        input_schema: json!({"type":"object","properties":{"every_secs":{"type":"number"},"window_secs":{"type":"number"},"stop":{"type":"boolean"},"status":{"type":"boolean"}}}),
    }, flux_sigil_kgauge_watch);
    registry.register(ToolDef {
        name: "flux_sigil_kgauge_series",
        description: "The recorded SIGIL K-parameter series (append-only JSONL written by flux_sigil_kgauge / _watch): last N readings plus K* mean/min/max — the data a paper or a letter should quote. Args: last (default 20).",
        input_schema: json!({"type":"object","properties":{"last":{"type":"integer"}}}),
    }, flux_sigil_kgauge_series);
}
