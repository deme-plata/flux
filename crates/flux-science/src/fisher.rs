// Fisher information — precision-weighted estimation of a remote monotone counter.
//
// WHY THIS EXISTS (a real production failure, 2026-07-28)
// ────────────────────────────────────────────────────────
// A live 21.4M-block chain shipped a "staleness decay" heuristic: when peer data
// went stale, it shaved 10% of the gap off the ESTIMATE of the network tip,
// floored at the node's own height:
//
//     decay = (network_height - local_height) / 10
//     network_height = (network_height - decay).max(local_height)
//
// Fresh nodes therefore converged to `network_height == local_height`, concluded
// `blocks_behind ≈ 0`, and stopped syncing — while 21.25 MILLION blocks behind.
// Observed decay trace: 21,364,541 → … → 115,289 → 115,285 → 115,282 → 115,270.
//
// The heuristic is not merely crude, it is SIGN-INVERTED. Model the counter as a
// Poisson arrival process at rate λ. A peer reported value `h` at staleness Δ:
//
//     E[value now] = h + λΔ        ← the true value is HIGHER than reported
//     Var[value now] = λΔ + σ²     ← and we are LESS certain about it
//
// Staleness must decay the PRECISION, never the ESTIMATE. Decaying the estimate
// toward your own state makes ignorance indistinguishable from completion, which
// is the one failure mode a catch-up protocol must never have.
//
// THE STRUCTURAL FIX
// ──────────────────
// Fisher information is precision: I = 1/Var, and the Cramér–Rao bound says no
// unbiased estimator can do better than Var ≥ 1/I. Fusing observations by their
// information (inverse-variance weighting) gives the minimum-variance estimate,
// and — the property that actually matters here — makes the safe behaviour
// STRUCTURAL rather than a tuned threshold:
//
//     no observations  ⇒  ΣI → 0  ⇒  Var → ∞  ⇒  upper bound → ∞  ⇒  "assume behind"
//
// The heuristic fails toward "caught up". This cannot.
//
// SCOPE — read this before reaching for the word "quantum"
// ───────────────────────────────────────────────────────
// This is CLASSICAL Fisher information. The quantum Fisher information (QFI)
// generalises it — F_Q = sup over observables of the classical F, and the QFI
// matrix is 4× the Bures metric — and its headline N → N² (standard-quantum-limit
// → Heisenberg) scaling requires real entanglement on real hardware. There is
// none here, and no such speedup is claimed. What transfers is the estimation
// theory, not the metrology.
//
// Refs: Tóth & Apellaniz, J. Phys. A 47, 424006 (2014), arXiv:1405.4878;
//       Liu, Yuan, Lu & Wang, J. Phys. A 53, 023001 (2020), arXiv:1907.08037.

use serde::{Deserialize, Serialize};

/// Arrival model for the quantity being estimated.
///
/// `rate_per_s` is λ — how fast the remote counter advances (e.g. blocks/sec).
/// Under a Poisson model both the expected advance and its variance over an
/// interval Δ are λΔ, which is what makes staleness cost precision.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ArrivalModel {
    /// λ — mean arrivals per second. Must be finite and ≥ 0.
    pub rate_per_s: f64,
}

impl ArrivalModel {
    /// Build a model, clamping a negative or non-finite rate to 0.
    pub fn new(rate_per_s: f64) -> Self {
        let rate_per_s = if rate_per_s.is_finite() && rate_per_s > 0.0 {
            rate_per_s
        } else {
            0.0
        };
        Self { rate_per_s }
    }
}

/// One observation of the remote counter, with how stale it is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StaleObservation {
    /// The value the source reported (e.g. a peer's claimed tip height).
    pub value: f64,
    /// Seconds elapsed since the observation was made.
    pub staleness_s: f64,
    /// σ² — intrinsic noise of the source, independent of staleness. Use this to
    /// downweight sources you trust less (see `flux-science` docs on
    /// information-as-trust); 0.0 means "no intrinsic noise".
    pub base_variance: f64,
}

impl StaleObservation {
    /// A fresh, noiseless observation.
    pub fn fresh(value: f64) -> Self {
        Self { value, staleness_s: 0.0, base_variance: 0.0 }
    }

    /// An observation with staleness and no intrinsic noise.
    pub fn aged(value: f64, staleness_s: f64) -> Self {
        Self { value, staleness_s: staleness_s.max(0.0), base_variance: 0.0 }
    }

