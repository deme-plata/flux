//! flux-game-log — game-event sink for flux-arena.
//!
//! Accepts HTTP POSTs on `/v1/game` whose body is a JSON envelope of the form:
//!
//! ```json
//! { "event": "kill_event",
//!   "timestamp": 1780053941,
//!   "seq": 8,
//!   "source": "flux-arena-server v0.1.0",
//!   "data": { "killer": "Viktor", "victim": "Torbjørn", "weapon": "Rifle",
//!             "headshot": true, "distance_cm": 3340 } }
//! ```
//!
//! Each accepted event is wrapped in an ingest record and appended as one
//! line to a JSONL file (default `/tmp/flux-game-events.jsonl`). Tail-based
//! consumers — `narrate-match.sh` in `bundle/server-deploy/`, the Compile
//! Garden, the `dogfooding-during-play.md` loop — read from there.
//!
//! Also exposes `GET /healthz` returning a small JSON status doc for
//! liveness probes.
//!
//! Replaces the dev-loop Python prototype at
//! `bundle/server-deploy/flux-game-event-logger.py`. Same wire format,
//! same on-disk format — a drop-in replacement.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::fs::OpenOptions;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::Duration;
use tracing::{debug, error, info, warn};

#[derive(Parser, Debug, Clone)]
#[command(name = "flux-game-log", version, about = "POST /v1/game → JSONL sink for flux-arena")]
struct Cli {
    /// Bind address.
    #[arg(long, env = "FLUX_GAME_LOG_LISTEN", default_value = "127.0.0.1:9990")]
    listen: SocketAddr,

    /// On-disk JSONL path. Append-only. Created if missing.
    #[arg(long, env = "FLUX_GAME_LOG_PATH", default_value = "/tmp/flux-game-events.jsonl")]
    path: PathBuf,

    /// Optional upstream URL to forward each event to as a fire-and-forget
    /// POST (e.g. `http://127.0.0.1:9989/v1/webhook` to fan out to
    /// `flux-ue-bridge`'s WebSocket subscribers). Empty disables.
    #[arg(long, env = "FLUX_GAME_LOG_FORWARD")]
    forward: Option<String>,

    /// Drop the connection if the body exceeds this many bytes. Belt-and-
    /// suspenders against runaway clients; the real game posts a few KB
    /// per event at most.
    #[arg(long, env = "FLUX_GAME_LOG_MAX_BODY", default_value_t = 256 * 1024)]
    max_body_bytes: usize,
}

/// Process-global counters surfaced via `/healthz`.
#[derive(Debug, Default)]
struct Metrics {
    events_accepted: AtomicU64,
    events_rejected: AtomicU64,
    bytes_written:   AtomicU64,
    forward_errors:  AtomicU64,
    started_at_unix: AtomicU64,
}

#[derive(Clone)]
struct AppState {
    cli:     Arc<Cli>,
    metrics: Arc<Metrics>,
    /// Tokio mutex around the async file handle so concurrent connections
    /// never produce torn writes. The handle is opened append + 0o644.
    file:    Arc<AsyncMutex<tokio::fs::File>>,
    /// Reqwest-free POST: we hand-rolled HTTP everywhere else in the
    /// project. Store the parsed forward target once so we don't re-parse
    /// on every event.
    forward_target: Arc<Mutex<Option<ForwardTarget>>>,
}

#[derive(Clone, Debug)]
struct ForwardTarget {
    host: String,
    port: u16,
    path: String,
}

impl ForwardTarget {
    fn parse(url: &str) -> Result<Self> {
        // Tiny URL parser. We only accept `http://host:port/path` because
        // the upstream is always localhost; no scheme negotiation needed.
        let rest = url.strip_prefix("http://").context("FORWARD must start with http://")?;
        let (authority, path) = rest.split_once('/').map(|(a, p)| (a, format!("/{p}"))).unwrap_or((rest, "/".to_string()));
        let (host, port_s) = authority.split_once(':').context("FORWARD must include :port")?;
        let port: u16 = port_s.parse().context("FORWARD port not a number")?;
        Ok(Self { host: host.to_string(), port, path })
    }
}

