//! The three clocks and the named quantities of Kristensen Time.
//!
//! * clock 1 — proper seconds `t` (physics; MEASURED from the system clock / UT1);
//! * clock 2 — Margolus–Levitin ticks `n = 2Et/πħ`: how many distinguishable states a
//!   system of energy `E` above its ground state can pass through in `t` (a theorem;
//!   DERIVED once `E` is chosen, and `E` is a MODEL CHOICE);
//! * clock 3 — agreed ticks `h` = block height: advances only when independent machines
//!   commit the same state (MEASURED from the node).
//!
//! `K* = 2π√((π/2)·N_ML·Δs)` bridges clocks 2 and 3; the repaired gauge `K_fix` replaces
//! seconds and ħ by a depth clock `τ_d` in BLOCKS (the 2026-09-14 battle test).

use crate::{HBAR, KB, PI};
use serde::{Deserialize, Serialize};

/// `sigil_dagknight::BraidConfig::default().final_depth` — the depth-rule fallback horizon.
pub const FINAL_DEPTH: f64 = 512.0;
/// What a 15 W core dissipates in one second — the page's default energy source.
pub const E_CORE_15W_1S_J: f64 = 15.0;
/// ITER's thermal energy in one shot (350 MJ) — the page's other energy source.
pub const E_ITER_J: f64 = 350.0e6;
/// K-family ladder: `<1` stable, `<3` elevated, `≥3` critical (flux-kgauge).
pub const LADDER: [f64; 2] = [1.0, 3.0];
/// Sybil-safe proposer weight divisor: `w_S = 1 − H_norm/4 ∈ [¾, 1]`.
pub const W_S_DIV: f64 = 4.0;

/// Margolus–Levitin: distinguishable state changes available to energy `e_j` in `secs`.
pub fn ml_ticks(e_j: f64, secs: f64) -> f64 {
    2.0 * e_j * secs / (PI * HBAR)
}

/// Agreement cost `A` = ML ticks spent per agreed tick (per block).
pub fn agreement_cost(e_j: f64, tau_block_s: f64) -> f64 {
    ml_ticks(e_j, tau_block_s)
}

/// Ledger temperature `T_L = ħ/(k_B τ_b)` — the thermal state whose thermal time flows one
/// tick per block interval (Connes–Rovelli). ANALOGY.
pub fn ledger_temperature_k(tau_block_s: f64) -> f64 {
    HBAR / (KB * tau_block_s)
}

/// Finality-horizon temperature `T_F = ħ/(2π k_B τ_F)` — Unruh–Hawking form. ANALOGY.
pub fn finality_horizon_temperature_k(tau_final_s: f64) -> f64 {
    HBAR / (2.0 * PI * KB * tau_final_s)
}

/// Fastest tick a system at temperature `T` can make: `πħ/(2 k_B T)` (heat sets the FASTEST
/// clock, not the existence of time).
pub fn fastest_tick_s(temp_k: f64) -> f64 {
    PI * HBAR / (2.0 * KB * temp_k)
}

/// Consensus dilation `γ_C` = follower height rate / producer height rate over the same
/// wall-clock window. `None` when the producer did not advance (undefined, not zero).
pub fn consensus_dilation(follower_dh: f64, producer_dh: f64) -> Option<f64> {
    if producer_dh > 0.0 { Some(follower_dh / producer_dh) } else { None }
}

/// The depth clock: persistence of a disagreement in BLOCKS, saturating at the finality
/// depth. `τ_d = (1 + min(d, 512)/512)/2 ∈ [½, 1]`.
pub fn tau_d(depth_blocks: f64) -> f64 {
    (1.0 + depth_blocks.max(0.0).min(FINAL_DEPTH) / FINAL_DEPTH) / 2.0
}

/// Proposer concentration weight `w_S = 1 − H_norm/4`, `H_norm ∈ [0,1]` the normalised
/// producer entropy. One key equivocating is the worst case; rotation buys at most 13.4 %.
pub fn w_s(h_norm: f64) -> f64 {
    1.0 - h_norm.clamp(0.0, 1.0) / W_S_DIV
}

/// `K_fix = 2π √(ΔH_c · τ_d · w_S)` — the repaired gauge (speed-invariant, Sybil-bounded).
pub fn k_fix(delta_h_c: f64, depth_blocks: f64, h_norm: f64) -> f64 {
    2.0 * PI * (delta_h_c.clamp(0.0, 1.0) * tau_d(depth_blocks) * w_s(h_norm)).sqrt()
}

pub fn regime(k: f64) -> &'static str {
    if k < LADDER[0] { "stable" } else if k < LADDER[1] { "elevated" } else { "critical" }
}

