//! diff — differential context updates (Task 2 of `docs/1M_CONTEXT_WINDOW_PLAN.md`).
//!
//! Instead of rebuilding the full 1M context every session, snapshot the workspace
//! fingerprints and diff against the last snapshot — only changed chunks get
//! re-serialized. Reuses **flux-rev**: BLAKE3 content fingerprints come from
//! [`crate::chunk`] (which calls `flux_rev::hash_bytes`), and snapshots persist
//! immutably in a content-addressed `flux_rev::Store` under `.flux-rev/`.

use crate::chunk::ChunkManifest;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Fingerprint of one chunk at snapshot time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChunkFingerprint {
    pub blake3_hex: String,
    pub mtime_ns: u64,
    pub ripple_score: f64,
    pub token_count: u64,
}

/// A point-in-time fingerprint of the whole workspace context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSnapshot {
    pub version: u64,
    pub created_at_ns: u64,
    pub workspace: String,
    pub total_tokens: u64,
    /// key = chunk path (crate dir, relative to workspace root)
    pub chunks: HashMap<String, ChunkFingerprint>,
}

impl ContextSnapshot {
    pub fn from_manifest(m: &ChunkManifest, version: u64, created_at_ns: u64) -> Self {
        let chunks = m
            .chunks
            .iter()
            .map(|c| {
                (
                    c.path.clone(),
                    ChunkFingerprint {
                        blake3_hex: c.blake3_hex.clone(),
                        mtime_ns: c.mtime_ns,
                        ripple_score: c.ripple_score,
                        token_count: c.estimated_tokens,
                    },
                )
            })
            .collect();
        ContextSnapshot {
            version,
            created_at_ns,
            workspace: m.workspace.clone(),
            total_tokens: m.total_tokens_estimated,
            chunks,
        }
    }
}

/// The diff between two snapshots.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContextDiff {
    pub from_version: u64,
    pub to_version: u64,
    pub added: Vec<String>,
    /// content hash changed
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
    /// same content, ripple shifted because the dep graph moved elsewhere
    pub stale_ripple: Vec<(String, f64, f64)>,
    pub delta_tokens: i64,
}

impl ContextDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.modified.is_empty()
            && self.deleted.is_empty()
            && self.stale_ripple.is_empty()
    }
    /// Paths whose serialized chunk must be regenerated (added ∪ modified).
    pub fn dirty(&self) -> Vec<&String> {
        self.added.iter().chain(self.modified.iter()).collect()
    }
}

/// Compute the diff `old` → `new`. Content (blake3) is authoritative for MODIFIED;
/// a same-content chunk whose ripple shifted is STALE (re-rank, no re-serialize).
pub fn diff_snapshots(old: &ContextSnapshot, new: &ContextSnapshot) -> ContextDiff {
    let mut d = ContextDiff {
        from_version: old.version,
        to_version: new.version,
        delta_tokens: new.total_tokens as i64 - old.total_tokens as i64,
        ..Default::default()
    };
    for (path, nf) in &new.chunks {
        match old.chunks.get(path) {
            None => d.added.push(path.clone()),
            Some(of) => {
                if of.blake3_hex != nf.blake3_hex {
                    d.modified.push(path.clone());
                } else if (of.ripple_score - nf.ripple_score).abs() > 1e-6 {
                    d.stale_ripple.push((path.clone(), of.ripple_score, nf.ripple_score));
                }
            }
        }
    }
    for path in old.chunks.keys() {
        if !new.chunks.contains_key(path) {
            d.deleted.push(path.clone());
        }
    }
    d.added.sort();
    d.modified.sort();
    d.deleted.sort();
    d.stale_ripple.sort_by(|a, b| a.0.cmp(&b.0));
    d
}

// ── persistence: flux-rev content-addressed Store + a small version index ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct VersionIndex {
    latest: u64,
    versions: Vec<(u64, String)>, // (version, snapshot blob hash)
}

fn index_path(ctx_dir: &Path) -> std::path::PathBuf {
    ctx_dir.join("snapshots-index.json")
}

