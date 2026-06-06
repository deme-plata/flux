//! Core types for Flux Filecoin.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// A content-addressed file stored on the network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredFile {
    /// BLAKE3 content hash (the CID)
    pub cid: [u8; 32],
    /// Original filename
    pub name: String,
    /// File size in bytes
    pub size: u64,
    /// MIME type
    pub mime_type: String,
    /// Number of shards (K for Reed-Solomon)
    pub shard_count: u32,
    /// Total shards including parity (N)
    pub total_shards: u32,
    /// Erasure coding parameters
    pub erasure_params: ErasureParams,
    /// When the file was stored
    pub created_at: u64,
    /// File owner's PQ address
    pub owner: [u8; 32],
    /// Whether the file is encrypted
    pub encrypted: bool,
    /// Optional search index attached
    pub has_search_index: bool,
    /// Replication factor target
    pub replication_target: u32,
}

/// Erasure coding parameters (K-of-N).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErasureParams {
    /// Data shards needed to reconstruct
    pub k: u32,
    /// Total shards (data + parity)
    pub n: u32,
}

/// A storage contract between a client and a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageContract {
    /// Unique contract ID
    pub id: [u8; 32],
    /// The file CID being stored
    pub file_cid: [u8; 32],
    /// Storage provider's PQ address
    pub provider: [u8; 32],
    /// Client's PQ address
    pub client: [u8; 32],
    /// Contract start time (unix seconds)
    pub start_time: u64,
    /// Contract duration in seconds
    pub duration: u64,
    /// Price per second in SIGIL base units
    pub price_per_second: u128,
    /// Total collateral from provider (slashed if proven missing)
    pub provider_collateral: u128,
    /// Total payment from client
    pub client_payment: u128,
    /// Contract status
    pub status: ContractStatus,
    /// Proof interval in seconds (how often to prove storage)
    pub proof_interval: u64,
    /// Last successful proof timestamp
    pub last_proof_time: u64,
    /// Number of missed proofs before penalty
    pub missed_proofs: u32,
}

/// Status of a storage contract.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ContractStatus {
    /// Contract proposed, awaiting acceptance
    Pending,
    /// Contract active, provider storing data
    Active,
    /// Contract completed successfully
    Completed,
    /// Provider failed proof — collateral slashed
    Slashed,
    /// Contract terminated early
    Terminated,
    /// Contract disputed
    Disputed,
}

/// A proof that a provider is still storing a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageProof {
    /// Contract ID this proves
    pub contract_id: [u8; 32],
    /// File CID
    pub file_cid: [u8; 32],
    /// Provider address
    pub provider: [u8; 32],
    /// Timestamp of proof
    pub timestamp: u64,
    /// Merkle root of stored shards (from flux-aether)
    pub merkle_root: [u8; 32],
    /// Challenge index
    pub challenge_index: u64,
    /// Challenge response (opened shard hash)
    pub response: [u8; 32],
    /// SQIsign signature by provider
    pub signature: Vec<u8>,
}

/// A storage provider's announcement on the P2P network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderAnnouncement {
    /// Provider's node ID
    pub node_id: String,
    /// Provider's PQ address
    pub address: [u8; 32],
    /// Available capacity in bytes
    pub available_capacity: u64,
    /// Used capacity in bytes
    pub used_capacity: u64,
    /// Price per GB/month
    pub price_per_gb_month: u128,
    /// Minimum contract duration (seconds)
    pub min_duration: u64,
    /// Supported features
    pub features: Vec<String>,
    /// Connection info
    pub multiaddrs: Vec<String>,
    /// Timestamp
    pub timestamp: u64,
    /// SQIsign signature
    pub signature: Vec<u8>,
}

/// A search query across the storage network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSearchQuery {
    /// Search text
    pub query: String,
    /// Maximum results
    pub max_results: u32,
    /// File type filter (optional)
    pub mime_filter: Option<String>,
    /// Minimum file size (optional)
    pub min_size: Option<u64>,
    /// Maximum file size (optional)
    pub max_size: Option<u64>,
}

/// A search result from the network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSearchResult {
    /// File CID
    pub cid: [u8; 32],
    /// File name
    pub name: String,
    /// File size
    pub size: u64,
    /// MIME type
    pub mime_type: String,
    /// Snippet of matching content
    pub snippet: String,
    /// TF-IDF relevance score
    pub score: f64,
    /// Number of providers storing this file
    pub provider_count: u32,
    /// Price estimate for retrieval
    pub estimated_price: u128,
}

impl StoredFile {
    /// Get the content ID as a hex string.
    pub fn cid_hex(&self) -> String {
        hex::encode(&self.cid)
    }
}

impl StorageContract {
    /// Check if the contract is expired.
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        now > self.start_time + self.duration
    }

    /// Check if a proof is due now.
    pub fn is_proof_due(&self) -> bool {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        now >= self.last_proof_time + self.proof_interval
    }

    /// Calculate how much the provider has earned so far.
    pub fn earned_so_far(&self) -> u128 {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let elapsed = now.saturating_sub(self.start_time);
        let duration = std::cmp::min(elapsed, self.duration);
        self.price_per_second.saturating_mul(duration as u128)
    }
}

impl ProviderAnnouncement {
    /// Calculate used percentage.
    pub fn usage_pct(&self) -> f64 {
        if self.available_capacity == 0 {
            return 0.0;
        }
        self.used_capacity as f64 / self.available_capacity as f64 * 100.0
    }
}
