//! The relay server — REST + WebSocket over a hand-rolled HTTP head parser
//! (flux-ue-bridge idiom; no axum/hyper). Generic over the stream type so the
//! same handler serves plain TCP and rustls TLS (the ue-bridge `peek()` trick
//! only works on TcpStream, so we read the head ourselves instead).
//!
//! Routes:
//!   GET  /v1/health                     — relay status JSON
//!   GET  /v1/events?channel&since&kind&limit — query the log (since = ms cursor)
//!   GET  /v1/channels                   — distinct channels w/ counts
//!   POST /v1/event                      — publish a signed event (id may be empty)
//!   GET  /v1/ws[?channel=X]             — WebSocket: live events out, publishes in
//!   GET  / | /buzz.html                 — embedded web UI
//!
//! All responses carry permissive CORS so the UI can be hosted on quillon.xyz
//! while the API lives on another origin.

use crate::event::BuzzEvent;
use crate::store::EventStore;
use anyhow::{bail, Context, Result};
use base64::Engine;
use futures::{SinkExt, StreamExt};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

const UI_HTML: &str = include_str!("../ui/buzz.html");
const MAX_HEAD: usize = 64 * 1024;
const MAX_BODY: usize = 1024 * 1024;
const MAX_CONTENT: usize = 64 * 1024;

pub struct RelayState {
    pub store: Mutex<EventStore>,
    pub tx: broadcast::Sender<BuzzEvent>,
    pub started: Instant,
}

impl RelayState {
    pub fn new(store: EventStore) -> Arc<Self> {
        let (tx, _) = broadcast::channel(1024);
        Arc::new(Self { store: Mutex::new(store), tx, started: Instant::now() })
    }
}

/// Verify + store + broadcast one event. Fills a missing id (browser clients
/// can't compute blake3). Returns the final event id.
pub fn publish(state: &RelayState, mut ev: BuzzEvent) -> Result<String> {
    if ev.content.len() > MAX_CONTENT {
        bail!("content too large ({} > {} bytes)", ev.content.len(), MAX_CONTENT);
    }
    if ev.tags.len() > 64 {
        bail!("too many tags");
    }
    if ev.id.is_empty() {
        ev.id = ev.compute_id();
    }
    ev.verify()?;
    let fresh = state
        .store
        .lock()
        .expect("store lock poisoned")
        .append(ev.clone())?;
    if fresh {
        let _ = state.tx.send(ev.clone());
    }
    Ok(ev.id)
}

/// Plain-TCP accept loop.
pub async fn run_plain(listener: TcpListener, state: Arc<RelayState>) -> Result<()> {
    info!("flux-buzz relay listening on http://{}", listener.local_addr()?);
    loop {
        let (socket, peer) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(socket, state).await {
                warn!("client {peer}: {e:#}");
            }
        });
    }
}

/// TLS accept loop (serve the API on a public port with a real cert while the
/// UI is hosted from the main site — no mixed-content).
pub async fn run_tls(
    listener: TcpListener,
    cert_path: &str,
    key_path: &str,
    state: Arc<RelayState>,
) -> Result<()> {
    use tokio_rustls::rustls;
    // Both ring (reqwest) and aws-lc-rs (tokio-rustls) sit in the dep graph,
    // so rustls cannot auto-select a process CryptoProvider — pick one.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let certs: Vec<_> = rustls_pemfile::certs(&mut std::io::BufReader::new(
        std::fs::File::open(cert_path).with_context(|| format!("opening {cert_path}"))?,
    ))
    .collect::<std::result::Result<_, _>>()?;
    let key = rustls_pemfile::private_key(&mut std::io::BufReader::new(
        std::fs::File::open(key_path).with_context(|| format!("opening {key_path}"))?,
    ))?
    .context("no private key found")?;
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    info!("flux-buzz relay listening on https://{}", listener.local_addr()?);
    loop {
        let (socket, peer) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let state = state.clone();
        tokio::spawn(async move {
            match acceptor.accept(socket).await {
                Ok(tls) => {
                    if let Err(e) = handle_conn(tls, state).await {
                        warn!("tls client {peer}: {e:#}");
                    }
                }
                Err(e) => warn!("tls handshake {peer}: {e}"),
            }
        });
    }
}

struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

