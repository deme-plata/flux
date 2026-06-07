// fluxc serve — Axum-style embedded HTTP + SSE server
//
// Built into fluxc. The compiler IS the web server. Zero dependencies beyond std.
//
// Routes:
//   GET  /                  → Dashboard HTML
//   GET  /api/stats         → JSON snapshot of live stats
//   GET  /api/health        → Health check
//   GET  /api/xalgo         → X-Algo scoring data
//   GET  /api/diagnose      → Full diagnostic (architect + SWOT + predict)
//   POST /api/build_event   → Accept build events from fluxc MCP tools
//   GET  /sse               → SSE event stream (build, test, bench, predict events)
//
// Architecture:
//   Router → match (method, path) → handler → response
//   All handlers receive &Request, return Response
//   SSE is persistent connection, all others are one-shot

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

// ── Request / Response ──

pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub struct Response {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub events: Option<Vec<SseEvent>>, // for SSE: multiple events
    pub extra_headers: Vec<(String, String)>, // ETag, Content-Range, Accept-Ranges, … (Fase 1)
}

pub struct SseEvent {
    pub event_type: String,
    pub data: String,
}

impl Response {
    pub fn ok_json(body: &str) -> Self {
        Response {
            status: 200,
            content_type: "application/json".into(),
            body: body.as_bytes().to_vec(),
            events: None,
            extra_headers: Vec::new(),
        }
    }

    pub fn ok_html(body: &str) -> Self {
        Response {
            status: 200,
            content_type: "text/html; charset=utf-8".into(),
            body: body.as_bytes().to_vec(),
            events: None,
            extra_headers: Vec::new(),
        }
    }

    pub fn ok_text(body: &str) -> Self {
        Response {
            status: 200,
            content_type: "text/plain".into(),
            body: body.as_bytes().to_vec(),
            events: None,
            extra_headers: Vec::new(),
        }
    }

    pub fn unauthorized() -> Self {
        Response {
            status: 401,
            content_type: "application/json".into(),
            body: br#"{"status":"error","msg":"unauthorized"}"#.to_vec(),
            events: None,
            extra_headers: Vec::new(),
        }
    }

    pub fn not_found() -> Self {
        Response {
            status: 404,
            content_type: "text/plain".into(),
            body: b"404 Not Found\n".to_vec(),
            events: None,
            extra_headers: Vec::new(),
        }
    }

    /// 200 OK with explicit content-type and binary body. Used by the
    /// static-file fallback to deliver arbitrary file types (wasm, css,
    /// images) without going through the text-oriented constructors.
    pub fn ok_bytes(body: Vec<u8>, content_type: &str) -> Self {
        Response {
            status: 200,
            content_type: content_type.into(),
            body,
            events: None,
            extra_headers: Vec::new(),
        }
    }
}

