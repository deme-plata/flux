//! flux-dj-mcp — standalone stdio MCP server exposing flux-dj's tools
//! (`dj_log_track`, `dj_get_history`, `dj_key_compatible`,
//! `dj_estimate_tempo`).
//!
//! Same hand-rolled JSON-RPC-over-stdio protocol `fluxc-mcp` uses
//! (`crates/fluxc-mcp/src/lib.rs`: `initialize` / `tools/list` /
//! `tools/call` / `notifications/initialized`), reimplemented here
//! standalone so this binary has no dependency on the rest of the Flux MCP
//! surface — point an MCP client (Claude Desktop, Codex, etc.) directly at
//! this binary's path, e.g.:
//! ```json
//! { "mcpServers": { "flux-dj": { "command": "/path/to/flux-dj-mcp" } } }
//! ```

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

use flux_dj::mcp::{build_registry, ToolRegistry};

fn main() {
    let registry = build_registry();
    eprintln!("flux-dj MCP server — stdio transport, {} tools", registry.tools_schema().len());
    eprintln!("  tools: dj_log_track, dj_get_history, dj_key_compatible, dj_estimate_tempo");

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let _ = writeln!(stdout, "{}", json!({"error": format!("parse: {}", e)}));
                continue;
            }
        };

        let response = handle_mcp_request(&request, &registry);
        let _ = writeln!(stdout, "{}", serde_json::to_string(&response).unwrap_or_default());
        let _ = stdout.flush();
    }
}

fn handle_mcp_request(request: &Value, registry: &ToolRegistry) -> Value {
    let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = request.get("id").cloned().unwrap_or(Value::Null);

    match method {
        "initialize" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "flux-dj-mcp", "version": env!("CARGO_PKG_VERSION")},
                "capabilities": {"tools": {}}
            }
        }),

        "tools/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {"tools": registry.tools_schema()}
        }),

        "tools/call" => {
            let tool_name = request.pointer("/params/name").and_then(|v| v.as_str()).unwrap_or("");
            let args = request.pointer("/params/arguments").cloned().unwrap_or(Value::Null);
            let result = registry.execute(tool_name, &args).unwrap_or_else(|| format!("Tool not found: {tool_name}"));
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"content": [{"type": "text", "text": result}]}
            })
        }

        "notifications/initialized" => json!({"jsonrpc": "2.0", "id": id}),

        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": format!("Method not found: {method}")}
        }),
    }
}