/// Envelope we serialize to disk. Wraps the client's payload with an
/// ingest timestamp + the remote socket for forensic value.
#[derive(Serialize, Deserialize, Debug)]
struct Record<'a> {
    ingest_t: i64,
    remote:   String,
    path:     &'a str,
    #[serde(borrow)]
    payload:  &'a serde_json::value::RawValue,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,hyper=warn")),
        )
        .init();

    let cli = Arc::new(Cli::parse());

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&cli.path)
        .await
        .with_context(|| format!("opening {}", cli.path.display()))?;
    info!(path=%cli.path.display(), "opened append-mode log");

    let forward_target = match cli.forward.as_deref() {
        Some(s) if !s.is_empty() => Some(ForwardTarget::parse(s)?),
        _ => None,
    };
    if let Some(t) = &forward_target {
        info!(host=%t.host, port=t.port, path=%t.path, "forwarding enabled");
    }

    let metrics = Arc::new(Metrics::default());
    metrics
        .started_at_unix
        .store(unix_now() as u64, Ordering::Relaxed);

    let state = AppState {
        cli: cli.clone(),
        metrics,
        file: Arc::new(AsyncMutex::new(file)),
        forward_target: Arc::new(Mutex::new(forward_target)),
    };

    let listener = TcpListener::bind(cli.listen)
        .await
        .with_context(|| format!("binding {}", cli.listen))?;
    info!(addr=%cli.listen, "flux-game-log listening — POST /v1/game, GET /healthz");

    let accept_loop = async {
        loop {
            let (socket, peer) = match listener.accept().await {
                Ok(p) => p,
                Err(e) => {
                    warn!(error=%e, "accept failed; backing off 100ms");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let state = state.clone();
            tokio::spawn(async move {
                if let Err(e) = handle(socket, peer, state).await {
                    debug!(peer=%peer, error=%e, "client closed");
                }
            });
        }
    };

    // Graceful shutdown on ctrl-c / SIGTERM. Drops the accept loop; in-
    // flight connections finish on their own and the file is fsync'd in
    // the Drop impl of tokio::fs::File on the way out.
    tokio::select! {
        _ = accept_loop => {},
        _ = shutdown_signal() => {
            info!("shutdown signal — finishing in-flight requests, then exiting");
        }
    }
    Ok(())
}

async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = term.recv()             => {},
    }
}

/// Dispatch one connection. Hand-rolled HTTP — matches the project style.
async fn handle(mut socket: TcpStream, peer: SocketAddr, state: AppState) -> Result<()> {
    let mut head = Vec::with_capacity(1024);
    let mut buf = [0u8; 1024];
    let body_start;
    loop {
        let n = socket.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        head.extend_from_slice(&buf[..n]);
        if let Some(pos) = find_double_crlf(&head) {
            body_start = pos + 4;
            break;
        }
        if head.len() > 16 * 1024 {
            return reply(&mut socket, 431, b"headers too large").await;
        }
    }

    // Parse request line + headers.
    let head_str = std::str::from_utf8(&head[..body_start - 4]).unwrap_or("");
    let mut lines = head_str.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let (method, target) = request_line.split_once(' ')
        .and_then(|(m, rest)| rest.split_once(' ').map(|(t, _)| (m, t)))
        .unwrap_or(("", ""));

    let mut content_length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
    }
    if content_length > state.cli.max_body_bytes {
        state.metrics.events_rejected.fetch_add(1, Ordering::Relaxed);
        return reply(&mut socket, 413, b"body too large").await;
    }

    // Drain body — anything already in `head` after the CRLFCRLF counts.
    let mut body = head[body_start..].to_vec();
    while body.len() < content_length {
        let n = socket.read(&mut buf).await?;
        if n == 0 { break; }
        body.extend_from_slice(&buf[..n]);
    }
    body.truncate(content_length);

    match (method, target_path(target)) {
        ("POST", "/v1/game")  => handle_post(&mut socket, peer, &body, state).await,
        ("GET",  "/healthz")  => handle_health(&mut socket, state).await,
        ("GET",  "/")         => reply(&mut socket, 200, b"flux-game-log up -- POST /v1/game\n").await,
        _                     => reply(&mut socket, 404, b"not found\n").await,
    }
}

/// Strip a query-string suffix from a request target.
fn target_path(t: &str) -> &str {
    t.split_once('?').map(|(p, _)| p).unwrap_or(t)
}

