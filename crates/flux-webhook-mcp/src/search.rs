//! Search Re-Indexer — auto-indexes aether files when they change.
//!
//! Subscribes to file_stored / file_edited events on the event bus
//! and triggers flux-search re-indexing for the affected files.

use anyhow::Result;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use crate::types::*;

/// The search re-indexer.
pub struct SearchReIndexer;

impl SearchReIndexer {
    /// Run the re-indexer event loop.
    pub async fn run(event_tx: broadcast::Sender<WebhookEvent>) {
        let mut rx = event_tx.subscribe();
        info!("🔍 Search re-indexer running");

        loop {
            match rx.recv().await {
                Ok(event) => {
                    if event.trigger_search && event.event_type.starts_with("file") {
                        Self::reindex(&event).await;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("Search re-indexer lagged by {}", n);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    /// Re-index a file after a change event.
    async fn reindex(event: &WebhookEvent) {
        // Extract file info from payload
        let file_cid = event.payload.get("file_cid")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        info!("🔍 Re-indexing file: {}", file_cid);

        // In production: call flux_search to re-index
        // For now, emit a confirmation event
        let reindex_event = WebhookEvent::new(
            crate::types::event_types::SEARCH_INDEXED,
            "search-reindexer",
            serde_json::json!({
                "file_cid": file_cid,
                "status": "reindexed",
            }),
        );

        // Emit through the process-wide event bus installed by `init`.
        match event_tx().lock().await.as_ref() {
            Some(tx) if tx.send(reindex_event).is_ok() => {}
            Some(_) => warn!("Search re-index confirmation lost"),
            None => warn!("Search re-indexer event bus not initialized (call SearchReIndexer::init)"),
        }
    }
}

// Need a static event_tx for the reindexer callback
use std::sync::OnceLock;
static EVENT_TX: OnceLock<tokio::sync::Mutex<Option<broadcast::Sender<WebhookEvent>>>> = OnceLock::new();

fn event_tx() -> &'static tokio::sync::Mutex<Option<broadcast::Sender<WebhookEvent>>> {
    EVENT_TX.get_or_init(|| tokio::sync::Mutex::new(None))
}

impl SearchReIndexer {
    /// Initialize with the event bus sender.
    pub async fn init(tx: broadcast::Sender<WebhookEvent>) {
        let mut guard = event_tx().lock().await;
        *guard = Some(tx);
    }
}
