//! flux-quorum (S6) — the one M-of-N signature quorum the whole stack shares.
//!
//! This session grew three things that each needed "≥ M of N parties signed
//! the same message," and each was about to roll its own:
//!   - **FLUXBURST VBC** — finalize a build once M workers' proofs agree.
//!   - **SIGIL DNS anchor** (DNS-5) — the `_sigil-tip` checkpoint must be
//!     quorum-signed, not single-key, to be trustless.
//!   - **v0.0.5 R8** — release announcements should be auditor-grade M-of-N,
//!     not one agent's signature.
//!
//! flux-quorum is that primitive, built once. Crucially it is **scheme-agnostic**
//! (crypto agility — the quorum logic must never hardcode SQIsign): the
//! signature scheme is injected through [`QuorumVerifier`]. The default real
//! scheme is SQIsign-L5 (behind the `sqisign` feature); a BLAKE3-MAC stand-in
//! ([`Blake3MacVerifier`]) makes the logic trivially testable.

#![warn(missing_docs)]

pub mod quorum;
pub mod debate;
#[cfg(feature = "sqisign")]
pub mod sqisign;
#[cfg(feature = "sqisign")]
pub use sqisign::SqiSignVerifier;

pub use debate::{run_debate, Audit, DebateOutcome, Proposal, Verdict};
pub use quorum::{
    verify_quorum, Blake3MacVerifier, QuorumMember, QuorumOutcome, QuorumPolicy, QuorumReport,
    QuorumVerifier, RejectReason, SignedShare,
};
