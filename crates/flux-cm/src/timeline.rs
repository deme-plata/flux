//! The timeline: one append-only event log for the whole community desk.
//!
//! Nothing here is ever edited or deleted — the timeline is the audit trail
//! that makes "who did what, when, and what did it pay" answerable in one
//! query, per CM or org-wide.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    Onboarded,
    StatusChanged,
    WorkPosted,
    Assigned,
    Submitted,
    Approved,
    Paid,
    Cancelled,
    Note,
    Kudos,
}

impl EventKind {
    pub fn glyph(&self) -> &'static str {
        match self {
            EventKind::Onboarded => "🤝",
            EventKind::StatusChanged => "🔄",
            EventKind::WorkPosted => "📋",
            EventKind::Assigned => "🎯",
            EventKind::Submitted => "📤",
            EventKind::Approved => "✅",
            EventKind::Paid => "💰",
            EventKind::Cancelled => "🛑",
            EventKind::Note => "📝",
            EventKind::Kudos => "🌟",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEvent {
    pub ts_ms: u64,
    /// CM the event is about, if any.
    pub cm_id: Option<String>,
    /// Work item the event is about, if any.
    pub work_id: Option<String>,
    pub kind: EventKind,
    pub detail: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Timeline {
    events: Vec<TimelineEvent>,
}

impl Timeline {
    pub fn record(
        &mut self,
        ts_ms: u64,
        cm_id: Option<&str>,
        work_id: Option<&str>,
        kind: EventKind,
        detail: impl Into<String>,
    ) {
        self.events.push(TimelineEvent {
            ts_ms,
            cm_id: cm_id.map(|s| s.to_string()),
            work_id: work_id.map(|s| s.to_string()),
            kind,
            detail: detail.into(),
        });
    }

    pub fn all(&self) -> &[TimelineEvent] {
        &self.events
    }

    /// Every event touching one CM, oldest first.
    pub fn for_cm(&self, cm_id: &str) -> Vec<&TimelineEvent> {
        self.events
            .iter()
            .filter(|e| e.cm_id.as_deref() == Some(cm_id))
            .collect()
    }

    /// Every event touching one work item, oldest first.
    pub fn for_work(&self, work_id: &str) -> Vec<&TimelineEvent> {
        self.events
            .iter()
            .filter(|e| e.work_id.as_deref() == Some(work_id))
            .collect()
    }

    /// Events in [from_ts_ms, to_ts_ms), oldest first.
    pub fn between(&self, from_ts_ms: u64, to_ts_ms: u64) -> Vec<&TimelineEvent> {
        self.events
            .iter()
            .filter(|e| e.ts_ms >= from_ts_ms && e.ts_ms < to_ts_ms)
            .collect()
    }

    /// Markdown render of a slice of events — one line per event:
    /// `2026-08-31 14:02 · 💰 Paid · 300 QUG to @mira (tx 0xabc…)`
    pub fn render_markdown(events: &[&TimelineEvent]) -> String {
        let mut out = String::new();
        for e in events {
            let (y, mo, d, h, mi) = utc_datetime(e.ts_ms);
            out.push_str(&format!(
                "{y:04}-{mo:02}-{d:02} {h:02}:{mi:02} · {} {:?} · {}\n",
                e.kind.glyph(),
                e.kind,
                e.detail
            ));
        }
        out
    }
}

/// Unix ms → (year, month, day, hour, minute) in UTC.
/// Civil-from-days per Howard Hinnant's algorithm — no chrono dependency.
pub fn utc_datetime(ts_ms: u64) -> (i64, u32, u32, u32, u32) {
    let secs = (ts_ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let hour = (rem / 3600) as u32;
    let minute = ((rem % 3600) / 60) as u32;

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d, hour, minute)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_date_known_values() {
        // 1970-01-01 00:00 UTC.
        assert_eq!(utc_datetime(0), (1970, 1, 1, 0, 0));
        // 2026-08-31 12:30:00 UTC = 1788179400 s (verified against date -u).
        assert_eq!(utc_datetime(1_788_179_400_000), (2026, 8, 31, 12, 30));
    }

    #[test]
    fn per_cm_filter_and_render() {
        let mut tl = Timeline::default();
        tl.record(1_000, Some("cm-a"), None, EventKind::Onboarded, "welcome @a");
        tl.record(2_000, Some("cm-b"), None, EventKind::Onboarded, "welcome @b");
        tl.record(3_000, Some("cm-a"), Some("wk-1"), EventKind::Assigned, "a takes wk-1");
        let a = tl.for_cm("cm-a");
        assert_eq!(a.len(), 2);
        let md = Timeline::render_markdown(&a);
        assert!(md.contains("🤝 Onboarded · welcome @a"));
        assert!(md.contains("🎯 Assigned · a takes wk-1"));
        assert_eq!(tl.between(1_500, 2_500).len(), 1);
    }
}
