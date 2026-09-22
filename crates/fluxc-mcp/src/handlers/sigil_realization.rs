//! `flux_sigil_realization*` — the **Kristensen Realization Functional** `K_R`,
//! evaluated against the LIVE SIGIL network and pushed to every registered
//! webhook, so an agent driving fluxc always knows not just *how stressed* the
//! chain is (that is `flux_sigil_kgauge`) but *what single constraint is stopping
//! it from being what it could be*.
//!
//! ## The two gauges compose; they do not compete
//!
//! ```text
//!   flux_sigil_kgauge        -> K*   "how stressed is consensus right now"
//!            |
//!            +--- feeds K_coord --->  flux_sigil_realization -> K_R
//!                                     "which constraint is binding, and what
//!                                      would relieving it be worth"
//! ```
//!
//! `K_coord` in the functional is **literally** `K*` from the k-gauge: this
//! module calls [`super::sigil_kgauge::measure`] rather than reimplementing the
//! proposer-entropy formula, so the two readings can never drift apart. One
//! sampling window serves both.
//!
//! ## How the live chain maps onto a "design"
//!
//! | term | SIGIL observable | provenance |
//! |---|---|---|
//! | `C` compute | `net_hps` against a DECLARED reference rate | Derived |
//! | `E` energy | — no power telemetry exists on sigil-api — | **Unavailable** |
//! | `V` verifiability | accepted / (accepted + rejected) shares, delta-over-delta | Measured |
//! | `A` availability | node reachable x peer coverage x block liveness | Measured |
//! | `X` expandability | `1 - HHI` of hash-rate concentration across rigs | Measured |
//! | `N_p^eff` | per-miner `u_i v_i r_i` from the live miner table | Measured |
//! | `K_coord` | `K*` from the k-gauge over the same window | Measured (lower bound) |
//! | `B` | max shortfall over the constraint set, and it NAMES the binder | Measured |
//!
//! **`E` is genuinely unmeasurable here and is therefore excluded from the
//! geometric mean, never substituted.** The reading says so in
//! `capability.excluded` and in `caveats`. A five-term mean with a guessed
//! energy term would look better and be a lie.
//!
//! Webhook event name: `sigil_realization`. Series file (append-only JSONL):
//! `SIGIL_REALIZATION_SERIES`, defaulting beside the k-gauge series.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{json, Value};

use crate::handlers::{ToolDef, ToolRegistry};
use flux_realization::{
    amplification, bottleneck, evaluate, feasibility, velocity, Capability, Constraint, Design, Peer, Provenance,
    Tracked,
};
use fluxc_webhooks::webhook;

pub const EVENT: &str = "sigil_realization";
const DEFAULT_SERIES: &str = "/home/storage/claude-code/k-parameter-paper/gauge-series/sigil-realization.jsonl";

/// Declared reference hash rate for the compute term. `C = net_hps / REF`, so
/// `C = 1` means "this network is delivering the reference rate". It is a
/// yardstick chosen by us, not a property of the chain — which is exactly why it
/// is named, exported in every reading, and overridable.
const DEFAULT_REF_HPS: f64 = 1.0e9;
/// Soft target for peer count — below this, mesh coverage is the binder.
const DEFAULT_TARGET_PEERS: f64 = 8.0;
/// Soft target for distinct mining rigs — the replication dimension: how many
/// independent boxes would have to go away before the network did.
const DEFAULT_TARGET_RIGS: f64 = 16.0;
/// Soft target for the fraction of hash rate arriving from shielded (STARK-
/// carrying) miners.
const DEFAULT_TARGET_SHIELDED_PCT: f64 = 50.0;
/// A miner not seen for this long counts as fully unreliable (`r_i = 0`).
const STALE_SECS: f64 = 600.0;
/// Shielded note amounts are range-constrained to `2^RANGE_BITS` over the 64-bit
/// Goldilocks field. This is the hard mathematical ceiling that caps SIGIL's
/// decimals at 10 — see the workspace notes on the decimal ceiling. It is a
/// genuine `Theta_phys` constraint: a supply above it is not a slow design, it
/// is an unrepresentable one.
const RANGE_BITS: u32 = 58;

static WATCH_RUNNING: AtomicBool = AtomicBool::new(false);

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key).ok().and_then(|s| s.trim().parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(default)
}

