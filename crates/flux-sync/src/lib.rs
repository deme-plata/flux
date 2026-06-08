//! flux-sync — content-addressed block backfill for flux + sigil chains.
//!
//! # The gap this closes (found 2026-06-08)
//!
//! `flux-p2p` is **gossip-only**: a fresh node collects blocks produced AFTER it
//! joins, but cannot backfill history — there is no block-range request/response.
//! Both the flux supercluster and the SIGIL chain hit this.
//!
//! # The fix
//!
//! Every block is **content-addressed** via `flux-aether`: BLAKE3 root, K-of-N
//! erasure-coded + encrypted shards, **verified on retrieval**. Nodes exchange a
//! compact [`SyncManifest`] (height → content_root); a peer computes what it is
//! [`missing`] and fetches exactly those blocks by root. Verify-don't-trust: a
//! retrieved block whose bytes don't hash to its root is rejected, so a lying peer
//! can't poison history.
//!
//! This crate is the *engine* (store + manifest + diff), testable in-process. The
//! wire layer — gossip the manifest over flux-p2p and fetch shards — is the thin
//! integration step in `flux-p2p` (flux sync) and `sigil-net`/`sigil-top` (sigil sync).

use flux_aether::{reassemble, shard_file, FileBlock, Hash, Shard};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A node's claim: "I hold the block at `height`, whose content_root is this."
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlockRef {
    pub height: u64,
    /// 64-hex BLAKE3 content root.
    pub content_root: String,
}

/// What a node holds — exchanged with peers to drive backfill.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncManifest {
    pub tip: u64,
    pub network_id: String,
    pub refs: Vec<BlockRef>,
}

impl SyncManifest {
    pub fn new(network_id: impl Into<String>) -> Self {
        Self { tip: 0, network_id: network_id.into(), refs: Vec::new() }
    }

    /// Record a block at `height` with `content_root` (latest wins per height).
    pub fn add(&mut self, height: u64, content_root: String) {
        if height > self.tip {
            self.tip = height;
        }
        self.refs.retain(|r| r.height != height);
        self.refs.push(BlockRef { height, content_root });
    }

    fn roots(&self) -> HashSet<&str> {
        self.refs.iter().map(|r| r.content_root.as_str()).collect()
    }
}

/// Blocks `remote` advertises that `local` lacks (by content_root), sorted by
/// height ascending — the backfill work-list, ready to apply in order.
pub fn missing(local: &SyncManifest, remote: &SyncManifest) -> Vec<BlockRef> {
    let have = local.roots();
    let mut out: Vec<BlockRef> = remote
        .refs
        .iter()
        .filter(|r| !have.contains(r.content_root.as_str()))
        .cloned()
        .collect();
    out.sort_by_key(|r| r.height);
    out
}

/// Content-addressed block store on flux-aether. Each block persists as one
/// `<dir>/<content_root_hex>.block` = bincode of `(FileBlock, Vec<Shard>)`.
pub struct BlockStore {
    dir: PathBuf,
    key: Vec<u8>,
}

