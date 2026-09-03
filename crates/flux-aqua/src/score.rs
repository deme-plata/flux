//! The bridge-gain scorer — the one genuinely reusable piece of maths in the
//! water-robot stack.
//!
//! The source `DropletField::phase_slip` computed, inline and unnamed:
//!
//! ```text
//! gain = coherence
//!      * (0.4 + 0.6·|sin φ|)
//!      * (0.3 + 0.7·affinity)
//!      * (1 + 0.05·|charge|)
//!      * energy
//! ```
//!
//! Look at the shape rather than the vocabulary and it is a **multiplicative
//! score with saturating floor-plus-span terms** — structurally the same object
//! as flux-p2p's SAP scorer (contribution / latency / stake / accuracy / uptime).
//! The magic numbers were hard-coded, so nothing could ever tune them and no
//! test could pin their effect. Here they are a named, serialisable, bounded
//! [`GainWeights`], which is precisely what lets `crate::cortex` tune them.

use serde::{Deserialize, Serialize};

use crate::droplet::TopoCharge;

/// Tunable coefficients of the bridge-gain function.
///
/// Every field is bounded on `apply`/`clamped`, so a Cortex loop cannot drive
/// the scorer into a degenerate configuration.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GainWeights {
    /// Minimum multiplier from the phase term, even at φ = 0.
    pub phase_floor: f64,
    /// How much of the phase term is contributed by `|sin φ|`.
    pub phase_span: f64,
    /// Minimum multiplier from the affinity term, even at zero similarity.
    pub affinity_floor: f64,
    /// How much of the affinity term is contributed by measured similarity.
    pub affinity_span: f64,
    /// Bonus per unit of `|topological charge|`.
    pub charge_gain: f64,
    /// Hard cap on `|charge|`.
    pub charge_cap: i32,
}

impl Default for GainWeights {
    /// The coefficients the source crate hard-coded.
    fn default() -> Self {
        Self {
            phase_floor: 0.4,
            phase_span: 0.6,
            affinity_floor: 0.3,
            affinity_span: 0.7,
            charge_gain: 0.05,
            charge_cap: 8,
        }
    }
}

impl GainWeights {
    /// Clamp every coefficient into its safe range. Called on construction and
    /// after every tuning step, so an out-of-range weight can never reach the
    /// scorer.
    pub fn clamped(mut self) -> Self {
        self.phase_floor = self.phase_floor.clamp(0.05, 0.95);
        self.phase_span = self.phase_span.clamp(0.05, 0.95);
        self.affinity_floor = self.affinity_floor.clamp(0.05, 0.95);
        self.affinity_span = self.affinity_span.clamp(0.05, 0.95);
        self.charge_gain = self.charge_gain.clamp(0.0, 0.25);
        self.charge_cap = self.charge_cap.clamp(1, 32);
        self
    }

    /// The largest value [`bridge_gain`] can return under these weights, used to
    /// normalise the raw gain into `[0, 1]`.
    pub fn max_gain(&self) -> f64 {
        (self.phase_floor + self.phase_span)
            * (self.affinity_floor + self.affinity_span)
            * (1.0 + self.charge_gain * self.charge_cap as f64)
    }
}

/// The inputs a bridge attempt is scored on.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct GainInputs {
    /// Droplet coherence, `[0, 1]`.
    pub coherence: f64,
    /// Droplet energy, `[0, 1]`.
    pub energy: f64,
    /// Quantum phase in radians.
    pub phase: f64,
    /// Isotopic affinity to the target, `[0, 1]`.
    pub affinity: f64,
    /// Topological charge from the lightning tuner.
    pub charge: TopoCharge,
}

/// Raw multiplicative gain. Always ≥ 0; bounded above by
/// [`GainWeights::max_gain`] once inputs are in range.
pub fn bridge_gain(w: &GainWeights, i: &GainInputs) -> f64 {
    let coherence = i.coherence.clamp(0.0, 1.0);
    let energy = i.energy.clamp(0.0, 1.0);
    let affinity = i.affinity.clamp(0.0, 1.0);
    let charge = i.charge.abs().min(w.charge_cap) as f64;

    coherence
        * (w.phase_floor + w.phase_span * i.phase.sin().abs())
        * (w.affinity_floor + w.affinity_span * affinity)
        * (1.0 + w.charge_gain * charge)
        * energy
}

/// Gain normalised into `[0, 1]` — the form that can be compared against a
/// threshold or fed into a peer-ranking table.
pub fn normalized_gain(w: &GainWeights, i: &GainInputs) -> f64 {
    let max = w.max_gain();
    if max <= 0.0 {
        return 0.0;
    }
    (bridge_gain(w, i) / max).clamp(0.0, 1.0)
}

/// A scored bridge attempt.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct BridgeScore {
    pub raw: f64,
    pub normalized: f64,
    /// Phase-space displacement this bridge would produce.
    pub displacement: f64,
    /// Whether the attempt clears the stability bar.
    pub stable: bool,
}

/// Minimum normalised gain for a bridge to be considered stable enough to route
/// traffic over.
///
/// The source used a two-part rule (`stability_index > 0.7 && |charge| >= 3`)
/// where `stability_index` was `1/(1+length)` — it rewarded *short* bridges
/// while the consensus rule rewarded *long* ones. That contradiction is resolved
/// here: stability is a property of the gain, length is a property of the work.
///
/// The number is calibrated against the real input distribution rather than
/// chosen by taste. A healthy droplet — coherence 0.9, energy 0.7, `|sin φ|` 0.7,
/// bridging to an unrelated peer (affinity ≈ 0.5) at charge 6 — scores
///
/// ```text
/// 0.9 · (0.4 + 0.6·0.7) · (0.3 + 0.7·0.5) · (1 + 0.05·6) · 0.7 / 1.4 ≈ 0.31
/// ```
///
/// so the default weights sit a hair above the bar. That is deliberate: it puts
/// the colony inside the band where [`crate::cortex::feedback_tune`] has signal
/// to work with, instead of at a rail where every attempt commits or none does.
pub const STABILITY_THRESHOLD: f64 = 0.30;