fn series_path() -> String {
    std::env::var("SIGIL_REALIZATION_SERIES").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| DEFAULT_SERIES.to_string())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn get_json(path: &str) -> Result<Value, String> {
    let url = format!("{}{}", crate::handlers::sigil_wallet::rpc_base(), path);
    let resp = ureq::get(&url).timeout(Duration::from_secs(12)).call();
    let body = match resp {
        Ok(r) => r.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(c, r)) => {
            return Err(format!("{url}: HTTP {c}: {}", r.into_string().unwrap_or_default().chars().take(200).collect::<String>()))
        }
        Err(e) => return Err(format!("{url}: {e}")),
    };
    serde_json::from_str(&body).map_err(|e| format!("{url}: not JSON ({e}): {}", body.chars().take(120).collect::<String>()))
}

fn data(v: Value) -> Value {
    v.get("data").cloned().unwrap_or(v)
}

fn f(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0)
}

/// One complete `K_R` reading over a `window_secs` sampling window.
///
/// The window is spent inside the k-gauge (which samples twice), so this costs
/// one window in wall clock, not two.
pub(crate) fn measure(window_secs: f64) -> Result<Value, String> {
    // ── K_coord comes from the k-gauge, over the same window ──────────────
    let kg = super::sigil_kgauge::measure(window_secs)?;
    let k_star = kg.get("K_star").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let kg_window = kg.get("window").cloned().unwrap_or(json!({}));
    let kg_inputs = kg.get("inputs").cloned().unwrap_or(json!({}));
    let tau = kg.get("tau_secs").and_then(|v| v.as_f64()).unwrap_or(window_secs);
    let blocks_added = f(&kg_window, "blocks_added");
    let distinct_producers = f(&kg_window, "distinct_producers");

    // ── one instantaneous read of the fleet, the mesh and the supply ──────
    let miners_raw = data(get_json("/v1/mining/miners")?);
    let topo = data(get_json("/v1/network/topology")?);
    let supply = data(get_json("/v1/supply")?);
    let healthy = get_json("/v1/health").map(|v| v.get("ok").and_then(|o| o.as_bool()).unwrap_or(false)).unwrap_or(false);

    let ref_hps = env_f64("SIGIL_REALIZATION_REF_HPS", DEFAULT_REF_HPS);
    let target_peers = env_f64("SIGIL_REALIZATION_TARGET_PEERS", DEFAULT_TARGET_PEERS);
    let target_rigs = env_f64("SIGIL_REALIZATION_TARGET_RIGS", DEFAULT_TARGET_RIGS);
    let target_shielded = env_f64("SIGIL_REALIZATION_TARGET_SHIELDED_PCT", DEFAULT_TARGET_SHIELDED_PCT);

    let net_hps = f(&miners_raw, "net_hps");
    let shielded_pct = f(&miners_raw, "shielded_hps_pct");
    let peer_count = f(&topo, "peer_count");
    let fan_out = f(&topo, "fan_out").max(1.0);
    let started = topo.get("started").and_then(|v| v.as_bool()).unwrap_or(false);
    let miners: Vec<Value> = miners_raw.get("miners").and_then(|m| m.as_array()).cloned().unwrap_or_default();

    // Accept ratio, delta-over-delta for the window (the k-gauge already did the
    // subtraction; reusing its deltas keeps the two readings arithmetically
    // consistent instead of merely similar).
    let acc_d = f(&kg_inputs, "shares_accepted_delta");
    let rej_d = f(&kg_inputs, "rejects_delta");
    let accept_ratio = if acc_d + rej_d > 0.0 { acc_d / (acc_d + rej_d) } else { 1.0 };
    let accept_ratio_measured = acc_d + rej_d > 0.0;

    // ── X: expandability = 1 - HHI of hash-rate concentration ─────────────
    // A network whose hash rate all comes from one rig cannot be replicated by
    // removing that rig; one spread evenly over many independent boxes can.
    // The Herfindahl index is the standard concentration measure and needs no
    // arbitrary threshold.
    let total_hps: f64 = miners.iter().map(|m| f(m, "hash_rate")).sum();
    let hhi: f64 = if total_hps > 0.0 {
        miners.iter().map(|m| { let s = f(m, "hash_rate") / total_hps; s * s }).sum()
    } else {
        1.0
    };
    let expandable = (1.0 - hhi).clamp(0.0, 1.0);
    let distinct_rigs = miners.iter().filter_map(|m| m.get("rig").and_then(|r| r.as_str())).collect::<std::collections::HashSet<_>>().len();

    // ── A: availability = reachable AND started, times mesh coverage, times
    //       block liveness. All three must hold, so they multiply.
    let mesh_coverage = (peer_count / (fan_out * 2.0)).clamp(0.0, 1.0);
    let liveness = if blocks_added > 0.0 { 1.0 } else { 0.0 };
    let up = if healthy && started { 1.0 } else { 0.0 };
    let availability = up * mesh_coverage.max(0.05) * liveness;

    // ── peers: one entry per live miner ───────────────────────────────────
    // u_i: this rig's own useful-work score against the reference rate, capped
    //      at 1 (a single enormous rig is not worth ten peers).
    // v_i: verification confidence. The API exposes accepts/rejects only for the
    //      FLEET, not per miner, so every miner inherits the fleet ratio — that
    //      makes v_i Derived, not Measured, and the reading says so.
    // r_i: liveness decay from last_seen.
    let per_miner_ref = (ref_hps / target_rigs).max(1.0);
    let peers: Vec<Peer> = miners
        .iter()
        .map(|m| {
            let hr = f(m, "hash_rate");
            let seen = f(m, "last_seen_secs_ago");
            let reliable = (1.0 - (seen / STALE_SECS)).clamp(0.0, 1.0);
            let id = m.get("rig").and_then(|r| r.as_str()).unwrap_or("unknown").to_string();
            Peer { id, useful: (hr / per_miner_ref).clamp(0.0, 1.0), verified: accept_ratio, reliable }
        })
        .collect();

    // ── constraints: the Landscape/Swampland gate and the bottleneck ──────
    let max_supply: f64 = supply
        .get("max_supply")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    let range_ceiling = 2f64.powi(RANGE_BITS as i32);

    let constraints = vec![
        Constraint::hard(
            "node_reachable",
            1.0,
            if healthy && started { 1.0 } else { 0.0 },
            Provenance::Measured,
            "sigil-api /v1/health answers and the p2p listener is started; nothing else can be measured without it",
        ),
        Constraint::hard(
            "block_liveness",
            1.0,
            liveness,
            Provenance::Measured,
            "at least one block settled inside the sampling window — a chain that is not advancing is not a design being realized",
        ),
        Constraint::hard(
            "note_range_ceiling",
            max_supply,
            range_ceiling,
            Provenance::Protocol,
            "shielded note amounts are range-constrained to 2^58 over Goldilocks; a supply above it is unrepresentable, not merely slow — this is the ceiling that caps SIGIL at 10 decimals",
        ),
        Constraint::soft(
            "peers",
            target_peers,
            peer_count,
            Provenance::Measured,
            "gossip peers connected; below target the mesh is the binder — check bootstrap peers and the memory-limit cap",
        ),
        Constraint::soft(
            "miner_diversity",
            target_rigs,
            distinct_rigs as f64,
            Provenance::Measured,
            "distinct mining rigs; this is the replication dimension — how many independent boxes would have to leave before the network did",
        ),
        Constraint::soft(
            "shielded_coverage",
            target_shielded,
            shielded_pct,
            Provenance::Measured,
            "share of hash rate arriving from shielded (STARK-carrying) miners; low coverage means most work is unproven at mint time",
        ),
        Constraint::soft(
            "share_acceptance",
            1.0,
            accept_ratio,
            if accept_ratio_measured { Provenance::Measured } else { Provenance::Unavailable },
            "accepted/(accepted+rejected) shares in the window; rejections are wasted work and usually mean stale tips",
        ),
        Constraint::soft(
            "proposer_plurality",
            2.0,
            distinct_producers,
            Provenance::Measured,
            "distinct block proposers in the window; a single producer means K* reads ~0 because there is no plurality to disagree",
        ),
    ];

    let capability = Capability {
        compute: Tracked::derived((net_hps / ref_hps).clamp(0.0, 1.0), "net_hps against the declared reference rate"),
        energy: Tracked::unavailable(0.0, "sigil-api exposes no power telemetry — excluded from the mean, never substituted"),
        verifiable: if accept_ratio_measured {
            Tracked::measured(accept_ratio, "accepted/(accepted+rejected) shares, delta over the window")
        } else {
            Tracked::unavailable(0.0, "no shares were submitted in the window — acceptance is unobserved, not perfect")
        },
        availability: Tracked::measured(availability, "health x started x mesh coverage x block liveness"),
        expandable: Tracked::measured(expandable, "1 - Herfindahl index of hash-rate concentration across rigs"),
    };

    let design = Design {
        name: "sigil-g2-live".to_string(),
        capability,
        constraints,
        peers,
        peer_provenance: Provenance::Derived,
        coordination: Tracked::measured(k_star, "K* from flux_sigil_kgauge over this same window (lower bound: byte asymmetry unavailable)"),
    };

    let r = evaluate(&design);
    // Realization velocity, with the ONLY defensible time input available from a
    // running chain: how long one block actually takes. Gamma_R here therefore
    // reads as "realized design-equivalents per block-time" — a cadence, not a
    // construction schedule. Naming that is the difference between a number and
    // a claim.
    let secs_per_block = if blocks_added > 0.0 { tau / blocks_added } else { 0.0 };
    let v = velocity(&r, secs_per_block, 0.0, 0.0);

    let mut out = serde_json::to_value(&r).map_err(|e| e.to_string())?;
    out["ok"] = json!(true);
    out["event"] = json!(EVENT);
    out["ts_ms"] = json!(now_ms());
    out["node"] = json!(crate::handlers::sigil_wallet::rpc_base());
    out["network"] = json!("sigil-g2");
    out["definition"] = json!(
        "K_R = Theta_phys · [C·E·V·A·X]^(1/n over MEASURED terms) · ln(1+N_p^eff) / (1 + K_coord + B). \
         Theta_phys is the Landscape/Swampland gate (0 kills the score outright); B = max_j (r_j-a_j)/r_j names the binding constraint; \
         K_coord is K* from flux_sigil_kgauge over the same window. An engineering figure of merit, not a physical law."
    );
    out["gamma_r"] = json!({
        "value": v.gamma_r,
        "units": "realized design-equivalents per block-time",
        "secs_per_block": secs_per_block,
        "note": "the only time input a running chain supplies is its own block interval; this is a cadence, not a construction schedule"
    });
    out["observed"] = json!({
        "net_hps": net_hps, "reference_hps": ref_hps, "shielded_hps_pct": shielded_pct,
        "peer_count": peer_count, "fan_out": fan_out, "distinct_rigs": distinct_rigs, "live_miners": miners.len(),
        "hhi_hashrate": hhi, "blocks_added": blocks_added, "distinct_producers": distinct_producers,
        "tau_secs": tau, "accept_ratio": accept_ratio, "max_supply": max_supply, "note_range_ceiling": range_ceiling,
        "targets": {"peers": target_peers, "rigs": target_rigs, "shielded_pct": target_shielded}
    });
    out["kgauge"] = json!({
        "K_star": k_star, "K": kg.get("K").cloned().unwrap_or(json!(null)),
        "delta_H": kg.get("delta_H").cloned().unwrap_or(json!(null)),
        "delta_s_bits": kg.get("delta_s_bits").cloned().unwrap_or(json!(null)),
        "note": "K_coord in the functional IS this K* — one window, one sample, two gauges"
    });
    Ok(out)
}

