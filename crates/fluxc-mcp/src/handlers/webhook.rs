use serde_json::{json, Value};
use crate::handlers::{ToolDef, ToolRegistry};
use fluxc_core::webhook;

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