fn load_index(ctx_dir: &Path) -> VersionIndex {
    std::fs::read(index_path(ctx_dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Save `snap` to the flux-rev Store under `ctx_dir/.flux-rev` and record it as
/// latest. Returns the content hash of the stored snapshot blob.
pub fn save_snapshot(ctx_dir: &Path, snap: &ContextSnapshot) -> std::io::Result<String> {
    std::fs::create_dir_all(ctx_dir)?;
    let store = flux_rev::Store::open(ctx_dir)?;
    let bytes = serde_json::to_vec(snap)?;
    let hash = store.put(&bytes)?;
    let mut idx = load_index(ctx_dir);
    idx.versions.retain(|(v, _)| *v != snap.version);
    idx.versions.push((snap.version, hash.clone()));
    idx.versions.sort_by_key(|(v, _)| *v);
    idx.latest = snap.version;
    std::fs::write(index_path(ctx_dir), serde_json::to_vec_pretty(&idx)?)?;
    Ok(hash)
}

/// Load the latest snapshot, if any.
pub fn load_latest(ctx_dir: &Path) -> Option<ContextSnapshot> {
    let idx = load_index(ctx_dir);
    let (_, hash) = idx.versions.iter().find(|(v, _)| *v == idx.latest)?;
    let store = flux_rev::Store::open(ctx_dir).ok()?;
    let bytes = store.get(hash).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Next version = latest + 1 (1 if no snapshots yet).
pub fn next_version(ctx_dir: &Path) -> u64 {
    let idx = load_index(ctx_dir);
    if idx.versions.is_empty() {
        1
    } else {
        idx.latest + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(h: &str, ripple: f64, tok: u64) -> ChunkFingerprint {
        ChunkFingerprint { blake3_hex: h.into(), mtime_ns: 0, ripple_score: ripple, token_count: tok }
    }
    fn snap(version: u64, items: &[(&str, ChunkFingerprint)]) -> ContextSnapshot {
        ContextSnapshot {
            version,
            created_at_ns: 0,
            workspace: "ws".into(),
            total_tokens: items.iter().map(|(_, f)| f.token_count).sum(),
            chunks: items.iter().map(|(p, f)| (p.to_string(), f.clone())).collect(),
        }
    }

    #[test]
    fn no_change_is_empty() {
        let a = snap(1, &[("a", fp("h1", 0.5, 10)), ("b", fp("h2", 0.3, 20))]);
        let d = diff_snapshots(&a, &a);
        assert!(d.is_empty(), "{d:?}");
        assert_eq!(d.delta_tokens, 0);
    }

    #[test]
    fn detects_add_modify_delete_stale() {
        let old = snap(
            1,
            &[("a", fp("h1", 0.5, 10)), ("b", fp("h2", 0.3, 20)), ("c", fp("h3", 0.9, 5))],
        );
        let new = snap(
            2,
            &[
                ("a", fp("h1", 0.5, 10)),  // unchanged
                ("b", fp("h2b", 0.3, 25)), // modified (hash changed)
                ("d", fp("h4", 0.1, 7)),   // added
                ("c", fp("h3", 0.6, 5)),   // stale ripple (same hash, 0.9→0.6)
            ],
        );
        let d = diff_snapshots(&old, &new);
        assert_eq!(d.added, vec!["d"]);
        assert_eq!(d.modified, vec!["b"]);
        assert!(d.deleted.is_empty());
        assert_eq!(d.stale_ripple.len(), 1);
        assert_eq!(d.stale_ripple[0].0, "c");
        // old tokens = 35, new = 10+25+7+5 = 47 → +12
        assert_eq!(d.delta_tokens, 12);
        assert_eq!(d.dirty().len(), 2); // b (modified) + d (added)
    }

    #[test]
    fn store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("flux-ctx-difftest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = snap(1, &[("a", fp("h1", 0.5, 10))]);
        let h = save_snapshot(&dir, &s).expect("save");
        assert!(!h.is_empty());
        let loaded = load_latest(&dir).expect("load");
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.chunks.len(), 1);
        assert_eq!(next_version(&dir), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
