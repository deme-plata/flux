use serde_json::{json, Value};
use crate::handlers::{ToolDef, ToolRegistry};
use fluxc_webhooks::webhook;

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_webhook_register",
            description: "Register a webhook endpoint to receive build/test/bench events. Events are POSTed with HMAC-SHA256 signatures.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Unique webhook identifier"},
                    "url": {"type": "string", "description": "Webhook endpoint URL (HTTPS recommended)"},
                    "secret": {"type": "string", "description": "HMAC-SHA256 signing secret"},
                    "events": {"type": "array", "items": {"type": "string"}, "description": "Event types: build_complete, build_failed, test_complete, bench_complete, iterate_complete, batch_complete"}
                },
                "required": ["id", "url", "secret", "events"]
            }),
        },
        flux_webhook_register,
    );
    registry.register(
        ToolDef {
            name: "flux_webhook_list",
            description: "List all registered webhook endpoints.",
            input_schema: json!({"type": "object", "properties": {}}),
        },
        flux_webhook_list,
    );
    registry.register(
        ToolDef {
            name: "flux_webhook_trigger",
            description: "Manually trigger a webhook event to test endpoints.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "event": {"type": "string", "description": "Event type to trigger: build_complete, build_failed, test_complete, bench_complete, iterate_complete, batch_complete"},
                    "package": {"type": "string", "description": "Package name for event data"},
                    "elapsed_ms": {"type": "integer", "description": "Elapsed milliseconds for event data"}
                },
                "required": ["event"]
            }),
        },
        flux_webhook_trigger,
    );
    registry.register(
        ToolDef {
            name: "flux_webhook_test",
            description: "Test a webhook endpoint by sending a signed ping. Verifies connectivity, HMAC signature, and HTTP response. Use before registering a webhook to ensure the URL is reachable.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "Webhook URL to test"},
                    "secret": {"type": "string", "description": "HMAC secret for signing (optional, defaults to 'test')"}
                },
                "required": ["url"]
            }),
        },
        flux_webhook_test,
    );
    registry.register(
        ToolDef {
            name: "flux_webhook_remove",
            description: "Remove a registered webhook by id.",
            input_schema: json!({
                "type": "object",
                "properties": {"id": {"type": "string", "description": "Webhook id to remove"}},
                "required": ["id"]
            }),
        },
        flux_webhook_remove,
    );
    registry.register(
        ToolDef {
            name: "flux_webhook_prune",
            description: "Remove dead webhook endpoints (auto-quarantined after 5 consecutive failures). Set include_failing=true to also drop endpoints with any current failure streak. Use to clean up stale registrations.",
            input_schema: json!({
                "type": "object",
                "properties": {"include_failing": {"type": "boolean", "description": "Also remove endpoints with a current failure streak (default false = quarantined only)"}}
            }),
        },
        flux_webhook_prune,
    );
    registry.register(
        ToolDef {
            name: "flux_webhook_health",
            description: "Machine-readable delivery health for every webhook: last status, success/fail counts, consecutive failures, quarantine state. Lets an agent decide what to prune or retry.",
            input_schema: json!({"type": "object", "properties": {}}),
        },
        flux_webhook_health,
    );
    registry.register(
        ToolDef {
            name: "flux_webhook_enable",
            description: "Enable or disable a webhook by id. Re-enabling clears quarantine and the failure streak.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Webhook id"},
                    "enabled": {"type": "boolean", "description": "true to enable, false to disable"}
                },
                "required": ["id", "enabled"]
            }),
        },
        flux_webhook_enable,
    );
}

fn flux_webhook_remove(args: &Value) -> String {
    let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
    if id.is_empty() {
        return "Error: 'id' parameter is required".into();
    }
    match webhook::remove_webhook(id) {
        Ok(msg) => msg,
        Err(e) => format!("✗ {}", e),
    }
}

fn flux_webhook_prune(args: &Value) -> String {
    let include_failing = args.get("include_failing").and_then(|v| v.as_bool()).unwrap_or(false);
    webhook::prune_webhooks(include_failing)
}

fn flux_webhook_health(_args: &Value) -> String {
    serde_json::to_string_pretty(&webhook::health_report())
        .unwrap_or_else(|e| format!("✗ {}", e))
}

fn flux_webhook_enable(args: &Value) -> String {
    let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let enabled = args.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
    if id.is_empty() {
        return "Error: 'id' parameter is required".into();
    }
    match webhook::set_webhook_enabled(id, enabled) {
        Ok(msg) => msg,
        Err(e) => format!("✗ {}", e),
    }
}

fn flux_webhook_register(args: &Value) -> String {
    let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let secret = args.get("secret").and_then(|v| v.as_str()).unwrap_or("");
    let events: Vec<String> = args.get("events")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    if id.is_empty() || url.is_empty() || secret.is_empty() || events.is_empty() {
        return "Error: id, url, secret, and events are all required".into();
    }

    if let Err(e) = webhook::register_webhook(id, url, secret, events.clone()) {
        return format!("✗ Webhook registration failed: {}", e);
    }

    webhook::auto_dispatch("webhook_registered", serde_json::json!({
        "id": id, "url": url, "events": events,
    }));

    format!(
        "🔗 Webhook registered: {}\n  URL: {}\n  Events: {}\n  HMAC: SHA-256 signed",
        id, url, events.join(", ")
    )
}

fn flux_webhook_list(_args: &Value) -> String {
    webhook::list_webhooks()
}

fn flux_webhook_trigger(args: &Value) -> String {
    let event = args.get("event").and_then(|v| v.as_str()).unwrap_or("");
    let package = args.get("package").and_then(|v| v.as_str()).unwrap_or("fluxc");
    let elapsed_ms = args.get("elapsed_ms").and_then(|v| v.as_u64()).unwrap_or(0);

    if event.is_empty() {
        return "Error: 'event' parameter is required".into();
    }

    let data: Value = match event {
        "build_complete" => webhook::build_event_data(package, true, elapsed_ms as u128, 0, 0),
        "build_failed" => webhook::build_event_data(package, false, elapsed_ms as u128, 0, 0),
        "test_complete" => json!({"package": package, "success": true, "elapsed_ms": elapsed_ms}),
        "bench_complete" => json!({"suite": "manual", "elapsed_ms": elapsed_ms}),
        "iterate_complete" => json!({"package": package, "total_ms": elapsed_ms}),
        "batch_complete" => json!({"total_packages": 1, "total_ms": elapsed_ms}),
        _ => return format!("Unknown event type: {}. Available: build_complete, build_failed, test_complete, bench_complete, iterate_complete, batch_complete", event),
    };

    webhook::auto_dispatch(event, data);
    format!("🔗 Webhook triggered: {} for {}", event, package)
}

fn flux_webhook_test(args: &Value) -> String {
    let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let secret = args.get("secret").and_then(|v| v.as_str()).unwrap_or("test");

    if url.is_empty() {
        return "Error: 'url' parameter is required".into();
    }

    let (ok, status, msg) = webhook::test_webhook_url(url, secret);
    if ok {
        format!("✓ Webhook test: {} → HTTP {} ({})", url, status, msg)
    } else {
        format!("✗ Webhook test failed: {} → HTTP {} ({})", url, status, msg)
    }
}
