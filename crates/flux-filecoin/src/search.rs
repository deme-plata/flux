//! Search Network — distributed search across the storage network.

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::info;

use crate::types::*;
use flux_search::{Document, SearchEngine, SearchQuery, SearchResponse};

/// A file entry in the search index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileIndexEntry {
    pub cid: [u8; 32],
    pub name: String,
    pub mime_type: String,
    pub size: u64,
    pub owner: [u8; 32],
}

/// Search statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchStats {
    pub indexed_files: u64,
    pub enabled: bool,
}

/// The search network.
pub struct SearchNetwork {
    engine: Mutex<SearchEngine>,
    file_index: Mutex<HashMap<[u8; 32], FileIndexEntry>>,
    enabled: bool,
}

impl SearchNetwork {
    pub fn new(enabled: bool) -> Self {
        Self {
            engine: Mutex::new(SearchEngine::new()),
            file_index: Mutex::new(HashMap::new()),
            enabled,
        }
    }

    pub async fn index_file(&self, file: &StoredFile, content: &[u8]) -> Result<()> {
        if !self.enabled { return Ok(()); }

        let text = match file.mime_type.as_str() {
            "text/plain" | "text/html" | "text/markdown" | "application/json" => {
                String::from_utf8_lossy(content).to_string()
            }
            _ => file.name.clone(),
        };

        let cid_hex = hex::encode(&file.cid);
        let doc = Document {
            id: cid_hex.clone(),
            url: format!("filecoin://{}", cid_hex),
            title: file.name.clone(),
            content: text,
            meta_description: Some(format!("Flux Filecoin storage: {} bytes", file.size)),
            language: None,
            category: None,
            page_rank: 1.0,
            readability_score: 0.5,
            word_count: 0,
            last_crawled: Some(file.created_at),
            content_hash: cid_hex.clone(),
        };

        let mut engine = self.engine.lock().await;
        engine.index_document(doc);
        self.file_index.lock().await.insert(file.cid, FileIndexEntry {
            cid: file.cid,
            name: file.name.clone(),
            mime_type: file.mime_type.clone(),
            size: file.size,
            owner: file.owner,
        });

        info!("📑 Indexed: {}", file.name);
        Ok(())
    }

    pub async fn search(&self, query_text: &str, max_results: u32) -> Vec<StorageSearchResult> {
        if !self.enabled || query_text.is_empty() {
            return vec![];
        }

        let query = SearchQuery {
            q: query_text.to_string(),
            page: 1,
            per_page: max_results as usize,
            category: None,
            language: None,
        };

        let mut engine = self.engine.lock().await;
        let response: SearchResponse = engine.search(query);

        let file_index = self.file_index.lock().await;
        response
            .results
            .into_iter()
            .filter_map(|r| {
                let cid_str = r.url.strip_prefix("filecoin://")?;
                let cid_bytes = hex::decode(cid_str).ok()?;
                let mut cid = [0u8; 32];
                if cid_bytes.len() != 32 { return None; }
                cid.copy_from_slice(&cid_bytes);

                let entry = file_index.get(&cid)?;
                Some(StorageSearchResult {
                    cid,
                    name: entry.name.clone(),
                    size: entry.size,
                    mime_type: entry.mime_type.clone(),
                    snippet: r.snippet,
                    score: r.score,
                    provider_count: 1,
                    estimated_price: (entry.size as f64 / 1_073_741_824.0 * 100_000.0) as u128,
                })
            })
            .collect()
    }

    pub async fn search_network(&self, query: &StorageSearchQuery) -> Vec<StorageSearchResult> {
        self.search(&query.query, query.max_results).await
    }

    pub fn stats(&self) -> SearchStats {
        SearchStats { enabled: self.enabled, indexed_files: 0 }
    }
}