/// One reading of the three clocks plus everything derived from them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreeClocks {
    /// clock 1: proper seconds since the Unix epoch (UT1 if the Earth feed supplied UT1−UTC).
    pub seconds: f64,
    /// clock 2: Margolus–Levitin ticks the chosen energy could have made in one block interval.
    pub ml_ticks_per_block: f64,
    /// clock 3: agreed ticks = block height.
    pub height: u64,
    pub energy_j: f64,
    pub energy_label: &'static str,
    pub tau_block_s: f64,
    pub tau_final_s: f64,
    pub agreement_cost: f64,
    pub ledger_temperature_k: f64,
    pub finality_horizon_temperature_k: f64,
    /// Certified-line temperature when a certificate exists (τ = (behind+1)·τ_b).
    pub certified_horizon_temperature_k: Option<f64>,
    pub consensus_dilation: Option<f64>,
    pub fastest_tick_at_300k_s: f64,
}

impl ThreeClocks {
    pub fn read(seconds: f64, height: u64, tau_block_s: f64, energy_j: f64, cert_behind: Option<u64>, dilation: Option<f64>) -> Self {
        let tau_final_s = FINAL_DEPTH * tau_block_s;
        let energy_label = if (energy_j - E_ITER_J).abs() < 1.0 { "ITER · 350 MJ thermal energy (MODEL CHOICE)" } else if (energy_j - E_CORE_15W_1S_J).abs() < 1e-9 { "15 J · what a 15 W core dissipates in one second (MODEL CHOICE)" } else { "operator-supplied energy (MODEL CHOICE)" };
        ThreeClocks {
            seconds,
            ml_ticks_per_block: ml_ticks(energy_j, tau_block_s),
            height,
            energy_j,
            energy_label,
            tau_block_s,
            tau_final_s,
            agreement_cost: agreement_cost(energy_j, tau_block_s),
            ledger_temperature_k: ledger_temperature_k(tau_block_s),
            finality_horizon_temperature_k: finality_horizon_temperature_k(tau_final_s),
            certified_horizon_temperature_k: cert_behind.map(|b| finality_horizon_temperature_k((b as f64 + 1.0) * tau_block_s)),
            consensus_dilation: dilation,
            fastest_tick_at_300k_s: fastest_tick_s(300.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_numbers_on_the_live_page_come_out() {
        // kristensen-time.html, 0.367 blk/s: A = 2.47e35 per block, T_L ≈ 2.8 pK, T_F ≈ 0.87 fK.
        let tau_b = 1.0 / 0.3666;
        let a = agreement_cost(E_CORE_15W_1S_J, tau_b);
        assert!((a / 2.47e35 - 1.0).abs() < 0.02, "A = {a:e}");
        let tl = ledger_temperature_k(tau_b);
        assert!((tl / 2.8e-12 - 1.0).abs() < 0.02, "T_L = {tl:e}");
        let tf = finality_horizon_temperature_k(FINAL_DEPTH * tau_b);
        assert!((tf / 8.7e-16 - 1.0).abs() < 0.02, "T_F = {tf:e}");
        // ITER 350 MJ → 5.8e42 per block.
        assert!((agreement_cost(E_ITER_J, tau_b) / 5.8e42 - 1.0).abs() < 0.02);
    }

    #[test]
    fn heat_sets_the_fastest_clock() {
        // Room temperature: πħ/2k_BT ≈ 4.0e-14 s. ITER at 1.7e8 K is faster, not "replacing time".
        let room = fastest_tick_s(300.0);
        assert!((room / 4.0e-14 - 1.0).abs() < 0.02, "{room:e}");
        assert!(fastest_tick_s(1.7e8) < room);
    }

    #[test]
    fn depth_clock_is_speed_invariant_and_sybil_bounded() {
        assert_eq!(tau_d(0.0), 0.5);
        assert_eq!(tau_d(512.0), 1.0);
        assert_eq!(tau_d(5000.0), 1.0);
        // one key (H_norm 0) reads at most √(4/3) = 1.155× a fully rotated set (H_norm 1)
        let one = k_fix(0.5, 100.0, 0.0);
        let many = k_fix(0.5, 100.0, 1.0);
        assert!((one / many - (4.0f64 / 3.0).sqrt()).abs() < 1e-9);
        // zero disagreement → 0, whatever the keys or depth
        assert_eq!(k_fix(0.0, 512.0, 0.0), 0.0);
        // a persistent two-party split stays critical regardless of key count (the /4 lesson)
        assert_eq!(regime(k_fix(0.61, 512.0, 1.0)), "critical");
        assert_eq!(regime(0.46), "stable");
    }

    #[test]
    fn dilation_is_undefined_not_zero_when_the_producer_stalls() {
        assert_eq!(consensus_dilation(3.0, 0.0), None);
        assert_eq!(consensus_dilation(50.0, 100.0), Some(0.5));
    }
}
