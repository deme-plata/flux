//! Platform combo audit — POST to Aether control surface :4178 + durable jsonl captures.
//! Complements fluxc_core::webhook::auto_dispatch (swarm feed) with MCP-combo audit trail.

use serde_json::{json, Value};
use std::path::PathBuf;

fn webhook_url() -> String {
    std::env::var("FLUX_PLATFORM_WEBHOOK_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:4178/api/mcp-webhook".into())
}

fn captures_dir() -> PathBuf {
    crate::handlers::ws().join("target/flux-platform-captures")
}

fn capture_path(event: &str) -> PathBuf {
    captures_dir().join(format!("{event}.jsonl"))
}

/// POST combo result to control surface and append one jsonl audit line.
pub fn dispatch(tool: &str, event: &str, payload: Value) {
    let envelope = json!({
        "source": "fluxc-mcp",
        "tool": tool,
        "event": event,
        "ts_ms": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
        "payload": payload,
    });

    let body = serde_json::to_string(&envelope).unwrap_or_default();
    let _ = std::process::Command::new("curl")
        .args([
            "-sS",
            "-m",
            "5",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "-d",
            &body,
            &webhook_url(),
        ])
        .output();

    if let Some(parent) = captures_dir().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::create_dir_all(&captures_dir());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(capture_path(event))
    {
        use std::io::Write;
        let _ = writeln!(f, "{body}");
    }
}