//! SHARDED-WRITER (follow-up to SHARDED-WRITER.md): N independent Database
//! shards behind one facade, keyed by a STABLE hash of the key.
//!
//! Why now: the design doc gated true sharding on measurement — "revisit only
//! if lock contention returns above ~500 MB/s". With the 20 TB ladder done the
//! array idles at 646 MB/s direct-write capability while the coalesced
//! single-writer path measured ~346 MB/s isolated: the single WAL/memtable
//! pipeline, not the disk, is the ceiling. Each shard owns its own WAL,
//! memtable, and compaction, so `put_many` partitions a batch and writes all
//! shards IN PARALLEL — N fsync pipelines against a RAID array built for
//! exactly that.
//!
//! Correctness invariants:
//! - Routing uses FNV-1a (inline, dependency-free) — deterministic across
//!   processes and versions, unlike std's per-process-seeded SipHash. A key
//!   written under shard i is found under shard i forever.
//! - The shard count is persisted in a `SHARDS` marker at first open; a
//!   reopen with a different count is a LOUD error, never silent misrouting.
//! - Batch semantics match `Database::put_many` per shard (one lock + one
//!   coalesced WAL write per shard per batch; empty value = tombstone).
//! - `sync_wal` barriers EVERY shard; a caller's durability marker is only
//!   advanced after all shards report durable (same contract the chronos
//!   harness used on the single store).

use crate::Database;
use std::path::{Path, PathBuf};

/// Stable, dependency-free FNV-1a 64-bit. NOT cryptographic — routing only.
#[inline]
pub fn route_hash(key: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in key {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub struct ShardedDb {
    shards: Vec<Database>,
}

impl ShardedDb {
    /// Open (or create) a sharded store at `path` with `n` shards in
    /// `shard-000..shard-(n-1)` subdirectories. The count is persisted; a
    /// mismatch on reopen errors loudly instead of misrouting reads.
    pub fn open(path: impl Into<PathBuf>, n: usize) -> Result<Self, String> {
        if n == 0 || n > 256 {
            return Err(format!("shard count {} out of range 1..=256", n));
        }
        let root: PathBuf = path.into();
        std::fs::create_dir_all(&root).map_err(|e| format!("create {}: {}", root.display(), e))?;
        let marker = root.join("SHARDS");
        match std::fs::read_to_string(&marker) {
            Ok(existing) => {
                let have: usize = existing.trim().parse().unwrap_or(0);
                if have != n {
                    return Err(format!(
                        "sharded store at {} was created with {} shards, reopened with {} — \
                         routing would silently miss keys; refusing",
                        root.display(), have, n));
                }
            }
            Err(_) => {
                std::fs::write(&marker, format!("{}\n", n))
                    .map_err(|e| format!("write SHARDS marker: {}", e))?;
            }
        }
        let mut shards = Vec::with_capacity(n);
        for i in 0..n {
            shards.push(Database::open(root.join(format!("shard-{:03}", i)))?);
        }
        Ok(ShardedDb { shards })
    }

    #[inline]
    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }

    #[inline]
    fn shard_for(&self, key: &[u8]) -> usize {
        (route_hash(key) % self.shards.len() as u64) as usize
    }

    /// Direct access to a shard (bench/inspection).
    pub fn shard(&self, i: usize) -> &Database {
        &self.shards[i]
    }

    pub fn put(&self, key: &[u8], value: &[u8]) -> Result<(), String> {
        self.shards[self.shard_for(key)].put(key, value)
    }

    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
        self.shards[self.shard_for(key)].get(key)
    }

    pub fn delete(&self, key: &[u8]) -> Result<(), String> {
        self.shards[self.shard_for(key)].delete(key)
    }

    /// Partition a batch by routing hash and write every shard's slice IN
    /// PARALLEL (scoped threads — one coalesced put_many per shard). Returns
    /// the first error, after all threads have joined.
    pub fn put_many<K: AsRef<[u8]> + Sync, V: AsRef<[u8]> + Sync>(
        &self,
        entries: &[(K, V)],
    ) -> Result<(), String> {
        if entries.is_empty() {
            return Ok(());
        }
        let n = self.shards.len();
        let mut parts: Vec<Vec<(&[u8], &[u8])>> = vec![Vec::new(); n];
        for (k, v) in entries {
            let k = k.as_ref();
            parts[(route_hash(k) % n as u64) as usize].push((k, v.as_ref()));
        }
        let results: Vec<Result<(), String>> = std::thread::scope(|s| {
            let handles: Vec<_> = self
                .shards
                .iter()
                .zip(parts.iter())
                .map(|(db, part)| {
                    s.spawn(move || {
                        if part.is_empty() { Ok(()) } else { db.put_many(part) }
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("shard writer panicked".into()))).collect()
        });
        for r in results {
            r?;
        }
        Ok(())
    }

    /// Durability barrier across EVERY shard (parallel fsyncs).
    pub fn sync_wal(&self) -> Result<(), String> {
        let results: Vec<Result<(), String>> = std::thread::scope(|s| {
            let handles: Vec<_> = self.shards.iter().map(|db| s.spawn(|| db.sync_wal())).collect();
            handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("sync panicked".into()))).collect()
        });
        for r in results {
            r?;
        }
        Ok(())
    }

    pub fn set_defer_compaction(&self, defer: bool) {
        for db in &self.shards {
            db.set_defer_compaction(defer);
        }
    }
}

