//! flux-zk::pq — Post-Quantum verifier umbrella.
//!
//! Re-exports the four verifier flavours vendored from Quillon Graph
//! (Beta 185.182.185.227 `q-narwhalknight` workspace) so downstream
//! callers — including fluxc-mcp — have a single import surface for
//! the entire PQ-zk stack.
//!
//! ## Backends
//!
//! | Module              | Type / fn                                | Strength            |
//! |---------------------|------------------------------------------|---------------------|
//! | [`stark`]           | [`StarkVerifier`], [`StarkProof`]        | 10ms verify gate    |
//! | [`lattice`]         | [`LatticeGuardVerifier`], `LatticeGuardProof` | PQ, no pairings, RLWE |
//! | [`recursive`]       | `tip_proof_v2::verify_chain_structure`   | Folds N proofs to 1 |
//! | [`stir`]            | `flux_tip_proof_stir::verify`            | Windowed FRI tip    |
//!
//! ## 10ms gate
//!
//! Wrap any verification in [`VerifyOutcome::measure`] to report whether
//! it met the target latency:
//!
//! ```ignore
//! use flux_zk::pq::{VerifyOutcome, TEN_MS};
//! let outcome = VerifyOutcome::measure("lattice-guard", TEN_MS, || {
//!     verifier.verify(&circuit, &public_inputs, &proof, &srs).is_ok()
//! });
//! assert!(outcome.meets_target, "missed 10ms gate: {} ms", outcome.elapsed_ms);
//! ```

use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Phase-3 verification latency target across the Quillon Graph network.
/// Any proof flavour that consistently lands at or under this gate is
/// considered "instant" from a UX standpoint (block production fits inside it).
pub const TEN_MS: u64 = 10;

/// Outcome of a timed verification, suitable for direct JSON return from
/// MCP tools. `meets_target` is `elapsed_ms <= target_ms`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerifyOutcome {
    pub ok: bool,
    pub elapsed_ms: u64,
    pub target_ms: u64,
    pub backend: String,
    pub meets_target: bool,
}

impl VerifyOutcome {
    pub fn measure<F>(backend: &str, target_ms: u64, f: F) -> Self
    where
        F: FnOnce() -> bool,
    {
        let t0 = Instant::now();
        let ok = f();
        let elapsed_ms = t0.elapsed().as_millis() as u64;
        VerifyOutcome {
            ok,
            elapsed_ms,
            target_ms,
            backend: backend.to_string(),
            meets_target: elapsed_ms <= target_ms,
        }
    }

    pub fn meets_10ms(&self) -> bool {
        self.elapsed_ms <= TEN_MS
    }
}

/// STARK backend — flux-zk-stark.
pub mod stark {
    pub use flux_zk_stark::stark_verifier::BatchVerifier;
    pub use flux_zk_stark::{StarkProof, StarkProver, StarkVerifier, VerificationResult};
}

/// Lattice-based PQ SNARK backend — flux-lattice-guard.
pub mod lattice {
    pub use flux_lattice_guard::{
        ArithmeticCircuit, LatticeGuard, LatticeGuardProof, LatticeGuardSRS,
        LatticeGuardVerifier, R1CSConstraint, RlweParams, SecurityLevel, VerifyingKey,
    };
}

/// Recursive proof folding backend — flux-recursive-proofs.
pub mod recursive {
    pub use flux_recursive_proofs::tip_proof_v1;
    pub use flux_recursive_proofs::tip_proof_v2;
}

/// Windowed FRI tip-proof backend — flux-tip-proof-stir.
pub mod stir {
    pub use flux_tip_proof_stir::{anchor, extend, verify, TipProofStir, WindowBuilder};
}

/// Shared type-shim — ValidatorId etc.
pub mod types {
    pub use flux_zk_types::{validator_id_from_label, ConsensusVote, NodeId, ProposalHash, ValidatorId};
}

/// Summary used by MCP tools to introspect which backends are active.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PqStatus {
    pub stark: bool,
    pub lattice: bool,
    pub recursive: bool,
    pub stir: bool,
    pub target_ms: u64,
    pub feature_pq: bool,
}

pub fn pq_status() -> PqStatus {
    PqStatus {
        stark: true,
        lattice: true,
        recursive: true,
        stir: true,
        target_ms: TEN_MS,
        feature_pq: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_records_elapsed_and_target() {
        let o = VerifyOutcome::measure("test", 50, || {
            std::thread::sleep(std::time::Duration::from_millis(2));
            true
        });
        assert!(o.ok);
        assert!(o.elapsed_ms >= 2);
        assert_eq!(o.target_ms, 50);
        assert!(o.meets_target, "2ms should meet 50ms target");
    }

    #[test]
    fn outcome_flags_missed_target() {
        let o = VerifyOutcome::measure("test", 1, || {
            std::thread::sleep(std::time::Duration::from_millis(15));
            true
        });
        assert!(!o.meets_target);
        assert!(!o.meets_10ms());
    }

    #[test]
    fn status_reports_all_backends() {
        let s = pq_status();
        assert!(s.stark && s.lattice && s.recursive && s.stir);
        assert_eq!(s.target_ms, TEN_MS);
    }
}