async fn handle_post(
    socket: &mut TcpStream,
    peer: SocketAddr,
    body: &[u8],
    state: AppState,
) -> Result<()> {
    // Parse so we reject garbage early.
    let payload: &serde_json::value::RawValue = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => {
            state.metrics.events_rejected.fetch_add(1, Ordering::Relaxed);
            return reply(socket, 400, b"{\"ok\":0,\"err\":\"not-json\"}\n").await;
        }
    };

    let record = Record {
        ingest_t: unix_now(),
        remote:   peer.ip().to_string(),
        path:     "/v1/game",
        payload,
    };
    let mut line = match serde_json::to_vec(&record) {
        Ok(v) => v,
        Err(_) => {
            state.metrics.events_rejected.fetch_add(1, Ordering::Relaxed);
            return reply(socket, 500, b"{\"ok\":0}\n").await;
        }
    };
    line.push(b'\n');

    {
        let mut f = state.file.lock().await;
        if let Err(e) = f.write_all(&line).await {
            error!(error=%e, "log write failed");
            return reply(socket, 500, b"{\"ok\":0}\n").await;
        }
    }
    state.metrics.events_accepted.fetch_add(1, Ordering::Relaxed);
    state.metrics.bytes_written.fetch_add(line.len() as u64, Ordering::Relaxed);

    // Fire-and-forget forward, if configured.
    if let Some(target) = state.forward_target.lock().clone() {
        let metrics = state.metrics.clone();
        let raw = payload.get().as_bytes().to_vec();
        tokio::spawn(async move {
            if let Err(e) = forward(target, &raw).await {
                metrics.forward_errors.fetch_add(1, Ordering::Relaxed);
                debug!(error=%e, "forward failed");
            }
        });
    }

    reply(socket, 200, b"{\"ok\":1}\n").await
}

async fn handle_health(socket: &mut TcpStream, state: AppState) -> Result<()> {
    let now = unix_now() as u64;
    let started = state.metrics.started_at_unix.load(Ordering::Relaxed);
    let body = format!(
        "{{\"ok\":1,\"log\":\"{}\",\"uptime_s\":{},\"accepted\":{},\"rejected\":{},\"bytes_written\":{},\"forward_errors\":{}}}\n",
        state.cli.path.display(),
        now.saturating_sub(started),
        state.metrics.events_accepted.load(Ordering::Relaxed),
        state.metrics.events_rejected.load(Ordering::Relaxed),
        state.metrics.bytes_written.load(Ordering::Relaxed),
        state.metrics.forward_errors.load(Ordering::Relaxed),
    );
    reply(socket, 200, body.as_bytes()).await
}

/// Hand-rolled HTTP/1.1 POST. Same convention as the rest of the
/// project — no http-client crate dependency.
async fn forward(target: ForwardTarget, body: &[u8]) -> Result<()> {
    let mut stream = tokio::net::TcpStream::connect((target.host.as_str(), target.port))
        .await
        .with_context(|| format!("forward connect {}:{}", target.host, target.port))?;
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
        path = target.path,
        host = target.host,
        port = target.port,
        len  = body.len(),
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    // We don't actually wait for the response — fire-and-forget. Closing
    // is enough.
    Ok(())
}

async fn reply(socket: &mut TcpStream, status: u16, body: &[u8]) -> Result<()> {
    let phrase = status_phrase(status);
    let head = format!(
        "HTTP/1.1 {status} {phrase}\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
        len = body.len(),
    );
    socket.write_all(head.as_bytes()).await?;
    socket.write_all(body).await?;
    let _ = socket.flush().await;
    Ok(())
}

fn status_phrase(s: u16) -> &'static str {
    match s {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        _   => "OK",
    }
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_target_parses_minimal() {
        let t = ForwardTarget::parse("http://127.0.0.1:9989/v1/webhook").unwrap();
        assert_eq!(t.host, "127.0.0.1");
        assert_eq!(t.port, 9989);
        assert_eq!(t.path, "/v1/webhook");
    }

    #[test]
    fn forward_target_rejects_https() {
        assert!(ForwardTarget::parse("https://x/y").is_err());
    }

    #[test]
    fn forward_target_rejects_missing_port() {
        assert!(ForwardTarget::parse("http://example.com/y").is_err());
    }

    #[test]
    fn find_double_crlf_locates_split() {
        let s = b"POST /v1/game HTTP/1.1\r\nHost: x\r\n\r\n{}";
        let i = find_double_crlf(s).unwrap();
        assert_eq!(&s[..i], b"POST /v1/game HTTP/1.1\r\nHost: x");
    }

    #[test]
    fn target_path_strips_query() {
        assert_eq!(target_path("/v1/game?seq=1"), "/v1/game");
        assert_eq!(target_path("/healthz"), "/healthz");
    }
}
