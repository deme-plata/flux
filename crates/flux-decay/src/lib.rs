//! flux-decay — bounded standing for polities whose members do not die.
//!
//! This crate is the executable form of the *entrenchment theorem* from
//! **"The Arithmetic of Long Life"** (SIGIL research library, 2026-07-25):
//!
//! > Let standing — capital, reputation, voting weight — compound at real rate
//! > `r` and decay at rate `λ`. It evolves as `e^{(r-λ)t}`, which is bounded
//! > **iff `λ > r`**.
//!
//! Every constitution ever written silently relies on mortality as its term
//! limit. A polity of long-lived members — or of software agents, which do not
//! die at all — must state the limit explicitly. That is what this crate is:
//! one inequality, enforced.
//!
//! # The two halves, and why they are separate
//!
//! **Design half** (`design`): `f64` helpers to *choose* parameters — minimum
//! decay rate, half-life conversions, saturation ceilings. Off-chain only.
//!
//! **Consensus half** ([`Standing`], [`DecayParams`]): integer fixed-point.
//! Floating point is **not** deterministic across platforms/compilers, so it
//! must never touch a state root. Decay here is an exact integer
//! multiply-then-divide on `u128`; identical on every node, forever.
//!
//! ```
//! use flux_decay::{design, DecayParams, Standing};
//!
//! // A citizen earning 4%/yr of standing needs decay faster than that.
//! let lambda_min = design::min_decay_rate(0.04);
//! assert!(design::half_life_from_rate(lambda_min) < 17.4); // years
//!
//! // Enforce it on-chain: 6%/yr decay, applied per annual tick.
//! let p = DecayParams::from_rate_per_tick(0.06).unwrap();
//! let mut s = Standing::ZERO;
//! for _ in 0..500 { s.accrue(1_000_000); s.tick(&p); }
//! // Bounded: 500 years of accrual does not run away.
//! assert!(s.get() < 20_000_000);
//! ```

#![forbid(unsafe_code)]

/// Fixed-point scale: standing is counted in micro-units (1 unit = 1e6 micro).
pub const MICRO: u128 = 1_000_000;

/// Denominator for the per-tick retention fraction. Chosen as a power of two so
/// the division is exact and cheap, and identical on every platform.
pub const DECAY_DEN: u128 = 1 << 32;

/// Per-tick retention, stored as the exact rational `num / DECAY_DEN`.
///
/// Retention `f` relates to a continuous decay rate `λ` by `f = e^{-λ}`.
/// Constructed off-chain (via `f64`), then **committed as integers** — nodes
/// replaying the chain use only `num`, never the float it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecayParams {
    num: u128,
}

impl DecayParams {
    /// Build from an explicit retention numerator over [`DECAY_DEN`].
    ///
    /// Returns `None` if `num >= DECAY_DEN` (retention ≥ 1 = no decay, which
    /// would leave standing unbounded and defeat the entire purpose).
    pub fn from_num(num: u128) -> Option<Self> {
        (num < DECAY_DEN).then_some(Self { num })
    }

    /// Build from a continuous decay rate per tick (e.g. 0.06 = 6%/tick).
    /// `rate` must be finite and > 0.
    pub fn from_rate_per_tick(rate: f64) -> Option<Self> {
        if !rate.is_finite() || rate <= 0.0 {
            return None;
        }
        let retention = (-rate).exp(); // in (0, 1)
        let num = (retention * DECAY_DEN as f64).floor() as u128;
        Self::from_num(num.min(DECAY_DEN - 1))
    }

    /// Build from a half-life expressed in ticks.
    pub fn from_half_life(half_life_ticks: f64) -> Option<Self> {
        if !half_life_ticks.is_finite() || half_life_ticks <= 0.0 {
            return None;
        }
        Self::from_rate_per_tick(std::f64::consts::LN_2 / half_life_ticks)
    }

    /// The retention numerator actually committed to state.
    pub fn num(&self) -> u128 {
        self.num
    }

    /// Retention as a float — **reporting only**, never for state transitions.
    pub fn retention(&self) -> f64 {
        self.num as f64 / DECAY_DEN as f64
    }

    /// Effective continuous decay rate per tick — reporting only.
    pub fn effective_rate(&self) -> f64 {
        -self.retention().ln()
    }
}

/// An accumulated standing balance, in micro-units.
///
/// Saturating and monotone: `tick` can never *increase* standing, and accrual
/// saturates rather than wrapping, so a hostile accrual stream cannot overflow
/// a node into a divergent state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Standing {
    micro: u128,
}

