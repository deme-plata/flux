//! Fork-detection glue — combines DagKnight BFT vote aggregation with
//! QTFT Khovanov fingerprints + Rasmussen *s*-invariant for fork
//! resolution.
//!
//! This is the layer that makes "DagKnight + QTFT" one consensus
//! engine rather than two side-by-side. NEW code, not lifted.
//!
//! Flow when two blocks arrive at the same height:
//!
//! 1. DagKnight vote aggregation determines whether both blocks
//!    received ≥2/3 votes (i.e. both are validly proposed).
//! 2. If both, compute the Khovanov fingerprint of each block's
//!    transaction knot via `super::khovanov::compute_khovanov` (when
//!    `feature = "qtft"`).
//! 3. If fingerprints differ, the blocks are genuinely topologically
//!    distinct → real fork. The block with the lower Rasmussen
//!    *s*-invariant wins (lower slice genus = simpler knot =
//!    canonical ordering).
//! 4. Fall back to DagKnight's heaviest-DAG fork choice if QTFT is
//!    disabled or returns equal fingerprints.

use serde::{Deserialize, Serialize};

/// Outcome of fork detection between two competing tips.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForkVerdict {
    /// No fork — either same tip, or one is an obvious extension.
    NoFork,
    /// Real fork detected; left tip wins after Rasmussen tie-break.
    LeftWins,
    /// Real fork detected; right tip wins.
    RightWins,
    /// Real fork detected; QTFT cannot resolve. Fall back to
    /// DagKnight heaviest-DAG.
    Unresolved,
}

/// Configuration for the fork-detection glue layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkDetectionConfig {
    /// Use Khovanov fingerprints when `feature = "qtft"` is enabled.
    pub use_khovanov: bool,
    /// Use Rasmussen *s*-invariant as the tie-break.
    pub use_rasmussen: bool,
}

impl Default for ForkDetectionConfig {
    fn default() -> Self {
        Self {
            use_khovanov: cfg!(feature = "qtft"),
            use_rasmussen: cfg!(feature = "qtft"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_feature_flag() {
        let c = ForkDetectionConfig::default();
        assert_eq!(c.use_khovanov, cfg!(feature = "qtft"));
        assert_eq!(c.use_rasmussen, cfg!(feature = "qtft"));
    }

    #[test]
    fn verdict_variants_are_distinct() {
        assert_ne!(ForkVerdict::NoFork, ForkVerdict::LeftWins);
        assert_ne!(ForkVerdict::LeftWins, ForkVerdict::RightWins);
        assert_ne!(ForkVerdict::RightWins, ForkVerdict::Unresolved);
    }
}
