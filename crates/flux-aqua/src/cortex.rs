//! Cortex-tuned gain weights.
//!
//! The source crate's coefficients (0.4/0.6/0.3/0.7/0.05) were literals inside a
//! method body. Nothing observed whether they were any good, and nothing could
//! change them. This module closes that loop: a colony reports what actually
//! happened, and the weights move.
//!
//! Two tuners live here, deliberately:
//!
//! * [`feedback_tune`] is **pure, dependency-free and always compiled**. It
//!   moves the weights from one measured number — the fraction of bridge
//!   attempts that committed — towards a target band. Every property of the
//!   tuner is testable without building the 40-crate Cortex closure. This is the
//!   FLUXFOOD discipline: put the logic in a pure function, test it offline.
//! * [`AquaCortex`] (feature `cortex`) runs the real `flux_cortex::Cortex` loop
//!   over a virtual workspace built from colony metrics, and folds its ranked
//!   actions into the same weights.
//!
//! Known interaction, stated rather than hidden: `flux_cortex::Cortex::run_loop`
//! unconditionally writes `~/.flux/cortex_state.json` at the end of every loop,
//! and that file is shared with `flux-p2p`'s `cortex_optimizer` and the
//! `flux_cortex_*` MCP tools. Running an Aqua tuning loop therefore appends to
//! the same global learning history they read. That is inherited behaviour, not
//! something this crate introduces, and it is why the pure tuner is the default
//! path.

use serde::{Deserialize, Serialize};

#[cfg(feature = "cortex")]
use crate::colony::ColonyMetrics;
use crate::score::GainWeights;

/// The commit-rate band the tuner steers towards.
///
/// Below the floor the colony is too strict — droplets burn energy on attempts
/// that never land. Above the ceiling it is too permissive: everything commits,
/// so the score carries no information and the chain accumulates junk work.
pub const TARGET_COMMIT_RATE: (f64, f64) = (0.25, 0.65);

/// Largest fractional change any single tuning step may make to a weight.
/// Bounds how fast the tuner can move, so one bad measurement cannot swing the
/// scorer.
pub const MAX_STEP: f64 = 0.10;

/// The result of a tuning step.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TuningResult {
    pub before: GainWeights,
    pub after: GainWeights,
    /// Measured commit rate that drove the step, `[0, 1]`.
    pub commit_rate: f64,
    /// Plain-language reason, suitable for a log line.
    pub rationale: String,
    /// Confidence in the step, `[0, 1]`.
    pub confidence: f64,
    /// How many Cortex loops contributed. `0` for the pure tuner.
    pub loops_run: u32,
}

impl TuningResult {
    /// `true` when the step actually changed something.
    pub fn changed(&self) -> bool {
        self.before != self.after
    }
}

/// Pure tuner: move the gain weights so the commit rate drifts into
/// [`TARGET_COMMIT_RATE`].
///
/// `commit_rate` is `committed_bridges / attempted_bridges` over the sampling
/// window. `samples` is how many attempts that rate is based on — confidence
/// grows with it, and below four samples the tuner refuses to move at all.
pub fn feedback_tune(w: GainWeights, commit_rate: f64, samples: usize) -> TuningResult {
    let rate = commit_rate.clamp(0.0, 1.0);
    let (lo, hi) = TARGET_COMMIT_RATE;
    let mut after = w;
    let rationale;

    if samples < 4 {
        rationale = format!("holding: {samples} samples is too few to tune on");
    } else if rate < lo {
        // Too strict — lift the floors so marginal droplets can still bridge.
        let step = 1.0 + (MAX_STEP * ((lo - rate) / lo).clamp(0.0, 1.0));
        after.phase_floor *= step;
        after.affinity_floor *= step;
        rationale = format!(
            "commit rate {rate:.2} below {lo:.2}: raising floors by {:.1}%",
            (step - 1.0) * 100.0
        );
    } else if rate > hi {
        // Too permissive — steepen the spans so the score discriminates again.
        let step = 1.0 - (MAX_STEP * ((rate - hi) / (1.0 - hi)).clamp(0.0, 1.0));
        after.phase_floor *= step;
        after.affinity_floor *= step;
        after.phase_span /= step.max(1e-6);
        after.affinity_span /= step.max(1e-6);
        rationale = format!(
            "commit rate {rate:.2} above {hi:.2}: lowering floors by {:.1}%",
            (1.0 - step) * 100.0
        );
    } else {
        rationale = format!("commit rate {rate:.2} inside [{lo:.2}, {hi:.2}]: no change");
    }

    let after = after.clamped();
    let confidence = (samples as f64 / 32.0).clamp(0.0, 1.0);

    TuningResult {
        before: w,
        after,
        commit_rate: rate,
        rationale,
        confidence,
        loops_run: 0,
    }
}