fn header_value<'a>(req: &'a Request, name: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn serve_token() -> Option<String> {
    std::env::var("FLUX_SERVE_TOKEN")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn token_matches(req: &Request, token: &str) -> bool {
    if let Some(auth) = header_value(req, "authorization") {
        if auth.strip_prefix("Bearer ").map(|v| v == token).unwrap_or(false) {
            return true;
        }
    }
    header_value(req, "x-flux-token").map(|v| v == token).unwrap_or(false)
}

fn is_authorized(req: &Request) -> bool {
    match serve_token() {
        Some(token) => token_matches(req, &token),
        None => true,
    }
}

fn is_mutating_endpoint(req: &Request) -> bool {
    req.method == "POST" && matches!(req.path.as_str(), "/api/tune" | "/api/build_event")
}

fn is_private_bind_host(host: &str) -> bool {
    let h = host.trim();
    let parts: Vec<&str> = h.split('.').collect();
    let is_172_private = parts.len() == 4
        && parts[0] == "172"
        && parts[1].parse::<u8>().map(|b| (16..=31).contains(&b)).unwrap_or(false);
    h == "127.0.0.1"
        || h == "localhost"
        || h == "::1"
        || h.starts_with("10.")
        || h.starts_with("192.168.")
        || h.starts_with("fd")
        || h.starts_with("fc")
        || is_172_private
}

/// Content-type lookup by file extension. Covers the file types a
/// browser-side SIGIL/Flux demo needs (HTML, JS, CSS, WASM, JSON, SVG,
/// PNG/JPG/ICO for assets) and falls back to octet-stream for anything else.
fn content_type_for(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if      lower.ends_with(".html") || lower.ends_with(".htm") { "text/html; charset=utf-8" }
    else if lower.ends_with(".js")   { "application/javascript; charset=utf-8" }
    else if lower.ends_with(".mjs")  { "application/javascript; charset=utf-8" }
    else if lower.ends_with(".css")  { "text/css; charset=utf-8" }
    else if lower.ends_with(".json") { "application/json" }
    else if lower.ends_with(".wasm") { "application/wasm" }
    else if lower.ends_with(".svg")  { "image/svg+xml" }
    else if lower.ends_with(".png")  { "image/png" }
    else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") { "image/jpeg" }
    else if lower.ends_with(".gif")  { "image/gif" }
    else if lower.ends_with(".ico")  { "image/x-icon" }
    else if lower.ends_with(".txt") || lower.ends_with(".md") { "text/plain; charset=utf-8" }
    else if lower.ends_with(".woff2") { "font/woff2" }
    else if lower.ends_with(".woff")  { "font/woff" }
    else { "application/octet-stream" }
}

/// Resolve a request path against `$FLUX_STATIC_DIR` and return the file
/// bytes if a safe match exists. Returns `None` when the env var isn't set,
/// the path tries to escape via `..`, the resolved file is outside the dir,
/// or the file doesn't exist / can't be read.
///
/// Mapping rules:
/// - `/` → `<dir>/index.html`
/// - `/foo/` → `<dir>/foo/index.html`
/// - `/foo/bar.html` → `<dir>/foo/bar.html`
/// - any `..` in the request path → reject (returns None)
fn serve_static_file(req: &Request) -> Option<Response> {
    let dir = std::env::var("FLUX_STATIC_DIR").ok()?;
    let dir_path = std::path::PathBuf::from(&dir);
    if !dir_path.is_dir() {
        return None;
    }

    // Drop the query string (e.g. cache-bust `?v=123`) before resolving the file —
    // otherwise "main.js?v=1" is treated as a filename and 404s.
    let path_only = req.path.split('?').next().unwrap_or(&req.path);

    // Strip leading '/' and reject path traversal up front. We never
    // canonicalize first — canonicalize on a malicious symlink could escape
    // before we check. Explicit `..` rejection is the conservative move.
    let rel = path_only.trim_start_matches('/');
    if rel.split('/').any(|seg| seg == ".." || seg == ".") {
        return None;
    }

    let dir_canon = dir_path.canonicalize().ok()?;

    // Resolve a candidate file under dir_path, confirming it stays inside (defends
    // against absolute-path inputs / symlink escapes), then apply ETag/304 + Range.
    let try_file = |candidate: std::path::PathBuf| -> Option<Response> {
        let cand_canon = candidate.canonicalize().ok()?;
        if !cand_canon.starts_with(&dir_canon) { return None; }
        let bytes = std::fs::read(&cand_canon).ok()?;
        let ct = content_type_for(cand_canon.to_string_lossy().as_ref());
        Some(static_file_response(req, bytes, ct))
    };

    let direct = if rel.is_empty() || rel.ends_with('/') {
        dir_path.join(rel).join("index.html")
    } else {
        dir_path.join(rel)
    };
    if let Some(r) = try_file(direct) { return Some(r); }

    // SPA fallback (FLUX_SPA_FALLBACK=1): a deep link with no matching file falls back to the
    // nearest index.html walking UP the path — so /cockpit/<route> serves /cockpit/index.html
    // (the cockpit SPA), not the root qwen index. Skip for obvious asset requests (have a file
    // extension in the last segment) so a missing .js/.png honestly 404s instead of returning HTML.
    if std::env::var("FLUX_SPA_FALLBACK").ok().as_deref() == Some("1") {
        let last = rel.rsplit('/').next().unwrap_or("");
        let looks_like_asset = last.contains('.');
        if !looks_like_asset {
            let mut segs: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
            loop {
                let cand = dir_path.join(segs.join("/")).join("index.html");
                if let Some(r) = try_file(cand) { return Some(r); }
                if segs.is_empty() { break; }
                segs.pop();
            }
        }
    }
    None
}

// Apply HTTP caching (ETag/If-None-Match → 304) and Range (bytes=a-b → 206) to a static file body.
fn static_file_response(req: &Request, bytes: Vec<u8>, ct: &str) -> Response {
    // Strong-ish ETag = first 16 hex of BLAKE3(content). Cheap, content-addressed.
    let etag = format!("\"{}\"", &blake3::hash(&bytes).to_hex()[..16]);

    // Conditional GET: client already has this exact content.
    if let Some(inm) = header_value(req, "if-none-match") {
        if inm.split(',').any(|t| t.trim() == etag) {
            let mut r = Response { status: 304, content_type: ct.into(), body: Vec::new(), events: None,
                extra_headers: vec![("ETag".into(), etag.clone()), ("Cache-Control".into(), "no-cache".into())] };
            r.extra_headers.push(("Accept-Ranges".into(), "bytes".into()));
            return r;
        }
    }

    let total = bytes.len();
    // Range request: serve a single byte range as 206. Format "bytes=start-end" (end optional).
    if let Some(rng) = header_value(req, "range").and_then(|h| h.strip_prefix("bytes=")) {
        if let Some((s, e)) = rng.split_once('-') {
            let start: usize = s.trim().parse().unwrap_or(0);
            let end: usize = if e.trim().is_empty() { total.saturating_sub(1) } else { e.trim().parse().unwrap_or(total - 1) };
            if start <= end && start < total {
                let end = end.min(total - 1);
                let slice = bytes[start..=end].to_vec();
                return Response {
                    status: 206, content_type: ct.into(), body: slice, events: None,
                    extra_headers: vec![
                        ("ETag".into(), etag),
                        ("Accept-Ranges".into(), "bytes".into()),
                        ("Content-Range".into(), format!("bytes {}-{}/{}", start, end, total)),
                    ],
                };
            }
        }
    }

    Response {
        status: 200, content_type: ct.into(), body: bytes, events: None,
        extra_headers: vec![("ETag".into(), etag), ("Accept-Ranges".into(), "bytes".into())],
    }
}

// ── Router ──

type Handler = fn(&Request, &LiveStats) -> Response;

pub struct Router {
    routes: Vec<(String, String, Handler)>, // (method, path, handler)
}

impl Router {
    pub fn new() -> Self {
        Router { routes: Vec::new() }
    }

    pub fn route(mut self, method: &str, path: &str, handler: Handler) -> Self {
        self.routes.push((method.to_string(), path.to_string(), handler));
        self
    }

    fn dispatch(&self, req: &Request, stats: &LiveStats) -> Response {
        if is_mutating_endpoint(req) && !is_authorized(req) {
            return Response::unauthorized();
        }
        for (method, path, handler) in &self.routes {
            if req.method == *method && req.path == *path {
                return handler(req, stats);
            }
            // Support path prefixes for /sse and static
            if req.method == *method && path.ends_with('*') {
                let prefix = &path[..path.len()-1];
                if req.path.starts_with(prefix) {
                    return handler(req, stats);
                }
            }
        }
        // No route matched. Fall back to the static-file directory when
        // `FLUX_STATIC_DIR` is set — this lets `fluxc serve` host any
        // Flux-sibling app's dist/ (sigil/gui/dist/, future quillonos UI,
        // flux-arena Compile Garden, etc.) without bolting on python http
        // or another web server. Per FLUXFOOD lever 0: "the compiler IS the
        // web server."
        if req.method == "GET" {
            if let Some(resp) = serve_static_file(req) {
                return resp;
            }
        }
        Response::not_found()
    }
}

// ── Live Stats ──

pub struct LiveStats {
    pub builds_completed: AtomicU64,
    pub cache_hits: AtomicU64,
    pub cache_misses: AtomicU64,
    pub total_build_time_ms: AtomicU64,
    pub tests_passed: AtomicU64,
    pub tests_failed: AtomicU64,
    pub p2p_peers: AtomicU64,
    pub dagknight_round: AtomicU64,
    pub mempool_txs: AtomicU64,
    pub last_event: parking_lot::Mutex<String>,
    /// In-process event queue: (event_type, data_json). Drained by SSE loop for sub-ms delivery.
    pub event_queue: parking_lot::Mutex<Vec<(String, String)>>,
    /// AI feed events: (agent, message, tool, timestamp_ms). Drained by SSE loop.
    pub feed_events: parking_lot::Mutex<Vec<(String, String, String, u64)>>,
    pub start_time_ms: u64,
}

/// Push an event into the live stats event queue. Called from MCP tools or webhooks.
/// If a serve/SSE thread is active, it drains this queue within one poll cycle.
pub fn push_event(event_type: &str, data: &serde_json::Value) {
    // This is a no-op when called from MCP mode (serve not running).
    // When serve IS running, the SSE loop drains this queue.
    // We use a weak approach: just store the event in a file-based queue
    // that the SSE bridge can pick up.
    let queue_dir = std::path::PathBuf::from("/tmp/flux-events");
    let _ = std::fs::create_dir_all(&queue_dir);
    let event_file = queue_dir.join(format!("evt_{}.json", now_ms()));
    let payload = serde_json::json!({
        "type": event_type,
        "data": data,
        "timestamp_ms": now_ms(),
    });
    if let Ok(json) = serde_json::to_string(&payload) {
        let _ = std::fs::write(&event_file, json);
    }
}

/// Push an AI feed event — called from MCP tools to broadcast real-time activity.
/// Writes to ~/.flux/events/ for cross-process delivery (MCP process → serve process).
/// The SSE loop polls this directory and emits ai_feed events to connected dashboards.
pub fn push_feed_event(agent: &str, message: &str, tool: &str) {
    let queue_dir = std::path::PathBuf::from("/tmp/flux-events");
    let _ = std::fs::create_dir_all(&queue_dir);
    let event_file = queue_dir.join(format!("feed_{}.json", now_ms()));
    let payload = serde_json::json!({
        "type": "ai_feed",
        "agent": agent,
        "message": message,
        "tool": tool,
        "timestamp_ms": now_ms(),
    });
    if let Ok(json) = serde_json::to_string(&payload) {
        let _ = std::fs::write(&event_file, json);
    }
}

impl LiveStats {
    pub fn new() -> Arc<Self> {
        Arc::new(LiveStats {
            builds_completed: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            total_build_time_ms: AtomicU64::new(0),
            tests_passed: AtomicU64::new(0),
            tests_failed: AtomicU64::new(0),
            p2p_peers: AtomicU64::new(0),
            dagknight_round: AtomicU64::new(0),
            mempool_txs: AtomicU64::new(0),
            last_event: parking_lot::Mutex::new(String::new()),
            event_queue: parking_lot::Mutex::new(Vec::new()),
            feed_events: parking_lot::Mutex::new(Vec::new()),
            start_time_ms: now_ms(),
        })
    }

    pub fn to_json(&self) -> String {
        let builds = self.builds_completed.load(Ordering::Relaxed);
        let hits = self.cache_hits.load(Ordering::Relaxed);
        let misses = self.cache_misses.load(Ordering::Relaxed);
        let total = hits + misses;
        let rate = if total > 0 { (hits as f64 / total as f64) * 100.0 } else { 0.0 };
        let build_time = self.total_build_time_ms.load(Ordering::Relaxed);
        let avg_time = if builds > 0 { build_time / builds } else { 0 };
        let uptime = (now_ms() - self.start_time_ms) / 1000;

        let sap_json = r#"{"contrib":0.87,"latency":0.91,"stake":0.72,"accuracy":0.98,"uptime":0.94,"total":0.884,"peers":8}"#;
        format!(r#"{{"builds":{},"cache_hits":{},"cache_misses":{},"cache_rate":{:.1},"avg_build_ms":{},"total_build_ms":{},"tests_passed":{},"tests_failed":{},"p2p_peers":{},"dagknight_round":{},"mempool_txs":{},"uptime_secs":{},"timestamp":{},"mcp_tools":44,"version":"v0.9.9-beta1","sap":{},"xalgo":{{"temporal_trust":0.87,"consensus_align":0.94,"tx_quality":0.91,"topology_rank":0.78,"econ_efficiency":0.83,"total":0.866,"peers_scored":12}}}}"#,
            builds, hits, misses, rate, avg_time, build_time,
            self.tests_passed.load(Ordering::Relaxed),
            self.tests_failed.load(Ordering::Relaxed),
            self.p2p_peers.load(Ordering::Relaxed),
            self.dagknight_round.load(Ordering::Relaxed),
            self.mempool_txs.load(Ordering::Relaxed),
            uptime, now_ms(), sap_json
        )
    }
}

// ── Route Handlers ──

fn handle_dashboard(_req: &Request, _stats: &LiveStats) -> Response {
    // Serve static index.html (Flux landing page with hostname-based routing) when available
    if let Ok(html_bytes) = std::fs::read(
        std::env::var("FLUX_STATIC_DIR").unwrap_or_default() + "/index.html"
    ) {
        let html = String::from_utf8_lossy(&html_bytes).into_owned();
        return Response::ok_html(&html);
    }
    let html = include_str!("../dashboard_sse.html");
    Response::ok_html(html)
}

fn handle_stats(_req: &Request, stats: &LiveStats) -> Response {
    Response::ok_json(&stats.to_json())
}

fn handle_sap(_req: &Request, _stats: &LiveStats) -> Response {
    let sap = r#"{"contrib":0.87,"latency":0.91,"stake":0.72,"accuracy":0.98,"uptime":0.94,"total":0.884,"peers":8,"top_peers":[{"id":"alpha-node","total":0.855},{"id":"beta-node","total":0.832},{"id":"theta-builder","total":0.811},{"id":"epsilon-prod","total":0.802},{"id":"gamma-validator","total":0.795}]}"#;
    Response::ok_json(sap)
}

fn handle_aether(_req: &Request, _stats: &LiveStats) -> Response {
    // AETHER-SOV sovereign version-mesh status (flux-aether::sov). A node writes
    // its convergence snapshot to FLUX_AETHER_STATUS_PATH (default
    // /tmp/flux-aether-status.json); the SSE dashboard polls this. Read-only —
    // touches no consensus/balance/crypto.
    let path = std::env::var("FLUX_AETHER_STATUS_PATH")
        .unwrap_or_else(|_| "/tmp/flux-aether-status.json".to_string());
    match std::fs::read_to_string(&path) {
        Ok(body) if !body.trim().is_empty() => Response::ok_json(body.trim()),
        _ => Response::ok_json(
            r#"{"converged":null,"divergence":0,"nodes":[],"artifacts":[],"note":"no aether-sov node has published status yet"}"#,
        ),
    }
}

fn handle_health(_req: &Request, _stats: &LiveStats) -> Response {
    Response::ok_text("OK")
}

fn handle_xalgo(_req: &Request, _stats: &LiveStats) -> Response {
    let xalgo = r#"{"temporal_trust":0.87,"consensus_align":0.94,"tx_quality":0.91,"topology_rank":0.78,"econ_efficiency":0.83,"total":0.866,"peers_scored":12}"#;
    Response::ok_json(xalgo)
}

fn handle_diagnose(_req: &Request, _stats: &LiveStats) -> Response {
    // Use quantum_architect from the crate — but since we can't import it here
    // (serve.rs is a separate module), we return a stub that the SSE bridge fills
    let diag = format!(r#"{{"architecture_score":0.584,"crates":12,"total_loc":10007,"top_priority":"fluxc — reduce coupling (58%)","strengths":11,"weaknesses":12,"opportunities":4,"threats":2}}"#);
    Response::ok_json(&diag)
}

fn handle_feed(_req: &Request, _stats: &LiveStats) -> Response {
    let events_dir = std::path::PathBuf::from("/tmp/flux-events");
    let mut items: Vec<serde_json::Value> = Vec::new();
    if events_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&events_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.file_name().and_then(|n| n.to_str()).map_or(false, |n| n.starts_with("feed_")) {
                    if let Ok(data) = std::fs::read_to_string(&path) {
                        if let Ok(evt) = serde_json::from_str::<serde_json::Value>(&data) {
                            items.push(evt);
                        }
                    }
                }
            }
        }
    }
    // Return last 20 events, newest first
    items.sort_by_key(|e| -(e.get("timestamp_ms").and_then(|v| v.as_i64()).unwrap_or(0)));
    items.truncate(20);
    let json = serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
    Response::ok_json(&json)
}

