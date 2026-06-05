//! flux-rev — Flux-native, content-addressed version control. The git replacement.
//!
//! Git's fragility on this cluster (branches diverged, `main` stale since 2025, the deployed q-flux
//! in no pushed branch) comes from a mutable, branch-centric, push-pull model. flux-rev is the
//! opposite by construction:
//!
//!   • **Content-addressed** — every file is a blob keyed by its BLAKE3 hash; a snapshot is a
//!     deterministic manifest (sorted `path → blob,mode`) keyed by ITS hash; a revision is keyed by
//!     the hash of `(parent, manifest, genesis, author, ts, message)`. Identical content ⇒ identical
//!     hash everywhere — no "diverged" possible, two nodes either hold the same object or they don't.
//!   • **Genesis-stamped** — the import of the current canonical source is a single signed-ish
//!     `Genesis` object (imported_from, workspace_version, author, ts). Every revision references it,
//!     so provenance is intrinsic, not a branch convention.
//!   • **Mesh-syncable** — objects are immutable blobs addressed by hash, so syncing is "do you have
//!     object X?" over flux-p2p (see `sync` layer), never a 3-way merge. A revision propagates by
//!     shipping the objects it transitively needs.
//!
//! This module is the pure core (no p2p, no clock surprises): [`Store`], [`snapshot`], [`checkout`],
//! [`diff`]. The p2p propagation + CLI layer build on top.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Revision propagation over flux-p2p (the git-push replacement).
pub mod sync;

/// Directories never captured (build output, vcs metadata, vendored deps).
pub const SKIP_DIRS: &[&str] = &[
    ".git", ".flux-rev", "target", "node_modules", "dist", "build", ".cargo",
    "__pycache__", ".venv", "venv", ".next", "out", ".target-shared",
];

/// One file in a snapshot manifest. `mode` keeps the unix exec bit (755 vs 644).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub path: String,
    pub hash: String,
    pub mode: u32,
}

/// A snapshot of a working tree: a sorted, content-addressed list of files. Keyed by its own hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub entries: Vec<Entry>,
}

impl Manifest {
    /// Deterministic content hash — sorted entries → canonical JSON → BLAKE3. The "tree id".
    pub fn id(&self) -> String {
        hash_bytes(&canon(self))
    }
}

/// The one-time import stamp. Every revision references this, so provenance is intrinsic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Genesis {
    pub imported_from: String,
    pub workspace_version: String,
    pub author: String,
    pub ts_unix: u64,
    pub note: String,
}

impl Genesis {
    pub fn id(&self) -> String {
        hash_bytes(&canon(self))
    }
}

/// A revision: a manifest + lineage + provenance, content-addressed by the hash of all of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    /// hash of the canonical body (everything below) — the revision id.
    pub id: String,
    pub parent: Option<String>,
    pub manifest: String,
    pub genesis: String,
    pub workspace_version: String,
    pub author: String,
    pub ts_unix: u64,
    pub message: String,
}

/// The body that gets hashed into the revision id (excludes `id` itself — it's derived).
#[derive(Serialize)]
struct RevBody<'a> {
    parent: &'a Option<String>,
    manifest: &'a str,
    genesis: &'a str,
    workspace_version: &'a str,
    author: &'a str,
    ts_unix: u64,
    message: &'a str,
}

impl Revision {
    /// Recompute the content address from the body — what `id` must equal. Used to verify a
    /// revision object received over the mesh (its stored bytes are `canon(full rev)`, so a plain
    /// re-hash wouldn't match; the address is the hash of the *body*).
    pub fn body_id(&self) -> String {
        hash_bytes(&canon(&RevBody {
            parent: &self.parent,
            manifest: &self.manifest,
            genesis: &self.genesis,
            workspace_version: &self.workspace_version,
            author: &self.author,
            ts_unix: self.ts_unix,
            message: &self.message,
        }))
    }
}