fn record(reading: &Value) -> Result<String, String> {
    use std::io::Write;
    let p = series_path();
    if let Some(dir) = std::path::Path::new(&p).parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut fh = std::fs::OpenOptions::new().create(true).append(true).open(&p).map_err(|e| format!("{p}: {e}"))?;
    writeln!(fh, "{}", reading).map_err(|e| e.to_string())?;
    Ok(p)
}

fn arg_f64(a: &Value, k: &str, d: f64) -> f64 {
    a.get(k).and_then(|v| v.as_f64()).unwrap_or(d)
}
fn arg_bool(a: &Value, k: &str, d: bool) -> bool {
    a.get(k).and_then(|v| v.as_bool()).unwrap_or(d)
}

fn flux_sigil_realization(args: &Value) -> String {
    let window = arg_f64(args, "window_secs", 60.0).clamp(10.0, 600.0);
    let do_hook = arg_bool(args, "webhook", true);
    let do_record = arg_bool(args, "record", true);
    match measure(window) {
        Ok(mut r) => {
            if do_record {
                match record(&r) {
                    Ok(p) => r["recorded_to"] = json!(p),
                    Err(e) => r["record_error"] = json!(e),
                }
            }
            if do_hook {
                webhook::auto_dispatch(EVENT, r.clone());
                r["webhook_event"] = json!(EVENT);
            }
            serde_json::to_string_pretty(&r).unwrap_or_else(|_| r.to_string())
        }
        Err(e) => json!({"ok": false, "error": e, "hint": "is sigil-api reachable? FLUX_SIGIL_RPC overrides the base (default http://127.0.0.1:18181)"}).to_string(),
    }
}

