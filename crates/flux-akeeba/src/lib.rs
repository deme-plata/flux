// flux-akeeba — Akeeba-Backup-style safety for the flux auto-updater.
//
// Joomla's Akeeba makes an update safe by taking a verifiable full backup first and
// offering one-click restore. flux-akeeba brings that to flux-hotswap/flux-self-heal:
// integrity-verified, CONTENT-ADDRESSED snapshots of {binary, state, config} taken
// automatically BEFORE every update, with point-in-time restore. On a failed
// post-update health check, self-heal restores the latest good snapshot.
//
// Store layout (a single directory):
//   <store>/blobs/<b3hex>   — component bytes, content-addressed → automatic dedup
//   <store>/manifest.json   — { next_id, snapshots: [Snapshot, …] }
//
// Incremental vs full: every snapshot's manifest is self-contained (restore needs no
// parent chase) — "incremental" just means most blobs already exist in the store and
// aren't rewritten. Dedup is by construction; integrity is by construction.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// blake3 hex digest — a component's content identity.
pub type B3 = String;

/// A backup kind, mirroring Akeeba's full vs incremental archives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Full,
    Incremental,
}

/// One backed-up component (binary / state tar / config), addressed by content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Component {
    pub name: String,
    pub b3: B3,
    pub size: u64,
}

/// A point-in-time snapshot — the unit you restore to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: u64,
    pub label: String,
    pub kind: Kind,
    pub parent: Option<u64>,
    pub created: u64,
    pub components: BTreeMap<String, Component>,
    /// blake3 over the sorted `name:b3` lines — the snapshot's identity (flux://b3/…).
    pub archive_b3: B3,
    pub total_size: u64,
}

