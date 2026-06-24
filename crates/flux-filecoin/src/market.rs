//! Storage Market — the economic layer.
//!
//! Manages:
//! - Provider announcements (who has space, at what price)
//! - Contract negotiation (client ↔ provider)
//! - Payment channels via flux-bank-core
//! - Pricing based on supply/demand
//!
//! Modeled after flux-gpu-market's fit_gate → score → budget_guard → reap pattern.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::types::*;

/// Storage marketplace — matches clients with providers.
pub struct StorageMarket {
    /// Known providers (by PQ address)
    providers: RwLock<HashMap<[u8; 32], ProviderAnnouncement>>,
    /// Active contracts
    contracts: RwLock<HashMap<[u8; 32], StorageContract>>,
    /// Recent storage prices (for market pricing)
    price_history: RwLock<Vec<MarketPrice>>,
    /// Our own provider announcement (if we're offering storage)
    our_announcement: RwLock<Option<ProviderAnnouncement>>,
}

/// A market price snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketPrice {
    pub timestamp: u64,
    pub avg_price_per_gb_month: f64,
    pub total_available_capacity: u64,
    pub total_used_capacity: u64,
    pub provider_count: u32,
    pub active_contracts: u32,
}

impl StorageMarket {
    /// Create a new storage market.
    pub fn new() -> Self {
        Self {
            providers: RwLock::new(HashMap::new()),
            contracts: RwLock::new(HashMap::new()),
            price_history: RwLock::new(Vec::new()),
            our_announcement: RwLock::new(None),
        }
    }

    /// Register or update a provider announcement.
    pub async fn register_provider(&self, announcement: ProviderAnnouncement) {
        self.providers
            .write()
            .await
            .insert(announcement.address, announcement);
    }

    /// Remove a provider.
    pub async fn remove_provider(&self, address: &[u8; 32]) {
        self.providers.write().await.remove(address);
    }

    /// Find the best provider for a storage need.
    /// Uses a scoring function: lowest price × reputation × capacity.
    pub async fn find_best_provider(
        &self,
        size_bytes: u64,
        duration_secs: u64,
    ) -> Option<ProviderAnnouncement> {
        let providers = self.providers.read().await;
        providers
            .values()
            .filter(|p| {
                p.available_capacity >= size_bytes
                    && p.min_duration <= duration_secs
            })
            .min_by_key(|p| p.price_per_gb_month)
            .cloned()
    }

    /// Create a storage contract between a client and a provider.
    pub async fn create_contract(
        &self,
        file_cid: [u8; 32],
        provider: [u8; 32],
        client: [u8; 32],
        duration: u64,
        file_size: u64,
    ) -> Result<StorageContract> {
        anyhow::ensure!(duration > 0, "Contract duration must be greater than zero");

        let provider_info = self.providers.read().await;
        let provider = provider_info
            .get(&provider)
            .cloned()
            .context("Provider not found")?;

        // Calculate price
        let gb = file_size as f64 / 1_073_741_824.0;
        let months = duration as f64 / 2_592_000.0; // 86400 * 30
        let total_price = (provider.price_per_gb_month as f64 * gb * months) as u128;

        // Generate contract ID
        let contract_id = blake3_hash(&[
            &file_cid,
            provider.address.as_slice(),
            client.as_slice(),
            &duration.to_le_bytes(),
        ].concat());

        let contract = StorageContract {
            id: contract_id,
            file_cid,
            provider: provider.address,
            client,
            start_time: now_secs(),
            duration,
            price_per_second: total_price / u128::from(duration),
            provider_collateral: total_price / 2, // 50% collateral
            client_payment: total_price,
            status: ContractStatus::Pending,
            proof_interval: 86400, // prove every 24h
            last_proof_time: 0,
            missed_proofs: 0,
        };

        self.contracts.write().await.insert(contract_id, contract.clone());
        info!("📋 Created storage contract: {} for {} SIGIL",
            hex::encode(&contract_id), total_price);
        Ok(contract)
    }

    /// Get market statistics.
    pub async fn market_stats(&self) -> MarketPrice {
        let providers = self.providers.read().await;
        let contracts = self.contracts.read().await;

        let provider_count = providers.len() as u32;
        let active_contracts = contracts.values()
            .filter(|c| c.status == ContractStatus::Active)
            .count() as u32;
        let total_available: u64 = providers.values().map(|p| p.available_capacity).sum();
        let total_used: u64 = providers.values().map(|p| p.used_capacity).sum();
        let avg_price = if provider_count > 0 {
            providers.values().map(|p| p.price_per_gb_month as f64).sum::<f64>() / provider_count as f64
        } else {
            0.0
        };

        MarketPrice {
            timestamp: now_secs(),
            avg_price_per_gb_month: avg_price,
            total_available_capacity: total_available,
            total_used_capacity: total_used,
            provider_count,
            active_contracts,
        }
    }

    /// List known providers.
    pub async fn list_providers(&self) -> Vec<ProviderAnnouncement> {
        self.providers.read().await.values().cloned().collect()
    }

    /// List active contracts.
    pub async fn list_contracts(&self) -> Vec<StorageContract> {
        self.contracts.read().await.values().cloned().collect()
    }

    /// Set our own provider announcement (if this node offers storage).
    pub async fn set_our_announcement(&self, announcement: ProviderAnnouncement) {
        let mut our = self.our_announcement.write().await;
        *our = Some(announcement);
    }
}

fn blake3_hash(data: &[u8]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(data);
    *h.finalize().as_bytes()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
