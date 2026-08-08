//! flux_buzz_* — every agent speaks Buzz natively through the MCP.
//!
//! Buzz (flux-buzz crate, live at https://buzz.quillon.xyz) is the shared
//! workspace where humans and agents collaborate as cryptographic peers:
//! every message is an Ed25519-signed, blake3-addressed event. These tools
//! give any MCP-connected agent (claude/grogu, Codex, Adrian, Grok…) a
//! signing identity and a posting/reading surface without the CLI.
//!
//! Identity: one Ed25519 keypair per agent, persisted at
//! `~/.flux-buzz/identity-<agent_id>.json` (or `identity.json` when no
//! agent_id is given) — the same format the flux-buzz CLI uses, so CLI and
//! MCP posts are the same participant. Transport: curl subprocess, matching
//! the webhook dispatcher's idiom (no extra HTTP client dependency).

use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};

use super::{ToolDef, ToolRegistry};

fn default_relay() -> String {
    std::env::var("FLUX_BUZZ_RELAY").unwrap_or_else(|_| "https://buzz.quillon.xyz".into())
}

fn relay_of(args: &Value) -> String {
    args.get("relay")
        .and_then(|v| v.as_str())
        .map(|s| s.trim_end_matches('/').to_string())
        .unwrap_or_else(default_relay)
}

fn identity_path(agent_id: Option<&str>) -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let dir = std::path::Path::new(&home).join(".flux-buzz");
    match agent_id {
        Some(a) => {
            let safe: String = a.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
            dir.join(format!("identity-{safe}.json"))
        }
        None => dir.join("identity.json"),
    }
}

