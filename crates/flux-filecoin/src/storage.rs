//! Storage provider — manages local disk space for the network.
//!
//! Wraps flux-aether for file sharding/storage and flux-db for the local index.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::types::*;
use flux_aether::{FileBlock, Shard};

/// Storage usage statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStats {
    pub total_capacity: u64,
    pub used_bytes: u64,
    pub file_count: u64,
    pub contract_count: u64,
}

/// Local storage provider — manages disk-backed file storage.
pub struct StorageProvider {
    config: crate::FilecoinConfig,
    files: RwLock<HashMap<[u8; 32], StoredFile>>,
    manifests: RwLock<HashMap<[u8; 32], FileBlock>>,
    contracts: RwLock<Vec<StorageContract>>,
    bytes_used: RwLock<u64>,
    data_dir: PathBuf,
}

impl StorageProvider {
    pub fn new(config: crate::FilecoinConfig) -> Result<Self> {
        let data_dir = PathBuf::from(&config.data_dir);
        std::fs::create_dir_all(&data_dir)?;
        std::fs::create_dir_all(data_dir.join("shards"))?;
        std::fs::create_dir_all(data_dir.join("manifests"))?;
        Ok(Self {
            bytes_used: RwLock::new(0),
            files: RwLock::new(HashMap::new()),
            manifests: RwLock::new(HashMap::new()),
            contracts: RwLock::new(Vec::new()),
            data_dir,
            config,
        })
    }

    /// Store a file using flux-aether sharding.
    pub async fn store_file(&self, name: &str, data: &[u8], mime: &str) -> Result<StoredFile> {
        let cid = blake3_hash(data);
        let size = data.len() as u64;
        let producer = [0u8; 32];
        let key = b"flux-filecoin-v1-key-0000000";

        // Shard via flux-aether
        let (file_block, shards) = flux_aether::shard_file(data, 65536, key, producer);

        // Save shards
        let shard_dir = self.data_dir.join("shards").join(hex::encode(&cid));
        std::fs::create_dir_all(&shard_dir)?;
        for shard in &shards {
            let path = shard_dir.join(format!("shard_{}", shard.index));
            tokio::fs::write(&path, &shard.bytes).await?;
        }

        // Save FileBlock manifest
        let manifest_path = self.data_dir.join("manifests").join(format!("{}.json", hex::encode(&cid)));
        let manifest_json = serde_json::to_string(&file_block)?;
        tokio::fs::write(&manifest_path, &manifest_json).await?;

        let stored = StoredFile {
            cid,
            name: name.to_string(),
            size,
            mime_type: mime.to_string(),
            shard_count: file_block.k,
            total_shards: file_block.n,
            erasure_params: ErasureParams { k: file_block.k, n: file_block.n },
            created_at: now_secs(),
            owner: [0u8; 32],
            encrypted: true,
            has_search_index: false,
            replication_target: 3,
        };

        {
            let mut bytes = self.bytes_used.write().await;
            *bytes += size;
        }
        self.files.write().await.insert(cid, stored.clone());
        self.manifests.write().await.insert(cid, file_block);

        info!("📦 Stored: {} ({} bytes, CID: {})", name, size, hex::encode(&cid));
        Ok(stored)
    }

    /// Retrieve a file by CID via flux-aether reassemble.
    pub async fn retrieve_file(&self, cid: &[u8; 32]) -> Result<Vec<u8>> {
        let shard_dir = self.data_dir.join("shards").join(hex::encode(cid));
        let key = b"flux-filecoin-v1-key-0000000";

        // Load manifest
        let manifest_path = self.data_dir.join("manifests").join(format!("{}.json", hex::encode(cid)));
        let manifest_json = tokio::fs::read_to_string(&manifest_path).await?;
        let file_block: FileBlock = serde_json::from_str(&manifest_json)?;

        // Load shards
        let mut shards = Vec::new();
        for i in 0..file_block.n {
            let path = shard_dir.join(format!("shard_{}", i));
            if !path.exists() { continue; }
            let bytes = tokio::fs::read(&path).await?;
            shards.push(flux_aether::Shard {
                index: i,
                is_parity: i >= file_block.k,
                cid: blake3_hash(&bytes),
                bytes,
            });
        }

        // Reassemble
        match flux_aether::reassemble(&file_block, &shards, key) {
            Ok(data) => {
                info!("📤 Retrieved: {} ({} bytes)", hex::encode(cid), data.len());
                Ok(data)
            }
            Err(e) => anyhow::bail!("Reassembly failed: {:?}", e),
        }
    }

    pub async fn available_capacity(&self) -> u64 {
        self.config.storage_capacity.saturating_sub(*self.bytes_used.read().await)
    }

    pub async fn list_files(&self) -> Vec<StoredFile> {
        self.files.read().await.values().cloned().collect()
    }

    pub async fn usage_stats(&self) -> StorageStats {
        StorageStats {
            total_capacity: self.config.storage_capacity,
            used_bytes: *self.bytes_used.read().await,
            file_count: self.files.read().await.len() as u64,
            contract_count: self.contracts.read().await.len() as u64,
        }
    }

    pub async fn active_contracts(&self) -> Vec<StorageContract> {
        self.contracts.read().await.clone()
    }
}

fn blake3_hash(data: &[u8]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(data);
    *h.finalize().as_bytes()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}
