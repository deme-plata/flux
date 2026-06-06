//! FilecoinNode — the top-level orchestrator.
//!
//! Ties together storage, market, proof, and search subsystems.
//! This is what users interact with.

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::market::StorageMarket;
use crate::proof::ProofOfStorage;
use crate::search::SearchNetwork;
use crate::storage::{StorageProvider, StorageStats};
use crate::market::MarketPrice;
use crate::search::SearchStats;
use crate::types::*;
use crate::FilecoinConfig;

/// The main Flux Filecoin node — orchestrates all subsystems.
pub struct FilecoinNode {
    config: FilecoinConfig,
    storage: Arc<StorageProvider>,
    market: Arc<StorageMarket>,
    proof: Arc<ProofOfStorage>,
    search: Arc<SearchNetwork>,
}

impl FilecoinNode {
    /// Create a new Filecoin node from config.
    pub async fn new(config: FilecoinConfig) -> Result<Self> {
        let storage = Arc::new(StorageProvider::new(config.clone())?);
        let market = Arc::new(StorageMarket::new());
        let proof = Arc::new(ProofOfStorage::new(config.identity_seed.unwrap_or([0u8; 32]).to_vec()));
        let search = Arc::new(SearchNetwork::new(config.enable_search));

        info!("✅ Flux Filecoin node initialized");
        info!("   Storage: {} bytes capacity", config.storage_capacity);
        info!("   Search: {}", if config.enable_search { "enabled" } else { "disabled" });

        Ok(Self {
            config,
            storage,
            market,
            proof,
            search,
        })
    }

    /// Store a file locally.
    pub async fn store(&self, name: &str, data: &[u8], mime: &str) -> Result<StoredFile> {
        self.storage.store_file(name, data, mime).await
    }

    /// Retrieve a file by CID.
    pub async fn retrieve(&self, cid: &[u8; 32]) -> Result<Vec<u8>> {
        self.storage.retrieve_file(cid).await
    }

    /// Index a stored file for search.
    pub async fn index_for_search(&self, file: &StoredFile, content: &[u8]) -> Result<()> {
        self.search.index_file(file, content).await
    }

    /// Search the storage network.
    pub async fn search(&self, query: &StorageSearchQuery) -> Vec<StorageSearchResult> {
        self.search.search_network(query).await
    }

    /// Announce our storage to the P2P network.
    pub async fn announce(&self) -> Result<()> {
        let announcement = ProviderAnnouncement {
            node_id: self.config.node_id.clone(),
            address: self.config.identity_seed.unwrap_or([0u8; 32]),
            available_capacity: self.storage.available_capacity().await,
            used_capacity: 0,
            price_per_gb_month: self.config.price_per_gb_month,
            min_duration: self.config.min_contract_duration,
            features: vec!["flux-aether".into(), "flux-search".into(), "pq-crypto".into()],
            multiaddrs: vec![],
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            signature: vec![],
        };

        self.market.register_provider(announcement.clone()).await;
        self.market.set_our_announcement(announcement).await;
        Ok(())
    }

    /// Start the proof-of-storage cycle (periodic proving).
    pub async fn start_proof_cycle(&self) -> Result<()> {
        let storage = self.storage.clone();
        let proof = self.proof.clone();
        let market = self.market.clone();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
                // Check each contract for proof deadlines
                let contracts = storage.active_contracts().await;
                for contract in contracts {
                    if contract.is_proof_due() {
                        if let Ok(file) = storage.retrieve_file(&contract.file_cid).await {
                            let shards = vec![file]; // simplified
                            if let Ok(p) = proof.generate_proof(
                                &contract.id, &contract.file_cid, &shards
                            ).await {
                                info!("🔐 Proof generated for contract {}", hex::encode(&contract.id));
                            }
                        }
                    }
                }
            }
        });

        Ok(())
    }

    /// Get storage stats.
    pub async fn storage_stats(&self) -> StorageStats {
        self.storage.usage_stats().await
    }

    /// Get market stats.
    pub async fn market_stats(&self) -> MarketPrice {
        self.market.market_stats().await
    }

    /// Get search stats.
    pub fn search_stats(&self) -> SearchStats {
        self.search.stats()
    }
}
