//! flux-ue-bridge — wire fluxc-mcp telemetry into a real-time visualization.
//!
//! Architecture:
//!
//!   fluxc-mcp ──webhook POST──► /webhook    ─┐
//!                                            ├─► broadcast::Sender<Event>
//!   (anyone)  ──HTTP GET──────► /workspace  ─┘    │
//!                                                 ▼
//!                                       WebSocket clients
//!                                       (UE5 + React app)
//!
//! Three endpoints:
//!   * GET  /v1/workspace   — JSON dump of `flux_quantum_architect`
//!                            output. Clients call this on connect to
//!                            populate their scene.
//!   * GET  /v1/events      — WebSocket. Server pushes typed JSON events
//!                            (see EventKind below) as they arrive.
//!   * POST /v1/webhook     — Receives fluxc-mcp webhook calls. Validates,
//!                            normalizes, broadcasts.
//!
//! Start the bridge, then register it as a webhook in flux-mcp:
//!
//!   $ flux-ue-bridge --listen 127.0.0.1:9989
//!   $ # in another shell / via MCP:
//!   $ mcp__fluxc__flux_webhook_register \
//!       id="ue-bridge" \
//!       url="http://127.0.0.1:9989/v1/webhook" \
//!       secret="changeme" \
//!       events=["build_complete","build_failed","test_complete",
//!               "iterate_complete","batch_complete"]

use anyhow::{Context, Result};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

mod workspace;

#[derive(Parser, Debug)]
#[command(name = "flux-ue-bridge")]
#[command(version, about = "Flux UE/Web telemetry bridge", long_about = None)]
struct Cli {
    /// TCP socket to listen on. WebSocket + HTTP share the port via
    /// minimal protocol sniffing on the request line.
    #[arg(long, default_value = "127.0.0.1:9989")]
    listen: SocketAddr,

    /// Path to a `flux_quantum_architect` JSON snapshot. If absent, the
    /// /v1/workspace endpoint returns an empty crate list and the
    /// browser populates lazily from event traffic.
    #[arg(long)]
    workspace_json: Option<String>,
}

/// One event broadcast to all connected viewers. The fluxc-mcp webhook
/// payload is normalized into one of these variants — viewers don't
/// have to know the raw upstream shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    /// fluxc-mcp built or check-built a crate.
    #[serde(rename = "iterate_complete")]
    IterateComplete {
        package: String,
        success: bool,
        compile_ms: u64,
        cache_rate: f64,
    },
    /// Tests finished for a crate.
    #[serde(rename = "test_complete")]
    TestComplete {
        package: String,
        passed: u64,
        failed: u64,
        elapsed_ms: u64,
    },
    /// A build failed loudly — distinct from a compile-ok / test-fail.
    #[serde(rename = "build_failed")]
    BuildFailed {
        package: String,
        compile_ms: u64,
        error_excerpt: Option<String>,
    },
    /// Connection-level hello sent to a freshly-connected client.
    #[serde(rename = "hello")]
    Hello {
        bridge_version: String,
        workspace_path: String,
    },
    /// Sent every 5s so disconnected sockets get dropped fast.
    #[serde(rename = "heartbeat")]
    Heartbeat { now_unix: u64 },
}

#[derive(Clone)]
struct AppState {
    tx: broadcast::Sender<Event>,
    workspace_json: Arc<parking_lot::RwLock<String>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();

    let (tx, _rx) = broadcast::channel::<Event>(1024);

    let _ = workspace::synthesize; // module is wired up; keep importer warm.

    let workspace_json = if let Some(path) = cli.workspace_json.as_deref() {
        match std::fs::read_to_string(path) {
            Ok(s) if s.trim_start().starts_with('{') => s,
            Ok(_) => {
                warn!("workspace JSON {path} doesn't look like JSON; synthesizing from disk");
                synthesize_workspace_json()
            }
            Err(e) => {
                warn!("could not read workspace JSON {path}: {e}; synthesizing from disk");
                synthesize_workspace_json()
            }
        }
    } else {
        synthesize_workspace_json()
    };

    let state = AppState {
        tx: tx.clone(),
        workspace_json: Arc::new(parking_lot::RwLock::new(workspace_json)),
    };

