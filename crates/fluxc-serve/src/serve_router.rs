// serve_router — extracted Router + dispatch + auth/static helpers
// Split from serve.rs god-file (1479LOC -> smaller) per legacy_plan + H&P (modularity for faster iteration).
// Router dispatch is the hot path for every request; keeping it lean + in own module improves cache locality.

use crate::serve::{LiveStats, Request, Response};

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

    pub fn dispatch(&self, req: &Request, stats: &LiveStats) -> Response {
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

// --- helpers moved with router (auth + static serving) ---

fn header_value<'a>(req: &'a Request, name: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

pub(crate) fn serve_token() -> Option<String> {
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
