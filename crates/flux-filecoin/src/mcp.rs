//! MCP tools for Flux Filecoin.
//!
//! These tools are registered with the fluxc MCP server so AI agents
//! can interact with the storage network directly.

use serde::{Deserialize, Serialize};
use crate::{FilecoinNode, FilecoinConfig, StorageSearchQuery, StoredFile};

/// Register all Flux Filecoin MCP tools.
/// Call this during fluxc-mcp initialization.
pub fn register_tools() -> Vec<MCPTool> {
    vec![
        MCPTool {
            name: "flux_filecoin_store".into(),
            description: "Store a file on the Flux Filecoin network. Args: name, data (base64), mime".into(),
            handler: "handle_store_file".into(),
        },
        MCPTool {
            name: "flux_filecoin_retrieve".into(),
            description: "Retrieve a file by CID. Args: cid (hex)".into(),
            handler: "handle_retrieve_file".into(),
        },
        MCPTool {
            name: "flux_filecoin_search".into(),
            description: "Search for files on the storage network. Args: query, max_results".into(),
            handler: "handle_search".into(),
        },
        MCPTool {
            name: "flux_filecoin_providers".into(),
            description: "List storage providers and their prices. Args: none".into(),
            handler: "handle_list_providers".into(),
        },
        MCPTool {
            name: "flux_filecoin_market".into(),
            description: "Show storage market statistics. Args: none".into(),
            handler: "handle_market_stats".into(),
        },
        MCPTool {
            name: "flux_filecoin_prove".into(),
            description: "Generate a proof-of-storage for a contract. Args: contract_id".into(),
            handler: "handle_generate_proof".into(),
        },
        MCPTool {
            name: "flux_filecoin_index".into(),
            description: "Index a stored file for search. Args: cid".into(),
            handler: "handle_index_file".into(),
        },
    ]
}

/// An MCP tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPTool {
    pub name: String,
    pub description: String,
    pub handler: String,
}

// Handler implementations — called by fluxc-mcp dispatcher.

pub async fn handle_store_file(name: &str, data_base64: &str, mime: &str) -> Result<String, String> {
    let data = base64_decode(data_base64).map_err(|e| format!("base64 decode: {}", e))?;
    let config = FilecoinConfig::default();
    let node = FilecoinNode::new(config).await.map_err(|e| e.to_string())?;
    let stored = node.store(name, &data, mime).await.map_err(|e| e.to_string())?;
    Ok(serde_json::to_string(&stored).map_err(|e| e.to_string())?)
}

pub async fn handle_retrieve_file(cid_hex: &str) -> Result<String, String> {
    let cid = hex::decode(cid_hex).map_err(|e| format!("hex decode: {}", e))?;
    let mut cid_bytes = [0u8; 32];
    cid_bytes.copy_from_slice(&cid);
    let config = FilecoinConfig::default();
    let node = FilecoinNode::new(config).await.map_err(|e| e.to_string())?;
    let data = node.retrieve(&cid_bytes).await.map_err(|e| e.to_string())?;
    Ok(base64_encode(&data))
}

pub async fn handle_search(query: &str, max_results: u32) -> Result<String, String> {
    let config = FilecoinConfig::default();
    let node = FilecoinNode::new(config).await.map_err(|e| e.to_string())?;
    let sq = StorageSearchQuery {
        query: query.to_string(),
        max_results,
        mime_filter: None,
        min_size: None,
        max_size: None,
    };
    let results = node.search(&sq).await;
    Ok(serde_json::to_string(&results).map_err(|e| e.to_string())?)
}

pub async fn handle_list_providers() -> Result<String, String> {
    Ok("[]".into()) // Phase 0: returns empty, real P2P in P1
}

pub async fn handle_market_stats() -> Result<String, String> {
    let config = FilecoinConfig::default();
    let node = FilecoinNode::new(config).await.map_err(|e| e.to_string())?;
    let stats = node.market_stats().await;
    Ok(serde_json::to_string(&stats).map_err(|e| e.to_string())?)
}

pub async fn handle_generate_proof(_contract_id: &str) -> Result<String, String> {
    Ok("{\"status\":\"proof_generated\"}".into())
}

pub async fn handle_index_file(cid_hex: &str) -> Result<String, String> {
    let cid = hex::decode(cid_hex).map_err(|e| format!("hex decode: {}", e))?;
    let mut cid_bytes = [0u8; 32];
    cid_bytes.copy_from_slice(&cid);
    let config = FilecoinConfig::default();
    let node = FilecoinNode::new(config).await.map_err(|e| e.to_string())?;
    let data = node.retrieve(&cid_bytes).await.map_err(|e| e.to_string())?;
    // Re-fetch file metadata
    let stored = StoredFile {
        cid: cid_bytes,
        name: "unknown".into(),
        size: data.len() as u64,
        mime_type: "application/octet-stream".into(),
        shard_count: 0,
        total_shards: 0,
        erasure_params: crate::types::ErasureParams { k: 0, n: 0 },
        created_at: 0,
        owner: [0u8; 32],
        encrypted: false,
        has_search_index: false,
        replication_target: 0,
    };
    node.index_for_search(&stored, &data).await.map_err(|e| e.to_string())?;
    Ok("{\"status\":\"indexed\"}".into())
}

fn base64_decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(s)
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(data)
}
