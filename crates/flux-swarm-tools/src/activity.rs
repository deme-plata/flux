//! Append-only activity log for the swarm.
//!
//! Every transition writes one JSON object per line via a single `write`
//! syscall under `O_APPEND` semantics — POSIX guarantees those are atomic
//! up to PIPE_BUF (4 KiB on Linux), so even concurrent MCP processes can
//! append without losing or interleaving each other's lines.
//!
//! This is the audit trail the existing JSON state file can't give: that
//! file is rewritten in full on every change, so concurrent writers
//! clobber each other. The log keeps every event regardless.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Registered,
    Claimed,
    Completed,
    Released,
    FileClaimed,
    FileReleased,
    /// Any other event a caller wants in the audit trail — stored under
    /// the `Custom(label)` form so we don't need to grow the enum every
    /// time someone adds a new transition.
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Activity {
    pub at: u64,
    pub agent: String,
    pub kind: ActivityKind,
    /// Free-text detail, e.g. "claimed flux-db", "settled 0.50 QUG".
    pub detail: String,
}

impl Activity {
    pub fn new(agent: &str, kind: ActivityKind, detail: impl Into<String>) -> Self {
        Self {
            at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            agent: agent.to_string(),
            kind,
            detail: detail.into(),
        }
    }
}

/// Append-only log handle.
pub struct ActivityLog {
    path: PathBuf,
}

impl Default for ActivityLog {
    fn default() -> Self {
        Self::at(paths::ACTIVITY_LOG)
    }
}

impl ActivityLog {
    pub fn at(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Append one event. Each event is one line; serialisation errors
    /// are swallowed (better to lose audit lines than to fail a swarm
    /// operation on logging — telemetry is best-effort).
    pub fn record(&self, ev: &Activity) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(ev).unwrap_or_else(|_| b"{}".to_vec());
        line.push(b'\n');
        // O_APPEND so the write atomically lands at end-of-file.
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        f.write_all(&line)?;
        Ok(())
    }

    /// Read all events in chronological order. For audit dumps and tests.
    pub fn read_all(&self) -> std::io::Result<Vec<Activity>> {
        let bytes = match std::fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out = Vec::new();
        for line in bytes.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            if let Ok(ev) = serde_json::from_slice::<Activity>(line) {
                out.push(ev);
            }
        }
        Ok(out)
    }

    /// All events for one agent, oldest-first.
    pub fn read_for_agent(&self, agent: &str) -> std::io::Result<Vec<Activity>> {
        Ok(self
            .read_all()?
            .into_iter()
            .filter(|e| e.agent == agent)
            .collect())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_log(name: &str) -> ActivityLog {
        let p = std::env::temp_dir().join(format!("flux-swarm-activity-test-{}.jsonl", name));
        let _ = std::fs::remove_file(&p);
        ActivityLog::at(p)
    }

    #[test]
    fn append_and_read_back() {
        let log = fresh_log("append-read");
        log.record(&Activity::new("gemini", ActivityKind::Claimed, "flux-db"))
            .unwrap();
        log.record(&Activity::new(
            "gemini",
            ActivityKind::Completed,
            "0.50 QUG",
        ))
        .unwrap();
        let evs = log.read_all().unwrap();
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].agent, "gemini");
        assert!(matches!(evs[0].kind, ActivityKind::Claimed));
        assert!(matches!(evs[1].kind, ActivityKind::Completed));
    }

    #[test]
    fn per_agent_filter() {
        let log = fresh_log("per-agent");
        log.record(&Activity::new("gemini", ActivityKind::Claimed, "x"))
            .unwrap();
        log.record(&Activity::new("deepseek", ActivityKind::Claimed, "y"))
            .unwrap();
        log.record(&Activity::new("gemini", ActivityKind::Completed, "x done"))
            .unwrap();
        let g = log.read_for_agent("gemini").unwrap();
        let d = log.read_for_agent("deepseek").unwrap();
        assert_eq!(g.len(), 2);
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn concurrent_appends_dont_lose_lines() {
        // Two threads each write a batch. O_APPEND atomicity per line
        // means we should see every event, even if their order is
        // interleaved.
        let log = fresh_log("concurrent");
        let p = log.path().to_path_buf();
        let log2 = ActivityLog::at(p);
        let handle = std::thread::spawn(move || {
            for i in 0..50 {
                log2.record(&Activity::new(
                    "deepseek",
                    ActivityKind::Custom("ping".into()),
                    format!("d{}", i),
                ))
                .unwrap();
            }
        });
        for i in 0..50 {
            log.record(&Activity::new(
                "gemini",
                ActivityKind::Custom("ping".into()),
                format!("g{}", i),
            ))
            .unwrap();
        }
        handle.join().unwrap();
        let evs = log.read_all().unwrap();
        assert_eq!(evs.len(), 100, "lost lines under concurrency");
        let g = evs.iter().filter(|e| e.agent == "gemini").count();
        let d = evs.iter().filter(|e| e.agent == "deepseek").count();
        assert_eq!(g, 50);
        assert_eq!(d, 50);
    }

    #[test]
    fn read_empty_returns_empty() {
        let log = fresh_log("empty");
        let evs = log.read_all().unwrap();
        assert!(evs.is_empty());
    }

    #[test]
    fn custom_kind_round_trips() {
        let log = fresh_log("custom");
        log.record(&Activity::new(
            "gemini",
            ActivityKind::Custom("merge_review".into()),
            "PR#42",
        ))
        .unwrap();
        let evs = log.read_all().unwrap();
        assert_eq!(evs.len(), 1);
        match &evs[0].kind {
            ActivityKind::Custom(s) => assert_eq!(s, "merge_review"),
            other => panic!("expected Custom, got {:?}", other),
        }
    }
}
