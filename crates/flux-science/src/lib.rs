// Flux Science — Quantum Gravity, Black Hole Evolution, Cosmological Inflation
//
// Ported from research.md (3042 lines, 153 classes/functions).
// GPU-accelerated via flux-gpu (Vera 8192 CU, NVIDIA CUDA, AMD Radeon).
//
// Modules:
//   constants     — Planck units, fundamental constants
//   relativity    — Schwarzschild metric, Ricci tensor, Einstein-Hilbert action
//   quantum       — Quantum gravity corrections (string theory + LQG)
//   blackhole     — Hawking evaporation with quantum corrections
//   inflation     — Starobinsky cosmological inflation
//   holographic   — Wilson loops, entanglement entropy, AdS/CFT
//
// DAGKnight Integration:
//   Each scientific computation is a DAGKnight vertex. Validators compute
//   Merkle trees over intermediate results, then use zero-message implicit
//   voting to agree on scientific truth. Building Flux building Flux means:
//   compiling the compiler is itself a DAG of compilation units, each
//   unit is a vertex, and the Merkle root of all compiled outputs is the
//   scientific consensus. No explicit voting messages — the DAG structure
//   IS the vote. O(n) complexity, sub-second finality.

pub mod constants;
pub mod relativity;
pub mod quantum;
pub mod blackhole;
pub mod inflation;
pub mod holographic;
pub mod fisher;

// Re-exports
pub use constants::*;
pub use relativity::SchwarzschildMetric;
pub use quantum::QuantumGravityCorrections;
pub use blackhole::BlackHoleEvolution;
pub use inflation::CosmologicalInflation;
pub use holographic::HolographicTheory;
pub use fisher::{fuse, ArrivalModel, FusedEstimate, StaleObservation};

/// How DAGKnight zero-message voting works with Merkle trees in self-hosting:
///
/// When Flux builds Flux (the compiler compiling itself):
///
///   1. Each crate is a DAGKnight vertex at a round
///   2. Each validator compiles its assigned crates
///   3. The compiled output is hashed (BLAKE3 Merkle tree)
///   4. The vertex references 2f+1 vertices from the previous round
///      (these ARE the implicit votes — no separate voting messages!)
///   5. After 2f+1 validators reference a round → commit
///   6. The Merkle root of all compilation outputs is the consensus
///
/// This means:
///   - No prepare/commit phases → O(n) not O(n^2)
///   - The DAG structure itself encodes the voting
///   - Merkle proofs enable SPV verification of any crate's compilation
///   - Scientific computations can be verified the same way

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants_consistency() {
        let c = constants::SPEED_OF_LIGHT;
        let h = constants::PLANCK_REDUCED;
        let g = constants::GRAVITATIONAL;
        let lp = constants::planck_length();
        assert!(lp > 0.0);
        assert!((c - 2.99792458e8).abs() < 1e4);
    }
}