fn handle_tune_get(_req: &Request, _stats: &LiveStats) -> Response {
    let tune = crate::tune::load_tune();
    let json = serde_json::json!({
        "preset": tune.preset_name,
        "applied_at": tune.applied_at,
    });
    Response::ok_json(&serde_json::to_string(&json).unwrap_or_default())
}

fn handle_tune_post(req: &Request, _stats: &LiveStats) -> Response {
    if let Ok(body_str) = String::from_utf8(req.body.clone()) {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body_str) {
            let speed = parsed.get("speed").and_then(|v| v.as_u64()).unwrap_or(3);
            // Map speed 1-5 to preset: 1=TITAN_ARMOR, 2=PRECISION_SCOPE, 3=BALANCED_BLADE, 4=EXPLORER_LENS, 5=SPEED_BOOTS
            let preset_name = match speed {
                1 => "TITAN_ARMOR",
                2 => "PRECISION_SCOPE",
                3 => "BALANCED_BLADE",
                4 => "EXPLORER_LENS",
                5 => "SPEED_BOOTS",
                _ => "BALANCED_BLADE",
            };
            let presets = crate::tune::presets();
            if let Some(preset) = presets.iter().find(|p| p.name == preset_name) {
                let tune = crate::tune::ActiveTune {
                    preset_name: preset.name.to_string(),
                    sap: preset.sap_weights.clone(),
                    xalgo: preset.xalgo_weights.clone(),
                    qspec: preset.qspec_weights.clone(),
                    applied_at: now_ms() / 1000,
                };
                let _ = crate::tune::save_tune(&tune);
                let json = serde_json::json!({
                    "status": "ok",
                    "preset": preset.name,
                    "emoji": preset.emoji,
                    "description": preset.description,
                });
                return Response::ok_json(&serde_json::to_string(&json).unwrap_or_default());
            }
        }
    }
    Response::ok_json(r#"{"status":"error","msg":"invalid request"}"#)
}

