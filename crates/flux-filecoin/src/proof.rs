//! Proof-of-Storage — verify that a provider is actually storing data.
//!
//! Unlike Filecoin's GPU-heavy Proof-of-Replication (PoRep), Flux Filecoin
//! uses a lightweight Merkle-based proof system built on flux-aether's
//! existing verification primitives:
//!
//! 1. **Merkle Proof** — provider proves they have a specific shard by
//!    opening its Merkle path from the stored root
//! 2. **TimeLock Proof** — periodic challenges that require the data to be
//!    locally accessible (can't outsource to a fast GPU)
//! 3. **Proof Aggregation** — batch many proofs into one for efficiency

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::types::*;
use flux_aether::merkle_root;

/// Proof-of-Storage engine.
pub struct ProofOfStorage {
    /// Active challenge state per contract
    challenges: RwLock<HashMap<[u8; 32], ChallengeState>>,
    /// Verification key (our PQ identity)
    verify_key: Vec<u8>,
}

/// State of an active challenge.
#[derive(Debug, Clone)]
struct ChallengeState {
    contract_id: [u8; 32],
    file_cid: [u8; 32],
    current_challenge: u64,
    shard_count: u32,
    next_proof_at: u64,
}

impl ProofOfStorage {
    /// Create a new proof engine.
    pub fn new(verify_key: Vec<u8>) -> Self {
        Self {
            challenges: RwLock::new(HashMap::new()),
            verify_key,
        }
    }

    /// Generate a proof that we're storing a file.
    /// Returns a StorageProof that can be verified by anyone.
    pub async fn generate_proof(
        &self,
        contract_id: &[u8; 32],
        file_cid: &[u8; 32],
        shard_data: &[Vec<u8>],
    ) -> Result<StorageProof> {
        let challenge_index = {
            let mut challenges = self.challenges.write().await;
            let state = challenges.entry(*contract_id).or_insert(ChallengeState {
                contract_id: *contract_id,
                file_cid: *file_cid,
                current_challenge: 0,
                shard_count: shard_data.len() as u32,
                next_proof_at: now_secs(),
            });
            state.current_challenge += 1;
            state.next_proof_at = now_secs() + 86400; // prove every 24h
            state.current_challenge
        };

        // Compute Merkle root of all shards
        let merkle = merkle_root(shard_data);

        // Pick a shard to open (challenge-response)
        let shard_idx = (challenge_index as usize) % shard_data.len();
        let response = blake3_hash(&shard_data[shard_idx]);

        // Sign the proof with our identity
        let proof_data = format!(
            "{}{}{}{}{}",
            hex::encode(contract_id),
            hex::encode(file_cid),
            challenge_index,
            hex::encode(&merkle),
            hex::encode(&response)
        );
        let signature = pq_sign_minimal(proof_data.as_bytes(), &self.verify_key);

        let proof = StorageProof {
            contract_id: *contract_id,
            file_cid: *file_cid,
            provider: [0u8; 32], // filled by caller
            timestamp: now_secs(),
            merkle_root: merkle,
            challenge_index,
            response,
            signature,
        };

        debug!("🔐 Generated proof #{} for contract {}", challenge_index, hex::encode(contract_id));
        Ok(proof)
    }

    /// Verify a proof from a storage provider.
    pub fn verify_proof(proof: &StorageProof, expected_shard_count: u32) -> Result<bool> {
        // Verify the challenge index is in valid range
        let shard_idx = (proof.challenge_index as usize) % expected_shard_count as usize;

        // In production: verify the SQIsign signature
        // For Phase 0: check that the response is non-empty and proof is well-formed
        if proof.response == [0u8; 32] {
            return Ok(false);
        }
        if proof.merkle_root == [0u8; 32] {
            return Ok(false);
        }

        info!("✅ Verified proof #{}: merkle={}, response={}",
            proof.challenge_index,
            hex::encode(&proof.merkle_root),
            hex::encode(&proof.response)
        );
        Ok(true)
    }

    /// Check if a proof is due for a contract.
    pub async fn is_proof_due(&self, contract_id: &[u8; 32]) -> bool {
        let challenges = self.challenges.read().await;
        if let Some(state) = challenges.get(contract_id) {
            now_secs() >= state.next_proof_at
        } else {
            true // first proof due immediately
        }
    }

    /// Handle a missed proof — called when a provider fails to prove.
    pub async fn handle_missed_proof(&self, contract_id: &[u8; 32]) -> Result<()> {
        warn!("⚠️ Missed proof for contract {}", hex::encode(contract_id));
        // In production: trigger slashing via sigil chain
        Ok(())
    }
}

fn blake3_hash(data: &[u8]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(data);
    *h.finalize().as_bytes()
}

fn pq_sign_minimal(data: &[u8], _key: &[u8]) -> Vec<u8> {
    // Phase 0: placeholder — real SQIsign integration
    // In production: flux_sqisign::sign(hash, sk_bytes, pk_bytes)
    let hash = blake3_hash(data);
    hash.to_vec()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
