//! The SIGIL datacenter clock face: one JSON reading that carries the three clocks, the
//! Earth hand, and the ledger as a CRT clock, every number labelled.

use crate::crt;
use crate::earth;
use crate::kristensen::{ThreeClocks, E_CORE_15W_1S_J};
use crate::Label;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Default ledger hands: nine primes whose product 223,092,870 exceeds any g2 height for years.
pub const DEFAULT_LEDGER_PERIODS: [u64; 9] = [2, 3, 5, 7, 11, 13, 17, 19, 23];
/// Webhook event name for a face reading.
pub const EVENT: &str = "sigil_earth_clock";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaceInputs {
    pub node: String,
    pub unix_s: f64,
    pub ut1_minus_utc_s: Option<f64>,
    pub height: u64,
    pub bps: f64,
    pub bps_basis: String,
    pub cert_height: Option<u64>,
    pub cert_age_s: Option<f64>,
    pub cert_bft: Option<bool>,
    pub cert_votes: Option<u32>,
    pub follower_dh: Option<f64>,
    pub producer_dh: Option<f64>,
    pub energy_j: f64,
    pub k_earth: Option<f64>,
    pub k_earth_regime: Option<String>,
    pub k_earth_p99: Option<f64>,
    pub lod_ms: Option<f64>,
    pub k_fix: Option<f64>,
    pub earth_hands: Vec<earth::EarthHand>,
    pub ledger_periods: Vec<u64>,
}

impl Default for FaceInputs {
    fn default() -> Self {
        FaceInputs { node: "http://127.0.0.1:18181".into(), unix_s: crate::now_unix(), ut1_minus_utc_s: None, height: 0, bps: 0.0, bps_basis: "unset".into(), cert_height: None, cert_age_s: None, cert_bft: None, cert_votes: None, follower_dh: None, producer_dh: None, energy_j: E_CORE_15W_1S_J, k_earth: None, k_earth_regime: None, k_earth_p99: None, lod_ms: None, k_fix: None, earth_hands: vec![], ledger_periods: DEFAULT_LEDGER_PERIODS.to_vec() }
    }
}

pub fn read(i: &FaceInputs) -> Value {
    let tau_b = if i.bps > 0.0 { 1.0 / i.bps } else { f64::NAN };
    let behind = i.cert_height.map(|c| i.height.saturating_sub(c));
    let dilation = match (i.follower_dh, i.producer_dh) { (Some(f), Some(p)) => crate::kristensen::consensus_dilation(f, p), _ => None };
    let seconds = i.unix_s + i.ut1_minus_utc_s.unwrap_or(0.0);
    let three = ThreeClocks::read(seconds, i.height, tau_b, i.energy_j, behind, dilation);
    let rot = i.ut1_minus_utc_s.map(|u| earth::rotation_hand(i.unix_s, u));
    // The ledger as a CRT clock: integer ticks, every hand read from the same height.
    let remainders: Vec<f64> = i.ledger_periods.iter().map(|&x| (i.height % x) as f64).collect();
    let rec = crt::reconstruct(&remainders, &i.ledger_periods).ok();
    let composed_ok = rec.as_ref().map(|r| r.integer == i.height as u128).unwrap_or(false);
    let calendar = if i.earth_hands.is_empty() { Value::Null } else { serde_json::to_value(earth::calendar_verdict(&i.earth_hands)).unwrap_or(Value::Null) };
    json!({
        "ok": true,
        "event": EVENT,
        "ts_ms": crate::now_ms(),
        "node": i.node,
        "three_clocks": {
            "t_seconds": {"value": seconds, "label": if i.ut1_minus_utc_s.is_some() { "UT1 = UTC + (UT1−UTC) from the IERS via /v1/earth" } else { "UTC (no UT1−UTC available)" }, "kind": Label::Measured},
            "n_ml_ticks_per_block": {"value": three.ml_ticks_per_block, "energy_j": i.energy_j, "energy": three.energy_label, "kind": Label::Derived},
            "h_agreed_ticks": {"value": i.height, "kind": Label::Measured},
            "tau_block_s": {"value": tau_b, "bps": i.bps, "basis": i.bps_basis, "kind": Label::Measured},
            "tau_final_s": {"value": three.tau_final_s, "depth": crate::kristensen::FINAL_DEPTH, "kind": Label::Derived},
            "agreement_cost_A": {"value": three.agreement_cost, "kind": Label::Derived},
            "ledger_temperature_T_L_K": {"value": three.ledger_temperature_k, "kind": Label::Analogy},
            "finality_horizon_temperature_T_F_K": {"value": three.finality_horizon_temperature_k, "kind": Label::Analogy},
            "certified_horizon_temperature_K": {"value": three.certified_horizon_temperature_k, "behind": behind, "votes": i.cert_votes, "bft": i.cert_bft, "cert_age_s": i.cert_age_s, "kind": Label::Analogy},
            "consensus_dilation_gamma_C": {"value": dilation, "kind": Label::Measured},
            "fastest_tick_at_300K_s": {"value": three.fastest_tick_at_300k_s, "kind": Label::Derived},
        },
        "earth": {
            "rotation_hand": rot,
            "k_earth": i.k_earth, "k_earth_regime": i.k_earth_regime, "k_earth_p99": i.k_earth_p99, "lod_ms": i.lod_ms,
            "hands": i.earth_hands,
            "calendar": calendar,
            "note": "the rotation hand is a phase (time of day mod one turn) and cannot count turns; the slow hands are fitted from the feed's series and carry their own σ_t",
        },
        "crt": {
            "paper": "arXiv:2608.07938 — quantum Chinese remainder clock",
            "unit": "block",
            "periods": i.ledger_periods,
            "range": crt::range(&i.ledger_periods).to_string(),
            "remainders": remainders,
            "reconstructed": rec.as_ref().map(|r| r.integer.to_string()),
            "composition_matches_height": composed_ok,
            "z": 1,
            "note": "single-node face: every hand is read from one height, so this only demonstrates the composition. The decentralized reading (one hand per peer, gossiped) is `flux-clock collect` / p2p::HandCollector.",
        },
        "k_fix": i.k_fix,
        "labels": {"MEASURED": "read off an instrument", "DERIVED": "computed from measured inputs and a theorem", "ANALOGY": "the formal shape of a physical law applied to the ledger; not a physical temperature", "MODEL_CHOICE": "a number someone chose (the energy source)"},
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_face_reading_composes_the_height_and_labels_everything() {
        let i = FaceInputs { height: 19_040_287, bps: 8.0, bps_basis: "test".into(), ut1_minus_utc_s: Some(-0.005), cert_height: Some(19_040_285), cert_votes: Some(2), cert_bft: Some(false), ..Default::default() };
        let v = read(&i);
        assert_eq!(v["crt"]["composition_matches_height"], true);
        assert_eq!(v["crt"]["reconstructed"], "19040287");
        assert_eq!(v["three_clocks"]["certified_horizon_temperature_K"]["behind"], 2);
        assert_eq!(v["three_clocks"]["ledger_temperature_T_L_K"]["kind"], "ANALOGY");
        assert!(v["earth"]["rotation_hand"]["era_deg"].as_f64().unwrap() < 360.0);
        // 8 blk/s: T_L = ħ/(k_B·0.125 s) ≈ 6.1e-11 K
        let tl = v["three_clocks"]["ledger_temperature_T_L_K"]["value"].as_f64().unwrap();
        assert!((tl / 6.11e-11 - 1.0).abs() < 0.02, "{tl:e}");
    }
}