    // Heartbeat task — keeps WS clients warm + lets servers detect dead clients.
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            loop {
                interval.tick().await;
                let _ = tx.send(Event::Heartbeat { now_unix: now_unix() });
            }
        });
    }

    let listener = TcpListener::bind(cli.listen)
        .await
        .with_context(|| format!("binding {}", cli.listen))?;
    info!("flux-ue-bridge listening on http://{} (POST /v1/webhook, GET /v1/events ws://, GET /v1/workspace)", cli.listen);

    loop {
        let (socket, peer) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(socket, peer, state).await {
                warn!("client {peer} closed: {e}");
            }
        });
    }
}

/// Routes one incoming connection. We sniff the first request line to
/// decide between WebSocket upgrade, plain HTTP GET, or POST.
async fn handle(socket: TcpStream, peer: SocketAddr, state: AppState) -> Result<()> {
    // Peek at the first request line so we can dispatch without a full
    // HTTP framework. ~200 byte buffer is plenty for the request line +
    // first headers.
    let mut peek_buf = [0u8; 512];
    let n = match socket.peek(&mut peek_buf).await {
        Ok(0) => return Ok(()),
        Ok(n) => n,
        Err(e) => return Err(e.into()),
    };
    let preview = String::from_utf8_lossy(&peek_buf[..n]);
    let first_line = preview.lines().next().unwrap_or("");
    info!("client {peer} → {first_line}");

    if first_line.starts_with("POST /v1/webhook") {
        return handle_webhook(socket, state).await;
    }
    if first_line.starts_with("GET /v1/workspace") {
        return handle_workspace(socket, state).await;
    }
    if first_line.starts_with("GET /v1/events") || first_line.starts_with("GET /v1/ws") {
        return handle_websocket(socket, state).await;
    }
    if first_line.starts_with("GET /") {
        // Health / index page.
        let _ = serve_index(socket).await;
        return Ok(());
    }
    let _ = serve_404(socket).await;
    Ok(())
}

#[flux_api::api(POST, "/v1/webhook", summary = "Receive a fluxc-mcp webhook event and broadcast to subscribers")]
async fn handle_webhook(mut socket: TcpStream, state: AppState) -> Result<()> {
    let body = read_http_body(&mut socket).await.unwrap_or_default();
    let json: serde_json::Value =
        serde_json::from_slice(&body).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(event) = normalize_webhook(&json) {
        let _ = state.tx.send(event);
    }
    let resp = b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\n{\"ok\":1}\n";
    let _ = socket.write_all(resp).await;
    Ok(())
}

/// Map fluxc-mcp's webhook envelope to our typed Event. We tolerate
/// missing fields — the upstream schema has wobbled between versions.
fn normalize_webhook(envelope: &serde_json::Value) -> Option<Event> {
    let event = envelope.get("event").and_then(|v| v.as_str())?;
    let data = envelope.get("data");
    let s = |k: &str| -> Option<String> {
        data?.get(k).and_then(|v| v.as_str()).map(String::from)
    };
    let n = |k: &str| -> Option<u64> {
        data?
            .get(k)
            .and_then(|v| v.as_u64())
            .or_else(|| data?.get(k).and_then(|v| v.as_f64()).map(|f| f as u64))
    };
    let b = |k: &str| -> Option<bool> { data?.get(k).and_then(|v| v.as_bool()) };
    let f = |k: &str| -> Option<f64> { data?.get(k).and_then(|v| v.as_f64()) };

    match event {
        "iterate_complete" => Some(Event::IterateComplete {
            package: s("package").unwrap_or_else(|| "?".into()),
            success: b("success").unwrap_or(false),
            compile_ms: n("compile_ms").unwrap_or(0),
            cache_rate: f("cache_rate").unwrap_or(0.0),
        }),
        "test_complete" => Some(Event::TestComplete {
            package: s("package").unwrap_or_else(|| "?".into()),
            passed: n("passed").unwrap_or(0),
            failed: n("failed").unwrap_or(0),
            elapsed_ms: n("elapsed_ms").unwrap_or(0),
        }),
        "build_failed" => Some(Event::BuildFailed {
            package: s("package").unwrap_or_else(|| "?".into()),
            compile_ms: n("compile_ms").unwrap_or(0),
            error_excerpt: s("error_excerpt"),
        }),
        _ => None,
    }
}

#[flux_api::api(GET, "/v1/workspace", summary = "Workspace topology snapshot (crate list with score/loc)")]
async fn handle_workspace(mut socket: TcpStream, state: AppState) -> Result<()> {
    let body = state.workspace_json.read().clone();
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = socket.write_all(resp.as_bytes()).await;
    Ok(())
}

#[flux_api::api(GET, "/v1/events", summary = "WebSocket: live event stream from the bridge")]
async fn handle_websocket(socket: TcpStream, state: AppState) -> Result<()> {
    let ws = tokio_tungstenite::accept_async(socket)
        .await
        .context("ws handshake")?;
    let (mut sink, mut stream) = ws.split();
    let mut rx = state.tx.subscribe();

    // Hello.
    let hello = Event::Hello {
        bridge_version: env!("CARGO_PKG_VERSION").into(),
        workspace_path: std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".into()),
    };
    let _ = sink
        .send(Message::Text(serde_json::to_string(&hello)?))
        .await;

    loop {
        tokio::select! {
            ev = rx.recv() => {
                let Ok(ev) = ev else { break };
                let json = serde_json::to_string(&ev)?;
                if sink.send(Message::Text(json)).await.is_err() { break; }
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(p))) => { let _ = sink.send(Message::Pong(p)).await; }
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
        }
    }
    Ok(())
}

