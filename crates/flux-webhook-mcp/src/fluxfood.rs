//! Fluxfood Auto-Trigger — automatically runs fluxfood iteration when files change.
//!
//! Subscribes to file change events and triggers `flux_iterate` on the affected
//! packages — the "fluxfooding" loop on autopilot.
//!
//! Key improvement: instead of manually calling flux_iterate after every edit,
//! the watcher + dispatcher combo handles it automatically within ~150ms.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

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
pub struct FluxfoodTrigger {
    event_tx: broadcast::Sender<WebhookEvent>,
}

impl FluxfoodTrigger {
    /// Construct a trigger bound to the event bus.
    pub fn new(event_tx: broadcast::Sender<WebhookEvent>) -> Self {
        Self { event_tx }
    }

    /// Run the fluxfood trigger event loop on the bound event bus.
    pub async fn run(&self) {
        Self::run_loop(self.event_tx.clone()).await;
    }

    /// Run the trigger loop.
    pub async fn run_loop(event_tx: broadcast::Sender<WebhookEvent>) {
        let mut rx = event_tx.subscribe();
        // Shared so the per-key 5s expiry task can clear its entry without
        // moving the set out of the loop.
        let recent_triggers: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
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
                    {
                        let mut seen = recent_triggers.lock().unwrap();
                        if seen.contains(&key) {
                            continue; // rate-limited
                        }
                        seen.insert(key.clone());
                    }

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

                    // Clear rate-limit entry after 5 seconds (shared handle).
                    let clear_key = key.clone();
                    let triggers = Arc::clone(&recent_triggers);
                    tokio::spawn(async move {
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                        triggers.lock().unwrap().remove(&clear_key);
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

#[cfg(test)]
mod path_to_package_tests {
    //! fluxfood path→package routing. A misroute fires flux_iterate against the
    //! wrong crate (or none). flux-webhook-mcp's first tests, added alongside the
    //! 2026-06-13 compile-breakage rescue.
    use super::path_to_package;

    #[test]
    fn maps_known_crate_paths() {
        assert_eq!(path_to_package("/x/flux/crates/flux-p2p/src/swarm.rs"), Some("flux-p2p"));
        assert_eq!(path_to_package("/x/flux/crates/flux-aether/src/lib.rs"), Some("flux-aether"));
        assert_eq!(path_to_package("/x/sigil/crates/sigil-state/src/lib.rs"), Some("sigil-state"));
        assert_eq!(path_to_package("/x/sigil/crates/sigil-node/src/main.rs"), Some("sigil-node"));
        assert_eq!(path_to_package("/x/sigil/gui/sigil-wallet/src/App.tsx"), Some("sigil-wallet"));
    }

    #[test]
    fn unknown_source_files_fall_back_to_auto_then_none() {
        assert_eq!(path_to_package("/tmp/scratch.rs"), Some("auto"));
        assert_eq!(path_to_package("/x/some/Cargo.toml"), Some("auto"));
        assert_eq!(path_to_package("/tmp/notes.txt"), None);
        assert_eq!(path_to_package("/var/log/syslog"), None);
    }
}
