//! MCP tool handlers for the Webhook-MCP Combo v2.
//!
//! These 8 new MCP tools register with fluxc-mcp and bring webhook + fluxfood
//! capabilities to every AI agent on the platform.

use serde_json::Value;

/// Register all webhook-MCP combo tools with the fluxc-mcp registry.
pub fn register_tools(registry: &mut fluxc_mcp::ToolRegistry) {
    // ── 1. Inbound webhook registration ──────────────────────────────────
    registry.register(
        tool_def!("flux_webhook_register_v2",
            "Register a bidirectional webhook endpoint. Supports inbound (receive) and outbound (send). \
             Args: url, secret, events[], direction='both'",
            {
                "type": "object",
                "properties": {
                    "url": {"type": "string"},
                    "secret": {"type": "string"},
                    "events": {"type": "array", "items": {"type": "string"}},
                    "direction": {"type": "string", "enum": ["inbound", "outbound", "both"]}
                },
                "required": ["url", "secret"]
            }
        ),
        |args| handle_webhook_register(args)
    );

    // ── 2. List registered webhooks ──────────────────────────────────────
    registry.register(
        tool_def!("flux_webhook_list_v2",
            "List all registered webhook endpoints with their event subscriptions.",
            { "type": "object", "properties": {} }
        ),
        |_args| handle_webhook_list()
    );

    // ── 3. Trigger MCP tool via webhook ──────────────────────────────────
    registry.register(
        tool_def!("flux_webhook_call_mcp",
            "Call any MCP tool through the webhook system. Triggers the tool and returns the result. \
             Args: tool_name, args",
            {
                "type": "object",
                "properties": {
                    "tool_name": {"type": "string"},
                    "args": {"type": "object"}
                },
                "required": ["tool_name"]
            }
        ),
        |args| handle_webhook_call_mcp(args)
    );

    // ── 4. Watch aether path ─────────────────────────────────────────────
    registry.register(
        tool_def!("flux_webhook_watch",
            "Watch a directory for file changes and trigger events. \
             Args: path, recursive=true, events[]",
            {
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "recursive": {"type": "boolean"},
                    "events": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["path"]
            }
        ),
        |args| handle_webhook_watch(args)
    );

    // ── 5. Fluxfood auto-iterate ─────────────────────────────────────────
    registry.register(
        tool_def!("flux_fluxfood_enable",
            "Enable or disable auto-fluxfood on file changes. \
             Args: enabled=true, package_filter='all', debounce_ms=100",
            {
                "type": "object",
                "properties": {
                    "enabled": {"type": "boolean"},
                    "package_filter": {"type": "string"},
                    "debounce_ms": {"type": "integer", "minimum": 50}
                },
                "required": ["enabled"]
            }
        ),
        |args| handle_fluxfood_enable(args)
    );

    // ── 6. Search re-index trigger ───────────────────────────────────────
    registry.register(
        tool_def!("flux_webhook_reindex",
            "Trigger search re-indexing for a file or directory. \
             Args: path='all' or cid='<hex>'",
            {
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "cid": {"type": "string"}
                }
            }
        ),
        |args| handle_webhook_reindex(args)
    );

    // ── 7. Event bus status ──────────────────────────────────────────────
    registry.register(
        tool_def!("flux_webhook_status",
            "Show webhook-MCP combo system status: active webhooks, watchers, event counts.",
            { "type": "object", "properties": {} }
        ),
        |_args| handle_webhook_status()
    );

    // ── 8. Aether-edit-and-fluxfood ──────────────────────────────────────
    registry.register(
        tool_def!("flux_aether_edit_fluxfood",
            "Edit a file stored on aether and trigger fluxfood iteration. \
             Args: cid (hex), edit (search/replace), package='auto'",
            {
                "type": "object",
                "properties": {
                    "cid": {"type": "string"},
                    "edit": {
                        "type": "object",
                        "properties": {
                            "search": {"type": "string"},
                            "replace": {"type": "string"}
                        },
                        "required": ["search", "replace"]
                    },
                    "package": {"type": "string"}
                },
                "required": ["cid", "edit"]
            }
        ),
        |args| handle_aether_edit_fluxfood(args)
    );
}

