// fluxc webhook — Outbound webhook dispatch for build events
//
// Webhooks are configured in ~/.flux/webhooks.json
// Each webhook has a URL, HMAC-SHA256 signing secret, and event filter.
// After every build/test/bench/iterate, matching webhooks are POSTed.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};
use sha2::Sha256;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};

type HmacSha256 = Hmac<Sha256>;

pub use crate::webhook_ssrf::{ssrf_check, ssrf_check_with, is_metadata_ip, is_private_ip};

/// Auto-quarantine an endpoint after this many consecutive delivery failures.
/// Quarantined endpoints are disabled so build-event dispatch stops wasting
/// curl subprocesses on a dead URL — and an AI agent can `flux_webhook_prune`
/// them in one call instead of eyeballing a list of 20+ stale registrations.
const QUARANTINE_THRESHOLD: u32 = 5;

// Serializes read-modify-write on the webhooks.json file. auto_dispatch fires a
// thread per webhook and each records its delivery health back into the same
// file; without this lock those concurrent saves would clobber each other.
static CONFIG_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn config_guard() -> MutexGuard<'static, ()> {
    CONFIG_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

// ── Data Model ──

/// Rolling delivery health for one endpoint. Lets an AI agent reason about
/// which webhooks are actually alive instead of trusting that every registered
/// URL still works.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebhookHealth {
    pub last_status: u16,             // last HTTP status (0 = unreachable)
    pub last_attempt_ts: u64,         // unix secs of last dispatch attempt
    pub last_success_ts: u64,         // unix secs of last 2xx
    pub consecutive_failures: u32,    // reset to 0 on any success
    pub total_delivered: u64,         // lifetime 2xx count
    pub total_failed: u64,            // lifetime non-2xx / unreachable count
    pub quarantined: bool,            // auto-disabled after QUARANTINE_THRESHOLD
}

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
    #[serde(default)]
    pub health: WebhookHealth,   // delivery observability (default = never delivered)
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
        health: WebhookHealth::default(),
    });

    save_config(&config)?;
    Ok(format!("✓ Webhook '{}' registered → {}", id, url))
}

/// Per-endpoint health badge for the list view.
fn health_badge(w: &Webhook) -> String {
    let h = &w.health;
    if h.quarantined {
        return "⛔ quarantined".into();
    }
    if !w.enabled {
        return "✗ disabled".into();
    }
    if h.last_attempt_ts == 0 {
        return "• untested".into();
    }
    if h.consecutive_failures > 0 {
        return format!("⚠ {} fail(s), last HTTP {}", h.consecutive_failures, h.last_status);
    }
    format!("✓ live (HTTP {})", h.last_status)
}

/// List all registered webhooks with delivery-health badges.
pub fn list_webhooks() -> String {
    let config = load_config();
    if config.webhooks.is_empty() {
        return "No webhooks registered. Use flux_webhook_register to add one.".into();
    }

    let live = config.webhooks.iter().filter(|w| w.enabled && !w.health.quarantined).count();
    let quarantined = config.webhooks.iter().filter(|w| w.health.quarantined).count();
    let mut lines = vec![format!(
        "⚡ {} webhook(s) — {} live, {} quarantined:",
        config.webhooks.len(), live, quarantined
    )];
    for w in &config.webhooks {
        lines.push(format!(
            "  [{}] {} → {} (events: {}) — {}✓/{}✗",
            health_badge(w),
            w.id,
            w.url,
            w.events.join(", "),
            w.health.total_delivered,
            w.health.total_failed,
        ));
    }
    if quarantined > 0 {
        lines.push(format!(
            "  → {} dead endpoint(s); call flux_webhook_prune to remove them.",
            quarantined
        ));
    }
    lines.join("\n")
}