// ═══════════════════════════════════════════════════════════════
// flux-cortex integration (feature = "cortex")
// ═══════════════════════════════════════════════════════════════

#[cfg(feature = "cortex")]
mod integration {
    use super::*;
    use flux_cortex::Cortex;
    use flux_graph::{CrateInfo, CrateType, WorkspaceGraph};
    use flux_optimize::OptimizationPreset;

    /// Cortex-driven weight tuner for an Aqua colony.
    pub struct AquaCortex {
        colony_id: String,
        history: Vec<TuningResult>,
    }

    impl AquaCortex {
        pub fn new(colony_id: impl Into<String>) -> Self {
            Self {
                colony_id: colony_id.into(),
                history: Vec::new(),
            }
        }

        pub fn colony_id(&self) -> &str {
            &self.colony_id
        }

        pub fn history(&self) -> &[TuningResult] {
            &self.history
        }

        /// Run one Cortex loop against the colony's metrics, fold the ranked
        /// actions into the gain weights, then apply the pure commit-rate tuner
        /// on top so the measured signal always has the last word.
        pub fn tune(
            &mut self,
            metrics: &ColonyMetrics,
            commit_rate: f64,
            samples: usize,
            preset: OptimizationPreset,
        ) -> TuningResult {
            let ws = self.virtual_workspace();
            let mut cortex = Cortex::new(ws);
            let loop_result = cortex.run_loop(preset);

            let mut w = metrics.weights;
            for action in &loop_result.top_actions {
                // Bound the swing exactly as the pure tuner does, so a wild
                // impact estimate cannot dominate.
                let impact = action.estimated_impact_pct.clamp(-20.0, 20.0);
                let factor = 1.0 + (impact / 100.0) * MAX_STEP;
                match action.dimension.as_str() {
                    "Vectorization" => w.phase_span *= factor,
                    "Cache" => w.affinity_span *= factor,
                    "I/O" => w.charge_gain *= factor,
                    "Memory" => w.affinity_floor *= factor,
                    _ => w.phase_floor *= factor,
                }
            }
            let w = w.clamped();

            let mut result = feedback_tune(w, commit_rate, samples);
            result.before = metrics.weights;
            result.loops_run = 1;
            result.confidence = (result.confidence
                + loop_result
                    .top_actions
                    .first()
                    .map(|a| a.confidence)
                    .unwrap_or(0.5))
                / 2.0;
            result.rationale = format!(
                "cortex({} findings, {} applied) + {}",
                loop_result.findings_count, loop_result.actions_applied, result.rationale
            );

            self.history.push(result.clone());
            if self.history.len() > 50 {
                self.history.remove(0);
            }
            result
        }