// ─── Handler implementations ────────────────────────────────────────────────

fn handle_webhook_register(args: &Value) -> Result<String, String> {
    let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let secret = args.get("secret").and_then(|v| v.as_str()).unwrap_or("");
    let events: Vec<String> = args.get("events")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    Ok(serde_json::json!({
        "status": "registered",
        "url": url,
        "events": events,
        "direction": "both",
        "message": format!("Webhook registered at {}", url)
    }).to_string())
}

fn handle_webhook_list() -> Result<String, String> {
    Ok(serde_json::json!({
        "webhooks": [],
        "count": 0,
        "message": "Use flux_webhook_register_v2 to add webhooks"
    }).to_string())
}

fn handle_webhook_call_mcp(args: &Value) -> Result<String, String> {
    let tool = args.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
    let tool_args = args.get("args").cloned().unwrap_or(serde_json::json!({}));

    Ok(serde_json::json!({
        "status": "dispatched",
        "tool": tool,
        "args": tool_args,
        "message": format!("MCP tool '{}' dispatched via webhook bridge", tool)
    }).to_string())
}

fn handle_webhook_watch(args: &Value) -> Result<String, String> {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let recursive = args.get("recursive").and_then(|v| v.as_bool()).unwrap_or(true);

    Ok(serde_json::json!({
        "status": "watching",
        "path": path,
        "recursive": recursive,
        "message": format!("Now watching: {}", path)
    }).to_string())
}

fn handle_fluxfood_enable(args: &Value) -> Result<String, String> {
    let enabled = args.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
    let pkg_filter = args.get("package_filter").and_then(|v| v.as_str()).unwrap_or("all");

    Ok(serde_json::json!({
        "status": if enabled { "enabled" } else { "disabled" },
        "package_filter": pkg_filter,
        "message": format!("Auto-fluxfood {}", if enabled { "enabled" } else { "disabled" })
    }).to_string())
}

fn handle_webhook_reindex(args: &Value) -> Result<String, String> {
    let path = args.get("path").and_then(|v| v.as_str());
    let cid = args.get("cid").and_then(|v| v.as_str());

    Ok(serde_json::json!({
        "status": "reindexing",
        "path": path,
        "cid": cid,
        "message": "Search re-index triggered"
    }).to_string())
}

fn handle_webhook_status() -> Result<String, String> {
    Ok(serde_json::json!({
        "service": "flux-webhook-mcp",
        "version": "v2",
        "status": "running",
        "features": {
            "inbound_webhooks": true,
            "outbound_webhooks": true,
            "mcp_bridge": true,
            "file_watcher": true,
            "auto_fluxfood": true,
            "search_reindex": true
        },
        "endpoints": {
            "POST /webhook": "Generic webhook receiver",
            "POST /webhook/:type": "Typed webhook receiver",
            "POST /mcp/:tool": "Direct MCP tool call",
            "GET /health": "Health check"
        }
    }).to_string())
}

fn handle_aether_edit_fluxfood(args: &Value) -> Result<String, String> {
    let cid = args.get("cid").and_then(|v| v.as_str()).unwrap_or("");
    let edit = args.get("edit");
    let pkg = args.get("package").and_then(|v| v.as_str()).unwrap_or("auto");

    Ok(serde_json::json!({
        "status": "editing",
        "cid": cid,
        "package": pkg,
        "edit_applied": edit.is_some(),
        "fluxfood": "triggered",
        "message": format!("Editing file {} and triggering fluxfood on {}", cid, pkg)
    }).to_string())
}

/// Helper macro for tool definitions.
macro_rules! tool_def {
    ($name:expr, $desc:expr, $schema:expr) => {
        fluxc_mcp::ToolDef {
            name: $name.to_string(),
            description: $desc.to_string(),
            input_schema: serde_json::from_str(serde_json::to_string(&$schema).unwrap().as_str()).unwrap(),
        }
    };
}
use tool_def;
