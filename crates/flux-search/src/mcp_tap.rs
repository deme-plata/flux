//! flux-search v2 — MCP-call → Document conversion.
//!
//! Subscribes to the fluxc-mcp tool-call stream (when wired) and produces
//! `Document`s with kind-aware titles/excerpts so swarm activity becomes
//! searchable in the same index that holds workspace docs.
//!
//! The wiring (subscribe to /tmp/flux-events or fluxc-mcp's tap) lives in
//! sigil-node or fluxc-serve — this module just provides the conversion.

use crate::Document;
use crate::secret_scrape::redact_args;
use serde::{Deserialize, Serialize};

/// One captured tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolCall {
    pub agent_id: String,
    pub tool: String,
    pub args: serde_json::Value,
    pub result_preview: Option<String>,
    pub ts_ms: u64,
    pub elapsed_ms: Option<u32>,
    pub success: bool,
}

/// One captured swarm broadcast.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmBroadcast {
    pub msg_id: u64,
    pub from: String,
    pub to: String,
    pub body: String,
    pub ts_ms: u64,
}

/// One captured settled task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettledTask {
    pub agent_id: String,
    pub task_id: String,
    pub crate_or_lane: String,
    pub qug: f64,
    pub wallet: String,
    pub ts_ms: u64,
}

/// Convert a captured tool call into a search Document.
/// Args are passed through `redact_args` so the index never holds raw secrets.
pub fn doc_from_tool_call(call: &McpToolCall) -> Document {
    let redacted = redact_args(&call.args);
    let title = format!("{} · {}", call.tool, call.agent_id);
    let mut content = format!(
        "agent: {}\ntool: {}\nts_ms: {}\nsuccess: {}\n",
        call.agent_id, call.tool, call.ts_ms, call.success
    );
    if let Some(ms) = call.elapsed_ms {
        content.push_str(&format!("elapsed_ms: {ms}\n"));
    }
    content.push_str(&format!(
        "args:\n{}\n",
        serde_json::to_string_pretty(&redacted).unwrap_or_default()
    ));
    if let Some(r) = &call.result_preview {
        content.push_str(&format!("result:\n{r}\n"));
    }

    let id = format!("tool:{}:{}", call.ts_ms, call.tool);
    let content_hash = blake3_short(&content);
    Document {
        id,
        url: format!("/api/v1/search/tool/{}", call.ts_ms),
        title,
        content,
        meta_description: call.result_preview.clone(),
        language: Some("en".into()),
        category: Some("tool".into()),
        page_rank: 0.5,
        readability_score: 0.8,
        word_count: 0, // filled by ranking layer
        last_crawled: Some(call.ts_ms),
        content_hash,
    }
}

/// Convert a swarm broadcast into a Document.
pub fn doc_from_broadcast(b: &SwarmBroadcast) -> Document {
    let id = format!("bcast:{}", b.msg_id);
    let title_excerpt = b
        .body
        .lines()
        .next()
        .unwrap_or("(empty broadcast)")
        .chars()
        .take(140)
        .collect::<String>();
    let title = format!("msg #{} · {} → {}", b.msg_id, b.from, b.to);
    let content = format!(
        "from: {}\nto: {}\nts_ms: {}\n\n{}",
        b.from, b.to, b.ts_ms, b.body
    );
    let content_hash = blake3_short(&content);
    Document {
        id,
        url: format!("/api/v1/swarm/message/{}", b.msg_id),
        title,
        content,
        meta_description: Some(title_excerpt),
        language: Some("en".into()),
        category: Some("broadcast".into()),
        page_rank: 0.5,
        readability_score: 0.8,
        word_count: 0,
        last_crawled: Some(b.ts_ms),
        content_hash,
    }
}

/// Convert a settled task into a Document.
pub fn doc_from_settled(s: &SettledTask) -> Document {
    let id = format!("settled:{}:{}", s.ts_ms, s.task_id);
    let title = format!(
        "{} · {} · {:.2} QUG",
        s.agent_id, s.crate_or_lane, s.qug
    );
    let content = format!(
        "agent: {}\ntask_id: {}\nlane: {}\nwallet: {}\nqug: {}\nts_ms: {}",
        s.agent_id, s.task_id, s.crate_or_lane, s.wallet, s.qug, s.ts_ms
    );
    let content_hash = blake3_short(&content);
    Document {
        id,
        url: format!("/api/v1/swarm/settled/{}", s.task_id),
        title,
        content,
        meta_description: Some(format!("+{:.2} QUG to {}", s.qug, s.wallet)),
        language: Some("en".into()),
        category: Some("settled".into()),
        page_rank: 0.6,
        readability_score: 0.9,
        word_count: 0,
        last_crawled: Some(s.ts_ms),
        content_hash,
    }
}

fn blake3_short(s: &str) -> String {
    let hash = blake3::hash(s.as_bytes());
    hash.to_hex().as_str().chars().take(16).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_call_becomes_doc_with_redaction() {
        let call = McpToolCall {
            agent_id: "rocky-sigil".into(),
            tool: "flux_combo".into(),
            args: serde_json::json!({
                "package": "flux-search",
                "api_key": "sk-deadbeef-this-should-be-redacted"
            }),
            result_preview: Some("Compile: ✓ · 18 tests passed".into()),
            ts_ms: 1_780_131_000_000,
            elapsed_ms: Some(31_803),
            success: true,
        };
        let doc = doc_from_tool_call(&call);
        assert!(doc.title.starts_with("flux_combo"));
        assert!(doc.content.contains("rocky-sigil"));
        assert!(doc.content.contains("[REDACTED]"));
        assert!(!doc.content.contains("sk-deadbeef"));
        assert_eq!(doc.category.as_deref(), Some("tool"));
    }

    #[test]
    fn broadcast_becomes_doc() {
        let b = SwarmBroadcast {
            msg_id: 139,
            from: "rocky-sigil".into(),
            to: "*".into(),
            body: "📦 SIGIL v0.0.5 — plan locked".into(),
            ts_ms: 1_780_125_000_000,
        };
        let doc = doc_from_broadcast(&b);
        assert!(doc.title.contains("139"));
        assert_eq!(doc.category.as_deref(), Some("broadcast"));
    }

    #[test]
    fn settled_becomes_doc() {
        let s = SettledTask {
            agent_id: "rocky-130".into(),
            task_id: "rocky-130".into(),
            crate_or_lane: "sigil-v005-R1".into(),
            qug: 0.5,
            wallet: "qnk7154929aff…1ccb".into(),
            ts_ms: 1_780_125_809_000,
        };
        let doc = doc_from_settled(&s);
        assert!(doc.title.contains("0.50 QUG"));
        assert!(doc.meta_description.as_deref().unwrap().contains("0.50"));
    }
}
