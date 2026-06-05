//! Parse Claude Code transcripts (~/.claude/projects/.../*.jsonl) into Event stream.
//!
//! Format is one JSON object per line, with shapes like:
//!   { "type": "user",      "message": { "content": "..." }, "timestamp": "..." }
//!   { "type": "assistant", "message": { "content": [ {"type":"text", "text":"..."},
//!                                                    {"type":"tool_use","name":"Bash",
//!                                                     "input":{"command":"...","description":"..."}} ] },
//!                          "timestamp": "..." }
//!   { "type": "tool_result", ... }
//!
//! We tolerate missing fields aggressively — transcripts evolve.

use anyhow::Result;
use serde::Deserialize;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    User,
    AssistantText,
    ToolUse(String), // tool name: "Bash" | "Read" | "Edit" | "Write" | ...
    ToolResult { is_error: bool },
    System,
}

#[derive(Debug, Clone)]
pub struct Event {
    /// Seconds from start of session (the first event is t=0).
    pub t_s: f64,
    pub kind: EventKind,
    /// Short human-readable label used by overlays (file path, command, etc).
    pub label: String,
    /// Longer body (assistant text, tool descriptions). Used for captions.
    pub body: String,
}

impl Event {
    pub fn tool_name(&self) -> Option<&str> {
        match &self.kind {
            EventKind::ToolUse(n) => Some(n.as_str()),
            _ => None,
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self.kind, EventKind::ToolResult { is_error: true })
    }
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    message: Option<serde_json::Value>,
    #[serde(default)]
    is_error: Option<bool>,
    #[serde(default)]
    tool_use_id: Option<String>,
    #[serde(default)]
    content: Option<serde_json::Value>,
}

pub fn load(path: &Path) -> Result<Vec<Event>> {
    let file = File::open(path)?;
    let mut events: Vec<(chrono::DateTime<chrono::Utc>, EventKind, String, String)> = Vec::new();

    for line in BufReader::new(file).lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let raw: Raw = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(_) => continue,
        };

        let ts = raw
            .timestamp
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&chrono::Utc))
            .unwrap_or_else(chrono::Utc::now);

        match raw.r#type.as_str() {
            "user" => {
                let body = extract_text(raw.message.as_ref());
                let label = preview(&body, 60);
                events.push((ts, EventKind::User, label, body));
            }
            "assistant" => {
                if let Some(msg) = raw.message.as_ref() {
                    let content = msg.get("content");
                    if let Some(items) = content.and_then(|v| v.as_array()) {
                        for item in items {
                            let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            match kind {
                                "text" => {
                                    let body = item
                                        .get("text")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    if !body.trim().is_empty() {
                                        let label = preview(&body, 60);
                                        events.push((
                                            ts,
                                            EventKind::AssistantText,
                                            label,
                                            body,
                                        ));
                                    }
                                }
                                "tool_use" => {
                                    let name = item
                                        .get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("Tool")
                                        .to_string();
                                    let input = item.get("input");
                                    let (label, body) = describe_tool(&name, input);
                                    events.push((
                                        ts,
                                        EventKind::ToolUse(name),
                                        label,
                                        body,
                                    ));
                                }
                                _ => {}
                            }
                        }
                    } else if let Some(s) = content.and_then(|v| v.as_str()) {
                        if !s.trim().is_empty() {
                            let body = s.to_string();
                            let label = preview(&body, 60);
                            events.push((ts, EventKind::AssistantText, label, body));
                        }
                    }
                }
            }
            "tool_result" | "user_tool_result" => {
                let is_error = raw.is_error.unwrap_or(false);
                let body = extract_text(raw.content.as_ref().or(raw.message.as_ref()));
                let label = preview(&body, 60);
                events.push((ts, EventKind::ToolResult { is_error }, label, body));
            }
            "system" | "" => {
                let body = extract_text(raw.message.as_ref());
                if !body.is_empty() {
                    let label = preview(&body, 60);
                    events.push((ts, EventKind::System, label, body));
                }
            }
            _ => {}
        }

        // tool_use_id field unused for now; keeping the parse so we don't lose info.
        let _ = &raw.tool_use_id;
    }

    if events.is_empty() {
        return Ok(Vec::new());
    }

    let t0 = events[0].0;
    Ok(events
        .into_iter()
        .map(|(ts, kind, label, body)| {
            let dt = (ts - t0).num_milliseconds().max(0) as f64 / 1000.0;
            Event { t_s: dt, kind, label, body }
        })
        .collect())
}