    /// E[value now] = reported + λΔ.
    ///
    /// NOTE THE SIGN: this is monotonically NON-DECREASING in staleness. A stale
    /// report is evidence the true value is *higher*, never lower. Any code that
    /// moves this estimate downward as data ages is inverted.
    pub fn expected_now(&self, model: &ArrivalModel) -> f64 {
        self.value + model.rate_per_s * self.staleness_s
    }

    /// Var[value now] = λΔ + σ².
    pub fn variance_now(&self, model: &ArrivalModel) -> f64 {
        model.rate_per_s * self.staleness_s + self.base_variance
    }

    /// Fisher information I = 1/Var — i.e. precision.
    ///
    /// A zero-variance observation carries infinite information (it pins the
    /// value exactly); that is mathematically correct and the fusion below
    /// handles it by letting such an observation dominate.
    pub fn fisher_information(&self, model: &ArrivalModel) -> f64 {
        let var = self.variance_now(model);
        if var <= 0.0 {
            f64::INFINITY
        } else {
            1.0 / var
        }
    }
}

/// The result of fusing several observations by their information.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FusedEstimate {
    /// θ̂ — the minimum-variance estimate.
    pub value: f64,
    /// 1/ΣI — variance of the estimate.
    pub variance: f64,
    /// ΣI — total Fisher information (precision) behind the estimate.
    pub total_information: f64,
    /// How many observations contributed.
    pub n_observations: usize,
}

impl FusedEstimate {
    /// An estimate backed by nothing at all: infinite variance, zero information.
    ///
    /// This is the state a node is in when it has no peers, and every decision
    /// helper below is written so that this state produces the CONSERVATIVE
    /// answer rather than a confident wrong one.
    pub fn no_information() -> Self {
        Self {
            value: 0.0,
            variance: f64::INFINITY,
            total_information: 0.0,
            n_observations: 0,
        }
    }

    /// θ̂ + z·σ. Returns +∞ when there is no information.
    pub fn upper_confidence_bound(&self, z: f64) -> f64 {
        if self.total_information <= 0.0 || !self.variance.is_finite() {
            return f64::INFINITY;
        }
        self.value + z * self.variance.sqrt()
    }

    /// θ̂ − z·σ. Returns −∞ when there is no information.
    pub fn lower_confidence_bound(&self, z: f64) -> f64 {
        if self.total_information <= 0.0 || !self.variance.is_finite() {
            return f64::NEG_INFINITY;
        }
        self.value - z * self.variance.sqrt()
    }

    /// THE FAIL-SAFE DECISION. "Might the remote counter be ahead of `local`?"
    ///
    /// Decided on the UPPER confidence bound, so uncertainty resolves toward
    /// "yes, keep working". With no observations this is unconditionally `true`
    /// — a node that knows nothing must never conclude it is finished. This is
    /// the exact property the production heuristic lacked.
    pub fn is_behind(&self, local: f64, z: f64) -> bool {
        self.upper_confidence_bound(z) > local
    }

    /// Cramér–Rao lower bound: no unbiased estimator of this quantity can have
    /// variance below 1/ΣI given these observations.
    pub fn cramer_rao_bound(&self) -> f64 {
        if self.total_information <= 0.0 {
            f64::INFINITY
        } else {
            1.0 / self.total_information
        }
    }

    /// Efficiency of an achieved estimator against the bound, in [0, 1].
    ///
    /// 1.0 means the estimator is optimal (saturates Cramér–Rao). A low value
    /// says there is headroom in the ESTIMATOR; a value near 1.0 says stop
    /// tuning the estimator and go improve the observations themselves.
    pub fn efficiency(&self, achieved_variance: f64) -> f64 {
        let bound = self.cramer_rao_bound();
        if !bound.is_finite() || achieved_variance <= 0.0 || !achieved_variance.is_finite() {
            return 0.0;
        }
        (bound / achieved_variance).clamp(0.0, 1.0)
    }
}

