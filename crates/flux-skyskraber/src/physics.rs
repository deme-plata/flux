//! Physics v0.1 — rough structural and life-safety envelopes.
//!
//! Be honest about the altitude: everything here is DERIVED from published
//! rules of thumb, not engineered. A structural engineer replaces these
//! numbers; the point is that the CONSTRAINTS exist as executable checks
//! from day one, so no later design change can silently violate them.
//!
//! The most interesting one is the sky bridge. Two tall asymmetric towers
//! move differently under wind and temperature; a rigid link at 60–61 would
//! tear itself apart. So the bridge carries an invariant:
//!
//!   |Δx_A(z) − (−Δx_B(z))| ≤ joint_budget    (worst case: out of phase)
//!
//! with drift estimated by the standard serviceability limit (top drift ≈
//! H/500) shaped by a cantilever mode, Δx(z) ≈ Δx_top · (z/H)².

use crate::elevator::FLOOR_HEIGHT_M;
use crate::tower::{Spire, TowerSpec, Zone};
use serde::{Deserialize, Serialize};

/// Serviceability drift limit: top deflection ≈ height / 500.
pub const DRIFT_RATIO: f64 = 500.0;
/// Safety factor applied to the worst-case differential for the joint spec.
pub const JOINT_SAFETY: f64 = 1.25;
/// Egress: sustained flow per stair, persons/second (rule of thumb ~1.1).
pub const STAIR_FLOW_PPS: f64 = 1.1;
/// Egress stairs in the tower core.
pub const STAIR_COUNT: u32 = 4;
/// Descent pace, seconds per floor, crowded stair.
pub const DESCENT_S_PER_FLOOR: f64 = 16.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeMovementReport {
    /// Top drift of each spire, metres (H/500).
    pub top_drift_a_m: f64,
    pub top_drift_b_m: f64,
    /// Drift of each spire at bridge height (cantilever shape (z/H)^2).
    pub drift_at_bridge_a_m: f64,
    pub drift_at_bridge_b_m: f64,
    /// Worst-case out-of-phase differential at the bridge, metres.
    pub differential_m: f64,
    /// Required movement-joint budget = differential × safety factor.
    pub joint_budget_m: f64,
}

/// Height of a level above ground, metres.
fn z_m(level: i32) -> f64 {
    (level.max(0) as f64) * FLOOR_HEIGHT_M
}

/// Wind-drift envelope of the sky bridge, from the blueprint alone.
pub fn bridge_movement(spec: &TowerSpec) -> Option<BridgeMovementReport> {
    let bridges = spec.levels_of(Zone::SkyBridge);
    let bridge_top = *bridges.last()?;
    let h_a = z_m(spec.spire_top(Spire::A)?);
    let h_b = z_m(spec.spire_top(Spire::B)?);
    let z = z_m(bridge_top);
    let top_a = h_a / DRIFT_RATIO;
    let top_b = h_b / DRIFT_RATIO;
    let at_a = top_a * (z / h_a).powi(2);
    let at_b = top_b * (z / h_b).powi(2);
    let differential = at_a + at_b; // out of phase: toward each other / apart
    Some(BridgeMovementReport {
        top_drift_a_m: top_a,
        top_drift_b_m: top_b,
        drift_at_bridge_a_m: at_a,
        drift_at_bridge_b_m: at_b,
        differential_m: differential,
        joint_budget_m: differential * JOINT_SAFETY,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvacuationReport {
    pub occupants: u32,
    /// Time for the whole population to pass through the stairs, seconds.
    pub flow_time_s: f64,
    /// Descent time from the top occupied level, seconds.
    pub descent_time_s: f64,
    /// Governing estimate: the slower of the two mechanisms.
    pub estimate_s: f64,
}

/// Full-building evacuation estimate: the population is limited either by
/// stair THROUGHPUT (occupants / total flow) or by stair LENGTH (descent
/// from the crown) — the governing time is the larger.
pub fn evacuation(spec: &TowerSpec) -> EvacuationReport {
    let occupants: u32 =
        spec.floors.iter().filter(|f| f.level > 0).map(|f| f.design_occupancy).sum();
    let flow_time_s = occupants as f64 / (STAIR_COUNT as f64 * STAIR_FLOW_PPS);
    let descent_time_s = spec.top_level() as f64 * DESCENT_S_PER_FLOOR;
    EvacuationReport {
        occupants,
        flow_time_s,
        descent_time_s,
        estimate_s: flow_time_s.max(descent_time_s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tower::TowerSpec;

    #[test]
    fn bridge_budget_matches_hand_arithmetic() {
        let spec = TowerSpec::quillon_default();
        let r = bridge_movement(&spec).unwrap();
        // H_A = 352 m, H_B = 312 m → top drifts 0.704 / 0.624 m.
        assert!((r.top_drift_a_m - 0.704).abs() < 1e-9);
        assert!((r.top_drift_b_m - 0.624).abs() < 1e-9);
        // z = 61·4 = 244 m; (244/352)² · 0.704 ≈ 0.338; (244/312)² · 0.624 ≈ 0.382.
        assert!((r.drift_at_bridge_a_m - 0.338).abs() < 0.002, "{}", r.drift_at_bridge_a_m);
        assert!((r.drift_at_bridge_b_m - 0.382).abs() < 0.002, "{}", r.drift_at_bridge_b_m);
        // Differential ≈ 0.72 m; joint budget ≈ 0.90 m.
        assert!((r.differential_m - 0.720).abs() < 0.005, "{}", r.differential_m);
        assert!(r.joint_budget_m > r.differential_m);
    }

    #[test]
    fn shorter_spire_drifts_more_at_the_bridge() {
        // Counterintuitive and true: at the same z, the shorter tower is
        // proportionally closer to its tip, so its shape factor is larger.
        let r = bridge_movement(&TowerSpec::quillon_default()).unwrap();
        assert!(r.drift_at_bridge_b_m > r.drift_at_bridge_a_m);
    }

    #[test]
    fn evacuation_estimate_matches_hand_arithmetic() {
        let spec = TowerSpec::quillon_default();
        let r = evacuation(&spec);
        // Occupants above ground, summed from the spec: 6,404.
        assert_eq!(r.occupants, 6_404);
        // Flow: 6,404 / (4 × 1.1) ≈ 1,455 s. Descent: 88 × 16 = 1,408 s.
        // Remarkably balanced — the stair COUNT and the stair LENGTH bind
        // within 3% of each other; flow narrowly governs at ~24 minutes.
        assert!((r.flow_time_s - 1455.5).abs() < 1.0, "{}", r.flow_time_s);
        assert!((r.descent_time_s - 1408.0).abs() < 1e-9);
        assert!((r.estimate_s - r.flow_time_s.max(r.descent_time_s)).abs() < 1e-9);
        assert!(r.estimate_s < 1_800.0, "must stay under 30 min: {}", r.estimate_s);
    }
}