        /// Represent each colony subsystem as a virtual crate so Cortex has a
        /// workspace to reason over — the same trick `flux-p2p`'s
        /// `cortex_optimizer` uses for SAP/X-Algo/batch/mesh/compile.
        fn virtual_workspace(&self) -> WorkspaceGraph {
            let names = [
                "aqua-droplet",
                "aqua-score",
                "aqua-ledger",
                "aqua-mesh",
                "aqua-species",
            ];
            WorkspaceGraph {
                root: std::path::PathBuf::from("virtual://flux-aqua"),
                crates: names
                    .iter()
                    .map(|n| CrateInfo {
                        name: (*n).into(),
                        path: std::path::PathBuf::from(format!("virtual://aqua/{n}")),
                        dependencies: vec![],
                        edition: "2021".into(),
                        crate_type: CrateType::Lib,
                        features: vec![],
                    })
                    .collect(),
                batches: vec![],
            }
        }
    }
}

#[cfg(feature = "cortex")]
pub use integration::AquaCortex;

#[cfg(test)]
mod tests {
    use super::*;

    fn w() -> GainWeights {
        GainWeights::default()
    }

    #[test]
    fn too_few_samples_never_moves_the_weights() {
        for n in 0..4 {
            let r = feedback_tune(w(), 0.99, n);
            assert!(!r.changed(), "tuned on {n} samples");
        }
    }

    #[test]
    fn a_rate_inside_the_band_is_left_alone() {
        let r = feedback_tune(w(), 0.45, 64);
        assert!(!r.changed());
        assert!(r.rationale.contains("no change"));
    }

    #[test]
    fn a_too_strict_colony_gets_higher_floors() {
        let r = feedback_tune(w(), 0.02, 64);
        assert!(r.after.phase_floor > r.before.phase_floor);
        assert!(r.after.affinity_floor > r.before.affinity_floor);
    }

    #[test]
    fn a_too_permissive_colony_gets_lower_floors() {
        let r = feedback_tune(w(), 0.98, 64);
        assert!(r.after.phase_floor < r.before.phase_floor);
        assert!(r.after.affinity_floor < r.before.affinity_floor);
    }

    #[test]
    fn a_single_step_is_bounded() {
        for rate in [0.0, 0.1, 0.5, 0.9, 1.0] {
            let r = feedback_tune(w(), rate, 1_000);
            let ratio = r.after.phase_floor / r.before.phase_floor;
            assert!(
                (1.0 - MAX_STEP - 1e-9..=1.0 + MAX_STEP + 1e-9).contains(&ratio),
                "rate {rate} moved phase_floor by {ratio}"
            );
        }
    }

    #[test]
    fn tuning_converges_into_the_band_and_stays() {
        // Simulate a colony whose commit rate responds to the floors: higher
        // floors → easier to clear → higher commit rate.
        let mut weights = w();
        let mut rate = 0.02;
        for _ in 0..40 {
            let r = feedback_tune(weights, rate, 64);
            weights = r.after;
            rate = (rate + (weights.phase_floor - 0.4) * 2.0).clamp(0.0, 1.0);
        }
        let (lo, hi) = TARGET_COMMIT_RATE;
        assert!(
            rate >= lo * 0.5 && rate <= hi * 1.5,
            "did not converge: {rate}"
        );
        // And whatever it converged to must still be a legal scorer.
        assert!(weights.max_gain() > 0.0);
        assert_eq!(weights, weights.clamped());
    }

    #[test]
    fn output_weights_are_always_clamped() {
        let wild = GainWeights {
            phase_floor: 0.94,
            phase_span: 0.94,
            affinity_floor: 0.94,
            affinity_span: 0.94,
            charge_gain: 0.24,
            charge_cap: 32,
        };
        let r = feedback_tune(wild, 0.0, 512);
        assert_eq!(r.after, r.after.clamped());
    }

    #[test]
    fn confidence_grows_with_sample_count() {
        let a = feedback_tune(w(), 0.1, 8).confidence;
        let b = feedback_tune(w(), 0.1, 64).confidence;
        assert!(b > a);
        assert!(b <= 1.0);
    }

    #[test]
    fn out_of_range_commit_rates_are_clamped() {
        assert_eq!(feedback_tune(w(), -5.0, 64).commit_rate, 0.0);
        assert_eq!(feedback_tune(w(), 5.0, 64).commit_rate, 1.0);
    }
}
