//! FLUXBURST — the self-paying, trustless build mesh.
//!
//! `fluxc build` auto-bursts compilation onto rented boxes over flux-p2p. The
//! danger of compiling on stranger hardware is that a malicious worker injects
//! a backdoor. FLUXBURST removes the need to trust any single worker via
//! **Verifiable Build Consensus (VBC)** — the primitive in [`vbc`].
//!
//! This is SIGIL's safety property, ported from state to compilation:
//!   - SIGIL: *state divergence is impossible to hide* (4 roots, exit-78).
//!   - FLUXBURST: *build divergence is impossible to hide* — N independent
//!     workers compile the same unit; if their artifact hashes disagree, one
//!     tampered, and the quorum exposes it.
//!
//! BURST-3 (this crate's first deliverable) is the **verifier core**: pure,
//! deterministic, dependency-light (blake3 + serde). The flux-p2p worker
//! transport (BURST-2), the rent→distribute→reap combo (BURST-4), and the
//! chronos byzantine-injection sim (BURST-5) compose around it.

#![warn(missing_docs)]

pub mod mesh;
pub mod vbc;

pub use vbc::{verify_consensus, BuildClaim, Byzantine, VbcOutcome, VbcReject, VbcReport};
