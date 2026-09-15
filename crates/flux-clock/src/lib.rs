//! # flux-clock — Kristensen Time, made of hands
//!
//! Three ideas, one crate:
//!
//! 1. **The three clocks** ([`kristensen`]) — proper seconds `t`, Margolus–Levitin ticks
//!    `n = 2Et/πħ`, and agreed ticks `h` = block height. The named quantities the SIGIL
//!    pages already show (agreement cost `A`, ledger temperature `T_L`, finality-horizon
//!    temperature `T_F`, consensus dilation `γ_C`, the depth clock `τ_d` and `K_fix`) are
//!    here as pure functions with their honesty labels.
//! 2. **The quantum Chinese-remainder clock** ([`crt`]) — arXiv:2608.07938 (Nagar, Hamma,
//!    Palmero, Radzihovsky, Yang, Lloyd, Aug 2026). `m` hands with pairwise-coprime periods,
//!    each reads `t mod x_j` with the Heisenberg-limited phase distribution of their Eq. (6);
//!    the fault-tolerant protocol (fractional parts → wrap check → CRT on the integer part →
//!    mean residual added back) is implemented verbatim and their Fig. 2 is reproduced as a
//!    unit test. Redundant hands let a reader spot the one hand that lies.
//! 3. **The Earth and the ledger as hands** ([`earth`], [`face`], [`p2p`]) — the Earth
//!    Rotation Angle from the IERS formula (UT1 = UTC + UT1−UTC from the sigil-earth feed) is
//!    a phase hand that cannot count days; the block height is a counter. With the `p2p`
//!    feature each flux-p2p peer holds ONE hand and gossips only its remainder on
//!    `/sigil/g2/clock`, so the time exists only as the composition of independent peers —
//!    which is what the paper's optimality proof says: entanglement between hands buys
//!    nothing, the information is in the composition.
//!
//! Constants are CODATA 2018/2022 (exact ħ and k_B).

pub mod crt;
pub mod earth;
pub mod face;
pub mod kristensen;
pub mod live;
#[cfg(feature = "p2p")]
pub mod p2p;

/// Reduced Planck constant, J·s (exact since the 2019 SI).
pub const HBAR: f64 = 1.054_571_817e-34;
/// Boltzmann constant, J/K (exact).
pub const KB: f64 = 1.380_649e-23;
/// π, spelled out so formulas read like the paper.
pub const PI: f64 = std::f64::consts::PI;

/// Honesty label carried beside every number the crate emits. Same vocabulary as the
/// kristensen-time page: what was read off an instrument, what was computed from it, what is
/// a formal analogy, and what is a choice someone made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Label {
    Measured,
    Derived,
    Analogy,
    ModelChoice,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}