/// Structured health report (for `flux_webhook_health`) — machine-readable so an
/// AI agent can decide what to prune/retry without parsing the list text.
pub fn health_report() -> serde_json::Value {
    let config = load_config();
    let endpoints: Vec<serde_json::Value> = config
        .webhooks
        .iter()
        .map(|w| {
            serde_json::json!({
                "id": w.id,
                "url": w.url,
                "enabled": w.enabled,
                "quarantined": w.health.quarantined,
                "last_status": w.health.last_status,
                "last_attempt_ts": w.health.last_attempt_ts,
                "last_success_ts": w.health.last_success_ts,
                "consecutive_failures": w.health.consecutive_failures,
                "total_delivered": w.health.total_delivered,
                "total_failed": w.health.total_failed,
            })
        })
        .collect();
    serde_json::json!({
        "total": config.webhooks.len(),
        "live": config.webhooks.iter().filter(|w| w.enabled && !w.health.quarantined).count(),
        "quarantined": config.webhooks.iter().filter(|w| w.health.quarantined).count(),
        "endpoints": endpoints,
    })
}

/// Enable or disable a webhook by ID. Disabling clears the quarantine flag's
/// auto-management (a manually-disabled hook stays disabled until re-enabled).
pub fn set_webhook_enabled(id: &str, enabled: bool) -> Result<String, String> {
    let _guard = config_guard();
    let mut config = load_config();
    let Some(w) = config.webhooks.iter_mut().find(|w| w.id == id) else {
        return Err(format!("Webhook '{}' not found", id));
    };
    w.enabled = enabled;
    if enabled {
        // Re-enabling clears quarantine + failure streak for a fresh chance.
        w.health.quarantined = false;
        w.health.consecutive_failures = 0;
    }
    save_config(&config)?;
    Ok(format!("✓ Webhook '{}' {}", id, if enabled { "enabled" } else { "disabled" }))
}

/// Remove a webhook by ID.
pub fn remove_webhook(id: &str) -> Result<String, String> {
    let _guard = config_guard();
    let mut config = load_config();
    let len_before = config.webhooks.len();
    config.webhooks.retain(|w| w.id != id);
    if config.webhooks.len() == len_before {
        return Err(format!("Webhook '{}' not found", id));
    }
    save_config(&config)?;
    Ok(format!("✓ Webhook '{}' removed", id))
}

