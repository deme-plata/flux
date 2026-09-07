//! `flux_sigil_kgauge*` — the Kristensen consensus gauge, MEASURED live on the SIGIL
//! network and pushed to every registered webhook, so an agent driving fluxc always
//! knows how hard it currently is for distributed views to become one shared state.
//!
//! # v2 (2026-09-07) — the sensor mapping was falsified, not the idea
//!
//! Eleven live readings (2026-09-06) showed v1's `ΔH = reject ratio + peer churn` was a
//! reject/churn DETECTOR, not a consensus-pressure gauge: two peers coming online
//! (4 → 6) produced the highest K\* ever recorded (10.37) although nothing about the
//! chain's state was in dispute, and 94 % of another reading's ΔH was a single reject
//! category (`stale_height` — a late share, i.e. network noise) weighted exactly like
//! an invalid state transition. Meanwhile a window with one producer and Δs = 0 would
//! have read K = 0 even under a catastrophic state fork, because the product form
//! √(ΔH·Δs) is zeroed by either factor.
//!
//! v2 therefore separates three things v1 conflated:
//!
//! * **State disagreement** `ΔH_c = Σ wᵢ·Dᵢ` over four channels — tip divergence,
//!   state-root mismatch, finality divergence, semantic conflicts. Only a channel with
//!   a real data source contributes; the others are reported as `unavailable` and
//!   their weight goes into the UPPER bound, never silently into a zero. `ΔH_c` is a
//!   lower bound by construction and says so.
//! * **Proposer structure** — Δs (Shannon bits), its normalised form `H_norm`, the
//!   effective producer count `N_eff = 2^Δs`, and the dominant producer's share.
//! * **Network diagnostics** — peers, churn, reject rate BY KIND (noise vs semantic),
//!   block rate. Shown, never fed into ΔH_c.
//!
//! The headline is
//!
//! ```text
//! K_C = 2π · √( ΔH_c · (τ/τ₀) · [ε + (1−ε)·H_norm] )        ε = 0.1
//! ```
//!
//! with τ the MEASURED finality time (`final_depth / block rate`) and τ₀ = 100 s a fixed
//! reference. Producer plurality amplifies coordination difficulty, but the ε floor
//! means a lone producer cannot zero out real disagreement. Observer coverage Ω does
//! not multiply K_C; it sets the confidence band `[K_C_low, K_C_high]`.
//!
//! **This is a dimensionless ENGINEERING score, not a Margolus–Levitin comparison.**
//! v1 called its `K* = 2π√(ΔH·τ·Δs/ħ)` (ħ := 1) "the ML boundary at K*≈1" while showing
//! K* = 3.26 and saying the chain sat "far below" it. Both halves were wrong: ΔH was a
//! reject proxy, not an energy uncertainty in joules, so ħ has no business in the
//! formula and the boundary claim is void. The v1 numbers are retained under
//! `legacy_v1` (and as the top-level `K` / `K_star` keys other handlers and the series
//! file already depend on) with a note saying exactly this.
//!
//! Honesty is part of the payload: every channel carries a `status` and a `basis`.
//!
//! Webhook event name: `sigil_kgauge`. Series file (append-only JSONL, one reading per
//! line): `SIGIL_KGAUGE_SERIES` or the default under /home/storage/claude-code.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{json, Value};

use crate::handlers::{ToolDef, ToolRegistry};
use flux_kgauge::observables::{BlockFact, BlockWindow, CounterSample, NetworkSize, Observables};
use flux_kgauge::{GaugeConfig, KGauge};
use fluxc_webhooks::webhook;

pub const EVENT: &str = "sigil_kgauge";
pub const GAUGE_VERSION: u32 = 2;
const DEFAULT_SERIES: &str = "/home/storage/claude-code/k-parameter-paper/gauge-series/sigil-kgauge.jsonl";
/// Steady-state SIGIL block rate used ONLY for the legacy diagnostic block-rate deviation.
const TARGET_BPS: f64 = 0.83;
/// Λ (commitment) denominator: κ·τ_confirm blocks, window-limited by construction.
const COMMIT_BLOCKS: f64 = 18.0 * 100.0;
/// `sigil_dagknight::BraidConfig::default().final_depth` — finality is depth, τ = depth / rate.
pub const FINAL_DEPTH: f64 = 512.0;
/// Reference finality time for τ/τ₀. Fixed, documented, NOT fitted: SIGIL's design intent
/// is finality within ~1.5–2 min; 100 s makes τ/τ₀ ≈ 1 at ~5 blk/s.
pub const TAU0_SECS: f64 = 100.0;
/// ε floor: producer plurality amplifies coordination difficulty but cannot zero out
/// real disagreement (one producer + a state fork must still read K_C > 0).
pub const EPSILON_FLOOR: f64 = 0.1;
/// Channel weights. State-root mismatch carries the most because agreement on state is
/// the strongest evidence of consensus there is; it is also the channel SIGIL cannot
/// measure today, which is why the upper bound never collapses to the point estimate.
pub const W_TIP: f64 = 0.20;
pub const W_STATE: f64 = 0.35;
pub const W_FINALITY: f64 = 0.20;
pub const W_CONFLICT: f64 = 0.25;

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