fn flux_sigil_realization_watch(args: &Value) -> String {
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
                Ok(mut r) => {
                    let _ = record(&r).map(|p| r["recorded_to"] = json!(p));
                    webhook::auto_dispatch(EVENT, r);
                }
                Err(e) => webhook::auto_dispatch(EVENT, json!({"ok": false, "event": EVENT, "error": e, "ts_ms": now_ms()})),
            }
            let rest = (every - window).max(1.0);
            let mut slept = 0.0;
            while slept < rest && WATCH_RUNNING.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_secs(1));
                slept += 1.0;
            }
        }
    });
    json!({"ok": true, "running": true, "every_secs": every, "window_secs": window, "event": EVENT, "series": series_path(),
           "note": "each reading is appended to the series file and dispatched to every webhook registered for this event; lives as long as this MCP process"}).to_string()
}

fn flux_sigil_realization_series(args: &Value) -> String {
    let n = args.get("last").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
    let p = series_path();
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) => return json!({"ok": false, "error": format!("{p}: {e}")}).to_string(),
    };
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail: Vec<Value> = lines.iter().rev().take(n).rev().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let ks: Vec<f64> = tail.iter().filter_map(|r| r.get("k_r").and_then(|v| v.as_f64())).collect();
    let mean = if ks.is_empty() { 0.0 } else { ks.iter().sum::<f64>() / ks.len() as f64 };
    let (min, max) = ks.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &x| (lo.min(x), hi.max(x)));
    // Which constraint has been binding, and how often — the series-level answer
    // to "what is actually holding this network back", which one reading cannot give.
    let mut binders: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for r in &tail {
        if let Some(b) = r.get("bottleneck").and_then(|b| b.get("binding")).and_then(|b| b.as_str()) {
            *binders.entry(b.to_string()).or_insert(0) += 1;
        }
    }
    let mut ranked: Vec<(String, usize)> = binders.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    json!({"ok": true, "series": p, "total_readings": lines.len(), "returned": tail.len(),
           "k_r": {"mean": mean, "min": if ks.is_empty() {0.0} else {min}, "max": if ks.is_empty() {0.0} else {max}},
           "binding_constraint_frequency": ranked,
           "readings": tail}).to_string()
}

