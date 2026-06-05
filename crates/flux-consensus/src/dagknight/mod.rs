//! DagKnight BFT engine — port target for Quillon `q-dag-knight` (~10K LOC).
//!
//! Pre-port scaffold. Submodules will land as the port progresses:
//!
//! - `bft` — vote rounds, two-chain commit rule
//! - `leader` — round-robin and quorum-based leader election
//! - `certificate` — quorum certificate aggregation
//! - `vote` — vote types and aggregation logic
//! - `fork_choice` — heaviest-DAG selection, bridges into
//!   `super::fork_detection` for the Khovanov-augmented decision

use serde::{Deserialize, Serialize};

/// Round counter — monotone, advances on quorum certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Round(pub u64);

impl Round {
    /// The genesis round.
    pub const GENESIS: Round = Round(0);

    /// Advance one round.
    pub fn next(self) -> Round {
        Round(self.0.saturating_add(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_advances_monotonically() {
        let r = Round::GENESIS;
        assert_eq!(r.next(), Round(1));
        assert_eq!(r.next().next(), Round(2));
    }

    #[test]
    fn round_saturates_at_u64_max() {
        let r = Round(u64::MAX);
        assert_eq!(r.next(), Round(u64::MAX));
    }
}
