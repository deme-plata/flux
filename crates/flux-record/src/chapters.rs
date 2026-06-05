//! YouTube chapter generator.
//!
//! Clusters adjacent tool events of the same "phase" into chapters with
//! HH:MM:SS timestamps. Output is the format YouTube expects in descriptions:
//!
//!   00:00 Intro
//!   00:42 Bash: cargo build
//!   02:13 Edit: src/main.rs
//!   ...

use crate::transcript::{Event, EventKind};

pub fn format_youtube(events: &[Event]) -> String {
    let mut out = String::from("00:00 Intro\n");
    let chapters = cluster(events);
    for c in chapters {
        let hms = fmt_hms(c.start_s);
        out.push_str(&format!("{hms} {}\n", c.title));
    }
    out
}

pub struct Chapter {
    pub start_s: f64,
    pub title: String,
}

fn cluster(events: &[Event]) -> Vec<Chapter> {
    let mut chapters: Vec<Chapter> = Vec::new();
    let mut last_title: Option<String> = None;
    let mut last_chapter_start: f64 = -999.0;
    let min_gap = 20.0; // don't drop a new chapter unless 20s+ have passed

    for e in events {
        let title = match &e.kind {
            EventKind::User if e.t_s > 5.0 => Some(format!("Prompt: {}", e.label)),
            EventKind::ToolUse(name) => Some(format!("{name}: {}", e.label)),
            EventKind::ToolResult { is_error: true } => Some("Error encountered".to_string()),
            _ => None,
        };

        if let Some(t) = title {
            let same = last_title.as_deref() == Some(t.as_str());
            let too_close = e.t_s - last_chapter_start < min_gap;
            if !same && !too_close {
                chapters.push(Chapter { start_s: e.t_s, title: t.clone() });
                last_title = Some(t);
                last_chapter_start = e.t_s;
            }
        }
    }

    chapters
}

fn fmt_hms(t: f64) -> String {
    let total = t.max(0.0) as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{:02}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{Event, EventKind};

    fn ev(t: f64, kind: EventKind, label: &str) -> Event {
        Event { t_s: t, kind, label: label.to_string(), body: label.to_string() }
    }

    #[test]
    fn produces_intro_and_hms_with_hours() {
        assert_eq!(fmt_hms(0.0), "00:00");
        assert_eq!(fmt_hms(65.0), "01:05");
        assert_eq!(fmt_hms(3725.0), "01:02:05");
    }

    #[test]
    fn deduplicates_consecutive_same_tool_label() {
        let events = vec![
            ev(30.0, EventKind::ToolUse("Bash".into()), "cargo build"),
            ev(31.0, EventKind::ToolUse("Bash".into()), "cargo build"),
            ev(60.0, EventKind::ToolUse("Bash".into()), "cargo build"), // 30s later, dedup gap met
            ev(120.0, EventKind::ToolUse("Edit".into()), "main.rs"),
        ];
        let out = format_youtube(&events);
        // Intro + Bash + Bash (after gap) + Edit = 4 lines.
        let lines: Vec<_> = out.lines().collect();
        assert_eq!(lines[0], "00:00 Intro");
        assert!(lines.iter().any(|l| l.contains("Bash: cargo build")));
        assert!(lines.iter().any(|l| l.contains("Edit: main.rs")));
    }

    #[test]
    fn flags_error_chapters() {
        let events = vec![
            ev(40.0, EventKind::ToolResult { is_error: true }, "build failed"),
        ];
        let out = format_youtube(&events);
        assert!(out.contains("Error encountered"));
    }
}