/// Verify a received object actually belongs at `hash` before trusting it. Blobs/manifests/genesis
/// are addressed by the hash of their own bytes; a [`Revision`] is addressed by its body hash. Both
/// cases checked here — the integrity gate for mesh sync.
pub fn verify_object(hash: &str, bytes: &[u8]) -> bool {
    if hash_bytes(bytes) == hash {
        return true;
    }
    serde_json::from_slice::<Revision>(bytes).map(|r| r.body_id() == hash).unwrap_or(false)
}

/// BLAKE3 hex of bytes — the one addressing function.
pub fn hash_bytes(b: &[u8]) -> String {
    blake3::hash(b).to_hex().to_string()
}

/// Canonical JSON (serde struct field order is stable; manifest entries are pre-sorted) — the bytes
/// we hash. Same value ⇒ same bytes ⇒ same hash on every node.
fn canon<T: Serialize>(v: &T) -> Vec<u8> {
    serde_json::to_vec(v).expect("serialize")
}

/// A content-addressed object store: `<root>/objects/<blake3hex>`. Immutable, dedup by construction.
pub struct Store {
    pub root: PathBuf,
}

impl Store {
    /// Open/create a store under `<dir>/.flux-rev`.
    pub fn open(work_dir: &Path) -> io::Result<Store> {
        let root = work_dir.join(".flux-rev");
        fs::create_dir_all(root.join("objects"))?;
        Ok(Store { root })
    }

    fn obj_path(&self, hash: &str) -> PathBuf {
        self.root.join("objects").join(hash)
    }

    /// Store bytes, return their hash. No-op if already present (content-addressed dedup).
    pub fn put(&self, bytes: &[u8]) -> io::Result<String> {
        let h = hash_bytes(bytes);
        self.put_at(&h, bytes)?;
        Ok(h)
    }

    /// Store bytes at an explicit key. Used for the [`Revision`] object, whose address is the hash
    /// of its *body* (not of the full struct incl. the `id` field), so it must be keyed by `rev.id`.
    pub fn put_at(&self, hash: &str, bytes: &[u8]) -> io::Result<()> {
        let p = self.obj_path(hash);
        if !p.exists() {
            fs::write(&p, bytes)?;
        }
        Ok(())
    }

    pub fn has(&self, hash: &str) -> bool {
        self.obj_path(hash).exists()
    }

    pub fn get(&self, hash: &str) -> io::Result<Vec<u8>> {
        fs::read(self.obj_path(hash))
    }

    pub fn put_json<T: Serialize>(&self, v: &T) -> io::Result<String> {
        self.put(&canon(v))
    }

    pub fn get_manifest(&self, hash: &str) -> io::Result<Manifest> {
        Ok(serde_json::from_slice(&self.get(hash)?)?)
    }
    pub fn get_revision(&self, id: &str) -> io::Result<Revision> {
        Ok(serde_json::from_slice(&self.get(id)?)?)
    }
    pub fn get_genesis(&self, id: &str) -> io::Result<Genesis> {
        Ok(serde_json::from_slice(&self.get(id)?)?)
    }

