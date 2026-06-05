//! flux-nations — identity, citizenship, and quorum attestation for SIGIL Nation.
//!
//! - [`Identity`] — a citizen/state-actor: a public key + a human alias (we
//!   prefer aliases over hashes for humans, ids over aliases for the protocol).
//! - **Attestation** — a citizen is only real if a *quorum* of attestors signed
//!   "identity X runs on a machine measured as quote Q" ([`verify_attestation`]),
//!   reusing [`flux_quorum`] so no single authority forges a citizen. The quote
//!   is the hardware/agent measurement (TPM/SGX, or the fluxc `.proof`) — the
//!   last trust gap SIGIL Nation closes.
//! - [`Nation`] — commits its citizen set in a merkle root anyone verifies in
//!   ~10 ms, the same shape as a chain state root.
#![warn(missing_docs)]
pub mod membership;
pub mod nations;
pub mod notify;
pub use membership::{prove_membership, verify_membership, MembershipProof};
pub use notify::notify_admitted;
pub use nations::{
    attestation_msg, citizen_root, verify_attestation, Identity, Nation,
};
// re-export the quorum surface attestors use.
pub use flux_quorum::{Blake3MacVerifier, QuorumMember, QuorumPolicy, QuorumVerifier, SignedShare};
