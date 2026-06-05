//! Build animated overlay "cards" from a transcript event stream.

use crate::transcript::{Event, EventKind};

pub struct Card {
    pub start_s: f64,
    pub dur_s: f64,
    pub badge: String,
    pub label: String,
    /// Hex literal like "0x4f46e5" used inside drawtext boxcolor.
    pub color: &'static str,
}

pub fn build_cards(events: &[Event]) -> Vec<Card> {
    let mut out = Vec::new();
    for e in events {
        let (badge, color) = match &e.kind {
            EventKind::ToolUse(name) => (name.to_uppercase(), color_for(name)),
            EventKind::AssistantText => ("CLAUDE".to_string(), "0x4f46e5"),
            EventKind::User => ("YOU".to_string(), "0x059669"),
            EventKind::ToolResult { is_error: true } => ("ERROR".to_string(), "0xdc2626"),
            EventKind::ToolResult { is_error: false } => continue, // too noisy
            EventKind::System => continue,
        };
        if e.label.trim().is_empty() {
            continue;
        }
        out.push(Card {
            start_s: e.t_s,
            dur_s: 3.0,
            badge,
            label: e.label.clone(),
            color,
        });
    }
    out
}

fn color_for(tool: &str) -> &'static str {
    match tool {
        "Bash" => "0x16a34a",
        "Read" => "0x2563eb",
        "Edit" | "Write" | "NotebookEdit" => "0xea580c",
        "Grep" | "Glob" => "0x7c3aed",
        "WebFetch" | "WebSearch" => "0x0891b2",
        "Task" | "Agent" => "0xdb2777",
        // Vite / browser events from the control server.
        "HMR" => "0xf59e0b",
        "DOM" => "0x14b8a6",
        "ROUTE" => "0x6366f1",
        t if t.starts_with("CONSOLE") => "0x64748b",
        _ => "0x4f46e5",
    }
}
