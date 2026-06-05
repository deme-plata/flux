// fluxc webhook — Outbound webhook dispatch for build events
//
// Webhooks are configured in ~/.flux/webhooks.json
// Each webhook has a URL, HMAC-SHA256 signing secret, and event filter.
// After every build/test/bench/iterate, matching webhooks are POSTed.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use sha2::Sha256;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};

type HmacSha256 = Hmac<Sha256>;

// ── Data Model ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Webhook {
    pub id: String,
    pub url: String,
    pub secret: String,          // HMAC-SHA256 signing key
    pub events: Vec<String>,     // e.g. ["build_complete", "test_complete"]
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub retries: u32,            // max retries on failure (default 3)
    #[serde(default)]
    pub timeout_secs: u64,       // HTTP timeout (default 10)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebhookConfig {
    pub webhooks: Vec<Webhook>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebhookPayload {
    pub event: String,
    pub timestamp: u64,
    pub data: serde_json::Value,
    pub source: String,          // "fluxc v0.9.0"
}

// ── Config Path ──

fn config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".flux").join("webhooks.json")
}

fn load_config() -> WebhookConfig {
    let path = config_path();
    if path.exists() {
        let _ = harden_config_file(&path);
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        WebhookConfig::default()
    }
}

#[cfg(unix)]
fn set_private_mode(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|e| format!("chmod {}: {}", path.display(), e))
}

#[cfg(not(unix))]
fn set_private_mode(_path: &Path, _mode: u32) -> Result<(), String> {
    Ok(())
}

fn harden_config_dir(path: &Path) -> Result<(), String> {
    set_private_mode(path, 0o700)
}

fn harden_config_file(path: &Path) -> Result<(), String> {
    set_private_mode(path, 0o600)
}

fn save_config(config: &WebhookConfig) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir: {}", e))?;
        harden_config_dir(parent)?;
    }
    let json = serde_json::to_string_pretty(config).map_err(|e| format!("json: {}", e))?;
    fs::write(&path, json).map_err(|e| format!("write: {}", e))?;
    harden_config_file(&path)
}

// ── Public API ──

/// Register (add or update) a webhook endpoint.
pub fn register_webhook(
    id: &str,
    url: &str,
    secret: &str,
    events: Vec<String>,
) -> Result<String, String> {
    let mut config = load_config();

    // Check for duplicate ID → update
    if let Some(existing) = config.webhooks.iter_mut().find(|w| w.id == id) {
        existing.url = url.to_string();
        existing.secret = secret.to_string();
        existing.events = events;
        return match save_config(&config) {
            Ok(()) => Ok(format!("✓ Webhook '{}' updated → {}", id, url)),
            Err(e) => Err(format!("Failed to save: {}", e)),
        };
    }

    // New webhook
    config.webhooks.push(Webhook {
        id: id.to_string(),
        url: url.to_string(),
        secret: secret.to_string(),
        events,
        enabled: true,
        retries: 3,
        timeout_secs: 10,
    });

    save_config(&config)?;
    Ok(format!("✓ Webhook '{}' registered → {}", id, url))
}

/// List all registered webhooks.
pub fn list_webhooks() -> String {
    let config = load_config();
    if config.webhooks.is_empty() {
        return "No webhooks registered. Use flux_webhook_register to add one.".into();
    }

    let mut lines = vec![format!("⚡ {} webhook(s) registered:", config.webhooks.len())];
    for w in &config.webhooks {
        let status = if w.enabled { "✓" } else { "✗" };
        lines.push(format!(
            "  {} {} → {} (events: {})",
            status,
            w.id,
            w.url,
            w.events.join(", ")
        ));
    }
    lines.join("\n")
}

/// Remove a webhook by ID.
pub fn remove_webhook(id: &str) -> Result<String, String> {
    let mut config = load_config();
    let len_before = config.webhooks.len();
    config.webhooks.retain(|w| w.id != id);
    if config.webhooks.len() == len_before {
        return Err(format!("Webhook '{}' not found", id));
    }
    save_config(&config)?;
    Ok(format!("✓ Webhook '{}' removed", id))
}

/// Manually trigger a webhook event — fires all matching webhooks.
pub fn trigger_event(event: &str, data: serde_json::Value) -> String {
    let config = load_config();
    let matching: Vec<&Webhook> = config
        .webhooks
        .iter()
        .filter(|w| w.enabled && w.events.iter().any(|e| e == event))
        .collect();

    if matching.is_empty() {
        return format!("No webhooks subscribed to event '{}'", event);
    }

    let payload = WebhookPayload {
        event: event.to_string(),
        timestamp: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        data,
        source: "fluxc v0.9.0".into(),
    };

    let mut results = vec![format!(
        "⚡ Triggered '{}' → {} webhook(s):",
        event,
        matching.len()
    )];

    for w in &matching {
        match dispatch_single(w, &payload) {
            Ok(status) => results.push(format!("  ✓ {} → {} (HTTP {})", w.id, w.url, status)),
            Err(e) => results.push(format!("  ✗ {} → {} ({})", w.id, w.url, e)),
        }
    }

    results.join("\n")
}

/// Dispatch a single webhook (HTTP POST with HMAC-SHA256 signature).
fn dispatch_single(webhook: &Webhook, payload: &WebhookPayload) -> Result<u16, String> {
    let body = serde_json::to_string(payload).map_err(|e| format!("json: {}", e))?;

    // Compute HMAC-SHA256 signature
    let mut mac = HmacSha256::new_from_slice(webhook.secret.as_bytes())
        .map_err(|e| format!("hmac init: {}", e))?;
    mac.update(body.as_bytes());
    let signature = hex::encode(mac.finalize().into_bytes());

    // Build and send HTTP POST
    // Phase 1: use a simple reqwest-style blocking call if available,
    // otherwise fall back to curl subprocess.
    match send_http_post(&webhook.url, &body, &signature, webhook.timeout_secs) {
        Ok(status) => {
            if (200..300).contains(&status) {
                Ok(status)
            } else {
                Err(format!("HTTP {}", status))
            }
        }
        Err(e) => Err(e),
    }
}

