//! Fluxfood Auto-Trigger — automatically runs fluxfood iteration when files change.
//!
//! Subscribes to file change events and triggers `flux_iterate` on the affected
//! packages — the "fluxfooding" loop on autopilot.
//!
//! Key improvement: instead of manually calling flux_iterate after every edit,
//! the watcher + dispatcher combo handles it automatically within ~150ms.

use std::collections::HashSet;

use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::types::*;

/// Maps file path patterns to package names for flux_iterate.
fn path_to_package(path: &str) -> Option<&'static str> {
    if path.contains("flux/crates/flux-aether") {
        Some("flux-aether")
    } else if path.contains("flux/crates/flux-search") {
        Some("flux-search")
    } else if path.contains("flux/crates/flux-p2p") {
        Some("flux-p2p")
    } else if path.contains("flux/crates/flux-filecoin") {
        Some("flux-filecoin")
    } else if path.contains("flux/crates/flux-webhook-mcp") {
        Some("flux-webhook-mcp")
    } else if path.contains("flux/crates/fluxc-mcp") {
        Some("fluxc-mcp")
    } else if path.contains("flux/crates/flux-sqisign") {
        Some("flux-sqisign")
    } else if path.contains("sigil/crates/sigil-state") {
        Some("sigil-state")
    } else if path.contains("sigil/crates/sigil-top") {
        Some("sigil-top")
    } else if path.contains("sigil/crates/sigil-node") {
        Some("sigil-node")
    } else if path.contains("sigil/gui/sigil-wallet") {
        Some("sigil-wallet")
    } else if path.ends_with(".rs") || path.ends_with(".toml") {
        Some("auto") // generic check
    } else {
        None
    }
}

/// The fluxfood auto-trigger.
pub struct FluxfoodTrigger;

impl FluxfoodTrigger {
    /// Run the fluxfood trigger event loop.
    pub async fn run(&self) {
        let mut guard = crate::search::SearchReIndexer::init;
        info!("🍽️ Fluxfood auto-trigger running");
    }

    /// Run the trigger loop.
    pub async fn run_loop(event_tx: broadcast::Sender<WebhookEvent>) {
        let mut rx = event_tx.subscribe();
        let mut recent_triggers: HashSet<String> = HashSet::new();
        info!("🍽️ Fluxfood auto-trigger running");

        loop {
            match rx.recv().await {
                Ok(event) => {
                    // Only process fluxfood triggers
                    if !event.trigger_fluxfood {
                        continue;
                    }

                    // Rate-limit: only trigger once per package per 5 seconds
                    let pkg = event.payload.get("triggered_by")
                        .and_then(|v| v.as_str())
                        .unwrap_or("auto");

                    let key = format!("{}:{}", pkg, event.event_type);
                    if recent_triggers.contains(&key) {
                        continue; // rate-limited
                    }
                    recent_triggers.insert(key.clone());

                    // Determine which package to iterate
                    let target_pkg = if event.event_type == crate::types::event_types::FILE_EDITED {
                        // Try to extract package from file path
                        event.payload.as_array()
                            .and_then(|arr| arr.first())
                            .and_then(|v| v.get("path"))
                            .and_then(|v| v.as_str())
                            .and_then(path_to_package)
                            .unwrap_or("auto")
                    } else {
                        "auto"
                    };

                    info!("🍽️ Fluxfood trigger: {} → flux_iterate --package {}", pkg, target_pkg);

                    // Dispatch flux_iterate MCP tool
                    let iterate_event = WebhookEvent::new(
                        crate::types::event_types::MCP_TOOL_CALLED,
                        "fluxfood",
                        serde_json::json!({
                            "package": target_pkg,
                        }),
                    )
                    .with_mcp_tool("flux_iterate");

                    if event_tx.send(iterate_event).is_err() {
                        warn!("Fluxfood iterate dispatch lost");
                    }

                    // Clear rate-limit entry after 5 seconds
                    let clear_key = key.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                        let _ = recent_triggers.remove(&clear_key);
                    });
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("Fluxfood trigger lagged by {}", n);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }
}
