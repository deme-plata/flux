//! Vite + HMR live-event capture.
//!
//! Two pieces:
//!   1. A tiny HTTP control server (stdlib-only TcpListener) that accepts
//!      POST /event with a JSON body and appends it to `flux-events.jsonl`.
//!      Other tools (a Vite plugin, a browser bookmarklet, a userscript)
//!      stream HMR events into it during a recording.
//!   2. A merge helper that loads `flux-events.jsonl` and converts each entry
//!      into a `transcript::Event` so the overlay/render pipeline picks them
//!      up identically to Claude tool calls.
//!
//! Wire format (one JSON object per POST, or one per JSONL line):
//!   { "type": "hmr",     "file": "src/App.tsx", "t": 12.3 }
//!   { "type": "dom",     "selector": "main",     "t": 13.1 }
//!   { "type": "console", "level": "error", "text": "...", "t": 14.0 }
//!   { "type": "route",   "to": "/dashboard",     "t": 15.2 }

use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use crate::transcript::{Event, EventKind};

pub struct ControlServer {
    pub addr: String,
    pub events_path: PathBuf,
}

impl ControlServer {
    pub fn new(addr: impl Into<String>, events_path: impl Into<PathBuf>) -> Self {
        Self { addr: addr.into(), events_path: events_path.into() }
    }

    /// Blocks. Run from main (or spawn it). Writes one JSON line per event.
    pub fn serve(self) -> Result<()> {
        let listener = TcpListener::bind(&self.addr)
            .with_context(|| format!("binding control server on {}", self.addr))?;
        eprintln!("flux-record: control server listening on http://{}", self.addr);
        eprintln!("           : events -> {}", self.events_path.display());

        let start = Instant::now();
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.events_path)
            .context("opening events file")?;
        let file = Arc::new(Mutex::new(file));

        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => continue,
            };
            let file = Arc::clone(&file);
            let elapsed = start.elapsed().as_secs_f64();
            thread::spawn(move || {
                let _ = handle_request(&mut stream, &file, elapsed);
            });
        }
        Ok(())
    }
}

fn handle_request(
    stream: &mut std::net::TcpStream,
    file: &Arc<Mutex<std::fs::File>>,
    server_start_offset: f64,
) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    // Parse "POST /event HTTP/1.1" or "OPTIONS /event HTTP/1.1" (preflight).
    let mut content_length = 0usize;
    let is_options = request_line.starts_with("OPTIONS");
    loop {
        let mut header = String::new();
        let n = reader.read_line(&mut header)?;
        if n == 0 || header == "\r\n" || header == "\n" {
            break;
        }
        let lower = header.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            content_length = rest.trim().parse().unwrap_or(0);
        }
        if lower.starts_with("origin:") {
            // Browser preflight — no body needed, just CORS.
            let _ = lower;
        }
    }

    // CORS preflight short-circuit.
    if is_options {
        let resp = "HTTP/1.1 204 No Content\r\n\
                    Access-Control-Allow-Origin: *\r\n\
                    Access-Control-Allow-Methods: POST, OPTIONS\r\n\
                    Access-Control-Allow-Headers: Content-Type\r\n\
                    Content-Length: 0\r\n\r\n";
        stream.write_all(resp.as_bytes())?;
        return Ok(());
    }
    let _ = is_options;

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }

    let body_str = String::from_utf8_lossy(&body);

    // If the caller didn't include a "t" field, stamp it with our server-side
    // monotonic offset so events still line up roughly with the video timeline.
    let mut value: serde_json::Value =
        serde_json::from_str(&body_str).unwrap_or_else(|_| serde_json::json!({}));
    if value.get("t").is_none() {
        if let Some(obj) = value.as_object_mut() {
            obj.insert("t".to_string(), serde_json::json!(server_start_offset));
        }
    }
    if value.get("type").is_none() {
        if let Some(obj) = value.as_object_mut() {
            obj.insert("type".to_string(), serde_json::json!("event"));
        }
    }

    let line = serde_json::to_string(&value)?;
    {
        let mut f = file.lock().unwrap();
        writeln!(f, "{line}")?;
    }

    let resp = "HTTP/1.1 200 OK\r\n\
                Access-Control-Allow-Origin: *\r\n\
                Content-Type: application/json\r\n\
                Content-Length: 9\r\n\r\n\
                {\"ok\":1}\n";
    stream.write_all(resp.as_bytes())?;
    Ok(())
}

