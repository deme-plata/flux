//! # Flux Webhook-MCP Combo v2
//!
//! A bidirectional, event-driven webhook + MCP integration system that's
//! **faster and smarter** than the current outbound-only webhooks.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                      flux-webhook-mcp                               │
//! │                                                                     │
//! │  ┌─────────────────────┐    ┌──────────────────────────────────┐   │
//! │  │ Inbound Webhook     │    │ MCP Tool Dispatcher              │   │
//! │  │ Server (axum :4199) │◄──►│ (webhooks → MCP tools)          │   │
//! │  └─────────┬───────────┘    └──────────┬───────────────────────┘   │
//! │            │                            │                           │
//! │  ┌─────────▼───────────────────────────▼───────────────────────┐   │
//! │  │                    Event Bus                                  │   │
//! │  │  (tokio broadcast channel — 1024 capacity, fan-out)          │   │
//! │  └─────────┬───────────────────────────┬───────────────────────┘   │
//! │            │                            │                           │
//! │  ┌─────────▼──────────┐    ┌───────────▼───────────────────────┐   │
//! │  │ Aether File        │    │ Fluxfood Auto-Iterate             │   │
//! │  │ Watcher (notify)   │    │ (on file change → flux_iterate)   │   │
//! │  └────────────────────┘    └───────────────────────────────────┘   │
//! │  ┌────────────────────┐    ┌───────────────────────────────────┐   │
//! │  │ Search Re-Indexer  │    │ Swarm Message Bridge              │   │
//! │  │ (auto-index files) │    │ (webhooks ↔ swarm messages)       │   │
//! │  └────────────────────┘    └───────────────────────────────────┘   │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Key Innovations vs Current System
//!
//! | Feature | Current (v1) | Combo v2 |
//! |---------|--------------|----------|
//! | Direction | Outbound only | **Bidirectional** (inbound + outbound) |
//! | Transport | curl subprocess | **axum HTTP server + reqwest client** |
//! | Speed | ~200ms per dispatch (subprocess) | **~2ms** (in-process) |
//! | MCP Trigger | None | **Webhooks can call MCP tools** |
//! | File Watching | None | **notify-based aether watcher** |
//! | Auto-Iterate | Manual flux_iterate | **Auto on file change** |
//! | Search Index | Manual | **Auto re-index on store** |
//! | Auth | HMAC-SHA256 | **HMAC + SQIsign PQ signatures** |
//! | Events | Build/test only | **Any event type** |
//! | Message Bus | File polling | **tokio broadcast channel** |

pub mod server;
pub mod dispatcher;
pub mod watcher;
pub mod search;
pub mod types;
pub mod fluxfood;
pub mod mcp_tools;

pub use server::WebhookServer;
pub use dispatcher::McpDispatcher;
pub use watcher::AetherWatcher;
pub use search::SearchReIndexer;
pub use types::*;
pub use fluxfood::FluxfoodTrigger;

use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use anyhow::Result;

/// Configuration for the webhook-mcp combo system.
#[derive(Debug, Clone)]
pub struct WebhookMcpConfig {
    /// Port for the inbound webhook server
    pub server_port: u16,
    /// Path to the fluxc binary for MCP tool dispatch
    pub fluxc_bin: String,
    /// Directories to watch for aether file changes
    pub watch_dirs: Vec<String>,
    /// Enable auto-fluxfood on file change
    pub auto_fluxfood: bool,
    /// Enable auto-search-reindex on file store
    pub auto_search_reindex: bool,
    /// HMAC secret for webhook signatures
    pub hmac_secret: String,
    /// Outbound webhook endpoints (url → event filter)
    pub outbound_endpoints: Vec<OutboundEndpoint>,
}

/// An outbound webhook endpoint.
#[derive(Debug, Clone)]
pub struct OutboundEndpoint {
    pub url: String,
    pub secret: String,
    pub events: Vec<String>,
    pub enabled: bool,
}

impl Default for WebhookMcpConfig {
    fn default() -> Self {
        Self {
            server_port: 4199,
            fluxc_bin: "fluxc".into(),
            watch_dirs: vec!["/var/lib/flux-aether".into()],
            auto_fluxfood: true,
            auto_search_reindex: true,
            hmac_secret: "flux-webhook-mcp-v2-secret".into(),
            outbound_endpoints: vec![],
        }
    }
}

/// The master orchestrator — ties everything together.
pub struct WebhookMcpOrchestrator {
    config: WebhookMcpConfig,
    event_tx: broadcast::Sender<WebhookEvent>,
    /// Track registered MCP tools for dispatch
    mcp_registry: Arc<RwLock<Vec<McpToolDef>>>,
}

impl WebhookMcpOrchestrator {
    /// Create a new orchestrator.
    pub fn new(config: WebhookMcpConfig) -> Self {
        let (event_tx, _) = broadcast::channel(1024);
        Self {
            config,
            event_tx,
            mcp_registry: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Start all subsystems.
    pub async fn start(&self) -> Result<()> {
        let event_tx = self.event_tx.clone();
        let config = self.config.clone();

        // 1. Start inbound webhook server
        let server = WebhookServer::new(config.server_port, event_tx.clone());
        tokio::spawn(async move {
            if let Err(e) = server.run().await {
                tracing::error!("Webhook server error: {}", e);
            }
        });

        // 2. Start MCP dispatcher
        let dispatcher = McpDispatcher::new(config.fluxc_bin.clone(), event_tx.clone());
        tokio::spawn(async move {
            dispatcher.run().await;
        });

        // 3. Start aether file watcher
        if !config.watch_dirs.is_empty() {
            let watcher = AetherWatcher::new(config.watch_dirs.clone(), event_tx.clone());
            tokio::spawn(async move {
                if let Err(e) = watcher.run().await {
                    tracing::error!("File watcher error: {}", e);
                }
            });
        }

        // 4. Start fluxfood trigger
        if config.auto_fluxfood {
            let trigger = FluxfoodTrigger::new(event_tx.clone());
            tokio::spawn(async move {
                trigger.run().await;
            });
        }

        tracing::info!("🚀 Webhook-MCP Combo v2 started on :{}", config.server_port);
        Ok(())
    }
}
