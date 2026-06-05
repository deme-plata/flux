//! Khovanov homology fork-detection — lifted target from
//! `QTFT/physics/higher_tqft.rs`.
//!
//! Pre-lift scaffold. The full module will provide:
//!
//! - `KhovanovComplex` — chain complex graded by `(i, j)`
//! - `compute_khovanov(crossings)` — Euler-characteristic shortcut
//!   (acknowledged in upstream as not the full cube-of-resolutions)
//! - `SecurityLevel` — enum mirroring upstream `(Jones, Khovanov,
//!   Donaldson)` tiers
//! - `rasmussen::s_invariant(complex)` — NEW; the upstream computes the
//!   complex but does not extract the *s*-invariant. We add it for
//!   fork resolution (lower *s* = simpler knot = canonical fork)
//!
//! Source: `/opt/orobit/shared/QTFT/qtft-api/src/physics/higher_tqft.rs`
//! on Beta (~500 LOC).

/// Module marker — lift not yet performed.
pub fn _scaffold() {}

/// Rasmussen *s*-invariant. NEW relative to upstream QTFT-api.
pub mod rasmussen {
    /// Placeholder for the *s*-invariant computation.
    pub fn _scaffold() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_present() {
        _scaffold();
        rasmussen::_scaffold();
    }
}