impl BlockStore {
    /// Open/create a store. The shard-encryption key comes from `FLUX_AETHER_KEY`
    /// (matches the rest of the aether stack) or a default.
    pub fn open(dir: impl AsRef<Path>) -> std::io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let key = std::env::var("FLUX_AETHER_KEY")
            .map(|s| s.into_bytes())
            .unwrap_or_else(|_| b"flux-sync-v0".to_vec());
        Ok(Self { dir, key })
    }

    fn producer() -> Hash {
        *blake3::hash(b"flux-sync").as_bytes()
    }

    fn path(&self, root_hex: &str) -> PathBuf {
        self.dir.join(format!("{root_hex}.block"))
    }

    /// Ingest a block's bytes: shard via aether, persist, return content_root hex.
    /// Idempotent — identical bytes → identical root → dedup (no rewrite).
    pub fn ingest(&self, bytes: &[u8]) -> std::io::Result<String> {
        let (fb, shards) = shard_file(bytes, 65536, &self.key, Self::producer());
        let root_hex = hex::encode(fb.content_root);
        let p = self.path(&root_hex);
        if !p.exists() {
            let blob = bincode::serialize(&(fb, shards))
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
            std::fs::write(&p, blob)?;
        }
        Ok(root_hex)
    }

    /// Import a raw `.block` blob received from a peer (the wire payload), keyed by
    /// its advertised root. Verifies on the next [`retrieve`]; returns whether the
    /// stored bytes actually hash to `root_hex` (reject mismatches — anti-poison).
    pub fn import_blob(&self, root_hex: &str, blob: &[u8]) -> std::io::Result<bool> {
        std::fs::write(self.path(root_hex), blob)?;
        // Verify-don't-trust: confirm it reassembles AND matches the claimed root.
        match self.retrieve(root_hex) {
            Some(_) => Ok(true),
            None => {
                let _ = std::fs::remove_file(self.path(root_hex)); // drop the bad blob
                Ok(false)
            }
        }
    }

    /// The on-wire payload for a block (what a peer sends on a `want`).
    pub fn blob(&self, root_hex: &str) -> Option<Vec<u8>> {
        std::fs::read(self.path(root_hex)).ok()
    }

    pub fn has(&self, root_hex: &str) -> bool {
        self.path(root_hex).exists()
    }

    /// Retrieve + BLAKE3-verify a block by content_root. None if absent/corrupt.
    pub fn retrieve(&self, root_hex: &str) -> Option<Vec<u8>> {
        let blob = std::fs::read(self.path(root_hex)).ok()?;
        let (fb, shards): (FileBlock, Vec<Shard>) = bincode::deserialize(&blob).ok()?;
        // reassemble() re-hashes the output and checks it == fb.content_root.
        let out = reassemble(&fb, &shards, &self.key).ok()?;
        // Belt-and-suspenders: the FILE name must match the verified content too.
        if hex::encode(fb.content_root) == root_hex {
            Some(out)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("flux-sync-{}-{}", std::process::id(), tag))
    }

    #[test]
    fn ingest_deterministic_dedup_and_roundtrip() {
        let dir = tmp("rt");
        let _ = std::fs::remove_dir_all(&dir);
        let s = BlockStore::open(&dir).unwrap();
        let b = b"sigil-block-header-h1-roots-aaaa";
        let r1 = s.ingest(b).unwrap();
        let r2 = s.ingest(b).unwrap();
        assert_eq!(r1, r2, "same bytes → same content_root");
        assert!(s.has(&r1));
        assert_eq!(s.retrieve(&r1).as_deref(), Some(&b[..]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_is_the_backfill_list() {
        let mut local = SyncManifest::new("sigil-g0");
        local.add(1, "aa".into());
        local.add(2, "bb".into());
        let mut remote = SyncManifest::new("sigil-g0");
        remote.add(1, "aa".into());
        remote.add(2, "bb".into());
        remote.add(3, "cc".into());
        remote.add(4, "dd".into());
        let want = missing(&local, &remote);
        assert_eq!(want.iter().map(|r| r.height).collect::<Vec<_>>(), vec![3, 4]);
        assert_eq!(remote.tip, 4);
    }

    #[test]
    fn end_to_end_backfill_with_verification() {
        let (da, db) = (tmp("A"), tmp("B"));
        let _ = std::fs::remove_dir_all(&da);
        let _ = std::fs::remove_dir_all(&db);
        let sa = BlockStore::open(&da).unwrap();
        let sb = BlockStore::open(&db).unwrap();
        let (mut ma, mut mb) = (SyncManifest::new("g0"), SyncManifest::new("g0"));

        let blocks: Vec<Vec<u8>> =
            (1..=4u64).map(|h| format!("block-{h}-payload").into_bytes()).collect();
        // A has all 4
        for (i, b) in blocks.iter().enumerate() {
            ma.add(i as u64 + 1, sa.ingest(b).unwrap());
        }
        // B has only the first 2
        for i in 0..2 {
            mb.add(i as u64 + 1, sb.ingest(&blocks[i]).unwrap());
        }

        let want = missing(&mb, &ma);
        assert_eq!(want.len(), 2, "B must backfill blocks 3 and 4");

        // Simulate the wire: A sends its blob, B imports + verifies, then applies.
        for r in &want {
            let blob = sa.blob(&r.content_root).expect("A serves the block");
            assert!(sb.import_blob(&r.content_root, &blob).unwrap(), "verified import");
            let bytes = sb.retrieve(&r.content_root).expect("verified retrieve");
            assert!(bytes.starts_with(b"block-"));
            mb.add(r.height, r.content_root.clone());
        }
        assert_eq!(mb.tip, 4);
        assert!(missing(&mb, &ma).is_empty(), "B is now fully synced to A");

        // Anti-poison: a tampered blob under a real root must be rejected.
        let good_root = &ma.refs[0].content_root;
        assert!(!sb.import_blob(good_root, b"garbage-not-the-block").unwrap());
        let _ = std::fs::remove_dir_all(&da);
        let _ = std::fs::remove_dir_all(&db);
    }
}