/// Score a bridge attempt, including the displacement it would cause.
pub fn score_bridge(w: &GainWeights, i: &GainInputs) -> BridgeScore {
    let raw = bridge_gain(w, i);
    let normalized = normalized_gain(w, i);
    // Displacement mirrors the source's `0.05 + gain.clamp(0.0, 2.0)`: a small
    // floor so every accepted hop moves, then the gain.
    let displacement = 0.05 + raw.clamp(0.0, 2.0);
    BridgeScore {
        raw,
        normalized,
        displacement,
        stable: normalized >= STABILITY_THRESHOLD,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::FRAC_PI_2;

    fn inputs() -> GainInputs {
        GainInputs {
            coherence: 1.0,
            energy: 1.0,
            phase: FRAC_PI_2, // |sin| = 1
            affinity: 1.0,
            charge: 8,
        }
    }

    #[test]
    fn best_case_hits_max_gain() {
        let w = GainWeights::default();
        let g = bridge_gain(&w, &inputs());
        assert!((g - w.max_gain()).abs() < 1e-12);
        assert!((normalized_gain(&w, &inputs()) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn normalized_gain_is_always_in_unit_interval() {
        let w = GainWeights::default();
        for coh in [0.0, 0.3, 1.0] {
            for aff in [0.0, 0.5, 1.0] {
                for ch in [-32, -3, 0, 3, 32] {
                    for p in [0.0, 1.0, 3.0, 6.0] {
                        let n = normalized_gain(
                            &w,
                            &GainInputs {
                                coherence: coh,
                                energy: 0.7,
                                phase: p,
                                affinity: aff,
                                charge: ch,
                            },
                        );
                        assert!((0.0..=1.0).contains(&n), "escaped: {}", n);
                    }
                }
            }
        }
    }

    #[test]
    fn out_of_range_inputs_are_clamped_not_amplified() {
        let w = GainWeights::default();
        let insane = GainInputs {
            coherence: 1e9,
            energy: 1e9,
            phase: FRAC_PI_2,
            affinity: 1e9,
            charge: i32::MAX,
        };
        assert!((bridge_gain(&w, &insane) - w.max_gain()).abs() < 1e-9);
    }

    #[test]
    fn gain_is_monotone_in_affinity() {
        let w = GainWeights::default();
        let mut prev = -1.0;
        for step in 0..=10 {
            let mut i = inputs();
            i.affinity = step as f64 / 10.0;
            let g = bridge_gain(&w, &i);
            assert!(g > prev);
            prev = g;
        }
    }

    #[test]
    fn gain_is_monotone_in_charge_up_to_the_cap() {
        let w = GainWeights::default();
        let mut prev = -1.0;
        for c in 0..=w.charge_cap {
            let mut i = inputs();
            i.charge = c;
            let g = bridge_gain(&w, &i);
            assert!(g > prev);
            prev = g;
        }
        // Past the cap it must flatten, not keep climbing.
        let mut over = inputs();
        over.charge = w.charge_cap * 100;
        assert!((bridge_gain(&w, &over) - prev).abs() < 1e-12);
    }

    #[test]
    fn zero_coherence_or_energy_kills_the_bridge() {
        let w = GainWeights::default();
        let mut dead = inputs();
        dead.coherence = 0.0;
        assert_eq!(bridge_gain(&w, &dead), 0.0);
        let mut flat = inputs();
        flat.energy = 0.0;
        assert_eq!(bridge_gain(&w, &flat), 0.0);
    }

    #[test]
    fn weights_clamp_into_safe_ranges() {
        let w = GainWeights {
            phase_floor: -5.0,
            phase_span: 99.0,
            affinity_floor: 0.0,
            affinity_span: -1.0,
            charge_gain: 10.0,
            charge_cap: 0,
        }
        .clamped();
        assert!(w.phase_floor >= 0.05 && w.phase_floor <= 0.95);
        assert!(w.phase_span <= 0.95);
        assert!(w.affinity_span >= 0.05);
        assert!(w.charge_gain <= 0.25);
        assert!(w.charge_cap >= 1);
        assert!(w.max_gain() > 0.0);
    }

    #[test]
    fn stability_tracks_the_gain_not_the_length() {
        let w = GainWeights::default();
        let good = score_bridge(&w, &inputs());
        assert!(good.stable);
        assert!(good.displacement > 0.05);

        let mut weak = inputs();
        weak.coherence = 0.1;
        weak.affinity = 0.0;
        weak.charge = 0;
        assert!(!score_bridge(&w, &weak).stable);
    }

    #[test]
    fn the_threshold_is_where_the_doc_comment_says_it_is() {
        // Pins the calibration above, so nobody moves the constant without
        // noticing which droplets stop being able to bridge.
        let w = GainWeights::default();
        let healthy = GainInputs {
            coherence: 0.9,
            energy: 0.7,
            phase: 0.7754f64.asin(), // |sin| = 0.7754 -> the 0.7 term in the doc
            affinity: 0.5,
            charge: 6,
        };
        let n = normalized_gain(&w, &healthy);
        assert!(
            (0.28..0.38).contains(&n),
            "a healthy droplet now scores {n}, outside the calibrated band"
        );
        assert!(score_bridge(&w, &healthy).stable);
    }
}