/// Prune dead endpoints: removes every quarantined webhook, plus (when
/// `include_failing` is set) any endpoint with at least one consecutive failure.
/// Returns a human-readable summary listing what was removed.
pub fn prune_webhooks(include_failing: bool) -> String {
    let _guard = config_guard();
    let mut config = load_config();
    let removed: Vec<String> = config
        .webhooks
        .iter()
        .filter(|w| w.health.quarantined || (include_failing && w.health.consecutive_failures > 0))
        .map(|w| format!("{} ({})", w.id, w.url))
        .collect();

    if removed.is_empty() {
        return "Nothing to prune — no quarantined or failing endpoints.".into();
    }

    config
        .webhooks
        .retain(|w| !(w.health.quarantined || (include_failing && w.health.consecutive_failures > 0)));

    if let Err(e) = save_config(&config) {
        return format!("✗ Prune failed to save: {}", e);
    }

    let mut out = vec![format!("🧹 Pruned {} dead endpoint(s):", removed.len())];
    for r in &removed {
        out.push(format!("  - {}", r));
    }
    out.push(format!("  {} webhook(s) remain.", config.webhooks.len()));
    out.join("\n")
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

/// Dispatch a single webhook (HTTP POST with HMAC-SHA256 signature), retrying
/// up to `webhook.retries` extra times with exponential backoff. Records the
/// outcome into the endpoint's persisted health before returning.
fn dispatch_single(webhook: &Webhook, payload: &WebhookPayload) -> Result<u16, String> {
    let body = serde_json::to_string(payload).map_err(|e| format!("json: {}", e))?;

    // Legacy body-only signature (kept so existing receivers keep verifying) plus a
    // replay-resistant v2 signature over "{timestamp}.{body}" — a captured POST can't
    // be replayed with a stale timestamp. The v2 digest also seeds a stable
    // per-delivery id (idempotency key for the receiver).
    let sig_legacy = hmac_hex(&webhook.secret, body.as_bytes())?;
    let sig_v2 = hmac_hex(&webhook.secret, format!("{}.{}", payload.timestamp, body).as_bytes())?;
    let delivery_id: String = sig_v2.chars().take(24).collect();

    // A 0s timeout is never intended (old configs default the field to 0); clamp
    // to a sane 10s. `retries` is honored verbatim — 0 means a single attempt.
    let timeout = if webhook.timeout_secs == 0 { 10 } else { webhook.timeout_secs };
    let attempts = webhook.retries.saturating_add(1);

    let mut last_status: u16 = 0;
    let mut last_err = String::new();
    for attempt in 0..attempts {
        match send_http_post(
            &webhook.url, &body, &sig_legacy, &sig_v2,
            &payload.event, payload.timestamp, &delivery_id, timeout,
        ) {
            Ok(status) => {
                last_status = status;
                if (200..300).contains(&status) {
                    record_delivery(&webhook.id, true, status);
                    return Ok(status);
                }
                last_err = format!("HTTP {}", status);
            }
            Err(e) => {
                last_status = 0;
                last_err = e;
            }
        }
        // Exponential backoff between attempts: 200ms, 400ms, 800ms…
        if attempt + 1 < attempts {
            let backoff_ms = 200u64 << attempt.min(5);
            std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
        }
    }

    record_delivery(&webhook.id, false, last_status);
    Err(last_err)
}

/// Record the outcome of a delivery attempt into the endpoint's persisted
/// health. Auto-quarantines (disables) an endpoint after
/// `QUARANTINE_THRESHOLD` consecutive failures so dead URLs stop being dialed.
/// Serialized via `CONFIG_LOCK` because auto_dispatch records from many threads.
fn record_delivery(id: &str, ok: bool, status: u16) {
    let _guard = config_guard();
    let mut config = load_config();
    let Some(w) = config.webhooks.iter_mut().find(|w| w.id == id) else { return };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    w.health.last_attempt_ts = now;
    w.health.last_status = status;
    if ok {
        w.health.last_success_ts = now;
        w.health.consecutive_failures = 0;
        w.health.total_delivered += 1;
        // A successful delivery lifts quarantine (endpoint came back to life).
        if w.health.quarantined {
            w.health.quarantined = false;
            w.enabled = true;
        }
    } else {
        w.health.consecutive_failures += 1;
        w.health.total_failed += 1;
        if w.health.consecutive_failures >= QUARANTINE_THRESHOLD && !w.health.quarantined {
            w.health.quarantined = true;
            w.enabled = false;
        }
    }
    let _ = save_config(&config);
}

/// Hex HMAC-SHA256 of `msg` under `secret`.
fn hmac_hex(secret: &str, msg: &[u8]) -> Result<String, String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|e| format!("hmac init: {}", e))?;
    mac.update(msg);
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// The signed delivery headers for a webhook POST. Extracted so the set is
/// unit-testable — in particular that `X-Flux-Event` carries the real event name
/// (it used to be hardcoded to the constant "flux-webhook"). Adds, on top of the
/// legacy body-only signature:
///   * `X-Flux-Signature-V2: t=<ts>,v1=<hmac of "{ts}.{body}">` — replay-resistant,
///   * `X-Flux-Timestamp` and `X-Flux-Delivery` (idempotency key) for the receiver.
/// All additive: an existing receiver that only reads `X-Flux-Signature` is unaffected.
fn delivery_headers(
    sig_legacy: &str,
    sig_v2: &str,
    event: &str,
    timestamp: u64,
    delivery_id: &str,
) -> Vec<(String, String)> {
    vec![
        ("Content-Type".into(), "application/json".into()),
        ("X-Flux-Signature".into(), format!("sha256={}", sig_legacy)),
        ("X-Flux-Signature-V2".into(), format!("t={},v1={}", timestamp, sig_v2)),
        ("X-Flux-Event".into(), event.to_string()),
        ("X-Flux-Timestamp".into(), timestamp.to_string()),
        ("X-Flux-Delivery".into(), delivery_id.to_string()),
    ]
}