/// Does a sharded store already exist at this path (marker present)?
pub fn exists(path: &Path) -> bool {
    path.join("SHARDS").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("flux-shard-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn routing_is_stable_and_reopen_finds_everything() {
        let dir = tmp("reopen");
        let keys: Vec<String> = (0..500).map(|i| format!("key-{:06}", i)).collect();
        {
            let db = ShardedDb::open(&dir, 4).unwrap();
            let entries: Vec<(&[u8], Vec<u8>)> = keys.iter()
                .map(|k| (k.as_bytes(), format!("val-{}", k).into_bytes())).collect();
            db.put_many(&entries).unwrap();
            db.sync_wal().unwrap();
        }
        // Fresh process-equivalent: new instance, SAME routing required.
        let db = ShardedDb::open(&dir, 4).unwrap();
        for k in &keys {
            let got = db.get(k.as_bytes()).unwrap();
            assert_eq!(got.as_deref(), Some(format!("val-{}", k).as_bytes()),
                "key {} must survive reopen under stable routing", k);
        }
        // Keys spread across shards (not all in one).
        let used = (0..4).filter(|&i| {
            keys.iter().any(|k| (route_hash(k.as_bytes()) % 4) as usize == i)
        }).count();
        assert!(used >= 3, "500 keys must spread across shards, used {}", used);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shard_count_mismatch_is_loud() {
        let dir = tmp("mismatch");
        drop(ShardedDb::open(&dir, 4).unwrap());
        let err = match ShardedDb::open(&dir, 8) {
            Err(e) => e,
            Ok(_) => panic!("mismatched shard count must not open"),
        };
        assert!(err.contains("refusing"), "mismatch must refuse: {}", err);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sharded_equals_single_semantics() {
        // Same batch through a 1-shard store and a plain Database: identical reads,
        // including the empty-value tombstone rule.
        let dir_s = tmp("eq-shard");
        let dir_p = tmp("eq-plain");
        let sdb = ShardedDb::open(&dir_s, 1).unwrap();
        let pdb = Database::open(dir_p.clone()).unwrap();
        let entries: Vec<(&[u8], &[u8])> = vec![
            (b"a", b"1" as &[u8]), (b"b", b"2"), (b"c", b"3"), (b"b", b""), // tombstone b
        ];
        sdb.put_many(&entries).unwrap();
        pdb.put_many(&entries).unwrap();
        for k in [b"a" as &[u8], b"b", b"c"] {
            assert_eq!(sdb.get(k).unwrap(), pdb.get(k).unwrap(),
                "sharded and plain must agree on {:?}", k);
        }
        assert_eq!(sdb.get(b"b").unwrap(), None, "empty value is a tombstone");
        let _ = std::fs::remove_dir_all(&dir_s);
        let _ = std::fs::remove_dir_all(&dir_p);
    }
}