/// What a `backup()` produced, plus how much was actually new (the incremental win).
#[derive(Clone, Debug)]
pub struct BackupReport {
    pub snapshot: Snapshot,
    /// Number of component blobs newly written this run (unchanged ones were deduped).
    pub blobs_written: usize,
    pub blobs_deduped: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Manifest {
    next_id: u64,
    snapshots: Vec<Snapshot>,
}

/// blake3 hex of arbitrary bytes — the content-address primitive.
pub fn b3_hex(bytes: &[u8]) -> B3 {
    blake3::hash(bytes).to_hex().to_string()
}

/// A component reference for `bytes`, content-addressed.
pub fn component(name: &str, bytes: &[u8]) -> Component {
    Component { name: name.to_string(), b3: b3_hex(bytes), size: bytes.len() as u64 }
}

/// Integrity check (the Akeeba "kickstart" guard): do these bytes match the recorded hash?
pub fn verify_bytes(c: &Component, bytes: &[u8]) -> bool {
    c.size == bytes.len() as u64 && c.b3 == b3_hex(bytes)
}

/// The flux:// URI for a content hash.
pub fn flux_uri(b3: &str) -> String {
    format!("flux://b3/{b3}")
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Atomic write: stage to `<path>.tmp` then rename over the target.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let tmp = path.with_extension("tmp.flxak");
    fs::write(&tmp, bytes).with_context(|| format!("stage {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("commit {}", path.display()))?;
    Ok(())
}

/// The backup engine — owns a content-addressed store and a snapshot manifest.
pub struct BackupEngine {
    store: PathBuf,
    manifest: Manifest,
}

impl BackupEngine {
    /// Open (or create) a backup store at `store`.
    pub fn open(store: impl AsRef<Path>) -> Result<Self> {
        let store = store.as_ref().to_path_buf();
        fs::create_dir_all(store.join("blobs")).context("create blob store")?;
        let mpath = store.join("manifest.json");
        let manifest = if mpath.exists() {
            serde_json::from_slice(&fs::read(&mpath)?).context("parse manifest")?
        } else {
            Manifest::default()
        };
        Ok(Self { store, manifest })
    }

    fn blob_path(&self, b3: &str) -> PathBuf {
        self.store.join("blobs").join(b3)
    }

    fn persist(&self) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(&self.manifest)?;
        atomic_write(&self.store.join("manifest.json"), &bytes)
    }

    /// Take a snapshot of `items` (logical name → bytes). Blobs already in the store are
    /// deduped (the incremental win); the snapshot manifest is always self-contained.
    pub fn backup(&mut self, items: &[(&str, &[u8])], kind: Kind, label: &str) -> Result<BackupReport> {
        let parent = self.manifest.snapshots.last().map(|s| s.id);
        let mut components = BTreeMap::new();
        let (mut written, mut deduped, mut total) = (0usize, 0usize, 0u64);
        for (name, bytes) in items {
            let c = component(name, bytes);
            let bp = self.blob_path(&c.b3);
            if bp.exists() {
                deduped += 1;
            } else {
                atomic_write(&bp, bytes)?;
                written += 1;
            }
            total += c.size;
            components.insert(name.to_string(), c);
        }
        let archive_b3 = {
            let mut s = String::new();
            for (n, c) in &components {
                s.push_str(n);
                s.push(':');
                s.push_str(&c.b3);
                s.push('\n');
            }
            b3_hex(s.as_bytes())
        };
        let id = self.manifest.next_id;
        self.manifest.next_id += 1;
        let snap = Snapshot {
            id,
            label: label.to_string(),
            kind,
            parent,
            created: now_unix(),
            components,
            archive_b3,
            total_size: total,
        };
        self.manifest.snapshots.push(snap.clone());
        self.persist()?;
        Ok(BackupReport { snapshot: snap, blobs_written: written, blobs_deduped: deduped })
    }

    /// The updater hook: full on first run, incremental thereafter.
    pub fn backup_before_update(&mut self, items: &[(&str, &[u8])]) -> Result<BackupReport> {
        let kind = if self.manifest.snapshots.is_empty() { Kind::Full } else { Kind::Incremental };
        self.backup(items, kind, "pre-update")
    }

    pub fn snapshots(&self) -> &[Snapshot] {
        &self.manifest.snapshots
    }

    pub fn get(&self, id: u64) -> Option<&Snapshot> {
        self.manifest.snapshots.iter().find(|s| s.id == id)
    }

    pub fn latest(&self) -> Option<&Snapshot> {
        self.manifest.snapshots.last()
    }

    /// Read one component's bytes from a snapshot, INTEGRITY-CHECKED (refuses corrupt data).
    pub fn read_component(&self, id: u64, name: &str) -> Result<Vec<u8>> {
        let snap = self.get(id).ok_or_else(|| anyhow!("no snapshot {id}"))?;
        let c = snap.components.get(name).ok_or_else(|| anyhow!("no component {name} in snapshot {id}"))?;
        let bytes = fs::read(self.blob_path(&c.b3)).with_context(|| format!("read blob {}", c.b3))?;
        if !verify_bytes(c, &bytes) {
            return Err(anyhow!("INTEGRITY FAILURE: blob {} for {name} does not match recorded hash — refusing to restore corrupt data", c.b3));
        }
        Ok(bytes)
    }

    /// Verify every component blob of a snapshot still matches its recorded hash.
    pub fn verify(&self, id: u64) -> bool {
        let snap = match self.get(id) {
            Some(s) => s,
            None => return false,
        };
        snap.components.values().all(|c| match fs::read(self.blob_path(&c.b3)) {
            Ok(b) => verify_bytes(c, &b),
            Err(_) => false,
        })
    }

    /// The newest snapshot that passes a full integrity verify — what self-heal restores to.
    pub fn latest_good(&self) -> Option<&Snapshot> {
        self.manifest.snapshots.iter().rev().find(|s| self.verify(s.id))
    }

    /// Atomically restore a snapshot's components to the given target paths.
    /// Each component is integrity-checked before it touches the live path.
    pub fn restore_to(&self, id: u64, paths: &BTreeMap<String, PathBuf>) -> Result<()> {
        let snap = self.get(id).ok_or_else(|| anyhow!("no snapshot {id}"))?;
        // verify ALL first (fail before writing anything — all-or-nothing)
        let mut staged: Vec<(&PathBuf, Vec<u8>)> = Vec::new();
        for (name, target) in paths {
            if !snap.components.contains_key(name) {
                return Err(anyhow!("snapshot {id} has no component {name}"));
            }
            staged.push((target, self.read_component(id, name)?));
        }
        for (target, bytes) in staged {
            atomic_write(target, &bytes)?;
        }
        Ok(())
    }

    /// flux:// URI of a snapshot's archive identity.
    pub fn uri(&self, id: u64) -> Option<String> {
        self.get(id).map(|s| flux_uri(&s.archive_b3))
    }

    /// Keep the newest `retain` snapshots; drop older ones and GC any blob no longer
    /// referenced by a surviving snapshot. Returns (snapshots_dropped, blobs_collected).
    pub fn prune(&mut self, retain: usize) -> Result<(usize, usize)> {
        let n = self.manifest.snapshots.len();
        if n <= retain {
            return Ok((0, 0));
        }
        let drop = n - retain;
        self.manifest.snapshots.drain(0..drop);
        // GC: any blob not referenced by a surviving snapshot
        let mut live = std::collections::HashSet::new();
        for s in &self.manifest.snapshots {
            for c in s.components.values() {
                live.insert(c.b3.clone());
            }
        }
        let mut collected = 0usize;
        for entry in fs::read_dir(self.store.join("blobs"))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !live.contains(&name) {
                fs::remove_file(entry.path()).ok();
                collected += 1;
            }
        }
        self.persist()?;
        Ok((drop, collected))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "flux-akeeba-{tag}-{}-{}",
            std::process::id(),
            now_unix_nanos()
        ));
        let _ = fs::remove_dir_all(&p);
        p
    }
    fn now_unix_nanos() -> u128 {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
    }

    #[test]
    fn content_address_and_integrity_gate() {
        let c = component("flux-bin", b"good binary");
        assert!(verify_bytes(&c, b"good binary"));
        assert!(!verify_bytes(&c, b"tampered binary"));
        assert_eq!(flux_uri(&c.b3), format!("flux://b3/{}", c.b3));
    }

    #[test]
    fn backup_restore_round_trip() {
        let store = tmp_store("rt");
        let mut e = BackupEngine::open(&store).unwrap();
        let r = e.backup(&[("bin", b"v1"), ("state", b"s1")], Kind::Full, "first").unwrap();
        assert_eq!(r.blobs_written, 2);
        assert_eq!(e.read_component(r.snapshot.id, "bin").unwrap(), b"v1");
        assert_eq!(e.read_component(r.snapshot.id, "state").unwrap(), b"s1");
        fs::remove_dir_all(&store).ok();
    }

    #[test]
    fn incremental_dedups_unchanged_components() {
        let store = tmp_store("inc");
        let mut e = BackupEngine::open(&store).unwrap();
        e.backup(&[("bin", b"v1"), ("state", b"s1")], Kind::Full, "a").unwrap();
        // change bin only; state unchanged → only 1 new blob written
        let r = e.backup_before_update(&[("bin", b"v2"), ("state", b"s1")]).unwrap();
        assert_eq!(r.snapshot.kind, Kind::Incremental);
        assert_eq!(r.blobs_written, 1, "only the changed bin blob is new");
        assert_eq!(r.blobs_deduped, 1, "state was deduped");
        fs::remove_dir_all(&store).ok();
    }

    #[test]
    fn point_in_time_restore() {
        let store = tmp_store("pit");
        let mut e = BackupEngine::open(&store).unwrap();
        let a = e.backup(&[("bin", b"v1")], Kind::Full, "a").unwrap().snapshot.id;
        let b = e.backup(&[("bin", b"v2")], Kind::Incremental, "b").unwrap().snapshot.id;
        assert_eq!(e.read_component(a, "bin").unwrap(), b"v1");
        assert_eq!(e.read_component(b, "bin").unwrap(), b"v2");
        // restore the OLDER snapshot to a live path
        let live = store.join("live-bin");
        let mut map = BTreeMap::new();
        map.insert("bin".to_string(), live.clone());
        e.restore_to(a, &map).unwrap();
        assert_eq!(fs::read(&live).unwrap(), b"v1");
        fs::remove_dir_all(&store).ok();
    }

    #[test]
    fn integrity_failure_blocks_restore() {
        let store = tmp_store("tamper");
        let mut e = BackupEngine::open(&store).unwrap();
        let id = e.backup(&[("bin", b"v1")], Kind::Full, "a").unwrap().snapshot.id;
        assert!(e.verify(id));
        // tamper the stored blob
        let c = e.get(id).unwrap().components.get("bin").unwrap().clone();
        fs::write(e.blob_path(&c.b3), b"corrupted!").unwrap();
        assert!(!e.verify(id), "verify must catch the tamper");
        assert!(e.read_component(id, "bin").is_err(), "restore must refuse corrupt data");
        assert!(e.latest_good().is_none(), "no good snapshot once the only one is corrupt");
        fs::remove_dir_all(&store).ok();
    }

    #[test]
    fn prune_retains_and_gcs_blobs() {
        let store = tmp_store("prune");
        let mut e = BackupEngine::open(&store).unwrap();
        e.backup(&[("bin", b"v1")], Kind::Full, "a").unwrap();
        e.backup(&[("bin", b"v2")], Kind::Incremental, "b").unwrap();
        let keep = e.backup(&[("bin", b"v3")], Kind::Incremental, "c").unwrap().snapshot.id;
        let (dropped, collected) = e.prune(1).unwrap();
        assert_eq!(dropped, 2);
        assert_eq!(collected, 2, "v1 + v2 blobs collected, v3 kept");
        assert_eq!(e.snapshots().len(), 1);
        assert!(e.verify(keep), "surviving snapshot still intact");
        fs::remove_dir_all(&store).ok();
    }
}