/// Read flux-events.jsonl into transcript-compatible Events so the cinematic
/// render pipeline shows them alongside Claude's tool calls.
pub fn load_events(path: &Path) -> Result<Vec<Event>> {
    let file = std::fs::File::open(path)?;
    let mut out: Vec<Event> = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let kind_str = v.get("type").and_then(|x| x.as_str()).unwrap_or("event");
        let t_s = v.get("t").and_then(|x| x.as_f64()).unwrap_or(0.0);
        let (kind, label, body) = match kind_str {
            "hmr" => {
                let file = v.get("file").and_then(|x| x.as_str()).unwrap_or("?");
                let name = file.rsplit('/').next().unwrap_or(file).to_string();
                (EventKind::ToolUse("HMR".to_string()), name, format!("Vite HMR: {file}"))
            }
            "dom" => {
                let sel = v.get("selector").and_then(|x| x.as_str()).unwrap_or("?");
                (EventKind::ToolUse("DOM".to_string()), sel.to_string(), format!("DOM update: {sel}"))
            }
            "console" => {
                let level = v.get("level").and_then(|x| x.as_str()).unwrap_or("log");
                let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let is_error = level == "error";
                let kind = if is_error {
                    EventKind::ToolResult { is_error: true }
                } else {
                    EventKind::ToolUse(format!("CONSOLE/{level}"))
                };
                (kind, text.clone(), text)
            }
            "route" => {
                let to = v.get("to").and_then(|x| x.as_str()).unwrap_or("?");
                (EventKind::ToolUse("ROUTE".to_string()), to.to_string(), format!("nav → {to}"))
            }
            _ => {
                let label = v.to_string();
                (EventKind::System, label.clone(), label)
            }
        };
        out.push(Event { t_s, kind, label, body });
    }
    Ok(out)
}

/// Merge two event streams by timestamp. Both inputs are assumed pre-sorted.
pub fn merge(mut a: Vec<Event>, mut b: Vec<Event>) -> Vec<Event> {
    a.append(&mut b);
    a.sort_by(|x, y| x.t_s.partial_cmp(&y.t_s).unwrap_or(std::cmp::Ordering::Equal));
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture(tag: &str, lines: &[&str]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("flux-record-vite-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("events-{tag}.jsonl"));
        let mut f = std::fs::File::create(&path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        path
    }

    #[test]
    fn hmr_event_becomes_tool_use_with_filename_label() {
        let path = fixture("hmr", &[
            r#"{"type":"hmr","file":"src/components/Button.tsx","t":12.3}"#,
        ]);
        let events = load_events(&path).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].tool_name(), Some("HMR"));
        assert_eq!(events[0].label, "Button.tsx");
        assert!((events[0].t_s - 12.3).abs() < 0.001);
    }

    #[test]
    fn console_error_marked_as_error_result() {
        let path = fixture("console", &[
            r#"{"type":"console","level":"error","text":"TypeError x is undefined","t":5.0}"#,
        ]);
        let events = load_events(&path).unwrap();
        assert!(events[0].is_error());
    }

    #[test]
    fn merge_orders_by_timestamp() {
        let a = vec![Event { t_s: 0.0, kind: EventKind::User, label: "a".into(), body: String::new() }];
        let b = vec![
            Event { t_s: 5.0, kind: EventKind::User, label: "c".into(), body: String::new() },
            Event { t_s: 2.0, kind: EventKind::User, label: "b".into(), body: String::new() },
        ];
        let merged = merge(a, b);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].label, "a");
        assert_eq!(merged[1].label, "b");
        assert_eq!(merged[2].label, "c");
    }
}
