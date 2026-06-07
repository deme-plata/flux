//! Event types for the webhook-MCP combo system.

use serde::{Deserialize, Serialize};

/// A webhook event — the universal message type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEvent {
    /// Unique event ID
    pub id: String,
    /// Event type (e.g., "file_stored", "file_edited", "build_complete")
    pub event_type: String,
    /// Source (e.g., "aether", "webhook", "mcp", "watcher")
    pub source: String,
    /// Timestamp (unix ms)
    pub timestamp_ms: u64,
    /// Event payload (JSON)
    pub payload: serde_json::Value,
    /// HMAC-SHA256 signature (if from external webhook)
    pub signature: Option<String>,
    /// Whether this event should trigger fluxfood
    pub trigger_fluxfood: bool,
    /// Whether this event should trigger search re-index
    pub trigger_search: bool,
    /// Target MCP tool to call (if any)
    pub target_mcp_tool: Option<String>,
    /// Priority (0=low, 5=normal, 10=critical)
    pub priority: u8,
}

/// An MCP tool definition (for the dispatcher registry).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// Result of dispatching an MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpDispatchResult {
    pub tool: String,
    pub success: bool,
    pub duration_ms: u64,
    pub output: String,
    pub error: Option<String>,
}

/// A file change event from the aether watcher.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChangeEvent {
    pub path: String,
    pub kind: FileChangeKind,
    pub file_cid: Option<String>,
    pub file_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FileChangeKind {
    Created,
    Modified,
    Deleted,
}

impl WebhookEvent {
    /// Create a new event.
    pub fn new(event_type: &str, source: &str, payload: serde_json::Value) -> Self {
        Self {
            id: format!("evt-{}", uuid::Uuid::new_v4()),
            event_type: event_type.to_string(),
            source: source.to_string(),
            timestamp_ms: now_ms(),
            payload,
            signature: None,
            trigger_fluxfood: false,
            trigger_search: false,
            target_mcp_tool: None,
            priority: 5,
        }
    }

    /// Mark this event to trigger fluxfood.
    pub fn with_fluxfood(mut self) -> Self {
        self.trigger_fluxfood = true;
        self
    }

    /// Mark this event to trigger search re-index.
    pub fn with_search(mut self) -> Self {
        self.trigger_search = true;
        self
    }

    /// Set the target MCP tool to dispatch.
    pub fn with_mcp_tool(mut self, tool: &str) -> Self {
        self.target_mcp_tool = Some(tool.to_string());
        self
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Standard event types used by the system.
pub mod event_types {
    pub const FILE_STORED: &str = "file_stored";
    pub const FILE_EDITED: &str = "file_edited";
    pub const FILE_DELETED: &str = "file_deleted";
    pub const BUILD_COMPLETE: &str = "build_complete";
    pub const BUILD_FAILED: &str = "build_failed";
    pub const TEST_COMPLETE: &str = "test_complete";
    pub const SEARCH_INDEXED: &str = "search_indexed";
    pub const MCP_TOOL_CALLED: &str = "mcp_tool_called";
    pub const SWARM_MESSAGE: &str = "swarm_message";
    pub const FLUXFOOD_TRIGGER: &str = "fluxfood_trigger";
    pub const WEBHOOK_RECEIVED: &str = "webhook_received";
    pub const AETHER_SYNC: &str = "aether_sync";
}
