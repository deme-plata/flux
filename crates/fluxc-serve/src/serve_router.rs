// serve_router — extracted Router + dispatch + auth/static helpers
// Split from serve.rs god-file (1479LOC -> smaller) per legacy_plan + H&P (modularity for faster iteration).
// Router dispatch is the hot path for every request; keeping it lean + in own module improves cache locality.

use crate::serve::{FileBody, LiveStats, Request, Response};

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
    // Media — correct Content-Type matters for <video>/<audio> playback and, with
    // Accept-Ranges, for seek support. These are already-compressed containers, so
    // this server never gzips them (no Content-Encoding is applied anywhere).
    else if lower.ends_with(".mp4")  || lower.ends_with(".m4v") { "video/mp4" }
    else if lower.ends_with(".webm") { "video/webm" }
    else if lower.ends_with(".mkv")  { "video/x-matroska" }
    else if lower.ends_with(".mov")  { "video/quicktime" }
    else if lower.ends_with(".ogv")  { "video/ogg" }
    else if lower.ends_with(".ts")   { "video/mp2t" }
    else if lower.ends_with(".m3u8") { "application/vnd.apple.mpegurl" }
    else if lower.ends_with(".mp3")  { "audio/mpeg" }
    else if lower.ends_with(".m4a")  || lower.ends_with(".aac") { "audio/mp4" }
    else if lower.ends_with(".ogg")  || lower.ends_with(".oga") { "audio/ogg" }
    else if lower.ends_with(".opus") { "audio/opus" }
    else if lower.ends_with(".wav")  { "audio/wav" }
    else if lower.ends_with(".flac") { "audio/flac" }
    else if lower.ends_with(".pdf")  { "application/pdf" }
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
    // against absolute-path inputs / symlink escapes), then build a STREAMING
    // response from metadata only. We deliberately never `std::fs::read` the whole
    // file here: for a multi-GB movie that would pull the entire file into memory
    // (and, with the old content-hash ETag, BLAKE3 it) on EVERY request — including
    // each tiny seek/range. Metadata (len + mtime) is O(1); the bytes are streamed
    // lazily by `write_response`.
    let try_file = |candidate: std::path::PathBuf| -> Option<Response> {
        let cand_canon = candidate.canonicalize().ok()?;
        if !cand_canon.starts_with(&dir_canon) { return None; }
        let meta = std::fs::metadata(&cand_canon).ok()?;
        if !meta.is_file() { return None; }
        let ct = content_type_for(cand_canon.to_string_lossy().as_ref());
        Some(static_file_response(req, cand_canon, &meta, ct))
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

/// Parse a single HTTP byte-range spec ("start-end", "start-", or "-suffixlen")
/// against a known total size. Returns the inclusive `(start, end)` to serve, or
/// `None` if the range is empty, multi-range (comma), or unsatisfiable — in which
/// case the caller falls back to a full 200 response. `total` must be > 0.
fn parse_single_range(spec: &str, total: u64) -> Option<(u64, u64)> {
    if spec.contains(',') || total == 0 { return None; } // multi-range unsupported
    let (s, e) = spec.split_once('-')?;
    let (s, e) = (s.trim(), e.trim());
    let last = total - 1;
    let (start, end) = if s.is_empty() {
        // Suffix range: last N bytes.
        let n: u64 = e.parse().ok()?;
        if n == 0 { return None; }
        (total.saturating_sub(n), last)
    } else {
        let start: u64 = s.parse().ok()?;
        let end: u64 = if e.is_empty() { last } else { e.parse().ok()? };
        (start, end.min(last))
    };
    if start <= end && start < total { Some((start, end)) } else { None }
}

/// Apply HTTP caching (ETag/If-None-Match → 304) and Range (bytes=a-b → 206) to a
/// static file, streaming the bytes from disk (never buffering the whole file).
///
/// The ETag is derived from file METADATA (size + mtime), not a content hash. The
/// old content-hash ETag re-read and BLAKE3'd the entire file on every request —
/// O(file size) per seek, which is fatal for large media. Size+mtime is what nginx
/// and every production static server use; it changes whenever the file changes.
fn static_file_response(req: &Request, path: std::path::PathBuf, meta: &std::fs::Metadata, ct: &str) -> Response {
    let total = meta.len();
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let etag = format!("\"{:x}-{:x}\"", total, mtime_ns);

    // Conditional GET: client already has this exact version — no read, no body.
    if let Some(inm) = header_value(req, "if-none-match") {
        if inm.split(',').any(|t| t.trim() == etag) {
            return Response {
                status: 304, content_type: ct.into(), body: Vec::new(), events: None, file: None,
                extra_headers: vec![
                    ("ETag".into(), etag),
                    ("Accept-Ranges".into(), "bytes".into()),
                    ("Cache-Control".into(), "no-cache".into()),
                ],
            };
        }
    }

    // Range request → stream exactly the requested window as 206.
    if let Some(rng) = header_value(req, "range").and_then(|h| h.strip_prefix("bytes=")) {
        if let Some((start, end)) = parse_single_range(rng, total) {
            return Response {
                status: 206, content_type: ct.into(), body: Vec::new(), events: None,
                file: Some(FileBody { path, offset: start, len: end - start + 1 }),
                extra_headers: vec![
                    ("ETag".into(), etag),
                    ("Accept-Ranges".into(), "bytes".into()),
                    ("Content-Range".into(), format!("bytes {}-{}/{}", start, end, total)),
                ],
            };
        }
    }

    // Full body, streamed from disk in bounded chunks.
    Response {
        status: 200, content_type: ct.into(), body: Vec::new(), events: None,
        file: Some(FileBody { path, offset: 0, len: total }),
        extra_headers: vec![("ETag".into(), etag), ("Accept-Ranges".into(), "bytes".into())],
    }
}

#[cfg(test)]
mod tests {
    use super::parse_single_range;

    #[test]
    fn open_ended_range_reaches_eof() {
        // "bytes=0-" → whole file as one 206 window.
        assert_eq!(parse_single_range("0-", 1000), Some((0, 999)));
        // "bytes=500-" → from 500 to end.
        assert_eq!(parse_single_range("500-", 1000), Some((500, 999)));
    }

    #[test]
    fn closed_range_is_inclusive_and_clamped() {
        assert_eq!(parse_single_range("0-99", 1000), Some((0, 99)));
        // end past EOF is clamped to last byte.
        assert_eq!(parse_single_range("990-100000", 1000), Some((990, 999)));
    }

    #[test]
    fn suffix_range_takes_last_n_bytes() {
        assert_eq!(parse_single_range("-500", 1000), Some((500, 999)));
        // Suffix larger than the file → whole file.
        assert_eq!(parse_single_range("-5000", 1000), Some((0, 999)));
        // "-0" is not a valid suffix range.
        assert_eq!(parse_single_range("-0", 1000), None);
    }

    #[test]
    fn unsatisfiable_and_multirange_fall_through() {
        assert_eq!(parse_single_range("2000-3000", 1000), None); // start past EOF
        assert_eq!(parse_single_range("0-99,200-299", 1000), None); // multi-range
        assert_eq!(parse_single_range("abc-def", 1000), None); // garbage
        assert_eq!(parse_single_range("0-99", 0), None); // empty file
    }
}