fn describe_tool(name: &str, input: Option<&serde_json::Value>) -> (String, String) {
    let input = match input {
        Some(v) => v,
        None => return (name.to_string(), String::new()),
    };

    let s = |k: &str| -> String {
        input.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
    };

    let label = match name {
        "Bash" => {
            let cmd = s("command");
            let desc = s("description");
            if !desc.is_empty() { desc } else { preview(&cmd, 60) }
        }
        "Read" | "Edit" | "Write" | "NotebookEdit" => {
            let p = s("file_path");
            p.rsplit('/').next().unwrap_or(&p).to_string()
        }
        "Grep" => format!("grep \"{}\"", preview(&s("pattern"), 40)),
        "Glob" => format!("glob {}", s("pattern")),
        "WebFetch" | "WebSearch" => s("url").min_or(s("query")),
        _ => preview(&input.to_string(), 60),
    };

    let body = serde_json::to_string_pretty(input).unwrap_or_default();
    (label, body)
}

fn extract_text(v: Option<&serde_json::Value>) -> String {
    let Some(v) = v else { return String::new() };
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    if let Some(items) = v.as_array() {
        let mut out = String::new();
        for item in items {
            if let Some(s) = item.as_str() {
                out.push_str(s);
                out.push('\n');
            } else if let Some(t) = item.get("text").and_then(|x| x.as_str()) {
                out.push_str(t);
                out.push('\n');
            } else if let Some(c) = item.get("content") {
                out.push_str(&extract_text(Some(c)));
            }
        }
        return out;
    }
    if let Some(obj) = v.as_object() {
        if let Some(t) = obj.get("content") {
            return extract_text(Some(t));
        }
        if let Some(t) = obj.get("text").and_then(|x| x.as_str()) {
            return t.to_string();
        }
    }
    v.to_string()
}

fn preview(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ").replace('\r', " ");
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= max {
        s
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// Rescale every event's `t_s` so the last event lands at `target_seconds`.
/// Useful when a multi-hour transcript needs to fit into an N-minute video.
pub fn compress_timeline(mut events: Vec<Event>, target_seconds: f64) -> Vec<Event> {
    if events.is_empty() {
        return events;
    }
    let span = events.last().map(|e| e.t_s).unwrap_or(0.0).max(1.0);
    let scale = target_seconds / span;
    for e in events.iter_mut() {
        e.t_s *= scale;
    }
    events
}

// Tiny helper trait for the (label, body) builder above.
trait MinOr {
    fn min_or(self, other: String) -> String;
}
impl MinOr for String {
    fn min_or(self, other: String) -> String {
        if self.is_empty() { other } else { self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture(tag: &str, lines: &[&str]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("flux-record-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("transcript-{tag}.jsonl"));
        let mut f = std::fs::File::create(&path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        path
    }

    #[test]
    fn parses_assistant_text_and_tool_use() {
        let path = fixture("parses", &[
            r#"{"type":"user","timestamp":"2026-05-29T10:00:00Z","message":{"content":"hi claude"}}"#,
            r#"{"type":"assistant","timestamp":"2026-05-29T10:00:05Z","message":{"content":[{"type":"text","text":"hello back"},{"type":"tool_use","name":"Bash","input":{"command":"ls -la","description":"List files"}}]}}"#,
            r#"{"type":"tool_result","timestamp":"2026-05-29T10:00:06Z","is_error":false,"content":"total 12"}"#,
        ]);
        let events = load(&path).unwrap();
        assert_eq!(events.len(), 4, "user + text + tool_use + tool_result");
        assert!(matches!(events[0].kind, EventKind::User));
        assert!(matches!(events[1].kind, EventKind::AssistantText));
        assert_eq!(events[2].tool_name(), Some("Bash"));
        assert_eq!(events[2].label, "List files");
        assert!(!events[3].is_error());
        // First event is t=0.
        assert!((events[0].t_s - 0.0).abs() < 0.001);
        // Bash should land at +5s.
        assert!((events[2].t_s - 5.0).abs() < 0.001);
    }

    #[test]
    fn flags_error_tool_results() {
        let path = fixture("error", &[
            r#"{"type":"tool_result","timestamp":"2026-05-29T10:00:00Z","is_error":true,"content":"command not found"}"#,
        ]);
        let events = load(&path).unwrap();
        assert!(events[0].is_error());
    }

    #[test]
    fn read_tool_label_uses_filename() {
        let path = fixture("read", &[
            r#"{"type":"assistant","timestamp":"2026-05-29T10:00:00Z","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/some/long/path/to/handlers.rs"}}]}}"#,
        ]);
        let events = load(&path).unwrap();
        assert_eq!(events[0].label, "handlers.rs");
    }

    #[test]
    fn empty_or_garbage_lines_are_skipped() {
        let path = fixture("garbage", &[
            "",
            "not json",
            r#"{"type":"user","timestamp":"2026-05-29T10:00:00Z","message":{"content":"ok"}}"#,
        ]);
        let events = load(&path).unwrap();
        assert_eq!(events.len(), 1);
    }
}