async fn handle_conn<S>(mut stream: S, state: Arc<RelayState>) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let req = match read_request(&mut stream).await? {
        Some(r) => r,
        None => return Ok(()),
    };

    match (req.method.as_str(), req.path.as_str()) {
        ("OPTIONS", _) => {
            write_raw(
                &mut stream,
                "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: content-type\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await
        }
        ("GET", "/v1/health") => {
            let (events, channels) = {
                let store = state.store.lock().expect("store lock");
                (store.len(), store.channels().len())
            };
            let body = serde_json::json!({
                "name": "flux-buzz",
                "version": env!("CARGO_PKG_VERSION"),
                "events": events,
                "channels": channels,
                "uptime_s": state.started.elapsed().as_secs(),
            });
            write_json(&mut stream, 200, &body.to_string()).await
        }
        ("GET", "/v1/events") => {
            let since: u64 = req.query.get("since").and_then(|s| s.parse().ok()).unwrap_or(0);
            let kind: Option<u32> = req.query.get("kind").and_then(|s| s.parse().ok());
            let limit: usize = req
                .query
                .get("limit")
                .and_then(|s| s.parse().ok())
                .unwrap_or(200)
                .min(1000);
            let channel = req.query.get("channel").map(|s| s.as_str());
            let events = state
                .store
                .lock()
                .expect("store lock")
                .query(channel, since, kind, limit);
            let body = serde_json::json!({ "events": events });
            write_json(&mut stream, 200, &body.to_string()).await
        }
        ("GET", "/v1/channels") => {
            let chans: Vec<serde_json::Value> = state
                .store
                .lock()
                .expect("store lock")
                .channels()
                .into_iter()
                .map(|(name, count, last)| {
                    serde_json::json!({"name": name, "events": count, "last_activity": last})
                })
                .collect();
            let body = serde_json::json!({ "channels": chans });
            write_json(&mut stream, 200, &body.to_string()).await
        }
        ("POST", "/v1/event") => {
            let ev: BuzzEvent = match serde_json::from_slice(&req.body) {
                Ok(ev) => ev,
                Err(e) => {
                    let body = serde_json::json!({"ok": false, "error": format!("bad event json: {e}")});
                    return write_json(&mut stream, 400, &body.to_string()).await;
                }
            };
            match publish(&state, ev) {
                Ok(id) => {
                    let body = serde_json::json!({"ok": true, "id": id});
                    write_json(&mut stream, 200, &body.to_string()).await
                }
                Err(e) => {
                    let body = serde_json::json!({"ok": false, "error": format!("{e:#}")});
                    write_json(&mut stream, 400, &body.to_string()).await
                }
            }
        }
        ("GET", "/v1/ws") => handle_ws(stream, req, state).await,
        ("GET", "/") | ("GET", "/buzz.html") | ("GET", "/index.html") => {
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-cache\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                UI_HTML.len(),
                UI_HTML
            );
            write_raw(&mut stream, &resp).await
        }
        _ => {
            write_json(&mut stream, 404, r#"{"ok":false,"error":"not found"}"#).await
        }
    }
}

/// Manual WebSocket upgrade (RFC 6455 accept key), then hand the raw stream to
/// tungstenite. Works over both TCP and TLS since we never need `peek()`.
async fn handle_ws<S>(mut stream: S, req: Request, state: Arc<RelayState>) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let key = req
        .headers
        .get("sec-websocket-key")
        .context("missing Sec-WebSocket-Key")?;
    let mut sha = Sha1::new();
    sha.update(key.as_bytes());
    sha.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let accept = base64::engine::general_purpose::STANDARD.encode(sha.finalize());
    let resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await?;

    let ws = tokio_tungstenite::WebSocketStream::from_raw_socket(stream, Role::Server, None).await;
    let (mut sink, mut source) = ws.split();
    let mut rx = state.tx.subscribe();
    let channel_filter = req.query.get("channel").cloned();

    loop {
        tokio::select! {
            ev = rx.recv() => {
                let Ok(ev) = ev else { break };
                if let Some(ref c) = channel_filter {
                    if ev.channel() != Some(c.as_str()) { continue; }
                }
                let json = serde_json::to_string(&ev)?;
                if sink.send(Message::Text(json)).await.is_err() { break; }
            }
            msg = source.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let reply = match serde_json::from_str::<BuzzEvent>(&text)
                            .map_err(anyhow::Error::from)
                            .and_then(|ev| publish(&state, ev))
                        {
                            Ok(id) => serde_json::json!({"ok": true, "id": id}),
                            Err(e) => serde_json::json!({"ok": false, "error": format!("{e:#}")}),
                        };
                        if sink.send(Message::Text(reply.to_string())).await.is_err() { break; }
                    }
                    Some(Ok(Message::Ping(p))) => { let _ = sink.send(Message::Pong(p)).await; }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
        }
    }
    Ok(())
}

