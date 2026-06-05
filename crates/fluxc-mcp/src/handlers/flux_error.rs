//! flux_error — MCP tools for catching SIGIL wallet client-side errors via
//! q-flux access-log beacons + (later) for dispatching them to registered
//! webhooks automatically.
//!
//! How errors get here:
//!   1. apiShim in the wallet listens to `window.error` + `unhandledrejection`.
//!   2. On capture, it does `navigator.sendBeacon('/sigil-error-log?msg=...&at=...&stack=...&t=...')`.
//!   3. q-flux 404s that path but writes one access-log line containing the
//!      query string (Combined Log Format).
//!   4. This tool greps the access log for `/sigil-error-log?` entries,
//!      URL-decodes the query, and returns them as JSON.
//!
//! Tools registered:
//!   • `flux_error_tail`  — return the most recent N error reports (poll on demand)
//!   • `flux_error_watch` — register a webhook URL to receive new error events
//!     (auto-fires once a background tail-poller in fluxc-serve picks them up)

use crate::handlers::{ToolDef, ToolRegistry};
use serde_json::{json, Value};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_error_tail",
            description: "Tail the q-flux access log for /sigil-error-log beacons (window.error captures from the SIGIL wallet). Returns the last N error reports as JSON, newest first. Use this to debug client-side bugs without leaving the agent loop.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "description": "Max entries to return (default 20, cap 200)" },
                    "since_ts_ms": { "type": "integer", "description": "Only return entries with ts > since_ts_ms (default 0)" },
                    "access_log": { "type": "string", "description": "Override path (default /home/storage/logs/q-flux/access.log)" }
                }
            }),
        },
        flux_error_tail,
    );

    registry.register(
        ToolDef {
            name: "flux_error_watch",
            description: "Register a webhook URL that fluxc will POST to whenever a new SIGIL wallet error is captured. Posts the same JSON shape as flux_error_tail entries. Use to wire errors into the running agent's MCP feed.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id":  { "type": "string", "description": "Unique watch id (idempotent)" },
                    "url": { "type": "string", "description": "Webhook URL — POSTed with the error JSON" }
                },
                "required": ["id", "url"]
            }),
        },
        flux_error_watch,
    );
}

fn default_log_path() -> PathBuf {
    PathBuf::from("/home/storage/logs/q-flux/access.log")
}

/// Standard percent-decode (ascii-safe, errors silently → empty).
fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'+' {
            out.push(b' ');
            i += 1;
        } else if b == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00"),
                16,
            ) {
                out.push(byte);
                i += 3;
            } else {
                out.push(b);
                i += 1;
            }
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse a query string `a=1&b=2` into a serde_json::Map.
fn parse_query(qs: &str) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    for pair in qs.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next().unwrap_or("").to_string();
        let v = it.next().map(url_decode).unwrap_or_default();
        if !k.is_empty() {
            m.insert(k, Value::String(v));
        }
    }
    m
}