    /// HEAD ref — the current revision id of the working copy. Plain file, easy to read over p2p.
    pub fn read_head(&self) -> Option<String> {
        fs::read_to_string(self.root.join("HEAD")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }
    pub fn write_head(&self, id: &str) -> io::Result<()> {
        fs::write(self.root.join("HEAD"), id)
    }
}

/// Stamp the one-time genesis (import of the current canonical source). Stored + returned (id, obj).
pub fn stamp_genesis(store: &Store, imported_from: &str, workspace_version: &str, author: &str, note: &str) -> io::Result<Genesis> {
    let g = Genesis {
        imported_from: imported_from.into(),
        workspace_version: workspace_version.into(),
        author: author.into(),
        ts_unix: now(),
        note: note.into(),
    };
    store.put_json(&g)?;
    Ok(g)
}

/// Snapshot a working dir into a new immutable [`Revision`] (blobs + manifest + revision all stored).
/// `parent` is the prior revision id (None = first after genesis). Pure except for fs + clock.
pub fn snapshot(
    work: &Path,
    store: &Store,
    parent: Option<String>,
    genesis_id: &str,
    workspace_version: &str,
    author: &str,
    message: &str,
) -> io::Result<Revision> {
    let mut entries = Vec::new();
    let mut files = Vec::new();
    walk(work, work, &mut files)?;
    for (rel, abs, mode) in files {
        let bytes = fs::read(&abs)?;
        let hash = store.put(&bytes)?;
        entries.push(Entry { path: rel, hash, mode });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let manifest = Manifest { entries };
    let manifest_id = store.put_json(&manifest)?;

    let ts = now();
    let id = {
        let body = RevBody {
            parent: &parent,
            manifest: &manifest_id,
            genesis: genesis_id,
            workspace_version,
            author,
            ts_unix: ts,
            message,
        };
        hash_bytes(&canon(&body))
    };
    let rev = Revision {
        id: id.clone(),
        parent,
        manifest: manifest_id,
        genesis: genesis_id.into(),
        workspace_version: workspace_version.into(),
        author: author.into(),
        ts_unix: ts,
        message: message.into(),
    };
    store.put_at(&rev.id, &canon(&rev))?; // keyed by id (= hash of body), so get_revision(id) finds it
    store.write_head(&id)?;
    Ok(rev)
}

/// Compute the manifest of the working tree WITHOUT storing anything (hash-only). Used by the
/// daemon to decide "did anything actually change?" before cutting a revision — a revision id
/// includes the timestamp, so blindly snapshotting on a timer would churn HEAD on every tick.
pub fn working_manifest(work: &Path) -> io::Result<Manifest> {
    let mut entries = Vec::new();
    let mut files = Vec::new();
    walk(work, work, &mut files)?;
    for (rel, abs, mode) in files {
        let bytes = fs::read(&abs)?;
        entries.push(Entry { path: rel, hash: hash_bytes(&bytes), mode });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Manifest { entries })
}

/// Snapshot ONLY if the working tree differs from HEAD's manifest. Returns `Ok(None)` when nothing
/// changed (so the daemon stays quiet). The auto-snapshot primitive.
pub fn snapshot_if_changed(
    work: &Path,
    store: &Store,
    genesis_id: &str,
    workspace_version: &str,
    author: &str,
    message: &str,
) -> io::Result<Option<Revision>> {
    let wm = working_manifest(work)?;
    if let Some(head) = store.read_head() {
        if let Ok(r) = store.get_revision(&head) {
            if r.manifest == wm.id() {
                return Ok(None); // nothing changed
            }
        }
    }
    let parent = store.read_head();
    Ok(Some(snapshot(work, store, parent, genesis_id, workspace_version, author, message)?))
}

/// Materialize a revision into `dest` (writes every file from the manifest, with its mode). The
/// content-addressed checkout: byte-identical to the snapshot on any node that holds the objects.
pub fn checkout(store: &Store, revision_id: &str, dest: &Path) -> io::Result<usize> {
    let rev = store.get_revision(revision_id)?;
    let manifest = store.get_manifest(&rev.manifest)?;
    let mut n = 0;
    for e in &manifest.entries {
        let bytes = store.get(&e.hash)?;
        let out = dest.join(&e.path);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&out, &bytes)?;
        set_mode(&out, e.mode);
        n += 1;
    }
    Ok(n)
}

/// What changed between two revisions (by path). The cheap, exact diff — no 3-way merge needed.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

pub fn diff(store: &Store, a: &str, b: &str) -> io::Result<Diff> {
    let ma = store.get_manifest(&store.get_revision(a)?.manifest)?;
    let mb = store.get_manifest(&store.get_revision(b)?.manifest)?;
    let am: BTreeMap<_, _> = ma.entries.iter().map(|e| (&e.path, &e.hash)).collect();
    let bm: BTreeMap<_, _> = mb.entries.iter().map(|e| (&e.path, &e.hash)).collect();
    let mut d = Diff::default();
    for (p, h) in &bm {
        match am.get(p) {
            None => d.added.push((*p).clone()),
            Some(ha) if ha != h => d.changed.push((*p).clone()),
            _ => {}
        }
    }
    for p in am.keys() {
        if !bm.contains_key(p) {
            d.removed.push((*p).clone());
        }
    }
    Ok(d)
}

/// The set of object hashes a revision transitively needs (revision + manifest + every blob). This
/// is exactly what a peer must hold to checkout it — the unit of mesh propagation.
pub fn closure(store: &Store, revision_id: &str) -> io::Result<Vec<String>> {
    let rev = store.get_revision(revision_id)?;
    let manifest = store.get_manifest(&rev.manifest)?;
    let mut out = vec![revision_id.to_string(), rev.manifest.clone(), rev.genesis.clone()];
    out.extend(manifest.entries.iter().map(|e| e.hash.clone()));
    out.sort();
    out.dedup();
    Ok(out)
}

/// Is `ancestor` an ancestor of (or equal to) `of`? Walks `of`'s parent chain. Used to keep HEAD
/// monotonic on a sync peer — only adopt a revision that is at-or-ahead of the current HEAD, never
/// flap back to an older one when announces arrive out of order. Requires the parent revisions to
/// be present in the store (they are, once a peer has applied the lineage).
pub fn is_ancestor(store: &Store, ancestor: &str, of: &str) -> bool {
    let mut cur = Some(of.to_string());
    let mut n = 0;
    while let Some(id) = cur {
        if id == ancestor {
            return true;
        }
        cur = store.get_revision(&id).ok().and_then(|r| r.parent);
        n += 1;
        if n > 1_000_000 {
            break;
        }
    }
    false
}

// ── helpers ──

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf, u32)>) -> io::Result<()> {
    for e in fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                walk(root, &p, out)?;
            }
        } else if p.is_file() {
            let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            out.push((rel, p.clone(), mode_of(&e)));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn mode_of(e: &fs::DirEntry) -> u32 {
    use std::os::unix::fs::MetadataExt;
    e.metadata().map(|m| m.mode() & 0o777).unwrap_or(0o644)
}
#[cfg(not(unix))]
fn mode_of(_e: &fs::DirEntry) -> u32 { 0o644 }

#[cfg(unix)]
fn set_mode(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(p, fs::Permissions::from_mode(mode));
}
#[cfg(not(unix))]
fn set_mode(_p: &Path, _mode: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("flux-rev-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn content_addressing_is_deterministic_and_dedups() {
        let work = tmp("cas");
        let store = Store::open(&work).unwrap();
        let h1 = store.put(b"fn main(){}").unwrap();
        let h2 = store.put(b"fn main(){}").unwrap();
        assert_eq!(h1, h2, "identical content ⇒ identical hash (dedup)");
        assert_ne!(h1, store.put(b"fn main(){ }").unwrap(), "one byte differs ⇒ different hash");
        assert!(store.has(&h1));
        assert_eq!(store.get(&h1).unwrap(), b"fn main(){}");
    }

    #[test]
    fn snapshot_checkout_roundtrips_byte_identical() {
        let work = tmp("rt-src");
        fs::create_dir_all(work.join("src")).unwrap();
        fs::write(work.join("src/lib.rs"), "pub fn a() -> u32 { 42 }\n").unwrap();
        fs::write(work.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        fs::create_dir_all(work.join("target")).unwrap();
        fs::write(work.join("target/junk.o"), "BUILD ARTIFACT").unwrap(); // must be skipped

        let store = Store::open(&work).unwrap();
        let g = stamp_genesis(&store, "git-daemon", "0.18.0", "claude-desktop-viktor", "import").unwrap();
        let rev = snapshot(&work, &store, None, &g.id(), "0.18.0", "claude-desktop-viktor", "genesis import").unwrap();
        assert_eq!(store.read_head().as_deref(), Some(rev.id.as_str()));

        // checkout into a fresh dir using ONLY the store → byte-identical, target/ absent
        let dest = tmp("rt-dst");
        let n = checkout(&store, &rev.id, &dest).unwrap();
        assert_eq!(n, 2, "two real files (target/ skipped)");
        assert_eq!(fs::read_to_string(dest.join("src/lib.rs")).unwrap(), "pub fn a() -> u32 { 42 }\n");
        assert!(!dest.join("target").exists(), "build artifacts never captured");
    }

    #[test]
    fn a_change_makes_a_new_revision_with_an_exact_diff() {
        let work = tmp("diff");
        fs::write(work.join("a.rs"), "1").unwrap();
        fs::write(work.join("b.rs"), "keep").unwrap();
        let store = Store::open(&work).unwrap();
        let g = stamp_genesis(&store, "git-daemon", "0.18.0", "claude-desktop-viktor", "import").unwrap();
        let r1 = snapshot(&work, &store, None, &g.id(), "0.18.0", "claude-desktop-viktor", "v1").unwrap();

        fs::write(work.join("a.rs"), "2").unwrap();        // changed
        fs::write(work.join("c.rs"), "new").unwrap();      // added
        fs::remove_file(work.join("b.rs")).unwrap();       // removed
        let r2 = snapshot(&work, &store, Some(r1.id.clone()), &g.id(), "0.18.0", "claude-desktop-viktor", "v2").unwrap();

        assert_ne!(r1.id, r2.id);
        assert_eq!(r2.parent.as_deref(), Some(r1.id.as_str()));
        let d = diff(&store, &r1.id, &r2.id).unwrap();
        assert_eq!(d.added, vec!["c.rs"]);
        assert_eq!(d.removed, vec!["b.rs"]);
        assert_eq!(d.changed, vec!["a.rs"]);
    }

    #[test]
    fn closure_is_what_a_peer_needs_to_checkout() {
        let work = tmp("clos");
        fs::write(work.join("f1.rs"), "one").unwrap();
        fs::write(work.join("f2.rs"), "two").unwrap();
        let store = Store::open(&work).unwrap();
        let g = stamp_genesis(&store, "git-daemon", "0.18.0", "claude-desktop-viktor", "i").unwrap();
        let r = snapshot(&work, &store, None, &g.id(), "0.18.0", "claude-desktop-viktor", "v").unwrap();
        let c = closure(&store, &r.id).unwrap();
        // revision + manifest + genesis + 2 blobs = 5 distinct objects
        assert_eq!(c.len(), 5);
        assert!(c.contains(&r.id) && c.contains(&r.manifest) && c.contains(&g.id()));
        // every object in the closure actually exists in the store (peer can fetch each)
        assert!(c.iter().all(|h| store.has(h)));
    }

    #[test]
    fn snapshot_if_changed_is_quiet_until_something_changes() {
        let work = tmp("watch");
        fs::write(work.join("x.rs"), "v1").unwrap();
        let store = Store::open(&work).unwrap();
        let g = stamp_genesis(&store, "git-daemon", "0.18.0", "claude-desktop-viktor", "i").unwrap();
        snapshot(&work, &store, None, &g.id(), "0.18.0", "claude-desktop-viktor", "genesis").unwrap();

        // no change → no new revision (daemon stays quiet)
        assert!(snapshot_if_changed(&work, &store, &g.id(), "0.18.0", "claude-desktop-viktor", "auto").unwrap().is_none());

        // a real change → exactly one new revision
        fs::write(work.join("x.rs"), "v2").unwrap();
        let r = snapshot_if_changed(&work, &store, &g.id(), "0.18.0", "claude-desktop-viktor", "auto").unwrap();
        assert!(r.is_some(), "a changed tree cuts a revision");
        assert_eq!(store.read_head().as_deref(), Some(r.unwrap().id.as_str()));

        // and quiet again right after
        assert!(snapshot_if_changed(&work, &store, &g.id(), "0.18.0", "claude-desktop-viktor", "auto").unwrap().is_none());
    }
}