/// Read + parse one HTTP request (head + Content-Length body). Returns None on
/// an immediately-closed connection.
async fn read_request<S>(stream: &mut S) -> Result<Option<Request>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buf: Vec<u8> = Vec::with_capacity(2048);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > MAX_HEAD {
            bail!("request head too large");
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            if buf.is_empty() {
                return Ok(None);
            }
            bail!("connection closed mid-head");
        }
        buf.extend_from_slice(&chunk[..n]);
    };

    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next().context("empty request")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().context("no method")?.to_uppercase();
    let target = parts.next().context("no path")?;
    let (path, query_str) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q),
        None => (target.to_string(), ""),
    };
    let query = parse_query(query_str);
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let mut body = buf[head_end + 4..].to_vec();
    let content_length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if content_length > MAX_BODY {
        bail!("body too large");
    }
    while body.len() < content_length {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            bail!("connection closed mid-body");
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);

    Ok(Some(Request { method, path, query, headers, body }))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn parse_query(q: &str) -> HashMap<String, String> {
    q.split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (url_decode(k), url_decode(v)))
        .collect()
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                if let Ok(byte) =
                    u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
                {
                    out.push(byte);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

async fn write_json<S: AsyncWrite + Unpin>(stream: &mut S, status: u16, body: &str) -> Result<()> {
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "OK",
    };
    let resp = format!(
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: content-type\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nCache-Control: no-cache\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    write_raw(stream, &resp).await
}

async fn write_raw<S: AsyncWrite + Unpin>(stream: &mut S, resp: &str) -> Result<()> {
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Identity, KIND_CHAT};

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "flux-buzz-relay-test-{}-{}-{}",
            tag,
            std::process::id(),
            crate::event::now_ms()
        ))
    }

    /// End-to-end over real HTTP: boot the relay on an ephemeral port, POST a
    /// signed event, read it back, and assert the CONTENT matches (verify
    /// outcomes, not just status codes).
    #[tokio::test]
    async fn e2e_post_then_query() {
        let dir = tmp_dir("e2e");
        let store = EventStore::open(&dir).unwrap();
        let state = RelayState::new(store);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(run_plain(listener, state));

        let alice = Identity::generate();
        let ev = alice.sign_event(
            KIND_CHAT,
            vec![vec!["c".into(), "general".into()]],
            "e2e hello".into(),
        );

        let client = reqwest::Client::new();
        let resp: serde_json::Value = client
            .post(format!("http://{addr}/v1/event"))
            .json(&ev)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(resp["ok"], true, "publish must succeed: {resp}");
        assert_eq!(resp["id"], serde_json::json!(ev.id));

        let got: serde_json::Value = client
            .get(format!("http://{addr}/v1/events?channel=general"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let events = got["events"].as_array().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["content"], "e2e hello");
        assert_eq!(events[0]["pubkey"], serde_json::json!(alice.pubkey_hex()));

        // A tampered event must be rejected end-to-end.
        let mut forged = ev.clone();
        forged.content = "forged".into();
        forged.id = forged.compute_id();
        let resp: serde_json::Value = client
            .post(format!("http://{addr}/v1/event"))
            .json(&forged)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(resp["ok"], false, "forged event must be rejected");

        // The browser path: id left empty, relay fills it.
        let bob = Identity::generate();
        let mut no_id = bob.sign_event(KIND_CHAT, vec![vec!["c".into(), "general".into()]], "from browser".into());
        no_id.id = String::new();
        let resp: serde_json::Value = client
            .post(format!("http://{addr}/v1/event"))
            .json(&no_id)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(resp["ok"], true, "empty-id event must be accepted: {resp}");

        let health: serde_json::Value = client
            .get(format!("http://{addr}/v1/health"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(health["events"], 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
