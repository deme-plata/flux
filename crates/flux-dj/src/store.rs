//! Append-only JSONL track-history store.
//!
//! Judgment call (see the flux-dj task report): the workspace's `flux-db` is
//! a full embedded LSM-tree engine built for high-throughput blockchain
//! state, not a 20-lines-of-code fit for "log a track, list recent tracks".
//! An append-only JSONL file is simple, human-inspectable, trivially
//! recoverable (`tail -f`, `grep`, `jq`), and correct for v0's access
//! pattern (append + read-all-then-take-recent). If/when history grows large
//! enough that "read the whole file" stops being fine, swap this module's
//! internals for `flux-db` without changing the public API.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::Track;

pub struct Store {
    path: PathBuf,
}

impl Store {
    /// Open (creating the parent directory if needed) a store backed by the
    /// JSONL file at `path`. Does not require the file itself to exist yet.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating data dir {}", parent.display()))?;
        }
        Ok(Self { path })
    }

    /// Default JSONL path: `$FLUX_DJ_DATA_DIR/tracks.jsonl`, falling back to
    /// a data dir under the Flux workspace when the env var isn't set.
    pub fn default_path() -> PathBuf {
        let dir = std::env::var("FLUX_DJ_DATA_DIR").unwrap_or_else(|_| "/home/storage/deepseek-codewhale/flux/data/flux-dj".to_string());
        Path::new(&dir).join("tracks.jsonl")
    }

    /// Open the default store (see [`Store::default_path`]).
    pub fn open_default() -> Result<Self> {
        Self::new(Self::default_path())
    }

    /// Append one track as a JSON line.
    pub fn append(&self, track: &Track) -> Result<()> {
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("opening {}", self.path.display()))?;
        let line = serde_json::to_string(track).context("serializing track")?;
        writeln!(f, "{line}").with_context(|| format!("writing to {}", self.path.display()))?;
        Ok(())
    }

    /// Most recent `limit` tracks, newest first. Malformed lines (should
    /// never happen given `append` always writes valid JSON, but a hand-
    /// edited file could produce one) are skipped rather than aborting the
    /// whole read.
    pub fn recent(&self, limit: usize) -> Result<Vec<Track>> {
        let content = match fs::read_to_string(&self.path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", self.path.display())),
        };
        let mut tracks: Vec<Track> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<Track>(l).ok())
            .collect();
        tracks.reverse();
        tracks.truncate(limit);
        Ok(tracks)
    }

    /// Total number of logged tracks (all-time, not just `recent`'s window).
    pub fn count(&self) -> Result<usize> {
        match fs::read_to_string(&self.path) {
            Ok(c) => Ok(c.lines().filter(|l| !l.trim().is_empty()).count()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(e) => Err(e).with_context(|| format!("reading {}", self.path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NewTrackRequest;

    fn track(artist: &str, title: &str, ts: &str) -> Track {
        NewTrackRequest {
            artist: artist.to_string(),
            title: title.to_string(),
            bpm: Some(128.0),
            key: Some("8A".to_string()),
            timestamp: Some(ts.to_string()),
        }
        .into_track()
    }

    #[test]
    fn append_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tracks.jsonl")).unwrap();
        store.append(&track("Valeria", "Track One", "2026-08-11T10:00:00Z")).unwrap();
        store.append(&track("Valeria", "Track Two", "2026-08-11T10:05:00Z")).unwrap();

        let recent = store.recent(10).unwrap();
        assert_eq!(recent.len(), 2);
        // newest first
        assert_eq!(recent[0].title, "Track Two");
        assert_eq!(recent[1].title, "Track One");
        assert_eq!(store.count().unwrap(), 2);
    }

    #[test]
    fn recent_respects_limit() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tracks.jsonl")).unwrap();
        for i in 0..5 {
            store.append(&track("A", &format!("T{i}"), "2026-08-11T10:00:00Z")).unwrap();
        }
        let recent = store.recent(2).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].title, "T4");
        assert_eq!(recent[1].title, "T3");
    }

    #[test]
    fn missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("does-not-exist.jsonl")).unwrap();
        assert_eq!(store.recent(10).unwrap(), Vec::new());
        assert_eq!(store.count().unwrap(), 0);
    }
}