/// Fuse observations by Fisher information (inverse-variance weighting).
///
///     θ̂ = Σ Iᵢ·E[valueᵢ now] / Σ Iᵢ        Var(θ̂) = 1 / Σ Iᵢ
///
/// This is the minimum-variance unbiased combination for independent sources,
/// and information is ADDITIVE — each extra source strictly increases precision.
///
/// An empty slice yields [`FusedEstimate::no_information`], not a panic and not
/// a silently-plausible number.
pub fn fuse(observations: &[StaleObservation], model: &ArrivalModel) -> FusedEstimate {
    if observations.is_empty() {
        return FusedEstimate::no_information();
    }

    // A zero-variance (infinitely informative) observation pins the estimate.
    // Handle it explicitly rather than letting ∞/∞ produce NaN.
    let exact: Vec<&StaleObservation> = observations
        .iter()
        .filter(|o| o.variance_now(model) <= 0.0)
        .collect();
    if !exact.is_empty() {
        // Several "exact" sources: average them; they claim the same precision.
        let sum: f64 = exact.iter().map(|o| o.expected_now(model)).sum();
        return FusedEstimate {
            value: sum / exact.len() as f64,
            variance: 0.0,
            total_information: f64::INFINITY,
            n_observations: observations.len(),
        };
    }

    let mut total_information = 0.0;
    let mut weighted_sum = 0.0;
    for obs in observations {
        let info = obs.fisher_information(model);
        if !info.is_finite() || info <= 0.0 {
            continue;
        }
        total_information += info;
        weighted_sum += info * obs.expected_now(model);
    }

    if total_information <= 0.0 {
        return FusedEstimate::no_information();
    }

    FusedEstimate {
        value: weighted_sum / total_information,
        variance: 1.0 / total_information,
        total_information,
        n_observations: observations.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAMBDA: f64 = 2.0; // blocks/sec
    fn model() -> ArrivalModel {
        ArrivalModel::new(LAMBDA)
    }

    // ── The sign fix — the whole point of this module ────────────────────────

    #[test]
    fn staleness_raises_the_expected_value_never_lowers_it() {
        let m = model();
        let fresh = StaleObservation::aged(1000.0, 0.0);
        let stale = StaleObservation::aged(1000.0, 60.0);
        assert_eq!(fresh.expected_now(&m), 1000.0);
        assert_eq!(stale.expected_now(&m), 1000.0 + LAMBDA * 60.0);
        assert!(
            stale.expected_now(&m) > fresh.expected_now(&m),
            "a stale report means the true value is HIGHER, not lower"
        );
    }

    #[test]
    fn staleness_lowers_precision() {
        let m = model();
        let fresh = StaleObservation::aged(1000.0, 1.0);
        let stale = StaleObservation::aged(1000.0, 100.0);
        assert!(stale.variance_now(&m) > fresh.variance_now(&m));
        assert!(stale.fisher_information(&m) < fresh.fisher_information(&m));
    }

    /// Regression test for the production incident: the old heuristic dragged the
    /// estimate toward local height. Ours must never do that.
    #[test]
    fn estimate_never_decays_toward_local() {
        let m = model();
        let local = 115_247.0;
        let peer_tip = 21_364_541.0;
        // Same observation, aging further and further.
        let mut previous = f64::NEG_INFINITY;
        for staleness in [0.0, 60.0, 600.0, 6_000.0, 60_000.0] {
            let est = fuse(&[StaleObservation::aged(peer_tip, staleness)], &m);
            assert!(
                est.value >= peer_tip,
                "estimate fell below the reported tip at staleness {staleness}"
            );
            assert!(est.value > local, "estimate collapsed toward local height");
            assert!(est.value >= previous, "estimate must not decrease with age");
            previous = est.value;
        }
    }

    // ── The fail-safe property ───────────────────────────────────────────────

    #[test]
    fn no_observations_means_assume_behind() {
        let est = fuse(&[], &model());
        assert_eq!(est.n_observations, 0);
        assert_eq!(est.total_information, 0.0);
        assert!(!est.variance.is_finite());
        assert!(est.upper_confidence_bound(2.0).is_infinite());
        // The property that the production heuristic lacked:
        assert!(est.is_behind(0.0, 2.0));
        assert!(est.is_behind(f64::MAX, 2.0), "ignorance must never read as 'done'");
    }

    #[test]
    fn stale_data_widens_the_bound_rather_than_moving_it_down() {
        let m = model();
        let local = 1_000_000.0;
        let fresh = fuse(&[StaleObservation::aged(1_000_001.0, 1.0)], &m);
        let stale = fuse(&[StaleObservation::aged(1_000_001.0, 3_600.0)], &m);
        assert!(stale.variance > fresh.variance);
        assert!(stale.upper_confidence_bound(2.0) > fresh.upper_confidence_bound(2.0));
        assert!(stale.is_behind(local, 2.0));
    }

    #[test]
    fn caught_up_is_only_declared_with_real_information() {
        let m = model();
        // Fresh, precise agreement that the tip equals our local height.
        let est = fuse(
            &[
                StaleObservation::aged(500.0, 0.5),
                StaleObservation::aged(500.0, 0.5),
                StaleObservation::aged(500.0, 0.5),
            ],
            &m,
        );
        assert!(est.total_information > 0.0);
        // With a tight bound and a local height comfortably above the estimate,
        // we are permitted to say "not behind".
        assert!(!est.is_behind(600.0, 2.0));
    }

    // ── Fusion properties ────────────────────────────────────────────────────

    #[test]
    fn information_is_additive_and_more_sources_are_more_precise() {
        let m = model();
        let one = fuse(&[StaleObservation::aged(100.0, 10.0)], &m);
        let three = fuse(
            &[
                StaleObservation::aged(100.0, 10.0),
                StaleObservation::aged(100.0, 10.0),
                StaleObservation::aged(100.0, 10.0),
            ],
            &m,
        );
        assert!((three.total_information - 3.0 * one.total_information).abs() < 1e-9);
        assert!(three.variance < one.variance);
    }

    #[test]
    fn fused_estimate_lies_within_the_observed_range() {
        let m = model();
        let obs = [
            StaleObservation::aged(1000.0, 5.0),
            StaleObservation::aged(2000.0, 5.0),
            StaleObservation::aged(1500.0, 5.0),
        ];
        let est = fuse(&obs, &m);
        let lo = obs.iter().map(|o| o.expected_now(&m)).fold(f64::MAX, f64::min);
        let hi = obs.iter().map(|o| o.expected_now(&m)).fold(f64::MIN, f64::max);
        assert!(est.value >= lo && est.value <= hi);
    }

    #[test]
    fn the_fresher_source_dominates() {
        let m = model();
        // Fresh says 1000, very stale says 5000. The fresh one is far more
        // informative, so the estimate must sit nearer its (advanced) value.
        let fresh = StaleObservation::aged(1000.0, 0.1);
        let stale = StaleObservation::aged(5000.0, 10_000.0);
        let est = fuse(&[fresh, stale], &m);
        let d_fresh = (est.value - fresh.expected_now(&m)).abs();
        let d_stale = (est.value - stale.expected_now(&m)).abs();
        assert!(d_fresh < d_stale, "the precise source must dominate the vague one");
    }

    #[test]
    fn base_variance_downweights_an_untrusted_source() {
        let m = model();
        let trusted = StaleObservation { value: 100.0, staleness_s: 1.0, base_variance: 0.0 };
        let noisy = StaleObservation { value: 900.0, staleness_s: 1.0, base_variance: 10_000.0 };
        assert!(trusted.fisher_information(&m) > noisy.fisher_information(&m));
        let est = fuse(&[trusted, noisy], &m);
        assert!(est.value < 500.0, "the noisy source must not drag the estimate");
    }

    #[test]
    fn an_exact_observation_pins_the_estimate_without_nan() {
        let m = ArrivalModel::new(0.0); // no drift ⇒ zero variance when σ²=0
        let est = fuse(&[StaleObservation::fresh(4242.0), StaleObservation::aged(1.0, 0.0)], &m);
        assert!(est.value.is_finite(), "must not produce NaN from ∞/∞");
        assert_eq!(est.variance, 0.0);
        assert!(est.total_information.is_infinite());
    }

    // ── Cramér–Rao ───────────────────────────────────────────────────────────

    #[test]
    fn cramer_rao_bound_is_the_inverse_of_information() {
        let m = model();
        let est = fuse(&[StaleObservation::aged(10.0, 4.0)], &m);
        assert!((est.cramer_rao_bound() - 1.0 / est.total_information).abs() < 1e-12);
        // The fused variance saturates the bound — this estimator IS optimal.
        assert!((est.efficiency(est.variance) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn a_worse_estimator_scores_below_one() {
        let m = model();
        let est = fuse(&[StaleObservation::aged(10.0, 4.0)], &m);
        assert!(est.efficiency(est.variance * 10.0) < 0.11);
        assert_eq!(est.efficiency(f64::INFINITY), 0.0);
        assert_eq!(est.efficiency(-1.0), 0.0);
    }

    #[test]
    fn no_information_has_an_infinite_bound() {
        assert!(FusedEstimate::no_information().cramer_rao_bound().is_infinite());
        assert_eq!(FusedEstimate::no_information().efficiency(1.0), 0.0);
    }

    #[test]
    fn arrival_model_rejects_nonsense_rates() {
        assert_eq!(ArrivalModel::new(-5.0).rate_per_s, 0.0);
        assert_eq!(ArrivalModel::new(f64::NAN).rate_per_s, 0.0);
        assert_eq!(ArrivalModel::new(f64::INFINITY).rate_per_s, 0.0);
        assert_eq!(ArrivalModel::new(2.5).rate_per_s, 2.5);
    }
}
