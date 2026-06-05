//! flux-zk-types — type-shim crate.
//!
//! Vendored ZK crates originally depended on `q-types` from the Quillon
//! Graph workspace. This crate provides the minimum closure so the ZK
//! stack compiles standalone inside Flux without pulling all of
//! q-types (~22 KLOC of blockchain consensus types we don't need).
//!
//! Type aliases mirror upstream q-types so callers that go through
//! blake3::Hash → [u8;32] etc continue to work without adapters.

use serde::{Deserialize, Serialize};

/// 32-byte node identifier (typically a hash of an Ed25519 public key).
/// Mirrors `q_types::NodeId`.
pub type NodeId = [u8; 32];

/// Validator identifier — alias of NodeId. Mirrors `q_types::ValidatorId`.
pub type ValidatorId = NodeId;

/// 32-byte proposal hash. Mirrors `q_types::ProposalHash`.
pub type ProposalHash = [u8; 32];

/// Consensus vote. Mirrors `q_types::ConsensusVote`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConsensusVote {
    pub epoch: u64,
    pub proposal_hash: ProposalHash,
    pub participated: bool,
}

/// Helper: deterministic ValidatorId from a UTF-8 string label.
/// Replacement for the upstream pattern `ValidatorId::from("alice")`
/// which only compiled when ValidatorId was a newtype struct.
pub fn validator_id_from_label(label: &str) -> ValidatorId {
    let h = blake3::hash(label.as_bytes());
    *h.as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consensus_vote_roundtrip() {
        let v = ConsensusVote {
            epoch: 42,
            proposal_hash: [3u8; 32],
            participated: true,
        };
        let j = serde_json::to_string(&v).unwrap();
        let v2: ConsensusVote = serde_json::from_str(&j).unwrap();
        assert_eq!(v.epoch, v2.epoch);
        assert_eq!(v.proposal_hash, v2.proposal_hash);
        assert_eq!(v.participated, v2.participated);
    }

    #[test]
    fn validator_id_from_label_deterministic() {
        let a = validator_id_from_label("alice");
        let b = validator_id_from_label("alice");
        let c = validator_id_from_label("bob");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