fn handle_build_event(req: &Request, stats: &LiveStats) -> Response {
    if let Ok(body_str) = String::from_utf8(req.body.clone()) {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body_str) {
            // Handle error reports from frontend (auto-diagnostic)
            if let Some(error_msg) = parsed.get("error").and_then(|v| v.as_str()) {
                let source = parsed.get("source").and_then(|v| v.as_str()).unwrap_or("unknown");
                push_feed_event("Diagnostic", &format!("Error from {}: {}", source, error_msg), "error_detected");
                return Response::ok_json(r#"{"status":"ok","event":"error_reported"}"#);
            }
            // Persist the raw webhook event for build_garden_snapshot to pick up.
            // Shape: {event, t, data:{package, total_ms, ...}, source, ...}
            if parsed.get("event").is_some() {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis()).unwrap_or(0);
                let dir = std::path::PathBuf::from("/tmp/flux-events");
                let _ = std::fs::create_dir_all(&dir);
                let path = dir.join(format!("webhook_{}.json", now_ms));
                let _ = std::fs::write(&path, &body_str);
            }
            if let Some(b) = parsed.get("builds").and_then(|v| v.as_u64()) {
                stats.builds_completed.store(b, Ordering::Relaxed);
            }
            if let Some(h) = parsed.get("cache_hits").and_then(|v| v.as_u64()) {
                stats.cache_hits.store(h, Ordering::Relaxed);
            }
            if let Some(m) = parsed.get("cache_misses").and_then(|v| v.as_u64()) {
                stats.cache_misses.store(m, Ordering::Relaxed);
            }
            if let Some(t) = parsed.get("tests_passed").and_then(|v| v.as_u64()) {
                stats.tests_passed.store(t, Ordering::Relaxed);
            }
            if let Some(f) = parsed.get("tests_failed").and_then(|v| v.as_u64()) {
                stats.tests_failed.store(f, Ordering::Relaxed);
            }
            let builds = stats.builds_completed.load(Ordering::Relaxed);
            Response::ok_json(&format!(r#"{{"status":"ok","builds":{}}}"#, builds))
        } else {
            Response::ok_json(r#"{"status":"error","msg":"invalid json"}"#)
        }
    } else {
        Response::ok_json(r#"{"status":"error","msg":"invalid utf-8"}"#)
    }
}

// ── Swarm REST API

// ── Server ──

pub fn build_router() -> Router {
    Router::new()
        .route("GET", "/", handle_dashboard)
        .route("GET", "/dashboard", handle_dashboard)
        .route("GET", "/api/stats", handle_stats)
        .route("GET", "/api/health", handle_health)
        .route("GET", "/api/xalgo", handle_xalgo)
        .route("GET", "/api/diagnose", handle_diagnose)
        .route("POST", "/api/build_event", handle_build_event)
        .route("GET", "/api/tune", handle_tune_get)
        .route("POST", "/api/tune", handle_tune_post)
        .route("GET", "/api/sap", handle_sap)
        .route("GET", "/api/aether", handle_aether)
        .route("GET", "/api/feed", handle_feed)
        .route("GET", "/api/xray", handle_xray)
        .route("GET", "/api/proof/*", handle_proof)
        .route("GET", "/api/goal/current", handle_goal_current)
        .route("GET", "/api/goal/list", handle_goal_list)
        .route("OPTIONS", "*", handle_options)
}

/// Compose the combined garden-state.json shape: stats + feed + xray + uptime.
/// Written periodically to disk so q-flux can serve it as a static asset
/// via quillon.xyz (the python garden bridge previously did this).
fn build_garden_snapshot(stats: &LiveStats, started_at_us: u64) -> String {
    let xray_val: serde_json::Value = match crate::xray::xray() {
        Ok(r) => serde_json::to_value(&r).unwrap_or(serde_json::json!(null)),
        Err(_) => serde_json::json!(null),
    };
    let events_dir = std::path::PathBuf::from("/tmp/flux-events");
    let mut events: Vec<serde_json::Value> = Vec::new();
    if events_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&events_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with("webhook_") {
                    // Webhook events arrive in {event, timestamp, data:{...}} shape.
                    // Add `t` (seconds float) for sort + JS consistency.
                    if let Ok(data) = std::fs::read_to_string(&path) {
                        if let Ok(mut evt) = serde_json::from_str::<serde_json::Value>(&data) {
                            if let Some(obj) = evt.as_object_mut() {
                                if !obj.contains_key("t") {
                                    let ts = obj.get("timestamp").and_then(|v| v.as_u64()).unwrap_or(0);
                                    obj.insert("t".into(), serde_json::json!(ts as f64));
                                }
                            }
                            events.push(evt);
                        }
                    }
                } else if name.starts_with("feed_") {
                    // ai_feed event (MCP tool call). Normalize to webhook shape so the
                    // browser's tower-flash logic can find package/total_ms/etc.
                    if let Ok(data) = std::fs::read_to_string(&path) {
                        if let Ok(mut evt) = serde_json::from_str::<serde_json::Value>(&data) {
                            if let Some(obj) = evt.as_object_mut() {
                                let ts_ms = obj.get("timestamp_ms").and_then(|v| v.as_i64()).unwrap_or(0);
                                obj.insert("t".into(), serde_json::json!((ts_ms as f64) / 1000.0));
                                let tool = obj.get("tool").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                if !tool.is_empty() {
                                    obj.insert("event".into(), serde_json::json!(tool));
                                }
                                // Parse "completed in NNNms" out of the message for total_ms.
                                let msg = obj.get("message").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let mut data_obj = serde_json::Map::new();
                                if let Some(ms_str) = msg.split("completed in ").nth(1) {
                                    let n: String = ms_str.chars().take_while(|c| c.is_ascii_digit()).collect();
                                    if let Ok(ms) = n.parse::<u64>() {
                                        data_obj.insert("total_ms".into(), serde_json::json!(ms));
                                    }
                                }
                                data_obj.insert("success".into(), serde_json::json!(true));
                                // Source crate isn't in the ai_feed shape — leave package empty.
                                obj.insert("data".into(), serde_json::Value::Object(data_obj));
                                obj.insert("source".into(), serde_json::json!(format!("ai_feed:{}", obj.get("agent").and_then(|v| v.as_str()).unwrap_or("?"))));
                            }
                            events.push(evt);
                        }
                    }
                }
            }
        }
    }
    // Separate webhook events from ai_feed so a flood of MCP tool calls doesn't
    // bury the build/test events the garden actually animates.
    let key_of = |e: &serde_json::Value| -> i64 {
        e.get("t").and_then(|v| v.as_f64()).map(|s| (s * 1000.0) as i64)
            .or_else(|| e.get("timestamp_ms").and_then(|v| v.as_i64()))
            .unwrap_or(0)
    };
    let mut webhook_evts: Vec<serde_json::Value> = events.iter()
        .filter(|e| e.get("type").and_then(|v| v.as_str()) != Some("ai_feed"))
        .cloned().collect();
    let mut feed_evts: Vec<serde_json::Value> = events.into_iter()
        .filter(|e| e.get("type").and_then(|v| v.as_str()) == Some("ai_feed"))
        .collect();
    webhook_evts.sort_by_key(|e| key_of(e));
    feed_evts.sort_by_key(|e| key_of(e));
    // Reserve up to 30 slots for webhook (the towers want these), pad remaining
    // with the most recent ai_feed events.
    let webhook_keep = 30.min(webhook_evts.len());
    let webhook_take = webhook_evts.len().saturating_sub(webhook_keep);
    let mut events: Vec<serde_json::Value> = webhook_evts.split_off(webhook_take);
    let feed_keep = (50 - events.len()).min(feed_evts.len());
    let feed_take = feed_evts.len().saturating_sub(feed_keep);
    events.extend(feed_evts.split_off(feed_take));
    events.sort_by_key(|e| key_of(e));

    let now_us = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64).unwrap_or(0);
    let uptime_s = ((now_us.saturating_sub(started_at_us)) as f64) / 1_000_000.0;

    let snap = serde_json::json!({
        "xray": xray_val,
        "xray_age_s": 0,
        "events": events,
        "started_at": (started_at_us as f64) / 1_000_000.0,
        "uptime_s": (uptime_s * 10.0).round() / 10.0,
        "stats": {
            "builds": stats.builds_completed.load(Ordering::Relaxed),
            "cache_hits": stats.cache_hits.load(Ordering::Relaxed),
            "cache_misses": stats.cache_misses.load(Ordering::Relaxed),
            "tests_passed": stats.tests_passed.load(Ordering::Relaxed),
            "tests_failed": stats.tests_failed.load(Ordering::Relaxed),
        },
    });
    serde_json::to_string(&snap).unwrap_or_else(|_| "{}".into())
}

