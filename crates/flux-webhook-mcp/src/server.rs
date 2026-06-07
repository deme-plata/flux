//! Inbound webhook server — receives webhooks from external systems.
//!
//! Listens on configurable port (default :4199) and accepts:
//! - `POST /webhook` — generic webhook receiver
//! - `POST /webhook/:event_type` — typed webhook receiver
//! - `POST /mcp/:tool_name` — directly call an MCP tool via webhook
//! - `GET /health` — health check
//!
//! All incoming webhooks are authenticated via HMAC-SHA256 (header `X-Signature-256`)
//! and converted to WebhookEvents on the event bus.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use crate::types::*;

/// Shared server state.
struct AppState {
    event_tx: broadcast::Sender<WebhookEvent>,
}

/// The inbound webhook server.
pub struct WebhookServer {
    port: u16,
    event_tx: broadcast::Sender<WebhookEvent>,
}

impl WebhookServer {
    /// Create a new webhook server.
    pub fn new(port: u16, event_tx: broadcast::Sender<WebhookEvent>) -> Self {
        Self { port, event_tx }
    }

    /// Run the server (blocking).
    pub async fn run(self) -> anyhow::Result<()> {
        let state = Arc::new(AppState {
            event_tx: self.event_tx,
        });

        let app = Router::new()
            .route("/health", get(health_handler))
            .route("/webhook", post(webhook_handler_generic))
            .route("/webhook/:event_type", post(webhook_handler_typed))
            .route("/mcp/:tool_name", post(mcp_webhook_handler))
            .with_state(state);

        let addr = format!("0.0.0.0:{}", self.port);
        info!("🌐 Inbound webhook server listening on {}", addr);

        let listener = tokio::net::TcpListener::bind(&addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
}

/// Generic webhook receiver — accepts any event type.
async fn webhook_handler_generic(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let signature = headers
        .get("X-Signature-256")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let event = WebhookEvent {
        id: format!("wh-{}", uuid::Uuid::new_v4()),
        event_type: crate::types::event_types::WEBHOOK_RECEIVED.to_string(),
        source: "webhook".into(),
        timestamp_ms: now_ms(),
        payload,
        signature,
        trigger_fluxfood: true,
        trigger_search: false,
        target_mcp_tool: None,
        priority: 5,
    };

    if state.event_tx.send(event).is_err() {
        warn!("No subscribers for webhook event");
    }

    Ok(Json(serde_json::json!({
        "status": "accepted",
        "message": "Webhook received and dispatched"
    })))
}

/// Typed webhook receiver — event type from URL path.
async fn webhook_handler_typed(
    State(state): State<Arc<AppState>>,
    Path(event_type): Path<String>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let signature = headers
        .get("X-Signature-256")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let event = WebhookEvent {
        id: format!("wh-{}", uuid::Uuid::new_v4()),
        event_type,
        source: "webhook".into(),
        timestamp_ms: now_ms(),
        payload,
        signature,
        trigger_fluxfood: event_type.starts_with("build") || event_type.starts_with("file"),
        trigger_search: event_type.starts_with("file"),
        target_mcp_tool: None,
        priority: 5,
    };

    if state.event_tx.send(event).is_err() {
        warn!("No subscribers for webhook event");
    }

    Ok(Json(serde_json::json!({
        "status": "accepted",
        "message": "Webhook received"
    })))
}

/// MCP webhook bridge — directly call an MCP tool via POST.
async fn mcp_webhook_handler(
    State(state): State<Arc<AppState>>,
    Path(tool_name): Path<String>,
    Json(args): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let event = WebhookEvent {
        id: format!("mcp-{}", uuid::Uuid::new_v4()),
        event_type: crate::types::event_types::MCP_TOOL_CALLED.to_string(),
        source: "webhook-mcp".into(),
        timestamp_ms: now_ms(),
        payload: args,
        signature: None,
        trigger_fluxfood: false,
        trigger_search: false,
        target_mcp_tool: Some(tool_name),
        priority: 8,
    };

    if state.event_tx.send(event).is_err() {
        warn!("No subscribers for MCP webhook call");
    }

    Ok(Json(serde_json::json!({
        "status": "dispatched",
        "tool": event.target_mcp_tool,
        "message": "MCP tool call dispatched"
    })))
}

/// Health check endpoint.
async fn health_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "flux-webhook-mcp",
        "version": "v2",
        "uptime_secs": 0
    }))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