impl Standing {
    /// Zero standing.
    pub const ZERO: Self = Self { micro: 0 };

    /// From raw micro-units.
    pub const fn from_micro(micro: u128) -> Self {
        Self { micro }
    }

    /// Raw micro-units.
    pub const fn get(&self) -> u128 {
        self.micro
    }

    /// Whole units (truncating) — for display.
    pub const fn units(&self) -> u128 {
        self.micro / MICRO
    }

    /// Add earned standing. Saturates instead of wrapping.
    pub fn accrue(&mut self, micro: u128) {
        self.micro = self.micro.saturating_add(micro);
    }

    /// Apply one tick of decay: `standing = standing * num / DECAY_DEN`.
    ///
    /// Exact integer arithmetic — the same on every node. Truncation is toward
    /// zero, so standing is monotonically non-increasing under `tick` and
    /// genuinely reaches zero rather than trailing an epsilon forever.
    pub fn tick(&mut self, p: &DecayParams) {
        // u128 * (< 2^32) can overflow only above 2^96 micro-units — far beyond
        // any real supply — but we widen defensively rather than assume it.
        self.micro = match self.micro.checked_mul(p.num) {
            Some(v) => v / DECAY_DEN,
            None => (self.micro / DECAY_DEN) * p.num,
        };
    }

    /// Accrue then decay, the canonical per-epoch order.
    ///
    /// The order matters and is part of the consensus rule. Under a constant
    /// inflow `a` and retention `f`, repeated `step` converges to
    /// `a·f / (1 − f)` — one accrual unit *below* the decay-then-accrue ceiling
    /// `a / (1 − f)`. Accruing first means this epoch's earnings are themselves
    /// decayed once, which is the conservative choice: no one banks a full
    /// undecayed epoch. Use [`design::saturation_micro`] to predict the ceiling.
    pub fn step(&mut self, earned_micro: u128, p: &DecayParams) {
        self.accrue(earned_micro);
        self.tick(p);
    }
}

/// Off-chain parameter design. **Never** call these inside a state transition.
pub mod design {
    /// The theorem: standing is bounded iff `λ > r`. This returns the infimum
    /// `r`; any admissible decay rate must be strictly greater.
    pub fn min_decay_rate(growth_rate: f64) -> f64 {
        growth_rate
    }

    /// Is a (growth, decay) pair bounded?
    pub fn is_bounded(growth_rate: f64, decay_rate: f64) -> bool {
        decay_rate > growth_rate
    }

    /// Half-life implied by a continuous decay rate.
    pub fn half_life_from_rate(decay_rate: f64) -> f64 {
        std::f64::consts::LN_2 / decay_rate
    }

    /// Continuous decay rate implied by a half-life.
    pub fn rate_from_half_life(half_life: f64) -> f64 {
        std::f64::consts::LN_2 / half_life
    }

    /// The maximum half-life a constitution may permit, given how fast standing
    /// compounds. At `r = 0.04`/yr this is ≈ 17.3 years — the figure computed in
    /// "The Arithmetic of Long Life".
    pub fn max_half_life(growth_rate: f64) -> f64 {
        half_life_from_rate(min_decay_rate(growth_rate))
    }

    /// Steady-state standing as a multiple of the per-unit-time inflow:
    /// `S* = a / (λ - r)`. `None` when unbounded (`λ ≤ r`) — an aristocracy.
    pub fn saturation_ratio(growth_rate: f64, decay_rate: f64) -> Option<f64> {
        is_bounded(growth_rate, decay_rate).then(|| 1.0 / (decay_rate - growth_rate))
    }

    /// Entrenchment factor: how much more standing a member living `long` years
    /// accumulates than one living `short`, absent decay. `e^{r Δt}`.
    pub fn entrenchment_factor(growth_rate: f64, short: f64, long: f64) -> f64 {
        (growth_rate * (long - short)).exp()
    }

