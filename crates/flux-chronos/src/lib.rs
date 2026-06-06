//! flux-chronos — deterministic multiverse simulation for Flux chains.
//!
//! This scaffold (CHRONOS-A) lands the core abstractions: virtual clock,
//! seeded RNG, in-memory message bus, and a [`SimNode`] trait that a chain
//! crate (e.g. sigil-node) implements to plug in. A demo at the bottom
//! shows two simulated nodes exchanging a message under virtual time —
//! a 1-hour gossip round trip resolves in microseconds because the clock
//! is whatever the test says it is.
//!
//! See `flux/docs/flux-chronos-spec.md` for the full architecture, the
//! multiverse-fork / time-travel / property-fuzz / MCP / browser-viz phases
//! that follow this scaffold, and the rationale for every design choice.

#![warn(missing_docs)]

mod clock;
mod net;
mod node;
pub mod tourbillon;
mod universe;

pub use clock::{TickId, VirtualClock};
pub use net::{Envelope, NetEdge, NodeId, ScheduledDelivery};
pub use node::{NodeStepResult, SimNode};
pub use tourbillon::{Injection, PermutationOutcome, TourbillonReport};
pub use universe::{ScenarioSeed, Universe};

/// Convenience: simulated duration in microseconds. We keep micros instead
/// of nanos so a u64 fits ~580 years of simulated time — enough for any
/// realistic chain scenario without integer overflow.
pub type SimDuration = u64;

/// Convert seconds → SimDuration.
pub const fn secs(n: u64) -> SimDuration {
    n * 1_000_000
}

/// Convert milliseconds → SimDuration.
pub const fn millis(n: u64) -> SimDuration {
    n * 1_000
}

/// Convert minutes → SimDuration.
pub const fn mins(n: u64) -> SimDuration {
    n * 60 * 1_000_000
}

/// Convert hours → SimDuration.
pub const fn hours(n: u64) -> SimDuration {
    n * 60 * 60 * 1_000_000
}

/// Plan the minimum gossip **redundancy** needed to hit a target delivery rate under packet loss.
///
/// Derived from the chronos benchmark: with per-message drop probability `drop_prob`, a single send
/// lands `1 - drop_prob` of the time, and `r` independent re-sends (sinks dedup by id) miss only when
/// *all* `r` are dropped — so unique-delivery ≈ `1 - drop_prob^r`. This returns the smallest `r` whose
/// expected delivery reaches `target_rate`. Lets an operator size gossip fan-out before deploying.
///
/// `drop_prob` and `target_rate` are clamped to sane ranges; returns `1` when no loss or an
/// already-met target, and caps at 32 to avoid pathological fan-out.
pub fn min_redundancy_for(drop_prob: f64, target_rate: f64) -> u32 {
    let d = drop_prob.clamp(0.0, 0.999_999);
    let t = target_rate.clamp(0.0, 0.999_999);
    if d == 0.0 { return 1; }
    let mut r = 1u32;
    while 1.0 - d.powi(r as i32) < t && r < 32 {
        r += 1;
    }
    r
}

#[cfg(test)]
mod redundancy_planner_tests {
    use super::min_redundancy_for;

    #[test]
    fn no_loss_needs_no_redundancy() {
        assert_eq!(min_redundancy_for(0.0, 0.99), 1);
    }

    #[test]
    fn matches_the_benchmark_curve() {
        // benchmark: drop 0.3 → r=1 gives ~70%, r=3 ~97.4%, r=5 ~99.7%.
        assert_eq!(min_redundancy_for(0.3, 0.70), 1); // one send clears 70%
        assert_eq!(min_redundancy_for(0.3, 0.97), 3); // need 3 to clear 97%
        assert_eq!(min_redundancy_for(0.3, 0.997), 5); // need 5 to clear 99.7%
    }

    #[test]
    fn heavier_loss_needs_more_redundancy() {
        assert!(min_redundancy_for(0.5, 0.99) > min_redundancy_for(0.3, 0.99));
    }
}
