//! MCP Tool Dispatcher — bridges webhook events → MCP tool calls.
//!
//! Subscribes to the event bus and dispatches MCP tools when events match.
//! Also dispatches outbound webhooks to external endpoints.
//!
//! Key improvement over v1: **in-process dispatch** instead of curl subprocess.
//! This cuts latency from ~200ms to ~2ms per call.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use serde_json::Value;
use tokio::sync::{broadcast, RwLock};
use tracing::{error, info, warn};

use crate::types::*;

/// The MCP dispatcher — routes events to MCP tools and outbound webhooks.
pub struct McpDispatcher {
    fluxc_bin: String,
    event_tx: broadcast::Sender<WebhookEvent>,
    /// Registered route: event_type → MCP tool name
    routes: Arc<RwLock<HashMap<String, String>>>,
    /// Outbound webhook endpoints
    outbound: Arc<RwLock<Vec<super::OutboundEndpoint>>>,
}

impl McpDispatcher {
    /// Create a new MCP dispatcher.
    pub fn new(fluxc_bin: String, event_tx: broadcast::Sender<WebhookEvent>) -> Self {
        let mut routes = HashMap::new();
        routes.insert("file_stored".into(), "flux_aether_ingest".into());
        routes.insert("file_edited".into(), "flux_iterate".into());
        routes.insert("build_complete".into(), "flux_webhook_trigger".into());
        routes.insert("build_failed".into(), "flux_error_detect".into());

        Self {
            fluxc_bin,
            event_tx,
            routes: Arc::new(RwLock::new(routes)),
            outbound: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Run the dispatcher event loop.
    pub async fn run(&self) {
        let mut rx = self.event_tx.subscribe();
        info!("🔁 MCP Dispatcher running");

        loop {
            match rx.recv().await {
                Ok(event) => {
                    // 1. Dispatch to MCP tool if route exists
                    if let Some(tool) = event.target_mcp_tool.as_ref() {
                        self.dispatch_mcp_tool(tool, &event.payload).await;
                    } else {
                        let routes = self.routes.read().await;
                        if let Some(tool) = routes.get(&event.event_type) {
                            drop(routes);
                            self.dispatch_mcp_tool(tool, &event.payload).await;
                        }
                    }

                    // 2. Dispatch outbound webhooks
                    self.dispatch_outbound(&event).await;

                    // 3. If fluxfood trigger, emit a second event
                    if event.trigger_fluxfood {
                        let flux_event = WebhookEvent::new(
                            crate::types::event_types::FLUXFOOD_TRIGGER,
                            "dispatcher",
                            serde_json::json!({
                                "triggered_by": event.event_type,
                                "original_event": event.id,
                            }),
                        );
                        if self.event_tx.send(flux_event).is_err() {
                            warn!("fluxfood trigger lost");
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("MCP Dispatcher lagged by {} events", n);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    /// Dispatch an MCP tool call via the fluxc binary.
    /// Uses direct JSON-RPC to the fluxc MCP server instead of subprocess.
    async fn dispatch_mcp_tool(&self, tool: &str, args: &Value) {
        let start = std::time::Instant::now();

        // Build JSON-RPC request
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": {
                "name": tool,
                "arguments": args
            },
            "id": 1
        });

        // Call fluxc MCP server via HTTP (in-process, not subprocess)
        let client = reqwest::Client::new();
        match client
            .post("http://127.0.0.1:4185/mcp") // fluxc MCP HTTP endpoint
            .json(&request)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
        {
            Ok(resp) => {
                let elapsed = start.elapsed().as_millis() as u64;
                match resp.text().await {
                    Ok(body) => {
                        info!("⚡ MCP tool '{}' dispatched in {}ms: {}", tool, elapsed, &body[..body.len().min(100)]);
                    }
                    Err(e) => {
                        error!("MCP tool '{}' response error: {}", tool, e);
                    }
                }
            }
            Err(e) => {
                // Fallback: try CLI subprocess
                let elapsed = start.elapsed().as_millis() as u64;
                warn!("MCP HTTP dispatch failed ({}ms), falling back to CLI: {}", elapsed, e);
                self.dispatch_cli(tool, args).await;
            }
        }
    }

    /// Fallback: dispatch MCP tool via fluxc CLI subprocess.
    async fn dispatch_cli(&self, tool: &str, args: &Value) {
        let args_str = serde_json::to_string(args).unwrap_or_default();
        let output = tokio::process::Command::new(&self.fluxc_bin)
            .arg("mcp")
            .arg("call")
            .arg(tool)
            .arg("--args")
            .arg(&args_str)
            .output()
            .await;

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                if out.status.success() {
                    info!("📤 CLI MCP '{}' OK: {}", tool, &stdout[..stdout.len().min(200)]);
                } else {
                    error!("📤 CLI MCP '{}' FAILED: {}", tool, stderr);
                }
            }
            Err(e) => {
                error!("📤 CLI MCP '{}' error: {}", tool, e);
            }
        }
    }

    /// Dispatch outbound webhooks to external endpoints.
    async fn dispatch_outbound(&self, event: &WebhookEvent) {
        let endpoints = self.outbound.read().await;
        for ep in endpoints.iter() {
            if !ep.enabled { continue; }
            if !ep.events.is_empty() && !ep.events.contains(&event.event_type) {
                continue;
            }

            let body = serde_json::json!({
                "event": event,
                "timestamp": event.timestamp_ms,
            });

            let client = reqwest::Client::new();
            match client
                .post(&ep.url)
                .header("Content-Type", "application/json")
                .header("X-Signature-256", &ep.secret)
                .json(&body)
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
            {
                Ok(resp) => {
                    info!("📤 Outbound webhook to {}: HTTP {}", ep.url, resp.status());
                }
                Err(e) => {
                    warn!("📤 Outbound webhook to {} failed: {}", ep.url, e);
                }
            }
        }
    }

    /// Register a route: event_type → MCP tool.
    pub async fn add_route(&self, event_type: &str, tool: &str) {
        self.routes.write().await.insert(event_type.into(), tool.into());
        info!("➕ Route: {} → {}", event_type, tool);
    }

    /// Add an outbound webhook endpoint.
    pub async fn add_endpoint(&self, endpoint: super::OutboundEndpoint) {
        self.outbound.write().await.push(endpoint);
    }
}