/// Extract the `/sigil-error-log?...` query string from one access-log line,
/// if present. Robust to q-flux's combined-log + nginx-style formats.
fn extract_query(line: &str) -> Option<&str> {
    let idx = line.find("/sigil-error-log?")?;
    let rest = &line[idx + "/sigil-error-log?".len()..];
    // terminate at the next space or quote (whichever comes first)
    let end = rest
        .find(|c: char| c == ' ' || c == '"' || c == '\n')
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

fn read_errors(path: &PathBuf, since_ts_ms: u64, limit: usize) -> Vec<Value> {
    let f = match File::open(path) {
        Ok(f) => f,
        Err(_) => return vec![],
    };
    let rdr = BufReader::new(f);
    let mut out: Vec<Value> = Vec::new();
    for line in rdr.lines().flatten() {
        if let Some(qs) = extract_query(&line) {
            let mut m = parse_query(qs);
            // ts is in `t` query param (unix ms)
            let ts_ms = m
                .get("t")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            if ts_ms <= since_ts_ms {
                continue;
            }
            m.insert("ts_ms".to_string(), Value::Number(ts_ms.into()));
            out.push(Value::Object(m));
        }
    }
    // newest first
    out.reverse();
    out.truncate(limit);
    out
}

fn flux_error_tail(args: &Value) -> String {
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(20)
        .min(200) as usize;
    let since_ts_ms = args
        .get("since_ts_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let path = args
        .get("access_log")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(default_log_path);

    let errors = read_errors(&path, since_ts_ms, limit);
    if errors.is_empty() {
        return format!(
            "📭 flux_error_tail: no /sigil-error-log entries in {} since ts={}\n  hint: the wallet's apiShim sends one via navigator.sendBeacon on every window.error.\n  if empty after a fresh repro, the bug is firing before listeners attach.",
            path.display(),
            since_ts_ms
        );
    }
    // pretty short summary + JSON dump
    let mut header = format!(
        "📨 flux_error_tail — {} recent error(s) in {}:\n",
        errors.len(),
        path.display()
    );
    for (i, e) in errors.iter().enumerate().take(5) {
        let msg = e.get("msg").and_then(|v| v.as_str()).unwrap_or("?");
        let at = e.get("at").and_then(|v| v.as_str()).unwrap_or("?");
        let ts = e.get("ts_ms").and_then(|v| v.as_u64()).unwrap_or(0);
        header.push_str(&format!("  {}. [{}ms] {}  @ {}\n", i + 1, ts, msg, at));
    }
    header.push('\n');
    let json = serde_json::to_string_pretty(&Value::Array(errors)).unwrap_or_default();
    format!("{}{}", header, json)
}

fn flux_error_watch(args: &Value) -> String {
    let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
    if id.is_empty() || url.is_empty() {
        return "❌ flux_error_watch: 'id' and 'url' are required".to_string();
    }
    // Persist registration in /tmp/flux-error-watches.json so the background
    // poller (when running) picks it up. Append-or-update semantics.
    let path = "/tmp/flux-error-watches.json";
    let mut current: Vec<Value> = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    current.retain(|v| v.get("id").and_then(|x| x.as_str()) != Some(id));
    current.push(json!({ "id": id, "url": url, "registered_ms": now_ms() }));
    let _ = std::fs::write(path, serde_json::to_string_pretty(&current).unwrap_or_default());
    format!(
        "🔔 flux_error_watch — registered\n  id:  {}\n  url: {}\n  total active watches: {}\n  note: actual dispatch starts when fluxc-serve's tail-poller picks them up.",
        id,
        url,
        current.len()
    )
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn url_decode_basics() {
        assert_eq!(url_decode("hello%20world"), "hello world");
        assert_eq!(url_decode("a%2Fb"), "a/b");
        assert_eq!(url_decode("plain"), "plain");
        assert_eq!(url_decode("a+b"), "a b");
    }

    #[test]
    fn parse_query_extracts_all_params() {
        let m = parse_query("msg=hi&at=x.js%3A10%3A2&t=12345");
        assert_eq!(m.get("msg").and_then(|v| v.as_str()), Some("hi"));
        assert_eq!(m.get("at").and_then(|v| v.as_str()), Some("x.js:10:2"));
        assert_eq!(m.get("t").and_then(|v| v.as_str()), Some("12345"));
    }

    #[test]
    fn extract_query_finds_signal_in_combined_log() {
        let line = r#"1.2.3.4 - - [30/May/2026:13:00:00 +0000] "GET /sigil-error-log?msg=oops&t=999 HTTP/1.1" 404 0 "https://sigilgraph.quillon.xyz/sigil-wallet/index.html" "Mozilla/5.0""#;
        assert_eq!(extract_query(line), Some("msg=oops&t=999"));
    }

    #[test]
    fn extract_query_ignores_unrelated_lines() {
        let line = r#"GET /api/v1/status HTTP/1.1 200"#;
        assert!(extract_query(line).is_none());
    }

    #[test]
    fn read_errors_filters_by_since_ts_and_reverses() {
        let dir = std::env::temp_dir().join(format!(
            "flux-err-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("access.log");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, r#"GET /sigil-error-log?msg=one&t=1000 HTTP/1.1"#).unwrap();
        writeln!(f, r#"unrelated"#).unwrap();
        writeln!(f, r#"GET /sigil-error-log?msg=two&t=2000 HTTP/1.1"#).unwrap();
        writeln!(f, r#"GET /sigil-error-log?msg=three&t=3000 HTTP/1.1"#).unwrap();
        let out = read_errors(&path, 1500, 10);
        assert_eq!(out.len(), 2);
        // newest first
        assert_eq!(out[0].get("msg").and_then(|v| v.as_str()), Some("three"));
        assert_eq!(out[1].get("msg").and_then(|v| v.as_str()), Some("two"));
    }

    #[test]
    fn read_errors_respects_limit() {
        let dir = std::env::temp_dir().join(format!(
            "flux-err-test2-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("access.log");
        let mut f = std::fs::File::create(&path).unwrap();
        for i in 1..=10 {
            writeln!(f, r#"GET /sigil-error-log?msg=e&t={}000 HTTP/1.1"#, i).unwrap();
        }
        assert_eq!(read_errors(&path, 0, 3).len(), 3);
    }

    #[test]
    fn flux_error_watch_persists_then_overwrites() {
        let r1 = flux_error_watch(&json!({"id": "test-watch-x", "url": "http://example.com/hook"}));
        assert!(r1.contains("registered"));
        let r2 = flux_error_watch(&json!({"id": "test-watch-x", "url": "http://example.com/hook2"}));
        assert!(r2.contains("registered"));
        // The same id should only appear once
        let raw = std::fs::read_to_string("/tmp/flux-error-watches.json").unwrap_or_default();
        let count = raw.matches("test-watch-x").count();
        assert_eq!(count, 1, "expected exactly one entry for test-watch-x, got {}", count);
    }
}