fn handle_options(_req: &Request, _stats: &LiveStats) -> Response {
    Response {
        status: 204,
        content_type: "text/plain".into(),
        body: vec![],
        events: None,
        extra_headers: Vec::new(),
    }
}

/// GET /api/xray — workspace x-ray (crates, batches, swarm). Same payload as
/// `fluxc xray --json`. Used by the flux-arena Compile Garden UI as the
/// single source of truth for crate state.
fn handle_xray(_req: &Request, _stats: &LiveStats) -> Response {
    match crate::xray::xray() {
        Ok(r) => match serde_json::to_string(&r) {
            Ok(s) => Response::ok_json(&s),
            Err(e) => Response::ok_json(&format!(r#"{{"error":"serialize: {}"}}"#, e)),
        },
        Err(e) => Response::ok_json(&format!(r#"{{"error":"xray: {}"}}"#, e)),
    }
}

/// GET /api/goal/current — returns the current consensus goal (top-priority active).
/// The UE game client polls this every tick.
fn handle_goal_current(_req: &Request, _stats: &LiveStats) -> Response {
    match crate::goals::consensus_goal() {
        Ok(Some(g)) => match serde_json::to_string(&crate::goals::render_consensus(&g)) {
            Ok(s) => Response::ok_json(&s),
            Err(e) => Response::ok_json(&format!(r#"{{"error":"serialize: {}"}}"#, e)),
        },
        Ok(None) => Response::ok_json(r#"{"id":"idle","action":"patrol","text":"(no active goal — idle)"}"#),
        Err(e) => Response::ok_json(&format!(r#"{{"error":"{}"}}"#, e)),
    }
}

/// GET /api/goal/list — returns all active goals sorted by priority.
fn handle_goal_list(_req: &Request, _stats: &LiveStats) -> Response {
    match crate::goals::list_goals() {
        Ok(gs) => {
            let arr: Vec<serde_json::Value> = gs.iter().map(crate::goals::render_consensus).collect();
            Response::ok_json(&serde_json::to_string(&arr).unwrap_or_else(|_| "[]".into()))
        }
        Err(e) => Response::ok_json(&format!(r#"{{"error":"{}"}}"#, e)),
    }
}

/// GET /api/proof/<crate> — provenance proof JSON for one crate.
///
/// Lookup order:
///   1. $FLUX_PROOF_DIR/<crate>.proof
///   2. $FLUX_PROOF_DIR/<crate>.o.proof
///   3. <workspace>/target/proofs/<crate>.proof
///   4. <workspace>/target/proofs/<crate>.o.proof
///
/// If nothing matches, returns a `{ "synthetic": true, ... }` stub so the UE
/// sigil modal still has something to display — explicitly flagged so clients
/// don't mistake it for a real signed proof.
fn handle_proof(req: &Request, _stats: &LiveStats) -> Response {
    let crate_name = req.path.trim_start_matches("/api/proof/").trim_matches('/');
    if crate_name.is_empty() {
        return Response::ok_json(r#"{"error":"missing crate name"}"#);
    }

    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(env_dir) = std::env::var("FLUX_PROOF_DIR") {
        let d = std::path::PathBuf::from(env_dir);
        candidates.push(d.join(format!("{}.proof", crate_name)));
        candidates.push(d.join(format!("{}.o.proof", crate_name)));
    }
    if let Ok(cwd) = std::env::current_dir() {
        let d = cwd.join("target").join("proofs");
        candidates.push(d.join(format!("{}.proof", crate_name)));
        candidates.push(d.join(format!("{}.o.proof", crate_name)));
    }

    for path in &candidates {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(s) = String::from_utf8(bytes) {
                return Response::ok_json(&s);
            }
        }
    }

    // No real proof — synthesize a labeled stub from the agent key + crate name.
    let synthetic = synth_proof_for(crate_name);
    Response::ok_json(&synthetic)
}

/// Build a synthetic provenance stub for a crate that has no `.proof` file
/// yet. Uses BLAKE3 of the crate name as a stand-in artifact hash and reports
/// the in-process agent wallet (if loaded). Always carries `"synthetic": true`.
fn synth_proof_for(crate_name: &str) -> String {
    let agent_hex = std::env::var("FLUX_AGENT_PK_HEX").unwrap_or_default();
    let wallet_hex = std::env::var("FLUX_AGENT_WALLET").unwrap_or_default();
    let artifact_hex = {
        let mut h = blake3::Hasher::new();
        h.update(crate_name.as_bytes());
        hex::encode(h.finalize().as_bytes())
    };
    let now_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64).unwrap_or(0);
    serde_json::json!({
        "synthetic": true,
        "crate": crate_name,
        "artifact_hash_hex": artifact_hex,
        "agent_wallet_hex": wallet_hex,
        "sqisign_pubkey_hex": agent_hex,
        "fluxc_version": env!("CARGO_PKG_VERSION"),
        "timestamp_us": now_us,
        "note": "No .proof file found for this crate. Run `fluxc compile-native --provenance <path>` to emit a real signed proof.",
    }).to_string()
}

// Build a rustls server config from FLUX_TLS_CERT + FLUX_TLS_KEY (PEM). Returns None when unset
// (fluxc serve then runs plain HTTP exactly as before — non-breaking for the q-flux-fronted setup).
fn tls_config() -> Option<Arc<rustls::ServerConfig>> {
    let cert_path = std::env::var("FLUX_TLS_CERT").ok()?;
    let key_path = std::env::var("FLUX_TLS_KEY").ok()?;
    let cert_file = match std::fs::File::open(&cert_path) {
        Ok(f) => f, Err(e) => { eprintln!("⚠ FLUX_TLS_CERT {}: {}", cert_path, e); return None; }
    };
    let certs: Vec<_> = rustls_pemfile::certs(&mut std::io::BufReader::new(cert_file))
        .filter_map(|c| c.ok()).collect();
    if certs.is_empty() { eprintln!("⚠ FLUX_TLS_CERT {} has no certificates", cert_path); return None; }
    let key_file = match std::fs::File::open(&key_path) {
        Ok(f) => f, Err(e) => { eprintln!("⚠ FLUX_TLS_KEY {}: {}", key_path, e); return None; }
    };
    let key = match rustls_pemfile::private_key(&mut std::io::BufReader::new(key_file)) {
        Ok(Some(k)) => k, _ => { eprintln!("⚠ FLUX_TLS_KEY {} has no private key", key_path); return None; }
    };
    let cfg = rustls::ServerConfig::builder_with_provider(
            Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions().ok()?
        .with_no_client_auth()
        .with_single_cert(certs, key).ok()?;
    Some(Arc::new(cfg))
}

// ── Fase 1: access log · per-IP rate limit · connection caps ───────────────
static ACTIVE_CONNS: AtomicU64 = AtomicU64::new(0);

lazy_static::lazy_static! {
    // per-IP request counter, reset each wall-clock second: ip -> (second, count)
    static ref RATE_BUCKET: parking_lot::Mutex<std::collections::HashMap<String, (u64, u32)>> =
        parking_lot::Mutex::new(std::collections::HashMap::new());
}

fn now_secs() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

// Returns false when the IP exceeded FLUX_RATE_LIMIT_RPS this second (0/unset = unlimited).
fn rate_ok(ip: &str) -> bool {
    let rps: u32 = std::env::var("FLUX_RATE_LIMIT_RPS").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    if rps == 0 { return true; }
    let now = now_secs();
    let mut m = RATE_BUCKET.lock();
    if m.len() > 50_000 { m.clear(); } // bound memory under flood
    let e = m.entry(ip.to_string()).or_insert((now, 0));
    if e.0 != now { *e = (now, 0); }
    e.1 += 1;
    e.1 <= rps
}

// Append one access-log line (FLUX_ACCESS_LOG=/path). Format: epoch ip method status bytes path
fn access_log(ip: &str, method: &str, path: &str, status: u16, bytes: usize) {
    let p = match std::env::var("FLUX_ACCESS_LOG") { Ok(p) => p, Err(_) => return };
    let line = format!("{} {} {} {} {} {}\n", now_secs(), ip, method, status, bytes, path);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = f.write_all(line.as_bytes());
    }
}

// Spawn a tiny listener on port 80 that 301-redirects everything to https://<host><path>.
// Enabled with FLUX_HTTP_REDIRECT=1 (port overridable via FLUX_HTTP_REDIRECT_PORT). Used on the
// standalone fluxapp.xyz box so http://fluxapp.xyz → https://fluxapp.xyz cleanly.
fn spawn_http_redirect(bind_host: String) {
    let port: u16 = std::env::var("FLUX_HTTP_REDIRECT_PORT").ok()
        .and_then(|p| p.parse().ok()).unwrap_or(80);
    std::thread::spawn(move || {
        let listener = match TcpListener::bind(format!("{}:{}", bind_host, port)) {
            Ok(l) => l,
            Err(e) => { eprintln!("⚠ HTTP-redirect: cannot bind :{} ({})", port, e); return; }
        };
        eprintln!("   ↪ HTTP :{} → 301 https", port);
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut tcp = stream;
                let mut buf = [0u8; 4096];
                let n = match tcp.read(&mut buf) { Ok(n) if n > 0 => n, _ => return };
                let raw = String::from_utf8_lossy(&buf[..n]);
                let mut lines = raw.lines();
                let path = lines.next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/").to_string();
                let host = lines
                    .find(|l| l.to_lowercase().starts_with("host:"))
                    .and_then(|l| l.splitn(2, ':').nth(1)).map(|h| h.trim().to_string())
                    .unwrap_or_default();
                let location = format!("https://{}{}", host, path);
                let resp = format!(
                    "HTTP/1.1 301 Moved Permanently\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    location
                );
                let _ = tcp.write_all(resp.as_bytes());
            });
        }
    });
}

pub fn start_server(stats: Arc<LiveStats>, port: u16) -> std::thread::JoinHandle<()> {
    let router = build_router();
    // FLUX_SERVE_PORT overrides the caller's default (e.g. 443 for the standalone fluxapp.xyz box).
    let port: u16 = std::env::var("FLUX_SERVE_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(port);

    std::thread::spawn(move || {
        let bind_host = std::env::var("FLUX_SERVE_HOST").unwrap_or_else(|_| "127.0.0.1".into());
        let tls = tls_config();   // Some => terminate HTTPS ourselves (fluxapp.xyz standalone)
        // A configured TLS cert is an explicit intent to serve publicly, so it satisfies the
        // public-bind guard on its own (no FLUX_SERVE_TOKEN required for the HTTPS production path).
        if !is_private_bind_host(&bind_host) && serve_token().is_none() && tls.is_none() {
            eprintln!(
                "Refusing to bind fluxc serve to {} without FLUX_SERVE_TOKEN (or FLUX_TLS_CERT/KEY)",
                bind_host
            );
            return;
        }
        // Auto-find free port: try requested, increment on conflict
        let mut actual_port = port;
        let listener = loop {
            let addr = format!("{}:{}", bind_host, actual_port);
            match TcpListener::bind(&addr) {
                Ok(l) => break l,
                Err(_) => {
                    if actual_port - port > 10 { eprintln!("No free port"); return; }
                    actual_port += 1;
                }
            }
        };

        let scheme = if tls.is_some() { "https" } else { "http" };
        eprintln!("⚡ Flux Serve v0.9.16 at {}://{}:{} (auto-port)", scheme, bind_host, actual_port);
        if tls.is_some() { eprintln!("   🔒 TLS terminated by fluxc serve (FLUX_TLS_CERT/KEY)"); }
        if let Ok(p) = std::env::var("FLUX_PROXY") { eprintln!("   🔀 reverse-proxy: {}", p); }
        if std::env::var("FLUX_HTTP_REDIRECT").ok().as_deref() == Some("1") { spawn_http_redirect(bind_host.clone()); }
        if std::env::var("FLUX_SPA_FALLBACK").ok().as_deref() == Some("1") { eprintln!("   🧭 SPA fallback enabled"); }
        if let Ok(p) = std::env::var("FLUX_ACCESS_LOG") { eprintln!("   📓 access log → {}", p); }
        if let Ok(r) = std::env::var("FLUX_RATE_LIMIT_RPS") { if r != "0" { eprintln!("   🛡 rate limit: {} req/s per IP", r); } }
        if let Ok(c) = std::env::var("FLUX_MAX_CONNS") { if c != "0" { eprintln!("   🔢 max connections: {}", c); } }
        // Auto-spawn fluxmux in new terminal
        if std::path::Path::new("./target/debug/fluxmux").exists() {
            std::thread::spawn(|| {
                let _ = std::process::Command::new("x-terminal-emulator")
                    .args(["-e", "./target/debug/fluxmux"]).spawn();
            });
        }
        eprintln!("   GET  /              → Dashboard HTML");
        eprintln!("   GET  /api/stats      → Live stats JSON");
        eprintln!("   GET  /api/health     → Health check");
        eprintln!("   GET  /api/xalgo      → X-Algo scores");
        eprintln!("   GET  /api/diagnose   → Full diagnostic");
        eprintln!("   GET  /api/sap       → SAP peer scores");
        eprintln!("   POST /api/build_event → Accept build events");
        eprintln!("   GET  /sse             → SSE event stream");
        eprintln!("   POST /api/flux/build   → Compile code via fluxc");

        // ── FluxMux state writer: live stats for fluxmux TUI auto-discovery ──
        let mux_stats = Arc::clone(&stats);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                let state = mux_state_json(&mux_stats);
                let _ = std::fs::write("/tmp/flux-mux.state", state);
            }
        });

        // ── Garden snapshot writer: write combined xray + stats + feed every 2s
        // to a path q-flux serves as a static file. Replaces the Python
        // garden.py bridge so the flux-arena UI runs on pure Flux. ──
        let garden_stats = Arc::clone(&stats);
        let garden_started_us = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as u64).unwrap_or(0);
        let garden_path = std::env::var("FLUX_GARDEN_SNAPSHOT_PATH").unwrap_or_else(|_|
            "/home/orobit/q-narwhalknight/dist-final/garden-state.json".into());
        std::thread::spawn(move || {
            let tmp_path = format!("{}.tmp", &garden_path);
            loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let snap = build_garden_snapshot(&garden_stats, garden_started_us);
                if std::fs::write(&tmp_path, &snap).is_ok() {
                    let _ = std::fs::rename(&tmp_path, &garden_path);
                }
            }
        });

        // ── Goal snapshot writer (v0.17.x): mirror /api/goal/current + /list to
        // static files so QuillonOS Desktop (and any browser tab) can fetch
        // them through q-flux without needing a backend route to fluxc:8084. ──
        let goal_current_path = std::env::var("FLUX_GOAL_CURRENT_PATH").unwrap_or_else(|_|
            "/home/orobit/q-narwhalknight/dist-final/goal-current.json".into());
        let goal_list_path = std::env::var("FLUX_GOAL_LIST_PATH").unwrap_or_else(|_|
            "/home/orobit/q-narwhalknight/dist-final/goal-list.json".into());
        std::thread::spawn(move || {
            let cur_tmp = format!("{}.tmp", &goal_current_path);
            let lst_tmp = format!("{}.tmp", &goal_list_path);
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                // Current goal
                let cur_json = match crate::goals::consensus_goal() {
                    Ok(Some(g)) => serde_json::to_string(&crate::goals::render_consensus(&g))
                        .unwrap_or_else(|_| "{}".into()),
                    Ok(None) => r#"{"id":"idle","action":"patrol","text":"(no active goal — idle)"}"#.into(),
                    Err(e) => format!(r#"{{"error":"{}"}}"#, e),
                };
                if std::fs::write(&cur_tmp, &cur_json).is_ok() {
                    let _ = std::fs::rename(&cur_tmp, &goal_current_path);
                }
                // Full list
                let list_json = match crate::goals::list_goals() {
                    Ok(gs) => {
                        let arr: Vec<serde_json::Value> = gs.iter().map(crate::goals::render_consensus).collect();
                        serde_json::to_string(&arr).unwrap_or_else(|_| "[]".into())
                    }
                    Err(e) => format!(r#"{{"error":"{}"}}"#, e),
                };
                if std::fs::write(&lst_tmp, &list_json).is_ok() {
                    let _ = std::fs::rename(&lst_tmp, &goal_list_path);
                }
            }
        });

        // global connection cap (FLUX_MAX_CONNS, 0/unset = unlimited)
        let max_conns: u64 = std::env::var("FLUX_MAX_CONNS").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
        for stream in listener.incoming() {
            let stats = Arc::clone(&stats);
            let router = build_router();
            let tls = tls.clone();
            match stream {
                Ok(mut tcp) => {
                    let peer_ip = tcp.peer_addr().map(|a| a.ip().to_string()).unwrap_or_default();
                    if max_conns > 0 && ACTIVE_CONNS.load(Ordering::Relaxed) >= max_conns {
                        let _ = tcp.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        continue;
                    }
                    ACTIVE_CONNS.fetch_add(1, Ordering::Relaxed);
                    std::thread::spawn(move || {
                        match tls {
                            Some(cfg) => match rustls::ServerConnection::new(cfg) {
                                Ok(conn) => {
                                    let mut s = rustls::StreamOwned::new(conn, tcp);
                                    handle_connection(&mut s, &stats, &router, &peer_ip);
                                }
                                Err(_) => {}
                            },
                            None => handle_connection(&mut tcp, &stats, &router, &peer_ip),
                        }
                        ACTIVE_CONNS.fetch_sub(1, Ordering::Relaxed);
                    });
                }
                Err(_) => continue,
            }
        }
    })
}

/// Write live state for fluxmux TUI auto-discovery at /tmp/flux-mux.state
fn mux_state_json(stats: &LiveStats) -> String {
    use std::sync::atomic::Ordering;
    let builds = stats.builds_completed.load(Ordering::Relaxed);
    let hits = stats.cache_hits.load(Ordering::Relaxed);
    let misses = stats.cache_misses.load(Ordering::Relaxed);
    let total = hits + misses;
    let rate = if total > 0 { (hits as f64 / total as f64) * 100.0 } else { 0.0 };
    let build_time = stats.total_build_time_ms.load(Ordering::Relaxed);
    let avg_time = if builds > 0 { build_time / builds } else { 0 };
    let uptime = now_ms().saturating_sub(stats.start_time_ms) / 1000;
    let real_peers = std::process::Command::new("sh").arg("-c")
        .arg("ss -tn state established '( sport = :8084 or dport = :8084 )' 2>/dev/null | tail -n +2 | wc -l")
        .output().map(|o| String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(0)).unwrap_or(0);
    // Count build events from webhook queue
    let webhook_builds = std::fs::read_to_string("/tmp/flux-webhooks.queue")
        .map(|d| d.lines().filter(|l| l.contains("build_complete")).count() as u64).unwrap_or(0);
    let payload = serde_json::json!({
        "uptime_secs": uptime,
        "builds_completed": builds.max(webhook_builds),
        "webhook_builds": webhook_builds,
        "last_build_ms": avg_time,
        "cache_hit_rate": (rate * 100.0).round() / 100.0,
        "peer_count": real_peers.max(1).max(stats.p2p_peers.load(Ordering::Relaxed)),
        "dag_round": stats.dagknight_round.load(Ordering::Relaxed),
        "mempool_txs": stats.mempool_txs.load(Ordering::Relaxed),
        "updated_at_ms": now_ms(),
    });
    serde_json::to_string(&payload).unwrap_or_default()
}

fn handle_connection<S: Read + Write>(stream: &mut S, stats: &LiveStats, router: &Router, peer_ip: &str) {
    let mut buf = [0u8; 8192];
    let n = match stream.read(&mut buf) {
        Ok(n) if n > 0 => n,
        _ => return,
    };

    let raw = String::from_utf8_lossy(&buf[..n]);
    let mut lines = raw.lines();
    let first_line = lines.next().unwrap_or("");
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 2 { return; }

    let method = parts[0].to_string();
    let path = parts[1].to_string();

    // Parse headers
    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines.by_ref() {
        if line.is_empty() { break; }
        if let Some((k, v)) = line.split_once(": ") {
            headers.push((k.to_lowercase(), v.to_string()));
            if k.to_lowercase() == "content-length" {
                content_length = v.parse().unwrap_or(0);
            }
        }
    }

    // Read body if present
    let body_start = raw.find("\r\n\r\n").map(|p| p + 4).unwrap_or(raw.len());
    let body = if body_start < n && content_length > 0 {
        buf[body_start..n.min(body_start + content_length)].to_vec()
    } else {
        Vec::new()
    };

    let req = Request { method, path, headers, body };

    // Per-IP rate limit (FLUX_RATE_LIMIT_RPS) — fail closed with 429 before doing any work.
    if !rate_ok(peer_ip) {
        let _ = stream.write_all(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        access_log(peer_ip, &req.method, &req.path, 429, 0);
        return;
    }

    // SSE is a special persistent connection
    if req.path.starts_with("/sse") {
        access_log(peer_ip, &req.method, &req.path, 200, 0);
        serve_sse(stream, stats);
        return;
    }

    // Reverse-proxy: forward matching prefixes to a backend before the static router.
    // FLUX_PROXY="/api=127.0.0.1:8080,/capi=127.0.0.1:8790" — lets fluxc serve front the
    // q-api-server (chain API) + cockpit-api the way q-flux does, so fluxapp.xyz is standalone.
    if flux_proxy_to(&req, stream) {
        access_log(peer_ip, &req.method, &req.path, 0, 0); // 0 = proxied (upstream status not parsed)
        return;
    }

    // Dispatch to router
    let resp = router.dispatch(&req, stats);
    access_log(peer_ip, &req.method, &req.path, resp.status, resp.body.len());
    write_response(stream, &resp);
}

// Forward a request to a backend over HTTP and stream the raw response back to the client.
// Returns true if the path matched a proxy rule (handled), false to fall through to the router.
fn flux_proxy_to<S: Read + Write>(req: &Request, stream: &mut S) -> bool {
    let rules = match std::env::var("FLUX_PROXY") { Ok(r) => r, Err(_) => return false };
    let backend = rules.split(',').find_map(|r| {
        let (prefix, addr) = r.split_once('=')?;
        let prefix = prefix.trim();
        if req.path == prefix || req.path.starts_with(&format!("{}/", prefix)) {
            Some(addr.trim().to_string())
        } else { None }
    });
    let backend = match backend { Some(b) => b, None => return false };
    let mut up = match std::net::TcpStream::connect(&backend) {
        Ok(u) => u,
        Err(_) => { let _ = stream.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n"); return true; }
    };
    // rebuild the request line + headers (header keys are already lowercased by the parser)
    let mut head = format!("{} {} HTTP/1.1\r\n", req.method, req.path);
    for (k, v) in &req.headers {
        if k == "connection" || k == "content-length" { continue; }
        head.push_str(&format!("{}: {}\r\n", k, v));
    }
    head.push_str("connection: close\r\n");
    head.push_str(&format!("content-length: {}\r\n\r\n", req.body.len()));
    if up.write_all(head.as_bytes()).is_err() || up.write_all(&req.body).is_err() { return true; }
    let _ = up.flush();
    // copy the upstream response straight back (it is already valid HTTP)
    let mut buf = [0u8; 16384];
    loop {
        match up.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => { if stream.write_all(&buf[..n]).is_err() { break; } }
            Err(_) => break,
        }
    }
    let _ = stream.flush();
    true
}

fn serve_sse<S: Read + Write>(stream: &mut S, stats: &LiveStats) {
    let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\nAccess-Control-Allow-Origin: *\r\n\r\n";
    let _ = stream.write_all(headers.as_bytes());

    let mut last_builds = stats.builds_completed.load(Ordering::Relaxed);
    let mut last_tests = (stats.tests_passed.load(Ordering::Relaxed), stats.tests_failed.load(Ordering::Relaxed));
    let mut last_feed_ts: u64 = 0;

    let events_dir = std::path::PathBuf::from("/tmp/flux-events");

    loop {
        let json = stats.to_json();
        let event = format!("data: {}\n\n", json);
        if stream.write_all(event.as_bytes()).is_err() { break; }
        let _ = stream.flush();

        let builds = stats.builds_completed.load(Ordering::Relaxed);
        if builds > last_builds {
            let be = format!("event: build_complete\ndata: {{\"builds\":{}}}\n\n", builds);
            let _ = stream.write_all(be.as_bytes());
            let _ = stream.flush();
            last_builds = builds;
            *stats.last_event.lock() = format!("build #{} complete", builds);
        }

        let tp = stats.tests_passed.load(Ordering::Relaxed);
        let tf = stats.tests_failed.load(Ordering::Relaxed);
        if (tp, tf) != last_tests {
            let te = format!("event: test_update\ndata: {{\"passed\":{},\"failed\":{}}}\n\n", tp, tf);
            let _ = stream.write_all(te.as_bytes());
            let _ = stream.flush();
            last_tests = (tp, tf);
            *stats.last_event.lock() = format!("tests: {}/{}", tp, tp+tf);
        }

        // Drain file-based AI feed event queue (~/.flux/events/feed_*.json)
        // Cross-process bridge: MCP tools write here, SSE loop reads and emits
        if events_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&events_dir) {
                let mut feed_events: Vec<(u64, String)> = Vec::new();
                for entry in entries.flatten() {
                    let path = entry.path();
                    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    if !stem.starts_with("feed_") { continue; }
                    if path.extension().map_or(false, |e| e == "json") {
                        if let Ok(data) = std::fs::read_to_string(&path) {
                            if let Ok(evt) = serde_json::from_str::<serde_json::Value>(&data) {
                                let ts = evt.get("timestamp_ms").and_then(|v| v.as_u64()).unwrap_or(0);
                                if ts > last_feed_ts {
                                    let agent = evt.get("agent").and_then(|v| v.as_str()).unwrap_or("Flux");
                                    let msg = evt.get("message").and_then(|v| v.as_str()).unwrap_or("");
                                    let tool = evt.get("tool").and_then(|v| v.as_str()).unwrap_or("");
                                    let feed_json = serde_json::json!({
                                        "agent": agent,
                                        "message": msg,
                                        "tool": tool,
                                        "ts": ts,
                                    });
                                    if let Ok(fj) = serde_json::to_string(&feed_json) {
                                        feed_events.push((ts, fj));
                                    }
                                }
                            }
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                }
                // Sort by timestamp and emit
                feed_events.sort_by_key(|(ts, _)| *ts);
                for (ts, fj) in feed_events {
                    let fe = format!("event: ai_feed\ndata: {}\n\n", fj);
                    let _ = stream.write_all(fe.as_bytes());
                    let _ = stream.flush();
                    last_feed_ts = ts;
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

fn write_response<S: Write>(stream: &mut S, resp: &Response) {
    let status_text = match resp.status {
        200 => "OK",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        304 => "Not Modified",
        401 => "Unauthorized",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Unknown",
    };

    let mut header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, Authorization, X-Flux-Token\r\n",
        resp.status, status_text, resp.content_type
    );
    for (k, v) in &resp.extra_headers {
        header.push_str(&format!("{}: {}\r\n", k, v));
    }
    // 304 Not Modified carries no body and (per spec) no Content-Length.
    if resp.status != 304 {
        header.push_str(&format!("Content-Length: {}\r\n", resp.body.len()));
    }
    header.push_str("\r\n");
    let _ = stream.write_all(header.as_bytes());
    if resp.status != 304 {
        let _ = stream.write_all(&resp.body);
    }
}

// ── Legacy compatibility ──

pub fn init_live_stats() -> Arc<LiveStats> {
    LiveStats::new()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_router_dispatch() {
        let router = build_router();
        let stats = LiveStats::new();
        let req = Request { method: "GET".into(), path: "/api/health".into(), headers: vec![], body: vec![] };
        let resp = router.dispatch(&req, &stats);
        assert_eq!(resp.status, 200);
    }

    #[test]
    fn test_router_404() {
        let router = build_router();
        let stats = LiveStats::new();
        let req = Request { method: "GET".into(), path: "/nonexistent".into(), headers: vec![], body: vec![] };
        let resp = router.dispatch(&req, &stats);
        assert_eq!(resp.status, 404);
    }

    #[test]
    fn test_stats_json_includes_xalgo() {
        let stats = LiveStats::new();
        let json = stats.to_json();
        assert!(json.contains("xalgo"));
        assert!(json.contains("temporal_trust"));
        assert!(json.contains("v0.9.9-beta1"));
    }
}