/// Compare candidate designs supplied by the caller — the "search the possibility
/// space without building anything" half of the idea, on arbitrary designs rather
/// than the live chain.
fn flux_sigil_realization_compare(args: &Value) -> String {
    let designs: Vec<Design> = match args.get("designs").cloned().map(serde_json::from_value::<Vec<Design>>) {
        Some(Ok(d)) => d,
        Some(Err(e)) => return json!({"ok": false, "error": format!("designs: {e}"), "hint": "each design needs name, capability{compute,energy,verifiable,availability,expandable each {value,provenance,note}}, constraints[], peers[], peer_provenance, coordination{value,provenance,note}"}).to_string(),
        None => return json!({"ok": false, "error": "designs is required (array)"}).to_string(),
    };
    if designs.is_empty() {
        return json!({"ok": false, "error": "designs is empty"}).to_string();
    }
    let times: Vec<f64> = args.get("time_to_realize").and_then(|v| v.as_array()).map(|a| a.iter().map(|x| x.as_f64().unwrap_or(1.0)).collect()).unwrap_or_else(|| vec![1.0; designs.len()]);

    let mut rows: Vec<Value> = Vec::new();
    for (i, d) in designs.iter().enumerate() {
        let r = evaluate(d);
        let t = times.get(i).copied().unwrap_or(1.0);
        let v = velocity(&r, t, 0.0, 0.0);
        rows.push(json!({
            "name": r.name, "k_r": r.k_r, "gamma_r": v.gamma_r, "time_to_realize": t,
            "in_landscape": r.feasibility.in_landscape, "limiting_factor": r.limiting_factor,
            "binding": r.bottleneck.binding, "capability_core": r.capability.core,
            "effective_peers": r.amplification.effective_peers, "friction": r.friction,
            "provenance": r.provenance, "caveats": r.caveats
        }));
    }
    let best_kr = rows.iter().max_by(|a, b| f(a, "k_r").partial_cmp(&f(b, "k_r")).unwrap_or(std::cmp::Ordering::Equal)).cloned();
    let best_gamma = rows.iter().max_by(|a, b| f(a, "gamma_r").partial_cmp(&f(b, "gamma_r")).unwrap_or(std::cmp::Ordering::Equal)).cloned();
    json!({"ok": true, "event": EVENT, "evaluated": rows.len(), "designs": rows,
           "best_by_k_r": best_kr, "best_by_gamma_r": best_gamma,
           "note": "K_R ranks single designs; Gamma_R ranks REALIZATION — a smaller replicable design routinely wins on Gamma_R while losing on K_R, which is the whole point of separating them"}).to_string()
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(ToolDef {
        name: "flux_sigil_realization",
        description: "MEASURE the Kristensen Realization Functional K_R live on SIGIL (sigil-g2) and, above all, NAME THE ONE CONSTRAINT THAT IS BINDING. K_R = Theta_phys · [C·E·V·A·X]^(1/n) · ln(1+N_p^eff) / (1 + K_coord + B): Theta_phys is the Landscape/Swampland feasibility gate (0 if any hard constraint fails — node unreachable, chain not advancing, supply above the 2^58 note-range ceiling); the capability core is a geometric mean over ONLY the measured terms (energy is excluded, not guessed — sigil-api has no power telemetry); N_p^eff weights each miner by useful×verified×reliable so Sybils add nothing; K_coord is K* from flux_sigil_kgauge over the SAME sampling window; B = max_j (r_j-a_j)/r_j and reports the binding constraint by name with a note on what relieves it. Also returns Gamma_R (realization velocity per block-time). Appends to a series file and dispatches webhook event `sigil_realization`. Args: window_secs (10..600, default 60), webhook (default true), record (default true).",
        input_schema: json!({"type":"object","properties":{"window_secs":{"type":"number"},"webhook":{"type":"boolean"},"record":{"type":"boolean"}}}),
    }, flux_sigil_realization);
    registry.register(ToolDef {
        name: "flux_sigil_realization_watch",
        description: "Always know what is binding: start a background sampler inside this MCP process that evaluates K_R on SIGIL every `every_secs` (default 300, min 60) over a `window_secs` window (default 60), appends each reading to the series file and pushes it to every registered webhook as `sigil_realization`. Args: every_secs, window_secs, stop (true to stop), status (true to report). Register a receiver first with flux_webhook_register(events:[\"sigil_realization\"]).",
        input_schema: json!({"type":"object","properties":{"every_secs":{"type":"number"},"window_secs":{"type":"number"},"stop":{"type":"boolean"},"status":{"type":"boolean"}}}),
    }, flux_sigil_realization_watch);
    registry.register(ToolDef {
        name: "flux_sigil_realization_series",
        description: "The recorded K_R series (append-only JSONL written by flux_sigil_realization / _watch): last N readings, K_R mean/min/max, and — the number one reading cannot give — how OFTEN each constraint has been the binding one. That frequency table is what tells you where to spend engineering effort. Args: last (default 20).",
        input_schema: json!({"type":"object","properties":{"last":{"type":"integer"}}}),
    }, flux_sigil_realization_series);
    registry.register(ToolDef {
        name: "flux_sigil_realization_compare",
        description: "Search the possibility space WITHOUT building anything: evaluate and rank several candidate designs under K_R and Gamma_R. Each design carries its own capability terms, hard/soft constraints, peers and coordination stress; infeasible ones (Swampland) score exactly 0 however attractive their numerator. Returns best-by-K_R and best-by-Gamma_R separately, because a small replicable design routinely loses on K_R and wins on realization velocity. Args: designs (array, required), time_to_realize (array of times, parallel to designs, default all 1.0).",
        input_schema: json!({"type":"object","properties":{"designs":{"type":"array","items":{"type":"object"}},"time_to_realize":{"type":"array","items":{"type":"number"}}},"required":["designs"]}),
    }, flux_sigil_realization_compare);
}
