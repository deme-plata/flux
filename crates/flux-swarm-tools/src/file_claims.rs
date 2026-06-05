//! File-level claim layer on top of crate-level swarm claims.
//!
//! The existing `fluxc-core::swarm` is crate-grained: any agent claiming
//! `flux-db` blocks every other agent from touching any file in that crate,
//! even disjoint ones. This module adds a finer-grained layer keyed by
//! absolute file path. It composes with the existing system — a caller
//! typically takes a crate-level claim first (to advertise intent), then a
//! file-level lease for the specific files they're about to edit.
//!
//! Persistence: `/tmp/flux-swarm-files.json`, written through
//! [`super::atomic_lock::with_locked`] so concurrent MCP processes don't
//! lose each other's writes.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::atomic_lock::{with_locked, LockError};
use crate::paths;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileClaim {
    pub path: String,
    pub agent: String,
    pub claimed_at: u64,
    /// Free-text "what am I doing here", surfaced to other agents.
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileClaimStore {
    /// Sorted-by-path for stable on-disk output.
    pub claims: BTreeMap<String, FileClaim>,
}

#[derive(Debug)]
pub enum FileClaimError {
    Locked { path: String, by: String },
    NotHolder { path: String },
    Io(String),
}

impl std::fmt::Display for FileClaimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileClaimError::Locked { path, by } => write!(f, "{} is locked by {}", path, by),
            FileClaimError::NotHolder { path } => write!(f, "{} is not your lock to release", path),
            FileClaimError::Io(s) => write!(f, "file-claim io: {}", s),
        }
    }
}

impl std::error::Error for FileClaimError {}

impl From<LockError> for FileClaimError {
    fn from(e: LockError) -> Self {
        FileClaimError::Io(e.to_string())
    }
}

impl FileClaimStore {
    fn load(bytes: &[u8]) -> Self {
        if bytes.is_empty() {
            return Self::default();
        }
        serde_json::from_slice(bytes).unwrap_or_default()
    }

    fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).unwrap_or_else(|_| b"{}".to_vec())
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Acquire a file-level lease. If any path in `files` is already claimed
/// by a different agent, the whole call fails — no partial acquisition.
/// Re-claiming a path you already hold is a no-op (idempotent, useful for
/// crash-restart).
pub fn claim_files(agent: &str, files: &[&Path], note: &str) -> Result<Vec<FileClaim>, FileClaimError> {
    let path_strings: Vec<String> = files
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let agent = agent.to_string();
    let note = note.to_string();
    let mut acquired: Vec<FileClaim> = Vec::new();
    let mut conflict: Option<(String, String)> = None;

    with_locked(paths::SWARM_LOCK, paths::FILE_CLAIMS, |cur| {
        let mut store = FileClaimStore::load(cur);
        // First pass: detect any conflict before mutating.
        for p in &path_strings {
            if let Some(existing) = store.claims.get(p) {
                if existing.agent != agent {
                    conflict = Some((p.clone(), existing.agent.clone()));
                    return cur.to_vec(); // unchanged
                }
            }
        }
        // Second pass: insert / refresh.
        let now = now_secs();
        for p in &path_strings {
            let claim = FileClaim {
                path: p.clone(),
                agent: agent.clone(),
                claimed_at: now,
                note: note.clone(),
            };
            store.claims.insert(p.clone(), claim.clone());
            acquired.push(claim);
        }
        store.to_bytes()
    })?;

    if let Some((path, by)) = conflict {
        return Err(FileClaimError::Locked { path, by });
    }
    Ok(acquired)
}

/// Release a file-level lease. Idempotent: releasing an unheld path is a
/// no-op (the path simply isn't in the store). Releasing a path held by a
/// different agent errors — use the steal_files helper if that's what you
/// actually mean to do.
pub fn release_files(agent: &str, files: &[&Path]) -> Result<usize, FileClaimError> {
    let path_strings: Vec<String> = files
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let agent = agent.to_string();
    let mut released = 0usize;
    let mut wrong_holder: Option<String> = None;

    with_locked(paths::SWARM_LOCK, paths::FILE_CLAIMS, |cur| {
        let mut store = FileClaimStore::load(cur);
        for p in &path_strings {
            match store.claims.get(p) {
                Some(c) if c.agent != agent => {
                    wrong_holder = Some(p.clone());
                    return cur.to_vec();
                }
                Some(_) => {
                    store.claims.remove(p);
                    released += 1;
                }
                None => {}
            }
        }
        store.to_bytes()
    })?;

    if let Some(path) = wrong_holder {
        return Err(FileClaimError::NotHolder { path });
    }
    Ok(released)
}

