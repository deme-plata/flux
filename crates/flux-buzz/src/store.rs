//! Append-only event log, persisted as JSONL.
//!
//! One line per verified event in `events.jsonl` under the data dir. On boot
//! the whole log is loaded into memory (chat-scale, not chain-scale — millions
//! of events would want an indexed store; see flux-db notes before reaching
//! for it: verify OUTCOMES, not timing). Corrupt trailing lines (torn write)
//! are skipped with a warning, matching the SkeletonStore torn-tail stance.

use crate::event::BuzzEvent;
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

pub struct EventStore {
    log_path: PathBuf,
    events: Vec<BuzzEvent>,
    ids: HashSet<String>,
}

impl EventStore {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating data dir {}", dir.display()))?;
        let log_path = dir.join("events.jsonl");
        let mut events = Vec::new();
        let mut ids = HashSet::new();
        if log_path.exists() {
            let file = std::fs::File::open(&log_path)?;
            let mut skipped = 0usize;
            for line in std::io::BufReader::new(file).lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<BuzzEvent>(&line) {
                    Ok(ev) => {
                        if ids.insert(ev.id.clone()) {
                            events.push(ev);
                        }
                    }
                    Err(_) => skipped += 1,
                }
            }
            if skipped > 0 {
                tracing::warn!("event log: skipped {skipped} unparseable line(s)");
            }
        }
        // The log is append-ordered, but clock skew between writers is
        // possible; keep query semantics simple by sorting on load.
        events.sort_by_key(|e| e.created_at);
        Ok(Self { log_path, events, ids })
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Append a (caller-verified) event. Returns false on duplicate id.
    pub fn append(&mut self, ev: BuzzEvent) -> Result<bool> {
        if !self.ids.insert(ev.id.clone()) {
            return Ok(false);
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)?;
        let mut line = serde_json::to_vec(&ev)?;
        line.push(b'\n');
        file.write_all(&line)?;
        file.flush()?;
        self.events.push(ev);
        Ok(true)
    }

    /// Newest-first scan, returned in chronological order. `since` filters on
    /// `created_at > since` (ms cursor); `limit` caps the result.
    pub fn query(
        &self,
        channel: Option<&str>,
        since: u64,
        kind: Option<u32>,
        limit: usize,
    ) -> Vec<BuzzEvent> {
        let mut out: Vec<BuzzEvent> = self
            .events
            .iter()
            .rev()
            .filter(|e| e.created_at > since)
            .filter(|e| kind.is_none_or(|k| e.kind == k))
            .filter(|e| channel.is_none_or(|c| e.channel() == Some(c)))
            .take(limit)
            .cloned()
            .collect();
        out.reverse();
        out
    }

    /// Distinct channels with event count and last-activity timestamp.
    pub fn channels(&self) -> Vec<(String, usize, u64)> {
        let mut map: HashMap<String, (usize, u64)> = HashMap::new();
        for e in &self.events {
            if let Some(c) = e.channel() {
                let entry = map.entry(c.to_string()).or_insert((0, 0));
                entry.0 += 1;
                entry.1 = entry.1.max(e.created_at);
            }
        }
        let mut out: Vec<(String, usize, u64)> =
            map.into_iter().map(|(k, (n, ts))| (k, n, ts)).collect();
        out.sort_by(|a, b| b.2.cmp(&a.2));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Identity, KIND_AGENT_ACTION, KIND_CHAT};

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "flux-buzz-test-{}-{}-{}",
            tag,
            std::process::id(),
            crate::event::now_ms()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn append_reload_roundtrip() {
        let dir = tmp_dir("roundtrip");
        let alice = Identity::generate();
        {
            let mut store = EventStore::open(&dir).unwrap();
            for i in 0..5 {
                let ev = alice.sign_event(
                    KIND_CHAT,
                    vec![vec!["c".into(), "general".into()]],
                    format!("msg {i}"),
                );
                assert!(store.append(ev.clone()).unwrap());
                // duplicate append must be a no-op
                assert!(!store.append(ev).unwrap());
            }
            assert_eq!(store.len(), 5);
        }
        // Reload from disk: every event must come back verifiable.
        let store = EventStore::open(&dir).unwrap();
        assert_eq!(store.len(), 5, "all 5 events must survive reload");
        for ev in store.query(None, 0, None, 100) {
            ev.verify().expect("reloaded event must still verify");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn query_filters() {
        let dir = tmp_dir("filters");
        let human = Identity::generate();
        let agent = Identity::generate();
        let mut store = EventStore::open(&dir).unwrap();
        store
            .append(human.sign_event(
                KIND_CHAT,
                vec![vec!["c".into(), "general".into()]],
                "hi".into(),
            ))
            .unwrap();
        store
            .append(agent.sign_event(
                KIND_AGENT_ACTION,
                vec![vec!["c".into(), "builds".into()]],
                "flux_combo green".into(),
            ))
            .unwrap();
        store
            .append(human.sign_event(
                KIND_CHAT,
                vec![vec!["c".into(), "builds".into()]],
                "nice".into(),
            ))
            .unwrap();

        assert_eq!(store.query(Some("general"), 0, None, 10).len(), 1);
        assert_eq!(store.query(Some("builds"), 0, None, 10).len(), 2);
        assert_eq!(store.query(None, 0, Some(KIND_AGENT_ACTION), 10).len(), 1);
        assert_eq!(store.query(None, 0, None, 2).len(), 2, "limit respected");

        let chans = store.channels();
        assert_eq!(chans.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