// ─────────────────────────────────────────────────────────────────────────────
// Reject taxonomy — the word "reject" is not enough.
// ─────────────────────────────────────────────────────────────────────────────

/// What a mining-reject category tells us about CONSENSUS. `sigil-api`'s
/// `RejectKind` today is {stale_height, non_canonical_wallet, duplicate,
/// verify_mismatch, no_tip}; the extra names are future-proofing for node-side
/// categories that would clearly be semantic if they appeared.
pub(crate) fn reject_class(kind: &str) -> &'static str {
    match kind {
        // A share for a height the tip already moved past, a resend, or "no tip yet":
        // network timing, nothing about state is in dispute.
        "stale_height" | "duplicate" | "already_known" | "late" | "no_tip" => "noise",
        // The proof does not verify / the submission violates protocol rules:
        // a competing or invalid claim about state.
        "verify_mismatch" | "non_canonical_wallet" | "invalid_signature" | "bad_proof"
        | "invalid_state" | "state_root_mismatch" | "conflicting_spend" | "wrong_parent"
        | "consensus_rule" => "semantic",
        _ => "unclassified",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The pure gauge — no network, fully testable.
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub(crate) struct ConsensusInputs {
    pub blocks: usize,
    pub merging_blocks: usize,
    pub red_blocks: usize,
    pub accepted_delta: u64,
    pub rejects_delta: HashMap<String, u64>,
    pub peer_heights: Vec<u64>,
    pub local_height: u64,
    pub distinct_producers: usize,
    pub entropy_bits: f64,
    pub dominant_count: u64,
    pub block_rate_bps: f64,
    pub peers: u64,
    pub tau_window_secs: f64,
}

#[derive(Clone, Debug)]
pub(crate) struct Channel {
    pub name: &'static str,
    pub weight: f64,
    pub value: Option<f64>,
    pub basis: String,
}

impl Channel {
    fn json(&self) -> Value {
        json!({
            "value": self.value, "weight": self.weight,
            "status": if self.value.is_some() { "measured" } else { "unavailable" },
            "basis": self.basis,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ConsensusReading {
    pub k_c: f64,
    pub k_c_low: f64,
    pub k_c_high: f64,
    pub regime: &'static str,
    pub confidence: &'static str,
    pub delta_h_consensus: f64,
    pub missing_weight: f64,
    pub channels: Vec<Channel>,
    pub h_norm: f64,
    pub n_eff: f64,
    pub dominant_share: f64,
    pub entropy_resolution_bits: f64,
    pub tau_finality_secs: f64,
    pub tau_ratio: f64,
    pub tau_basis: &'static str,
    pub omega: f64,
    pub rejects_noise: u64,
    pub rejects_semantic: u64,
    pub rejects_unclassified: u64,
    pub reject_rate_total: f64,
    pub reject_rate_semantic: f64,
    pub rejects_by_kind: HashMap<String, (u64, &'static str)>,
}

fn shannon_bits_of(counts: impl Iterator<Item = u64>) -> f64 {
    let v: Vec<u64> = counts.collect();
    let total: u64 = v.iter().sum();
    if total == 0 { return 0.0; }
    let n = total as f64;
    (-v.iter().map(|&c| { let p = c as f64 / n; if p > 0.0 { p * p.log2() } else { 0.0 } }).sum::<f64>()).max(0.0)
}

/// The smallest non-zero Δs a window of `n` blocks can show: H(n−1/n, 1/n). Over the
/// node's 200-block feed that is 0.0454 bits — Δs is a staircase, not a smooth signal,
/// and the reader should know the step size.
pub(crate) fn entropy_resolution_bits(n: usize) -> f64 {
    if n < 2 { return 0.0; }
    shannon_bits_of([n as u64 - 1, 1].into_iter())
}

pub(crate) fn k_c_of(delta_h: f64, tau_ratio: f64, h_norm: f64) -> f64 {
    let plur = EPSILON_FLOOR + (1.0 - EPSILON_FLOOR) * h_norm.clamp(0.0, 1.0);
    2.0 * std::f64::consts::PI * (delta_h.max(0.0) * tau_ratio.max(0.0) * plur).sqrt()
}

pub(crate) fn regime_of(k_c: f64) -> &'static str {
    if k_c < 1.0 { "stable" } else if k_c < 3.0 { "elevated" } else { "critical" }
}

/// Ω = 1 − e^(−peers/n_total) with n_total = max(peers+2, 8) SATURATES at 1 − e⁻¹ ≈ 0.632
/// (it assumes we see all but two nodes), so the thresholds live inside that range:
/// 5 peers → 0.465 medium, 4 → 0.393 low, 40 → 0.614 high.
pub(crate) fn confidence_of(omega: f64) -> &'static str {
    if omega >= 0.55 { "high" } else if omega >= 0.40 { "medium" } else { "low" }
}

pub(crate) fn consensus_gauge(i: &ConsensusInputs) -> ConsensusReading {
    let n = i.blocks.max(1) as f64;

    // ── rejects by kind ──
    let mut by_kind: HashMap<String, (u64, &'static str)> = HashMap::new();
    let (mut noise, mut semantic, mut unclassified) = (0u64, 0u64, 0u64);
    for (k, &v) in &i.rejects_delta {
        let class = reject_class(k);
        match class { "noise" => noise += v, "semantic" => semantic += v, _ => unclassified += v }
        by_kind.insert(k.clone(), (v, class));
    }
    let rej_total = noise + semantic + unclassified;
    let denom = i.accepted_delta + rej_total;
    let rate = |x: u64| if denom > 0 { x as f64 / denom as f64 } else { 0.0 };

    // ── the four disagreement channels ──
    let d_tip = i.merging_blocks as f64 / n;
    let red_frac = i.red_blocks as f64 / n;
    let d_conflict = (red_frac + rate(semantic)).min(1.0);
    let d_finality = if i.peer_heights.is_empty() { None } else {
        let m = i.peer_heights.iter().map(|&h| ((h as f64 - i.local_height as f64).abs() / FINAL_DEPTH).min(1.0)).sum::<f64>() / i.peer_heights.len() as f64;
        Some(m)
    };
    let channels = vec![
        Channel { name: "tip_divergence", weight: W_TIP, value: Some(d_tip),
            basis: format!("{} of {} blocks in the window carry merge-parents (the frontier had >1 tip that had to be reconciled); 0 on a straight chain is a measurement, not a gap", i.merging_blocks, i.blocks) },
        Channel { name: "state_root_mismatch", weight: W_STATE, value: None,
            basis: "no cross-node state root on sigil-api — peers do not publish theirs, so this node cannot compare; the strongest consensus evidence is the one SIGIL cannot measure today".into() },
        Channel { name: "finality_divergence", weight: W_FINALITY, value: d_finality,
            basis: if d_finality.is_some() { format!("mean |h_peer − h_local| / final_depth({}) over {} peer height(s)", FINAL_DEPTH as u64, i.peer_heights.len()) }
                   else { "peer_heights map on /v1/network/topology is empty (flux-p2p mesh_health not fed by sigil-node); nothing to compare".into() } },
        Channel { name: "semantic_conflicts", weight: W_CONFLICT, value: Some(d_conflict),
            basis: format!("red (non-blue) blocks {}/{} + semantic reject ratio {:.4} ({} verify_mismatch/non_canonical of {} submissions); stale_height/duplicate/no_tip are NOISE and excluded", i.red_blocks, i.blocks, rate(semantic), semantic, denom) },
    ];
    let delta_h: f64 = channels.iter().filter_map(|c| c.value.map(|v| c.weight * v)).sum();
    let missing_weight: f64 = channels.iter().filter(|c| c.value.is_none()).map(|c| c.weight).sum();

    // ── proposer structure ──
    let h_norm = if i.distinct_producers >= 2 { (i.entropy_bits / (i.distinct_producers as f64).log2()).clamp(0.0, 1.0) } else { 0.0 };
    let n_eff = 2f64.powf(i.entropy_bits.max(0.0));
    let dominant_share = if i.blocks > 0 { i.dominant_count as f64 / n } else { 0.0 };

    // ── τ: real convergence time, not the sampling window ──
    let (tau_fin, tau_basis) = if i.block_rate_bps > 0.0 { (FINAL_DEPTH / i.block_rate_bps, "final_depth / measured block rate") } else { (i.tau_window_secs, "no blocks in window → sampling window (fallback)") };
    let tau_ratio = tau_fin / TAU0_SECS;

    // ── Ω → confidence band, not a multiplier ──
    let n_total = (i.peers + 2).max(8) as f64;
    let omega = 1.0 - (-(i.peers as f64) / n_total).exp();
    let unclassified_rate = rate(unclassified) * W_CONFLICT;
    let d_lo = delta_h * omega;
    let seen = (delta_h + missing_weight + unclassified_rate).min(1.0);
    let d_hi = (seen + (1.0 - omega) * (1.0 - seen)).min(1.0);

    let k_c = k_c_of(delta_h, tau_ratio, h_norm);
    ConsensusReading {
        k_c, k_c_low: k_c_of(d_lo, tau_ratio, h_norm), k_c_high: k_c_of(d_hi, tau_ratio, h_norm),
        regime: regime_of(k_c), confidence: confidence_of(omega),
        delta_h_consensus: delta_h, missing_weight, channels,
        h_norm, n_eff, dominant_share, entropy_resolution_bits: entropy_resolution_bits(i.blocks),
        tau_finality_secs: tau_fin, tau_ratio, tau_basis, omega,
        rejects_noise: noise, rejects_semantic: semantic, rejects_unclassified: unclassified,
        reject_rate_total: rate(rej_total), reject_rate_semantic: rate(semantic), rejects_by_kind: by_kind,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sampling the live node
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
struct Sample { ts_ms: u64, height: u64, shares_accepted: u64, rejects_by_kind: HashMap<String, u64>, peers: u64, peer_heights: Vec<u64> }

impl Sample { fn rejects(&self) -> u64 { self.rejects_by_kind.values().sum() } }

fn sample() -> Result<Sample, String> {
    let miners = get_json("/v1/mining/miners")?;
    let topo = get_json("/v1/network/topology")?;
    let m = miners.get("data").cloned().unwrap_or(miners);
    let t = topo.get("data").cloned().unwrap_or(topo);
    let mut rejects_by_kind = HashMap::new();
    if let Some(arr) = m.get("rejects").and_then(|r| r.as_array()) {
        for e in arr {
            let kind = e.get(0).and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
            let n = e.get(1).and_then(|v| v.as_u64()).unwrap_or(0);
            *rejects_by_kind.entry(kind).or_insert(0) += n;
        }
    }
    let peer_heights = t.get("peer_heights").and_then(|v| v.as_object()).map(|o| o.values().filter_map(|v| v.as_u64()).filter(|&h| h > 0).collect()).unwrap_or_default();
    Ok(Sample {
        ts_ms: now_ms(),
        height: m.get("height").and_then(|v| v.as_u64()).unwrap_or(0),
        shares_accepted: m.get("shares_accepted").and_then(|v| v.as_u64()).unwrap_or(0),
        rejects_by_kind,
        peers: t.get("peer_count").and_then(|v| v.as_u64()).unwrap_or(0),
        peer_heights,
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

fn legacy_regime(k: f64) -> &'static str { if k < 5.0 { "stable" } else if k < 10.0 { "approaching" } else { "critical" } }

/// One complete reading: two counter samples `window_secs` apart, the blocks that landed
/// in between (or, on a quiet window, the most recent ones — flagged), the v2 consensus
/// gauge, the legacy v1 numbers, and the crate's reference gauge over the same observables.
pub(crate) fn measure(window_secs: f64) -> Result<Value, String> {
    let a = sample()?;
    std::thread::sleep(Duration::from_secs_f64(window_secs));
    let b = sample()?;
    let all_blocks = recent_blocks()?;
    let tau = ((b.ts_ms.saturating_sub(a.ts_ms)) as f64 / 1000.0).max(window_secs * 0.5);

    // Blocks produced INSIDE the window; fall back to the recent set if the feed has none.
    let in_window: Vec<&Value> = all_blocks.iter().filter(|bk| bk.get("height").and_then(|h| h.as_u64()).unwrap_or(0) > a.height).collect();
    let (blocks, window_only): (Vec<&Value>, bool) = if in_window.len() >= 2 { (in_window, true) } else { (all_blocks.iter().collect(), false) };

    let mut counts: HashMap<[u8; 32], u64> = HashMap::new();
    for bk in &blocks { *counts.entry(bytes32(bk.get("producer"))).or_insert(0) += 1; }
    let ds = shannon_bits_of(counts.values().copied());
    let distinct = counts.len();
    let dominant_count = counts.values().copied().max().unwrap_or(0);
    let blue_false = blocks.iter().filter(|bk| bk.get("is_blue").and_then(|v| v.as_bool()) == Some(false)).count();
    let merging = blocks.iter().filter(|bk| bk.get("merge_parents").and_then(|m| m.as_array()).map(|m| !m.is_empty()).unwrap_or(false)).count();

    // Rejects and accepts are BOTH cumulative counters on /v1/mining/miners; delta-over-delta.
    let sub_delta = b.shares_accepted.saturating_sub(a.shares_accepted);
    let mut rejects_delta: HashMap<String, u64> = HashMap::new();
    for (k, &vb) in &b.rejects_by_kind {
        let va = a.rejects_by_kind.get(k).copied().unwrap_or(0);
        let d = vb.saturating_sub(va);
        if d > 0 { rejects_delta.insert(k.clone(), d); }
    }
    let rej_delta: u64 = rejects_delta.values().sum();
    let rej_ratio = if sub_delta + rej_delta > 0 { rej_delta as f64 / (sub_delta + rej_delta) as f64 } else { 0.0 };
    let churn = if a.peers > 0 { (b.peers as f64 - a.peers as f64).abs() / a.peers as f64 } else { 0.0 };
    let d_commit = b.height.saturating_sub(a.height);
    let obs_bps = d_commit as f64 / tau;

    // ── v2: the consensus gauge ──
    let inputs = ConsensusInputs {
        blocks: blocks.len(), merging_blocks: merging, red_blocks: blue_false,
        accepted_delta: sub_delta, rejects_delta: rejects_delta.clone(),
        peer_heights: b.peer_heights.clone(), local_height: b.height,
        distinct_producers: distinct, entropy_bits: ds, dominant_count,
        block_rate_bps: obs_bps, peers: b.peers, tau_window_secs: tau,
    };
    let c = consensus_gauge(&inputs);

    // ── legacy v1 (retained for series continuity; NOT the headline) ──
    let dh_v1 = rej_ratio + churn;
    let k_v1 = if tau > 0.0 { 2.0 * std::f64::consts::PI * (dh_v1 * ds).sqrt() / tau } else { 0.0 };
    let k_star_v1 = 2.0 * std::f64::consts::PI * (dh_v1 * tau * ds).sqrt();
    let br_dev = (obs_bps - TARGET_BPS).abs() / TARGET_BPS;
    let lambda = 1.0 - (-(d_commit as f64) / COMMIT_BLOCKS).exp();

    // ── reference gauge: flux-kgauge Eq. 10/25 over the SAME observables ──
    let net_height = b.peer_heights.iter().copied().max().unwrap_or(0); // 0 → flux-kgauge marks sync_divergence absent
    let to_counter = |s: &Sample| CounterSample {
        mining_submitted: s.shares_accepted + s.rejects(), mining_accepted: s.shares_accepted,
        p2p_bytes_in: 0, p2p_bytes_out: 0, peer_count: s.peers, local_height: s.height, network_height: net_height,
    };
    let facts: Vec<BlockFact> = blocks.iter().map(|bk| BlockFact {
        height: bk.get("height").and_then(|h| h.as_u64()).unwrap_or(0),
        producer: bytes32(bk.get("producer")),
        merge_parent_count: bk.get("merge_parents").and_then(|m| m.as_array()).map(|m| m.len() as u32).unwrap_or(0),
        blue_score: bk.get("blue_score").and_then(|v| v.as_u64()),
        is_blue: bk.get("is_blue").and_then(|v| v.as_bool()),
    }).collect();
    let n_total = (b.peers + 2).max(8) as f64;
    let obs = Observables { previous: to_counter(&a), current: to_counter(&b), window_secs: tau, window: BlockWindow { blocks: facts }, network_size: NetworkSize::Estimated(n_total as u64), kappa: 0.0 };
    let report = KGauge::new(GaugeConfig::sigil()).observe(&obs);
    let reference = json!({
        "definition": "flux-kgauge (Theoretical Minimum v4) Eq.10 base / Eq.25 enhanced — a STRESS score, low = healthy; not the same quantity as K_C",
        "k_base": report.base.k_base, "k_enhanced": report.k_enhanced,
        "phase": report.phase.as_str(), "confidence": format!("{:?}", report.base.confidence),
        "provenance": format!("{:?}", report.base.provenance), "actionable": report.is_actionable(),
        "caveats": report.caveats.iter().map(|c| c.detail.clone()).collect::<Vec<_>>(),
    });

    let channels_json: serde_json::Map<String, Value> = c.channels.iter().map(|ch| (ch.name.to_string(), ch.json())).collect();
    let by_kind_json: serde_json::Map<String, Value> = c.rejects_by_kind.iter().map(|(k, (n, class))| (k.clone(), json!({"delta": n, "class": class}))).collect();
    let lifetime_json: serde_json::Map<String, Value> = b.rejects_by_kind.iter().map(|(k, n)| (k.clone(), json!(n))).collect();
    let measured = ["proposer entropy (Δs)", "tip divergence (DAG merge-parents)", "semantic conflicts (red blocks + verify_mismatch)", "block rate → finality time τ", "mining rejects BY KIND", "peer count / churn (diagnostic only)"];
    let unavailable: Vec<String> = c.channels.iter().filter(|ch| ch.value.is_none()).map(|ch| format!("{} — {}", ch.name, ch.basis)).chain(["P2P byte asymmetry (no byte counters on sigil-api)".to_string()]).collect();

    Ok(json!({
        "ok": true, "event": EVENT, "version": GAUGE_VERSION, "ts_ms": b.ts_ms, "node": crate::handlers::sigil_wallet::rpc_base(),
        "network": "sigil-g2",
        "K_C": c.k_c, "K_C_low": c.k_c_low, "K_C_high": c.k_c_high,
        "regime": c.regime, "confidence": c.confidence,
        "definition": "K_C = 2π·√(ΔH_c · (τ/τ₀) · [ε + (1−ε)·H_norm]); ΔH_c = Σ wᵢ·Dᵢ over the AVAILABLE state-disagreement channels (tip 0.20, state-root 0.35, finality 0.20, semantic 0.25) — a LOWER bound; ε = 0.1; H_norm = Δs / log₂(N_producers); τ = final_depth/block-rate (measured finality time), τ₀ = 100 s. Dimensionless ENGINEERING score, no ħ — NOT a Margolus–Levitin comparison. Ω sets the confidence band, it does not multiply K_C. Regime: <1 stable · 1–3 elevated · ≥3 critical (provisional thresholds).",
        "reading": format!("K_C = {:.3} [{:.3}, {:.3}] · Ω = {:.3} ({} confidence) · ΔH_c = {:.4} (lower bound, {:.0}% of channel weight unmeasured) · N_eff = {:.2} of {} producer(s), dominant {:.1}%",
            c.k_c, c.k_c_low, c.k_c_high, c.omega, c.confidence, c.delta_h_consensus, c.missing_weight * 100.0, c.n_eff, distinct, c.dominant_share * 100.0),
        "state_disagreement": {
            "delta_H_consensus": c.delta_h_consensus, "lower_bound": true, "missing_weight": c.missing_weight,
            "channels": channels_json,
        },
        "proposer_structure": {
            "entropy_bits": ds, "entropy_norm": c.h_norm, "effective_producers": c.n_eff,
            "active_producers": distinct, "dominant_share": c.dominant_share,
            "entropy_resolution_bits": c.entropy_resolution_bits,
            "note": format!("Δs over {} blocks is a staircase with step {:.4} bits (199/1 → 0.0454, 198/2 → 0.0808); N_eff = 2^Δs is the intuitive figure", blocks.len(), c.entropy_resolution_bits),
        },
        "network_diagnostics": {
            "peers": b.peers, "peers_start": a.peers, "peer_churn": churn,
            "reject_rate": c.reject_rate_total, "reject_rate_semantic": c.reject_rate_semantic,
            "rejects_noise": c.rejects_noise, "rejects_semantic": c.rejects_semantic, "rejects_unclassified": c.rejects_unclassified,
            "rejects_by_kind": by_kind_json, "rejects_lifetime_by_kind": lifetime_json,
            "shares_accepted_delta": sub_delta, "block_rate_bps": obs_bps,
            "note": "peer churn and noise rejects are shown here and deliberately NOT part of ΔH_c — a peer coming online is topology, not disagreement",
        },
        "tau": { "finality_secs": c.tau_finality_secs, "tau0_secs": TAU0_SECS, "ratio": c.tau_ratio, "basis": c.tau_basis, "final_depth": FINAL_DEPTH, "sampling_window_secs": tau },
        "omega_observer_coverage": c.omega, "lambda_commitment": lambda,
        "window": { "blocks": blocks.len(), "window_only": window_only, "distinct_producers": distinct,
                    "tip_start": a.height, "tip_end": b.height, "blocks_added": d_commit,
                    "block_rate_bps": obs_bps, "block_rate_dev_vs_target": br_dev, "target_bps_diagnostic": TARGET_BPS,
                    "is_blue_false": blue_false, "merging_blocks": merging },
        "inputs": { "reject_ratio": rej_ratio, "rejects_delta": rej_delta, "rejects_lifetime": b.rejects(), "shares_accepted_delta": sub_delta,
                    "peers_start": a.peers, "peers_end": b.peers, "peer_churn": churn, "peer_heights_seen": b.peer_heights.len() },
        // Compatibility: these top-level keys are read by sigil_realization and the series stats.
        "K": k_v1, "K_star": k_star_v1, "delta_H": dh_v1, "delta_s_bits": ds, "tau_secs": tau, "dominant": if c.delta_h_consensus > 0.0 { "state disagreement (ΔH_c)" } else if ds > 0.0 { "proposer plurality (Δs)" } else { "none (quiet chain)" },
        "legacy_v1": {
            "K": k_v1, "K_star": k_star_v1, "delta_H": dh_v1, "regime": legacy_regime(k_v1),
            "definition": "v1: K = 2π·√(ΔH·Δs)/τ, K* = 2π·√(ΔH·τ·Δs) with ħ:=1 and ΔH = reject ratio + peer churn.",
            "note": "Retained for series continuity only. Measured 2026-09-06 to be a reject/churn detector: two peers joining (4→6) produced the max K* (10.37) with nothing in dispute. ΔH here is not an energy, so ħ:=1 does not make K* a Margolus–Levitin number and 'K*≈1 = ML boundary' does not apply. Read K_C.",
        },
        "provenance": {
            "measured": measured,
            "unavailable": unavailable,
            "diagnostic_only": ["peer churn", "noise rejects (stale_height/duplicate/no_tip)", "block-rate deviation vs 0.83/s"],
            "note": if window_only { "blocks counted are exactly those produced inside the window" } else { "fewer than 2 blocks landed inside the window; Δs and DAG channels computed over the node's recent-block feed instead" },
            "next_to_wire_on_the_node": ["publish state root + tip hash in the peer-heights heartbeat so state_root_mismatch and finality_divergence become measurable", "feed flux-p2p mesh_health.peer_heights per peer_id from the peer-heights topic handler"],
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

fn stats(vals: &[f64]) -> Value {
    if vals.is_empty() { return json!({"mean": 0.0, "min": 0.0, "max": 0.0, "n": 0}); }
    let mean = vals.iter().sum::<f64>() / vals.len() as f64;
    let (min, max) = vals.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &x| (lo.min(x), hi.max(x)));
    json!({"mean": mean, "min": min, "max": max, "n": vals.len()})
}

fn flux_sigil_kgauge_series(args: &Value) -> String {
    let n = args.get("last").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
    let p = series_path();
    let text = match std::fs::read_to_string(&p) { Ok(t) => t, Err(e) => return json!({"ok": false, "error": format!("{p}: {e}")}).to_string() };
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail: Vec<Value> = lines.iter().rev().take(n).rev().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let kc: Vec<f64> = tail.iter().filter_map(|r| r.get("K_C").and_then(|v| v.as_f64())).collect();
    let ks: Vec<f64> = tail.iter().filter_map(|r| r.get("K_star").and_then(|v| v.as_f64())).collect();
    let v2 = tail.iter().filter(|r| r.get("version").and_then(|v| v.as_u64()) == Some(2)).count();
    json!({"ok": true, "series": p, "total_readings": lines.len(), "returned": tail.len(), "v2_readings": v2,
           "K_C": stats(&kc),
           "K_star_legacy_v1": stats(&ks),
           "note": "K_C (v2, consensus gauge) is the headline; K_star is the retained v1 reject/churn detector, kept only so the pre-2026-09-07 rows stay comparable",
           "readings": tail}).to_string()
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(ToolDef {
        name: "flux_sigil_kgauge",
        description: "MEASURE the Kristensen consensus gauge K_C live on SIGIL (sigil-g2): samples the node twice `window_secs` apart and returns K_C = 2π√(ΔH_c·(τ/τ₀)·[ε+(1−ε)·H_norm]) with a confidence band from observer coverage Ω. ΔH_c is STATE DISAGREEMENT over four weighted channels — tip divergence (DAG merge-parents), state-root mismatch (unavailable on sigil-api today, reported as such), finality divergence (peer heights, if published), semantic conflicts (red blocks + verify_mismatch rejects) — a lower bound; peer churn and stale_height/duplicate rejects are network DIAGNOSTICS and never enter ΔH_c. Also proposer structure (Δs bits, N_eff = 2^Δs, dominant share, staircase resolution), τ = measured finality time (512/block-rate), the legacy v1 K/K* (retained, NOT a Margolus–Levitin number), and the flux-kgauge crate's Eq.10/25 reference score. Appends to the series file and dispatches event `sigil_kgauge` to every registered webhook. Args: window_secs (10..600, default 60), webhook (default true), record (default true).",
        input_schema: json!({"type":"object","properties":{"window_secs":{"type":"number"},"webhook":{"type":"boolean"},"record":{"type":"boolean"}}}),
    }, flux_sigil_kgauge);
    registry.register(ToolDef {
        name: "flux_sigil_kgauge_watch",
        description: "Always know K_C: start a background sampler inside this MCP process that measures the SIGIL consensus gauge every `every_secs` (default 300, min 60) over a `window_secs` window (default 60), appends each reading to the series file and pushes it to every registered webhook as `sigil_kgauge`. Args: every_secs, window_secs, stop (true to stop), status (true to report). Register a receiver first with flux_webhook_register(events:[\"sigil_kgauge\"]).",
        input_schema: json!({"type":"object","properties":{"every_secs":{"type":"number"},"window_secs":{"type":"number"},"stop":{"type":"boolean"},"status":{"type":"boolean"}}}),
    }, flux_sigil_kgauge_watch);
    registry.register(ToolDef {
        name: "flux_sigil_kgauge_series",
        description: "The recorded SIGIL K-gauge series (append-only JSONL written by flux_sigil_kgauge / _watch): last N readings plus K_C mean/min/max (v2 rows) and legacy K* stats (v1 rows) — the data a paper or a letter should quote. Args: last (default 20).",
        input_schema: json!({"type":"object","properties":{"last":{"type":"integer"}}}),
    }, flux_sigil_kgauge_series);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ConsensusInputs {
        ConsensusInputs { blocks: 200, merging_blocks: 0, red_blocks: 0, accepted_delta: 150, rejects_delta: HashMap::new(),
            peer_heights: vec![], local_height: 4_000_000, distinct_producers: 2, entropy_bits: 0.0454146923337941, dominant_count: 199,
            block_rate_bps: 4.45, peers: 5, tau_window_secs: 60.0 }
    }

    #[test]
    fn peer_churn_does_not_move_consensus_pressure() {
        // v1's worst reading: 4 → 6 peers, nothing in dispute, K* = 10.37.
        let quiet = consensus_gauge(&base());
        let mut churned = base(); churned.peers = 6; // churn is a diagnostic input only
        let c = consensus_gauge(&churned);
        assert_eq!(quiet.delta_h_consensus, c.delta_h_consensus);
        assert_eq!(quiet.k_c, c.k_c);
        assert_eq!(quiet.regime, "stable");
    }

    #[test]
    fn stale_height_rejects_are_noise_not_disagreement() {
        let mut i = base();
        i.rejects_delta.insert("stale_height".into(), 2_000); // 93 % reject rate — v1 read ΔH = 0.943
        let c = consensus_gauge(&i);
        assert_eq!(c.rejects_noise, 2_000);
        assert_eq!(c.rejects_semantic, 0);
        assert!(c.reject_rate_total > 0.9, "diagnostic still shows it: {}", c.reject_rate_total);
        assert_eq!(c.delta_h_consensus, 0.0, "but it is not consensus pressure");
        assert_eq!(c.k_c, 0.0);
    }

    #[test]
    fn verify_mismatch_is_semantic_and_lights_the_conflict_channel() {
        let mut i = base();
        i.rejects_delta.insert("verify_mismatch".into(), 50); // 50 of 200 submissions
        let c = consensus_gauge(&i);
        assert_eq!(c.rejects_semantic, 50);
        let conflict = c.channels.iter().find(|ch| ch.name == "semantic_conflicts").unwrap().value.unwrap();
        assert!((conflict - 0.25).abs() < 1e-9, "{conflict}");
        assert!((c.delta_h_consensus - W_CONFLICT * 0.25).abs() < 1e-9);
        assert!(c.k_c > 0.0);
    }

    #[test]
    fn one_producer_cannot_zero_out_a_real_fork() {
        // v1: Δs = 0 ⇒ K = 0 whatever ΔH did. v2: the ε floor keeps K_C > 0.
        let mut i = base();
        i.distinct_producers = 1; i.entropy_bits = 0.0; i.dominant_count = 200;
        i.merging_blocks = 200; i.red_blocks = 200; // every block merged, every block red
        let c = consensus_gauge(&i);
        assert_eq!(c.h_norm, 0.0);
        assert!(c.delta_h_consensus > 0.0);
        assert!(c.k_c > 0.0, "K_C must not vanish with a lone producer: {}", c.k_c);
        let expected = 2.0 * std::f64::consts::PI * (c.delta_h_consensus * c.tau_ratio * EPSILON_FLOOR).sqrt();
        assert!((c.k_c - expected).abs() < 1e-9);
        assert_ne!(c.regime, "stable");
    }

    #[test]
    fn plurality_amplifies_but_is_bounded_by_2pi_at_full_disagreement() {
        let mut i = base();
        i.merging_blocks = 200; i.red_blocks = 200; i.entropy_bits = 1.0; i.dominant_count = 100; i.block_rate_bps = FINAL_DEPTH / TAU0_SECS;
        i.peer_heights = vec![i.local_height + 5_000];
        let c = consensus_gauge(&i);
        assert!((c.h_norm - 1.0).abs() < 1e-9);
        assert!((c.tau_ratio - 1.0).abs() < 1e-9);
        // ΔH_c = 0.20·1 + 0.20·1 + 0.25·1 = 0.65 (state root still unmeasured)
        assert!((c.delta_h_consensus - 0.65).abs() < 1e-9, "{}", c.delta_h_consensus);
        assert!(c.k_c < 2.0 * std::f64::consts::PI);
        assert_eq!(c.regime, "critical");
    }

    #[test]
    fn omega_widens_the_band_instead_of_scaling_the_point() {
        let mut few = base(); few.peers = 2;
        let mut many = base(); many.peers = 40;
        let (a, b) = (consensus_gauge(&few), consensus_gauge(&many));
        assert_eq!(a.k_c, b.k_c, "Ω must not multiply K_C");
        assert!(a.omega < b.omega);
        assert!(a.k_c_high > b.k_c_high, "less coverage → wider upper bound");
        assert_eq!(a.confidence, "low");
        assert_eq!(b.confidence, "high");
        assert!(a.k_c_low <= a.k_c && a.k_c <= a.k_c_high);
    }

    #[test]
    fn unmeasured_channels_keep_the_upper_bound_open() {
        let c = consensus_gauge(&base());
        assert_eq!(c.k_c, 0.0);
        assert!((c.missing_weight - (W_STATE + W_FINALITY)).abs() < 1e-9);
        assert!(c.k_c_high > 0.0, "cannot rule out a state fork we do not measure");
        let m = c.channels.iter().find(|ch| ch.name == "state_root_mismatch").unwrap();
        assert!(m.value.is_none());
    }

    #[test]
    fn effective_producers_and_dominant_share_match_the_hand_calculation() {
        let c = consensus_gauge(&base());
        assert!((c.n_eff - 2f64.powf(0.0454146923337941)).abs() < 1e-12);
        assert!(c.n_eff > 1.03 && c.n_eff < 1.04, "{}", c.n_eff);
        assert!((c.dominant_share - 0.995).abs() < 1e-9);
        assert!((c.entropy_resolution_bits - 0.0454146923337941).abs() < 1e-9, "199/1 step");
        assert!((entropy_resolution_bits(200) - 0.0454146923337941).abs() < 1e-9);
    }

    #[test]
    fn tau_is_finality_time_not_the_sampling_window() {
        let c = consensus_gauge(&base());
        assert!((c.tau_finality_secs - FINAL_DEPTH / 4.45).abs() < 1e-9);
        assert_eq!(c.tau_basis, "final_depth / measured block rate");
        let mut i = base(); i.block_rate_bps = 0.0;
        let c = consensus_gauge(&i);
        assert_eq!(c.tau_finality_secs, 60.0);
    }

    #[test]
    fn finality_divergence_reads_peer_heights_when_present() {
        let mut i = base();
        i.peer_heights = vec![i.local_height, i.local_height - 256];
        let c = consensus_gauge(&i);
        let f = c.channels.iter().find(|ch| ch.name == "finality_divergence").unwrap().value.unwrap();
        assert!((f - 0.25).abs() < 1e-9, "{f}"); // mean(0, 256/512)
        assert!((c.missing_weight - W_STATE).abs() < 1e-9);
    }

    #[test]
    fn reject_taxonomy() {
        assert_eq!(reject_class("stale_height"), "noise");
        assert_eq!(reject_class("duplicate"), "noise");
        assert_eq!(reject_class("no_tip"), "noise");
        assert_eq!(reject_class("verify_mismatch"), "semantic");
        assert_eq!(reject_class("non_canonical_wallet"), "semantic");
        assert_eq!(reject_class("something_new"), "unclassified");
        let mut i = base();
        i.rejects_delta.insert("something_new".into(), 100);
        let c = consensus_gauge(&i);
        assert_eq!(c.delta_h_consensus, 0.0, "unclassified never enters the point estimate");
        assert!(c.k_c_high > consensus_gauge(&base()).k_c_high, "but it widens the upper bound");
    }

    #[test]
    fn no_margolus_levitin_claim_survives_in_v2_text() {
        // The v1 topbar said "K*≈1 = ML boundary" and "the live chain sits far below it"
        // while showing K* = 3.26. The v2 definition must not repeat it.
        let mut registry = ToolRegistry::new();
        register(&mut registry);
        let d = registry.tools_schema().into_iter().find(|t| t.get("name").and_then(|n| n.as_str()) == Some("flux_sigil_kgauge"))
            .and_then(|t| t.get("description").and_then(|d| d.as_str()).map(|d| d.to_lowercase())).unwrap();
        assert!(!d.contains("k*≈1 = margolus"), "{d}");
        assert!(d.contains("not a margolus"));
    }
}
