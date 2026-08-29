//! The science annex — tower physics recomputed from `flux-science`'s
//! CODATA constants at generation time.
//!
//! The flux-arxiv discipline (the one that produced the Legibility-Dividend
//! and Idle-Machine papers): a paper's numbers are COMPUTED when the paper
//! is generated, from first constants, never pasted from a draft. This
//! module is the skyscraper's version of that: c, G, g and α come from
//! `flux_science::constants` (CODATA), the geometry comes from the live
//! `TowerSpec`, and the whitepaper generator calls these functions — so if
//! either the physics constants or the blueprint change, the published
//! numbers change with them or the build fails. No stale figures, ever.

use crate::elevator::FLOOR_HEIGHT_M;
use crate::tower::TowerSpec;
use crate::vault::VAULT_CAPACITY_BARS;
use flux_science::constants::{
    planck_length, FINE_STRUCTURE_INV, GRAVITATIONAL, SPEED_OF_LIGHT, STANDARD_GRAVITY,
};
use serde::Serialize;

/// Beacon mast above the roof, metres.
pub const BEACON_M: f64 = 40.0;
/// Vault depth below ground, metres (B4 slab).
pub const VAULT_DEPTH_M: f64 = 18.0;
/// Seconds per century (Julian).
const CENTURY_S: f64 = 100.0 * 365.25 * 86_400.0;
/// Good Delivery bar, kg.
const BAR_KG: f64 = 12.4;

/// Architectural height: top level × floor height + beacon.
pub fn tower_height_m(spec: &TowerSpec) -> f64 {
    spec.top_level() as f64 * FLOOR_HEIGHT_M + BEACON_M
}

#[derive(Debug, Clone, Serialize)]
pub struct ScienceAnnex {
    /// One beacon pulse's photon transit, base to tip, microseconds.
    pub photon_transit_us: f64,
    /// Gravitational time-dilation rate between vault slab and beacon tip
    /// (weak-field: gΔh/c²), dimensionless.
    pub dilation_rate: f64,
    /// The crown's clock gain over the vault's, microseconds per century.
    pub crown_gain_us_per_century: f64,
    /// Schwarzschild radius of the vault's gold at capacity, metres.
    pub vault_schwarzschild_m: f64,
    /// That radius in Planck lengths (how far from collapse, in the units
    /// where the question stops making sense).
    pub vault_rs_planck_lengths: f64,
    /// CODATA 2022 inverse fine-structure constant.
    pub alpha_inv: f64,
    /// How much the tower's 137 joke flatters the true value (relative).
    pub joke_flattery_rel: f64,
}

pub fn annex(spec: &TowerSpec) -> ScienceAnnex {
    let h = tower_height_m(spec);
    let c = SPEED_OF_LIGHT;
    let delta_h = h + VAULT_DEPTH_M;
    let dilation_rate = STANDARD_GRAVITY * delta_h / (c * c);
    let vault_mass_kg = VAULT_CAPACITY_BARS as f64 * BAR_KG;
    let rs = 2.0 * GRAVITATIONAL * vault_mass_kg / (c * c);
    ScienceAnnex {
        photon_transit_us: h / c * 1e6,
        dilation_rate,
        crown_gain_us_per_century: dilation_rate * CENTURY_S * 1e6,
        vault_schwarzschild_m: rs,
        vault_rs_planck_lengths: rs / planck_length(),
        alpha_inv: FINE_STRUCTURE_INV,
        joke_flattery_rel: (FINE_STRUCTURE_INV - 137.0) / FINE_STRUCTURE_INV,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tower::TowerSpec;

    #[test]
    fn annex_matches_hand_arithmetic() {
        let a = annex(&TowerSpec::quillon_default());
        // 392 m / c ≈ 1.308 µs per pulse.
        assert!((a.photon_transit_us - 1.3076).abs() < 0.001, "{}", a.photon_transit_us);
        // gΔh/c² with Δh = 410 m ≈ 4.474e-14.
        assert!((a.dilation_rate - 4.4737e-14).abs() < 1e-17, "{}", a.dilation_rate);
        // ≈ 141 µs per century of crown gain over the vault.
        assert!((a.crown_gain_us_per_century - 141.2).abs() < 0.5, "{}", a.crown_gain_us_per_century);
        // 99.2 t of gold: r_s ≈ 1.47e-22 m ≈ 9.1e12 Planck lengths.
        assert!((a.vault_schwarzschild_m - 1.4735e-22).abs() < 1e-25, "{}", a.vault_schwarzschild_m);
        assert!(a.vault_rs_planck_lengths > 9.0e12 && a.vault_rs_planck_lengths < 9.3e12);
        // The 137 joke flatters α⁻¹ by ~0.026%.
        assert!((a.joke_flattery_rel - 2.627e-4).abs() < 1e-6, "{}", a.joke_flattery_rel);
    }

    #[test]
    fn constants_come_from_flux_science_not_local_copies() {
        // If flux-science's CODATA values drift, this crate's annex drifts
        // with them — that is the point. Pin the linkage, not the values.
        assert_eq!(annex(&TowerSpec::quillon_default()).alpha_inv, FINE_STRUCTURE_INV);
    }
}