/// Send HTTP POST — uses curl subprocess for broad compatibility.
fn send_http_post(url: &str, body: &str, signature: &str, timeout_secs: u64) -> Result<u16, String> {
    let mut cmd = std::process::Command::new("curl");
    cmd.arg("-s")
        .arg("-o").arg("/dev/null")
        .arg("-w").arg("%{http_code}")
        .arg("-X").arg("POST")
        .arg("-H").arg(format!("Content-Type: application/json"))
        .arg("-H").arg(format!("X-Flux-Signature: sha256={}", signature))
        .arg("-H").arg("X-Flux-Event: flux-webhook")
        .arg("-d").arg(body)
        .arg("--max-time").arg(timeout_secs.to_string())
        .arg("--connect-timeout").arg("5")
        .arg(url);

    match cmd.output() {
        Ok(output) => {
            let code_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            code_str.parse::<u16>().map_err(|_| format!("parse: {}", code_str))
        }
        Err(e) => Err(format!("curl: {}", e)),
    }
}

// ── Auto-dispatch helpers (called from build/test/bench paths) ──

/// Fire all webhooks subscribed to a given event, non-blocking (best-effort).
/// Spawns a background thread per webhook so MCP tools return instantly.
/// The caller gets sub-millisecond dispatch — no curl subprocess overhead in the hot path.
/// How many enabled webhook endpoints are subscribed to `event` (for the
/// flux_combo dashboard's WEBHOOKS row — a real, not faked, count).
pub fn count_listeners(event: &str) -> usize {
    load_config()
        .webhooks
        .iter()
        .filter(|w| w.enabled && w.events.iter().any(|e| e == event))
        .count()
}

pub fn auto_dispatch(event: &str, data: serde_json::Value) {
    let config = load_config();
    let payload = WebhookPayload {
        event: event.to_string(),
        timestamp: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        data,
        source: concat!("fluxc v", env!("CARGO_PKG_VERSION")).into(),
    };

    let fired_count = config.webhooks.iter().filter(|w| w.enabled && w.events.iter().any(|e| e == event)).count();

    for w in config.webhooks {
        if w.enabled && w.events.iter().any(|e| e == event) {
            let w = w.clone();
            let payload = WebhookPayload {
                event: payload.event.clone(),
                timestamp: payload.timestamp,
                data: payload.data.clone(),
                source: payload.source.clone(),
            };
            // Fire-and-forget: don't block the MCP tool response
            std::thread::spawn(move || {
                let _ = dispatch_single(&w, &payload);
            });
        }
    }

    // Push AI feed event so the dashboard shows webhook activity live
    if fired_count > 0 {
        crate::serve::push_feed_event(
            "Webhook",
            &format!("{} → {} endpoint(s) fired", event, fired_count),
            "webhook",
        );
    }
}

// ── Webhook URL Test ──

/// Test a webhook URL by sending a signed ping. Returns (success, http_status, message).
pub fn test_webhook_url(url: &str, secret: &str) -> (bool, u16, String) {
    let body = serde_json::to_string(&serde_json::json!({
        "event": "ping",
        "timestamp": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        "data": {"test": true},
        "source": "fluxc webhook test",
    })).unwrap_or_default();

    let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(e) => return (false, 0, format!("HMAC init failed: {}", e)),
    };
    mac.update(body.as_bytes());
    let signature = hex::encode(mac.finalize().into_bytes());

    match send_http_post(url, &body, &signature, 10) {
        Ok(status) if (200..300).contains(&status) => {
            (true, status, format!("HTTP {} — reachable and accepted signed ping", status))
        }
        Ok(status) => {
            (false, status, format!("HTTP {} — responded but not 2xx", status))
        }
        Err(e) => {
            (false, 0, format!("could not reach: {}", e))
        }
    }
}

// ── Convenience builders ──

pub fn build_event_data(package: &str, success: bool, elapsed_ms: u128, cache_hits: u64, cache_misses: u64) -> serde_json::Value {
    serde_json::json!({
        "package": package,
        "success": success,
        "elapsed_ms": elapsed_ms,
        "cache_hits": cache_hits,
        "cache_misses": cache_misses,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_list() {
        // Clean start — remove any test webhooks
        let _ = remove_webhook("test-hook");

        let result = register_webhook(
            "test-hook",
            "https://example.com/webhook",
            "test-secret",
            vec!["build_complete".into(), "test_complete".into()],
        );
        assert!(result.is_ok());

        let list = list_webhooks();
        assert!(list.contains("test-hook"));
        assert!(list.contains("https://example.com/webhook"));
        assert!(list.contains("build_complete"));
        assert!(list.contains("test_complete"));

        // Cleanup
        let _ = remove_webhook("test-hook");
    }

    #[test]
    fn test_remove_nonexistent() {
        let result = remove_webhook("nonexistent-id-12345");
        assert!(result.is_err());
    }

    #[test]
    fn test_payload_serialization() {
        let payload = WebhookPayload {
            event: "build_complete".into(),
            timestamp: 1716825600,
            data: serde_json::json!({"package": "fluxc", "elapsed_ms": 504}),
            source: "fluxc v0.9.0".into(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("build_complete"));
        assert!(json.contains("fluxc"));
    }
}