/// Send HTTP POST — uses curl subprocess for broad compatibility.
#[allow(clippy::too_many_arguments)]
fn send_http_post(
    url: &str,
    body: &str,
    sig_legacy: &str,
    sig_v2: &str,
    event: &str,
    timestamp: u64,
    delivery_id: &str,
    timeout_secs: u64,
) -> Result<u16, String> {
    // SEC-006: re-check at the dispatch chokepoint — catches DNS rebinding and
    // any webhook entry that predates the register-time guard.
    ssrf_check(url)?;
    let mut cmd = std::process::Command::new("curl");
    cmd.arg("-s")
        .arg("-o").arg("/dev/null")
        .arg("-w").arg("%{http_code}")
        .arg("-X").arg("POST");
    for (k, v) in delivery_headers(sig_legacy, sig_v2, event, timestamp, delivery_id) {
        cmd.arg("-H").arg(format!("{}: {}", k, v));
    }
    cmd.arg("-d").arg(body)
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
        fluxc_util::serve_events::push_feed_event(
            "Webhook",
            &format!("{} → {} endpoint(s) fired", event, fired_count),
            "webhook",
        );
    }
}

// ── Webhook URL Test ──

/// Test a webhook URL by sending a signed ping. Returns (success, http_status, message).
pub fn test_webhook_url(url: &str, secret: &str) -> (bool, u16, String) {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let body = serde_json::to_string(&serde_json::json!({
        "event": "ping",
        "timestamp": ts,
        "data": {"test": true},
        "source": "fluxc webhook test",
    })).unwrap_or_default();

    let sig_legacy = match hmac_hex(secret, body.as_bytes()) {
        Ok(s) => s,
        Err(e) => return (false, 0, format!("HMAC init failed: {}", e)),
    };
    let sig_v2 = match hmac_hex(secret, format!("{}.{}", ts, body).as_bytes()) {
        Ok(s) => s,
        Err(e) => return (false, 0, format!("HMAC init failed: {}", e)),
    };
    let delivery_id: String = sig_v2.chars().take(24).collect();

    match send_http_post(url, &body, &sig_legacy, &sig_v2, "ping", ts, &delivery_id, 10) {
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

    // Uses the pure ssrf_check_with(url, allow_private) — no global env, no race.
    #[test]
    fn ssrf_guard() {
        // default policy (allow_private = false)
        // metadata endpoint is ALWAYS refused
        assert!(ssrf_check_with("http://169.254.169.254/latest/meta-data/", false).is_err());
        // loopback / private / link-local refused by default
        assert!(ssrf_check_with("http://127.0.0.1:8080/api", false).is_err());
        assert!(ssrf_check_with("http://localhost:9991/hook", false).is_err());
        assert!(ssrf_check_with("http://[::1]:8080/x", false).is_err());
        assert!(ssrf_check_with("http://10.0.0.5/x", false).is_err());
        assert!(ssrf_check_with("http://192.168.1.1/x", false).is_err());
        // userinfo can't smuggle a public host past a private resolve
        assert!(ssrf_check_with("http://example.com@127.0.0.1/x", false).is_err());
        // non-http scheme refused
        assert!(ssrf_check_with("file:///etc/passwd", false).is_err());
        assert!(ssrf_check_with("gopher://127.0.0.1/x", false).is_err());

        // opt-in (allow_private = true): same-host allowed, metadata STILL blocked
        assert!(ssrf_check_with("http://169.254.169.254/", true).is_err());
        assert!(ssrf_check_with("http://127.0.0.1:8084/api/build_event", true).is_ok());

        // literal public IP allowed under either policy (no DNS needed in sandbox)
        assert!(ssrf_check_with("https://8.8.8.8/webhook", false).is_ok());
        assert!(ssrf_check_with("https://1.1.1.1:443/hook", true).is_ok());
    }

    #[test]
    fn test_register_and_list() {
        // Must run under an isolated HOME like the other config-mutating tests:
        // otherwise this test writes the process-global ~/.flux/webhooks.json while a
        // parallel `with_temp_home` test has HOME repointed, corrupting that test's
        // store (the pre-existing flake that failed test_prune/test_register at random).
        with_temp_home("register_list", || {
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
        });
    }

    #[test]
    fn test_remove_nonexistent() {
        let result = remove_webhook("nonexistent-id-12345");
        assert!(result.is_err());
    }

    // Config-mutating tests run against an isolated HOME so the live
    // ~/.flux/webhooks.json (with its real registrations) is never touched. Uses the
    // crate-shared helper so the one process-wide lock also serializes against the
    // other modules' HOME-swapping persistence tests (predict/benchmark/tune).
    use fluxc_util::test_home::with_temp_home;

    #[test]
    fn test_quarantine_after_threshold() {
        with_temp_home("quarantine", || {
            register_webhook("dead", "http://127.0.0.1:0/x", "s", vec!["build_complete".into()]).unwrap();
            // Simulate QUARANTINE_THRESHOLD consecutive failures.
            for _ in 0..QUARANTINE_THRESHOLD {
                record_delivery("dead", false, 0);
            }
            let cfg = load_config();
            let w = cfg.webhooks.iter().find(|w| w.id == "dead").unwrap();
            assert!(w.health.quarantined, "should auto-quarantine");
            assert!(!w.enabled, "quarantine must disable the endpoint");
            assert_eq!(w.health.consecutive_failures, QUARANTINE_THRESHOLD);
        });
    }

    #[test]
    fn test_success_lifts_quarantine_and_resets_streak() {
        with_temp_home("lift", || {
            register_webhook("flaky", "http://127.0.0.1:0/x", "s", vec!["build_complete".into()]).unwrap();
            for _ in 0..QUARANTINE_THRESHOLD {
                record_delivery("flaky", false, 0);
            }
            record_delivery("flaky", true, 200);
            let cfg = load_config();
            let w = cfg.webhooks.iter().find(|w| w.id == "flaky").unwrap();
            assert!(!w.health.quarantined, "success should lift quarantine");
            assert!(w.enabled);
            assert_eq!(w.health.consecutive_failures, 0);
            assert_eq!(w.health.total_delivered, 1);
            assert_eq!(w.health.total_failed, QUARANTINE_THRESHOLD as u64);
        });
    }

    #[test]
    fn test_prune_removes_quarantined_only() {
        with_temp_home("prune", || {
            register_webhook("good", "http://127.0.0.1:1/x", "s", vec!["build_complete".into()]).unwrap();
            register_webhook("bad", "http://127.0.0.1:0/x", "s", vec!["build_complete".into()]).unwrap();
            for _ in 0..QUARANTINE_THRESHOLD {
                record_delivery("bad", false, 0);
            }
            let out = prune_webhooks(false);
            assert!(out.contains("bad"), "prune should report removed endpoint");
            let cfg = load_config();
            assert!(cfg.webhooks.iter().any(|w| w.id == "good"));
            assert!(!cfg.webhooks.iter().any(|w| w.id == "bad"), "quarantined should be gone");
        });
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

    #[test]
    fn test_delivery_headers_carry_real_event_and_v2_sig() {
        // Regression: X-Flux-Event used to be hardcoded to "flux-webhook", so every
        // receiver saw the same constant regardless of what actually happened.
        let hs = delivery_headers("legacysig", "v2sig", "build_complete", 1716825600, "deadbeefcafe");
        let get = |k: &str| hs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("X-Flux-Event").as_deref(), Some("build_complete"));
        assert_ne!(get("X-Flux-Event").as_deref(), Some("flux-webhook"));
        assert_eq!(get("X-Flux-Timestamp").as_deref(), Some("1716825600"));
        assert_eq!(get("X-Flux-Signature").as_deref(), Some("sha256=legacysig"));
        assert_eq!(get("X-Flux-Signature-V2").as_deref(), Some("t=1716825600,v1=v2sig"));
        assert_eq!(get("X-Flux-Delivery").as_deref(), Some("deadbeefcafe"));
    }

    #[test]
    fn test_v2_signature_is_replay_resistant() {
        // Same body, different timestamp → different v2 signature, so a captured
        // POST can't be replayed under a stale timestamp.
        let body = r#"{"event":"build_complete"}"#;
        let a = hmac_hex("secret", format!("{}.{}", 1000u64, body).as_bytes()).unwrap();
        let b = hmac_hex("secret", format!("{}.{}", 2000u64, body).as_bytes()).unwrap();
        assert_ne!(a, b);
        // Deterministic for identical inputs.
        assert_eq!(a, hmac_hex("secret", format!("{}.{}", 1000u64, body).as_bytes()).unwrap());
    }
}