/// Force-release a path no matter who holds it. For garbage collection of
/// crashed-agent leases. Returns whether something was actually removed.
pub fn steal_file(path: &Path) -> Result<bool, FileClaimError> {
    let key = path.to_string_lossy().into_owned();
    let mut removed = false;
    with_locked(paths::SWARM_LOCK, paths::FILE_CLAIMS, |cur| {
        let mut store = FileClaimStore::load(cur);
        if store.claims.remove(&key).is_some() {
            removed = true;
        }
        store.to_bytes()
    })?;
    Ok(removed)
}

/// Read-only view of every file currently leased. Sorted by path.
pub fn list_claims() -> Result<Vec<FileClaim>, FileClaimError> {
    let mut out = Vec::new();
    with_locked(paths::SWARM_LOCK, paths::FILE_CLAIMS, |cur| {
        let store = FileClaimStore::load(cur);
        out = store.claims.into_values().collect();
        cur.to_vec() // unchanged
    })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fresh_test_paths(test_name: &str) -> (PathBuf, PathBuf) {
        // Each test uses its own (claims_file, lock_file) so they don't
        // race each other on the real /tmp/flux-swarm-files.json.
        let claims = std::env::temp_dir()
            .join(format!("flux-swarm-tools-claims-{}.json", test_name));
        let lock = claims.with_extension("lock");
        let _ = std::fs::remove_file(&claims);
        let _ = std::fs::remove_file(&lock);
        (claims, lock)
    }

    // The public API uses fixed paths::* constants, so for isolation we
    // exercise the helpers directly with overridden paths. Helper:
    fn do_claim(
        claims: &Path,
        lock: &Path,
        agent: &str,
        files: &[&Path],
        note: &str,
    ) -> Result<Vec<FileClaim>, FileClaimError> {
        let path_strings: Vec<String> = files
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let agent = agent.to_string();
        let note = note.to_string();
        let mut acquired: Vec<FileClaim> = Vec::new();
        let mut conflict: Option<(String, String)> = None;
        with_locked(lock, claims, |cur| {
            let mut store = FileClaimStore::load(cur);
            for p in &path_strings {
                if let Some(existing) = store.claims.get(p) {
                    if existing.agent != agent {
                        conflict = Some((p.clone(), existing.agent.clone()));
                        return cur.to_vec();
                    }
                }
            }
            let now = now_secs();
            for p in &path_strings {
                let c = FileClaim {
                    path: p.clone(),
                    agent: agent.clone(),
                    claimed_at: now,
                    note: note.clone(),
                };
                store.claims.insert(p.clone(), c.clone());
                acquired.push(c);
            }
            store.to_bytes()
        })?;
        if let Some((path, by)) = conflict {
            return Err(FileClaimError::Locked { path, by });
        }
        Ok(acquired)
    }

    #[test]
    fn claim_then_re_claim_same_agent_is_idempotent() {
        let (claims, lock) = fresh_test_paths("idempotent");
        let f = PathBuf::from("/tmp/some-file.rs");
        let a = do_claim(&claims, &lock, "gemini", &[&f], "first").unwrap();
        assert_eq!(a.len(), 1);
        // Re-claim same file as same agent — should not error.
        let b = do_claim(&claims, &lock, "gemini", &[&f], "second").unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].note, "second", "note refreshed on re-claim");
    }

    #[test]
    fn different_agent_claim_collides() {
        let (claims, lock) = fresh_test_paths("collide");
        let f = PathBuf::from("/tmp/some-file.rs");
        do_claim(&claims, &lock, "gemini", &[&f], "mine").unwrap();
        let err = do_claim(&claims, &lock, "deepseek", &[&f], "mine too").unwrap_err();
        match err {
            FileClaimError::Locked { path, by } => {
                assert!(path.ends_with("some-file.rs"));
                assert_eq!(by, "gemini");
            }
            other => panic!("expected Locked, got {:?}", other),
        }
    }

    #[test]
    fn all_or_nothing_on_partial_conflict() {
        // claiming [A, B] where B is held by someone else must leave A
        // un-claimed too — no partial acquisition.
        let (claims, lock) = fresh_test_paths("all-or-nothing");
        let a = PathBuf::from("/tmp/file-A.rs");
        let b = PathBuf::from("/tmp/file-B.rs");
        do_claim(&claims, &lock, "rocky", &[&b], "rocky's").unwrap();
        let err = do_claim(&claims, &lock, "gemini", &[&a, &b], "wants both").unwrap_err();
        assert!(matches!(err, FileClaimError::Locked { .. }));
        // Verify A was NOT claimed by gemini.
        let cur = std::fs::read(&claims).unwrap();
        let store = FileClaimStore::load(&cur);
        assert!(
            store.claims.get(&a.to_string_lossy().into_owned()).is_none(),
            "partial acquisition leaked: {:?}",
            store.claims
        );
    }
}