    /// Predicted steady-state standing in micro-units for the discrete
    /// accrue-then-decay loop [`super::Standing::step`]: `a·f / (1 − f)`.
    ///
    /// This is the ceiling an immortal member converges to — the number a
    /// constitution should quote when it claims standing is bounded.
    pub fn saturation_micro(inflow_micro: f64, retention: f64) -> f64 {
        inflow_micro * retention / (1.0 - retention)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theorem_bounded_iff_decay_exceeds_growth() {
        assert!(design::is_bounded(0.04, 0.06));
        assert!(!design::is_bounded(0.04, 0.04)); // equality is NOT enough
        assert!(!design::is_bounded(0.04, 0.02));
    }

    #[test]
    fn paper_figure_max_half_life_at_four_percent() {
        // "The Arithmetic of Long Life": half-life must be < 17.3 yr at r = 4%.
        let hl = design::max_half_life(0.04);
        assert!((hl - 17.329).abs() < 0.01, "got {hl}");
    }

    #[test]
    fn paper_figure_entrenchment_300_vs_80_years() {
        // 80 → 300 years at 4% real compounding ≈ 6634×.
        let f = design::entrenchment_factor(0.04, 80.0, 300.0);
        assert!((f / 6634.0 - 1.0).abs() < 0.01, "got {f}");
    }

    #[test]
    fn decay_is_monotone_and_reaches_zero() {
        let p = DecayParams::from_rate_per_tick(0.5).unwrap();
        let mut s = Standing::from_micro(1_000_000);
        let mut last = s.get();
        for _ in 0..500 {
            s.tick(&p);
            assert!(s.get() <= last, "decay must never increase standing");
            last = s.get();
        }
        assert_eq!(s.get(), 0, "truncation must terminate at exactly zero");
    }

    #[test]
    fn constant_inflow_saturates_near_predicted_ceiling() {
        // Accrue-then-decay steady state: S* = a·f / (1 - f). Note this sits
        // exactly one accrual unit below the naive a/(1-f).
        let p = DecayParams::from_rate_per_tick(0.06).unwrap();
        let a: u128 = 1_000_000;
        let predicted = design::saturation_micro(a as f64, p.retention());
        assert!(
            (a as f64 / (1.0 - p.retention()) - predicted - a as f64).abs() < 1.0,
            "the two conventions must differ by exactly one accrual unit"
        );
        let mut s = Standing::ZERO;
        for _ in 0..5_000 {
            s.step(a, &p);
        }
        let got = s.get() as f64;
        assert!(
            (got / predicted - 1.0).abs() < 0.01,
            "saturation {got} vs predicted {predicted}"
        );
    }

    #[test]
    fn immortal_incumbent_cannot_outgrow_a_newcomer_without_bound() {
        // The whole point: an agent accruing for 10 000 ticks holds only a
        // bounded multiple of one accruing for 100.
        let p = DecayParams::from_rate_per_tick(0.06).unwrap();
        let (mut old, mut new) = (Standing::ZERO, Standing::ZERO);
        for _ in 0..10_000 {
            old.step(1_000_000, &p);
        }
        for _ in 0..100 {
            new.step(1_000_000, &p);
        }
        let ratio = old.get() as f64 / new.get() as f64;
        assert!(ratio < 1.05, "immortality advantage unbounded: {ratio}×");
    }

    #[test]
    fn half_life_round_trips_through_integer_params() {
        let p = DecayParams::from_half_life(17.0).unwrap();
        let mut s = Standing::from_micro(1_000_000_000);
        for _ in 0..17 {
            s.tick(&p);
        }
        let frac = s.get() as f64 / 1_000_000_000.0;
        assert!((frac - 0.5).abs() < 0.001, "after one half-life: {frac}");
    }

    #[test]
    fn params_reject_non_decaying_configurations() {
        assert!(DecayParams::from_num(DECAY_DEN).is_none());
        assert!(DecayParams::from_rate_per_tick(0.0).is_none());
        assert!(DecayParams::from_rate_per_tick(-1.0).is_none());
        assert!(DecayParams::from_half_life(0.0).is_none());
    }

    #[test]
    fn accrual_saturates_rather_than_wrapping() {
        let mut s = Standing::from_micro(u128::MAX);
        s.accrue(u128::MAX);
        assert_eq!(s.get(), u128::MAX);
        // And a tick on a saturated balance must not panic or overflow.
        let p = DecayParams::from_rate_per_tick(0.06).unwrap();
        s.tick(&p);
        assert!(s.get() < u128::MAX);
    }

    #[test]
    fn decay_is_deterministic_across_repeated_runs() {
        let p = DecayParams::from_num(4_000_000_000).unwrap();
        let run = || {
            let mut s = Standing::ZERO;
            for i in 0..1_000 {
                s.step((i % 7) as u128 * 1_000, &p);
            }
            s.get()
        };
        assert_eq!(run(), run());
    }
}
