//! # Flux Filecoin — Decentralized Storage on Flux
//!
//! A complete decentralized storage network built on the Flux substrate:
//!
//! ## Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────┐
//! │                        flux-filecoin                             │
//! │  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐  │
//! │  │ Storage      │  │ Proof-of-    │  │ Search Network       │  │
//! │  │ Marketplace  │  │ Storage      │  │ (flux-search + P2P)  │  │
//! │  └──────┬───────┘  └──────┬───────┘  └──────────┬───────────┘  │
//! │         │                 │                      │              │
//! │  ┌──────▼─────────────────▼──────────────────────▼───────────┐  │
//! │  │                    Core Layer                               │  │
//! │  │  flux-aether (shard/mix) + flux-db (index) + flux-p2p      │  │
//! │  └────────────────────────────────────────────────────────────┘  │
//! │  ┌────────────────────────────────────────────────────────────┐  │
//! │  │                    MCP Tools (65+ tools)                   │  │
//! │  │  flux_filecoin_store · flux_filecoin_retrieve ·            │  │
//! │  │  flux_filecoin_search · flux_filecoin_contract ·           │  │
//! │  │  flux_filecoin_prove · flux_filecoin_market                │  │
//! │  └────────────────────────────────────────────────────────────┘  │
//! └──────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Why this beats Filecoin
//!
//! | Feature | Filecoin | Flux Filecoin |
//! |---------|----------|---------------|
//! | Proof-of-Storage | PoRep (GPU-heavy, ~hours to seal) | Merkle + TimeLock (µs verify) |
//! | Erasure coding | None (full replicas) | K-of-N Reed-Solomon |
//! | Search | None | BLAKE3 TF-IDF + PageRank |
//! | P2P | Kademlia DHT | libp2p gossipsub + DHT |
//! | File identity | CID (multihash) | BLAKE3 content-addressed |
//! | Privacy | Optional | Built-in PIR (Private Information Retrieval) |

pub mod market;
pub mod storage;
pub mod proof;
pub mod search;
pub mod types;
pub mod mcp;
pub mod node;

pub use market::StorageMarket;
pub use storage::StorageProvider;
pub use proof::ProofOfStorage;
pub use search::SearchNetwork;
pub use types::*;
pub use node::FilecoinNode;

use std::sync::Arc;
use tokio::sync::RwLock;
use anyhow::Result;

/// Configuration for a Flux Filecoin node.
#[derive(Debug, Clone)]
pub struct FilecoinConfig {
    /// Node identity (PQ keypair-based)
    pub node_id: String,
    /// Storage capacity to offer (bytes). 0 = no storage offered.
    pub storage_capacity: u64,
    /// Storage price per GB/month in SIGIL base units
    pub price_per_gb_month: u128,
    /// Data directory for stored shards
    pub data_dir: String,
    /// Whether to participate in the search network
    pub enable_search: bool,
    /// Whether to accept storage contracts
    pub accept_contracts: bool,
    /// Minimum contract duration in seconds
    pub min_contract_duration: u64,
    /// Bootstrap peers for P2P
    pub bootstrap_peers: Vec<String>,
    /// PQ identity seed (optional, generates deterministic identity)
    pub identity_seed: Option<[u8; 32]>,
}

impl Default for FilecoinConfig {
    fn default() -> Self {
        Self {
            node_id: format!("ffc-{}", hex::encode(&[rand_byte()])),
            storage_capacity: 10_737_418_240, // 10 GB default
            price_per_gb_month: 100_000,       // 0.001 SIGIL per GB/month
            data_dir: "/var/lib/flux-filecoin".into(),
            enable_search: true,
            accept_contracts: true,
            min_contract_duration: 86400 * 30, // 30 days
            bootstrap_peers: vec![],
            identity_seed: None,
        }
    }
}

fn rand_byte() -> u8 {
    use std::time::{SystemTime, UNIX_EPOCH};
    (SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() % 256) as u8
}