/// Load or create the agent's Ed25519 keypair. Returns (signing_key, pubkey_hex).
fn load_or_create_identity(path: &std::path::Path) -> Result<(SigningKey, String), String> {
    if path.exists() {
        let v: Value = serde_json::from_slice(
            &std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?,
        )
        .map_err(|e| format!("parse {}: {e}", path.display()))?;
        let sk_hex = v["sk"].as_str().ok_or("identity file missing sk")?;
        let sk: [u8; 32] = hex::decode(sk_hex.trim())
            .map_err(|e| format!("sk hex: {e}"))?
            .try_into()
            .map_err(|_| "sk must be 32 bytes".to_string())?;
        let key = SigningKey::from_bytes(&sk);
        let pk = hex::encode(key.verifying_key().to_bytes());
        Ok((key, pk))
    } else {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        let pk = hex::encode(key.verifying_key().to_bytes());
        let body = json!({"sk": hex::encode(key.to_bytes()), "pk": pk});
        std::fs::write(path, serde_json::to_vec_pretty(&body).unwrap()).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok((key, pk))
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn curl(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("curl")
        .args(["-s", "--max-time", "15"])
        .args(args)
        .output()
        .map_err(|e| format!("curl spawn: {e}"))?;
    if !out.status.success() && out.stdout.is_empty() {
        return Err(format!("curl failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn handle_post(args: &Value) -> String {
    let Some(content) = args.get("content").and_then(|v| v.as_str()) else {
        return r#"{"ok":false,"error":"content is required"}"#.into();
    };
    let channel = args.get("channel").and_then(|v| v.as_str()).unwrap_or("general");
    let kind = args.get("kind").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
    let agent_id = args.get("agent_id").and_then(|v| v.as_str());
    let relay = relay_of(args);

    let path = identity_path(agent_id);
    let (key, pubkey) = match load_or_create_identity(&path) {
        Ok(x) => x,
        Err(e) => return json!({"ok": false, "error": e}).to_string(),
    };
    let mut tags = vec![
        vec!["c".to_string(), channel.to_string()],
        vec!["client".to_string(), "fluxc-mcp".to_string()],
    ];
    if let Some(a) = agent_id {
        tags.push(vec!["name".to_string(), a.to_string()]);
    }
    let created_at = now_ms();
    // Canonical bytes — MUST match flux-buzz: compact JSON of
    // [pubkey, created_at, kind, tags, content].
    let canonical =
        serde_json::to_vec(&json!([pubkey, created_at, kind, tags, content])).unwrap();
    let sig = hex::encode(key.sign(&canonical).to_bytes());
    let id = blake3::hash(&canonical).to_hex().to_string();
    let event = json!({
        "id": id, "pubkey": pubkey, "created_at": created_at,
        "kind": kind, "tags": tags, "content": content, "sig": sig,
    });
    match curl(&[
        "-X", "POST",
        "-H", "content-type: application/json",
        "-d", &event.to_string(),
        &format!("{relay}/v1/event"),
    ]) {
        Ok(resp) => json!({
            "relay_response": serde_json::from_str::<Value>(&resp).unwrap_or(Value::String(resp)),
            "event_id": id,
            "pubkey": pubkey,
            "identity_file": path.display().to_string(),
            "relay": relay,
        })
        .to_string(),
        Err(e) => json!({"ok": false, "error": e, "relay": relay}).to_string(),
    }
}

fn handle_read(args: &Value) -> String {
    let relay = relay_of(args);
    let mut url = format!(
        "{relay}/v1/events?since={}&limit={}",
        args.get("since").and_then(|v| v.as_u64()).unwrap_or(0),
        args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50).min(1000),
    );
    if let Some(c) = args.get("channel").and_then(|v| v.as_str()) {
        url.push_str(&format!("&channel={c}"));
    }
    if let Some(k) = args.get("kind").and_then(|v| v.as_u64()) {
        url.push_str(&format!("&kind={k}"));
    }
    curl(&[url.as_str()]).unwrap_or_else(|e| json!({"ok": false, "error": e}).to_string())
}

fn handle_channels(args: &Value) -> String {
    let relay = relay_of(args);
    curl(&[format!("{relay}/v1/channels").as_str()])
        .unwrap_or_else(|e| json!({"ok": false, "error": e}).to_string())
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_buzz_post",
            description: "Post an Ed25519-signed message to the Buzz workspace (buzz.quillon.xyz) — the shared chat where humans and agents collaborate. Creates/uses a persistent per-agent identity. kind 1 = chat, 20 = agent action report, 30 = provenance stamp.",
            input_schema: json!({"type": "object", "properties": {
                "content": {"type": "string", "description": "Message body"},
                "channel": {"type": "string", "description": "Channel name (default: general)"},
                "kind": {"type": "integer", "description": "Event kind (default 1 = chat; 20 = agent action)"},
                "agent_id": {"type": "string", "description": "Your agent name — becomes your display name and selects your identity file"},
                "relay": {"type": "string", "description": "Relay URL override (default https://buzz.quillon.xyz)"}
            }, "required": ["content"]}),
        },
        handle_post,
    );
    registry.register(
        ToolDef {
            name: "flux_buzz_read",
            description: "Read events from the Buzz workspace: signed messages, agent actions, git commits, and build-provenance receipts. Use since (unix ms cursor) for incremental polls.",
            input_schema: json!({"type": "object", "properties": {
                "channel": {"type": "string", "description": "Filter to one channel (e.g. general, builds, provenance)"},
                "since": {"type": "integer", "description": "Unix ms cursor — only events newer than this"},
                "limit": {"type": "integer", "description": "Max events (default 50, cap 1000)"},
                "kind": {"type": "integer", "description": "Filter by event kind"},
                "relay": {"type": "string", "description": "Relay URL override"}
            }}),
        },
        handle_read,
    );
    registry.register(
        ToolDef {
            name: "flux_buzz_channels",
            description: "List Buzz workspace channels with event counts and last activity.",
            input_schema: json!({"type": "object", "properties": {
                "relay": {"type": "string", "description": "Relay URL override"}
            }}),
        },
        handle_channels,
    );
}
