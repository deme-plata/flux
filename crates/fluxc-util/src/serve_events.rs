// serve_events — Event pushing and now_ms for serve
// Extracted as part of refactor.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Push an event into the live stats event queue. Called from MCP tools or webhooks.
pub fn push_event(event_type: &str, data: &serde_json::Value) {
    let queue_dir = std::path::PathBuf::from("/tmp/flux-events");
    let _ = std::fs::create_dir_all(&queue_dir);
    let event_file = queue_dir.join(format!("evt_{}.json", now_ms()));
    let payload = serde_json::json!({
        "type": event_type,
        "data": data,
        "timestamp_ms": now_ms(),
    });
    if let Ok(json) = serde_json::to_string(&payload) {
        let _ = std::fs::write(&event_file, json);
    }
}

/// Push an AI feed event — called from MCP tools to broadcast real-time activity.
pub fn push_feed_event(agent: &str, message: &str, tool: &str) {
    let queue_dir = std::path::PathBuf::from("/tmp/flux-events");
    let _ = std::fs::create_dir_all(&queue_dir);
    let event_file = queue_dir.join(format!("feed_{}.json", now_ms()));
    let payload = serde_json::json!({
        "type": "ai_feed",
        "agent": agent,
        "message": message,
        "tool": tool,
        "timestamp_ms": now_ms(),
    });
    if let Ok(json) = serde_json::to_string(&payload) {
        let _ = std::fs::write(&event_file, json);
    }
}