async fn read_http_body(socket: &mut TcpStream) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];
    let mut header_end = None;
    loop {
        let n = socket.read(&mut tmp).await?;
        if n == 0 { break; }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(idx) = find_subslice(&buf, b"\r\n\r\n") {
            header_end = Some(idx + 4);
            break;
        }
    }
    let Some(end) = header_end else { return Ok(Vec::new()); };
    let headers = &buf[..end];
    let mut content_length = 0usize;
    for line in std::str::from_utf8(headers).unwrap_or("").lines() {
        if let Some(rest) = line
            .to_ascii_lowercase()
            .strip_prefix("content-length:")
        {
            content_length = rest.trim().parse().unwrap_or(0);
        }
    }
    let already = buf.len() - end;
    if already >= content_length {
        return Ok(buf[end..end + content_length].to_vec());
    }
    let mut remaining = content_length - already;
    let mut body = buf[end..].to_vec();
    while remaining > 0 {
        let n = socket.read(&mut tmp).await?;
        if n == 0 { break; }
        let take = remaining.min(n);
        body.extend_from_slice(&tmp[..take]);
        remaining -= take;
    }
    Ok(body)
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

async fn serve_index(mut socket: TcpStream) -> Result<()> {
    let body = include_str!("../static/index.html");
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = socket.write_all(resp.as_bytes()).await;
    Ok(())
}

async fn serve_404(mut socket: TcpStream) -> Result<()> {
    let resp = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
    let _ = socket.write_all(resp).await;
    Ok(())
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Synthesize a workspace JSON. Resolves the workspace root via the canonical
/// `fluxc_core::version::workspace_root()` (`Q_FLUX_WORKSPACE` env →
/// `current_exe()` parent → cwd → `.`), so the bridge survives launchers
/// that inherit a parent cwd outside the Flux tree (which is how the
/// parked v0.11.x instance ended up reporting `crates: 0`).
fn synthesize_workspace_json() -> String {
    let probe = fluxc_core::version::workspace_root();
    if probe.join("crates").is_dir() {
        let ws = workspace::synthesize(&probe);
        info!(
            "synthesized workspace from {} — {} crates",
            probe.display(),
            ws.crates.len()
        );
        return serde_json::to_string(&ws)
            .unwrap_or_else(|_| "{\"crates\":[]}".into());
    }
    // Last-resort fallback: walk up from cwd (preserves the v0.11.x
    // behaviour for anyone who relied on the cwd-walk path).
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let mut p = cwd.clone();
    loop {
        if p.join("crates").is_dir() {
            let ws = workspace::synthesize(&p);
            info!(
                "synthesized workspace from {} (cwd fallback) — {} crates",
                p.display(),
                ws.crates.len()
            );
            return serde_json::to_string(&ws)
                .unwrap_or_else(|_| "{\"crates\":[]}".into());
        }
        if !p.pop() {
            break;
        }
    }
    "{\"crates\":[]}".to_string()
}
