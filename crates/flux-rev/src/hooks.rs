// flux-rev/src/hooks.rs — Webhook + AI workflow hooks for revision events
//
// Fires webhooks and triggers cortex AI analysis when revisions are created
// or checked out. Enables the "faster iterative coding" loop:
//   edit → fluxc heal → flux-rev snapshot → webhook fires → CI builds → cortex AI reviews
//
// Replaces the manual "did you push?" gate with automatic notification.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

/// Configuration for post-revision hooks.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HooksConfig {
    /// Webhook URLs to POST revision events to.
    #[serde(default)]
    pub webhook_urls: Vec<String>,
    /// HMAC secret for signing webhook payloads.
    #[serde(default)]
    pub webhook_secret: Option<String>,
    /// Run cortex AI analysis on new revisions.
    #[serde(default)]
    pub cortex_ai_on_snapshot: bool,
    /// Run `fluxc heal` on the working tree after checkout.
    #[serde(default)]
    pub heal_on_checkout: bool,
    /// Run `fluxc test-native` after snapshot.
    #[serde(default)]
    pub test_on_snapshot: bool,
    /// Fire `fluxc cortex-ai review` on changed files.
    #[serde(default)]
    pub ai_review_on_snapshot: bool,
}

/// Event payload sent to webhooks on revision events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionEvent {
    pub event: String,             // "snapshot" | "checkout"
    pub revision_id: String,
    pub parent: Option<String>,
    pub author: String,
    pub message: String,
    pub workspace_version: String,
    pub ts_unix: u64,
    pub files_changed: usize,      // count of changed blobs
    pub files_added: Vec<String>,  // new files in this revision
    pub files_removed: Vec<String>,// files removed since parent
}

impl HooksConfig {
    /// Load from ~/.flux/hooks.toml, or return defaults.
    pub fn load() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let path = PathBuf::from(home).join(".flux").join("hooks.toml");
        if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| toml::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            HooksConfig::default()
        }
    }
}

/// Fire all configured hooks for a snapshot event.
pub fn on_snapshot(
    config: &HooksConfig,
    revision_id: &str,
    parent: Option<&str>,
    author: &str,
    message: &str,
    workspace_version: &str,
    ts_unix: u64,
    files_changed: usize,
) {
    let event = RevisionEvent {
        event: "snapshot".into(),
        revision_id: revision_id.into(),
        parent: parent.map(|s| s.to_string()),
        author: author.into(),
        message: message.into(),
        workspace_version: workspace_version.into(),
        ts_unix,
        files_changed,
        files_added: Vec::new(),
        files_removed: Vec::new(),
    };

    // Fire webhooks
    fire_webhooks(config, &event);

    // Trigger AI review
    if config.ai_review_on_snapshot {
        trigger_ai_review(&event);
    }

    // Run tests
    if config.test_on_snapshot {
        trigger_tests();
    }
}

/// Fire hooks for a checkout event.
pub fn on_checkout(config: &HooksConfig, revision_id: &str, author: &str) {
    let event = RevisionEvent {
        event: "checkout".into(),
        revision_id: revision_id.into(),
        parent: None,
        author: author.into(),
        message: String::new(),
        workspace_version: String::new(),
        ts_unix: 0,
        files_changed: 0,
        files_added: Vec::new(),
        files_removed: Vec::new(),
    };

    fire_webhooks(config, &event);

    if config.heal_on_checkout {
        trigger_heal();
    }
}

fn fire_webhooks(config: &HooksConfig, event: &RevisionEvent) {
    let payload = serde_json::to_string(event).unwrap_or_default();

    for url in &config.webhook_urls {
        let mut cmd = Command::new("curl");
        cmd.arg("-s").arg("-X").arg("POST")
            .arg("-H").arg("Content-Type: application/json")
            .arg("-H").arg("X-Flux-Event: flux-rev")
            .arg("-d").arg(&payload)
            .arg("--max-time").arg("5")
            .arg(url);

        let _ = cmd.output(); // Fire-and-forget
    }

    // Also fire via fluxc webhook system if available
    let _ = Command::new("fluxc")
        .args(["webhook-fire", "flux_rev_snapshot", &payload])
        .output();
}

fn trigger_ai_review(event: &RevisionEvent) {
    let _ = Command::new("fluxc")
        .args(["cortex-ai", "review", "--json"])
        .env("FLUX_REV_EVENT", serde_json::to_string(event).unwrap_or_default())
        .output();
}

fn trigger_tests() {
    let _ = Command::new("fluxc")
        .args(["test-native"])
        .output();
}

fn trigger_heal() {
    let _ = Command::new("fluxc")
        .args(["heal", ".", "-n", "3"])
        .output();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hooks_config_default() {
        let config = HooksConfig::default();
        assert!(config.webhook_urls.is_empty());
        assert!(!config.cortex_ai_on_snapshot);
    }

    #[test]
    fn test_revision_event_serialization() {
        let event = RevisionEvent {
            event: "snapshot".into(),
            revision_id: "abc123".into(),
            parent: Some("def456".into()),
            author: "test".into(),
            message: "fix: something".into(),
            workspace_version: "0.26.0".into(),
            ts_unix: 1716825600,
            files_changed: 3,
            files_added: vec!["src/new.rs".into()],
            files_removed: vec!["src/old.rs".into()],
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("snapshot"));
        assert!(json.contains("abc123"));
    }

    #[test]
    fn test_on_snapshot_no_panic_without_webhooks() {
        let config = HooksConfig::default();
        on_snapshot(&config, "abc", None, "test", "msg", "1.0", 0, 0);
        // Should not panic even with no webhooks configured
    }
}
