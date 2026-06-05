//! # flux-consensus — DagKnight BFT + QTFT knot-theory fork detection
//!
//! The consensus heart of SIGIL. Combines:
//!
//! - **DagKnight BFT engine** ported from Quillon Graph's `q-dag-knight`
//!   crate (leader election, certificate aggregation, vote rounds,
//!   fork-choice).
//! - **QTFT knot-theory primitives** lifted from the QTFT-api research
//!   crate (`/opt/orobit/shared/QTFT/qtft-api` on Beta): transactions as
//!   knot diagrams, Jones polynomial, Khovanov homology fork
//!   fingerprints, Rasmussen *s*-invariant for fork resolution, higher
//!   TQFT engine.
//!
//! ## Modules
//!
//! | Module | Role |
//! |---|---|
//! | [`dagknight`] | BFT engine. Always compiled. |
//! | [`knot`] | Transaction-knot diagrams + Jones polynomial. Compiled with `feature = "qtft"`. |
//! | [`khovanov`] | Khovanov complex + Rasmussen *s*-invariant. Compiled with `feature = "qtft"`. |
//! | [`tqft`] | Higher TQFT engine + Donaldson invariants. Compiled with `feature = "qtft"`. |
//! | [`fork_detection`] | Glue layer combining DagKnight vote-agg with Khovanov fingerprints. |
//!
//! ## Stability
//!
//! Pre-v0 scaffold (2026-05-29). Public surface will stabilise after
//! the first SIGIL testnet block lands. See `SIGIL_GENESIS_v0.md` for
//! the spec this crate implements.

#![warn(missing_docs)]

pub mod dagknight;

#[cfg(feature = "qtft")]
pub mod knot;

#[cfg(feature = "qtft")]
pub mod khovanov;

#[cfg(feature = "qtft")]
pub mod tqft;

pub mod fork_detection;

/// Crate version reported at runtime. Matches the workspace version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Snapshot of which modules are compiled in. Useful for `flux_zk_combo`-style
/// status reporting and for `sigil-node` `/api/v1/status` pages.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConsensusModules {
    /// DagKnight BFT engine — always present.
    pub dagknight: bool,
    /// QTFT knot diagrams (`feature = "qtft"`).
    pub knot: bool,
    /// Khovanov homology fork detection (`feature = "qtft"`).
    pub khovanov: bool,
    /// Higher TQFT engine + Donaldson invariants (`feature = "qtft"`).
    pub tqft: bool,
    /// Fork-detection glue layer.
    pub fork_detection: bool,
}

impl ConsensusModules {
    /// Returns the modules compiled into this build. Pure const-eval; no I/O.
    pub const fn current() -> Self {
        Self {
            dagknight: true,
            knot: cfg!(feature = "qtft"),
            khovanov: cfg!(feature = "qtft"),
            tqft: cfg!(feature = "qtft"),
            fork_detection: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_non_empty() {
        assert!(!version().is_empty());
    }

    #[test]
    fn dagknight_always_present() {
        let m = ConsensusModules::current();
        assert!(m.dagknight, "dagknight is the core module — always on");
        assert!(m.fork_detection, "fork_detection glue is always on");
    }

    #[test]
    fn qtft_modules_match_feature_flag() {
        let m = ConsensusModules::current();
        assert_eq!(m.knot, cfg!(feature = "qtft"));
        assert_eq!(m.khovanov, cfg!(feature = "qtft"));
        assert_eq!(m.tqft, cfg!(feature = "qtft"));
    }
}
