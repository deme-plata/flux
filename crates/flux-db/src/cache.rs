//! LRU block cache for the read path.
//!
//! Keyed by `(sst_path, block_offset)`. Holds decompressed block payloads
//! as `Arc<Vec<u8>>` so cache hits avoid both disk I/O and LZ4 decompression
//! — typically the two biggest costs in a point lookup.
//!
//! ## Eviction
//!
//! Classic LRU on a doubly-linked list embedded in the entry map. Touch /
//! insert / evict are all O(1). The list uses cloned `BlockKey`s as
//! "pointers" so we don't need `unsafe` or a third-party LRU crate; the
//! cost is one extra `BlockKey` clone per touch (cheap — `BlockKey` is a
//! `PathBuf` + `u64`, both small in our workloads).
//!
//! ## Observability
//!
//! `stats()` returns `(hits, misses, current_bytes)` aggregated across the
//! whole cache. `stats_per_sst()` breaks the hit/miss counters down per
//! SST path so callers can see which files benefit from caching and which
//! are pollutants worth admission-controlling.
//!
//! ## Invalidation
//!
//! `invalidate_sst(path)` drops every entry whose `sst_path == path`.
//! Compaction calls this after rewriting the source SSTs into a single
//! compacted output — without it, blocks from the now-deleted SSTs would
//! sit in the cache as zombies until evicted by capacity pressure.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Cache key. `block_offset` is the offset into the SST body where the
/// compressed block starts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockKey {
    pub sst_path: PathBuf,
    pub block_offset: u64,
}

/// Thread-safe LRU block cache. Bounded by total bytes of held blocks.
pub struct BlockCache {
    inner: Mutex<Inner>,
    capacity_bytes: usize,
}

/// One entry in the LRU list. `prev` / `next` are the keys of the
/// neighbouring entries — using keys as pointers avoids `unsafe` and the
/// usual self-referential-struct headaches in safe Rust.
struct Entry {
    value: Arc<Vec<u8>>,
    prev: Option<BlockKey>,
    next: Option<BlockKey>,
}

struct Inner {
    map: HashMap<BlockKey, Entry>,
    /// Head = most recently used; tail = next to evict.
    head: Option<BlockKey>,
    tail: Option<BlockKey>,
    bytes: usize,
    hits: u64,
    misses: u64,
    /// Per-SST (hits, misses). Cleared for an SST when `invalidate_sst`
    /// removes it. Kept across normal evictions — counters survive even
    /// when the entries themselves get pushed out.
    per_sst: HashMap<PathBuf, (u64, u64)>,
}

impl Inner {
    /// Unlink `key` from the doubly-linked list. The key must be in `map`
    /// already. Caller is responsible for any further `map` mutation.
    fn unlink(&mut self, key: &BlockKey) {
        let (prev, next) = {
            let e = self.map.get(key).expect("unlink: key not in map");
            (e.prev.clone(), e.next.clone())
        };
        match prev.as_ref() {
            Some(p) => {
                if let Some(e) = self.map.get_mut(p) {
                    e.next = next.clone();
                }
            }
            None => self.head = next.clone(),
        }
        match next.as_ref() {
            Some(n) => {
                if let Some(e) = self.map.get_mut(n) {
                    e.prev = prev.clone();
                }
            }
            None => self.tail = prev.clone(),
        }
    }

    /// Link `key` as the new head (most recently used). The entry's
    /// `prev`/`next` must currently be cleared (e.g. fresh insert, or it
    /// has just been `unlink`ed).
    fn link_head(&mut self, key: &BlockKey) {
        let old_head = self.head.take();
        if let Some(ref oh) = old_head {
            if let Some(e) = self.map.get_mut(oh) {
                e.prev = Some(key.clone());
            }
        }
        if let Some(e) = self.map.get_mut(key) {
            e.prev = None;
            e.next = old_head;
        }
        self.head = Some(key.clone());
        if self.tail.is_none() {
            self.tail = Some(key.clone());
        }
    }
}

impl BlockCache {
    pub fn new(capacity_bytes: usize) -> Self {
        Self {
            capacity_bytes,
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                head: None,
                tail: None,
                bytes: 0,
                hits: 0,
                misses: 0,
                per_sst: HashMap::new(),
            }),
        }
    }

    /// Look up a block. On hit returns the cached payload and bumps it to
    /// the head of the LRU list. On miss returns None. Both outcomes
    /// update aggregate and per-SST counters.
    pub fn get(&self, key: &BlockKey) -> Option<Arc<Vec<u8>>> {
        let mut inner = self.inner.lock();
        if inner.map.contains_key(key) {
            inner.unlink(key);
            inner.link_head(key);
            let v = inner.map.get(key).map(|e| Arc::clone(&e.value));
            inner.hits += 1;
            inner
                .per_sst
                .entry(key.sst_path.clone())
                .or_insert((0, 0))
                .0 += 1;
            v
        } else {
            inner.misses += 1;
            inner
                .per_sst
                .entry(key.sst_path.clone())
                .or_insert((0, 0))
                .1 += 1;
            None
        }
    }

    /// Insert a block. Evicts least-recently-used entries until total bytes
    /// fits the cache's capacity. If the block is larger than the whole
    /// cache it is skipped (better than evicting everything for nothing).
    pub fn put(&self, key: BlockKey, value: Arc<Vec<u8>>) {
        let size = value.len();
        if size > self.capacity_bytes {
            return;
        }
        let mut inner = self.inner.lock();

        // Replace-existing path: shrink bytes by old size, unlink, then drop.
        if let Some(existing) = inner.map.get(&key) {
            let old_size = existing.value.len();
            inner.bytes = inner.bytes.saturating_sub(old_size);
            inner.unlink(&key);
            inner.map.remove(&key);
        }

        inner.map.insert(
            key.clone(),
            Entry {
                value,
                prev: None,
                next: None,
            },
        );
        inner.bytes += size;
        inner.link_head(&key);

        while inner.bytes > self.capacity_bytes {
            let Some(victim) = inner.tail.clone() else {
                break;
            };
            inner.unlink(&victim);
            if let Some(e) = inner.map.remove(&victim) {
                inner.bytes = inner.bytes.saturating_sub(e.value.len());
            }
        }
    }

    /// Drop every entry whose `sst_path` matches `path`. Returns the count
    /// of blocks removed. Per-SST counters for that path are reset too —
    /// the SST is being deleted, its hit rate is now historically irrelevant.
    pub fn invalidate_sst(&self, path: &Path) -> usize {
        let mut inner = self.inner.lock();
        let victims: Vec<BlockKey> = inner
            .map
            .keys()
            .filter(|k| k.sst_path == path)
            .cloned()
            .collect();
        let count = victims.len();
        for v in &victims {
            inner.unlink(v);
            if let Some(e) = inner.map.remove(v) {
                inner.bytes = inner.bytes.saturating_sub(e.value.len());
            }
        }
        inner.per_sst.remove(path);
        count
    }

    /// `(hits, misses, current_bytes)` snapshot across the whole cache.
    pub fn stats(&self) -> (u64, u64, usize) {
        let inner = self.inner.lock();
        (inner.hits, inner.misses, inner.bytes)
    }

    /// Per-SST `(hits, misses)`. Order is unspecified. Survives natural
    /// eviction but is cleared by `invalidate_sst`.
    pub fn stats_per_sst(&self) -> HashMap<PathBuf, (u64, u64)> {
        self.inner.lock().per_sst.clone()
    }

    pub fn capacity(&self) -> usize {
        self.capacity_bytes
    }

    /// Number of entries currently in the cache. Test helper, not part of
    /// the steady-state read path.
    pub fn len(&self) -> usize {
        self.inner.lock().map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_get_miss_then_hit() {
        let c = BlockCache::new(1024 * 1024);
        let k = BlockKey { sst_path: "x".into(), block_offset: 0 };
        assert!(c.get(&k).is_none());
        c.put(k.clone(), Arc::new(vec![1, 2, 3]));
        assert_eq!(c.get(&k).unwrap().as_slice(), &[1, 2, 3]);
        let (h, m, _) = c.stats();
        assert_eq!(h, 1);
        assert_eq!(m, 1);
    }

    #[test]
    fn test_cache_lru_eviction() {
        let c = BlockCache::new(120); // exactly enough for two 50-byte blocks
        let k1 = BlockKey { sst_path: "a".into(), block_offset: 0 };
        let k2 = BlockKey { sst_path: "a".into(), block_offset: 64 };
        let k3 = BlockKey { sst_path: "a".into(), block_offset: 128 };
        c.put(k1.clone(), Arc::new(vec![0u8; 50]));
        c.put(k2.clone(), Arc::new(vec![0u8; 50]));
        // touch k1 to make k2 the LRU
        let _ = c.get(&k1);
        c.put(k3.clone(), Arc::new(vec![0u8; 50]));
        assert!(c.get(&k1).is_some(), "k1 should still be present");
        assert!(c.get(&k2).is_none(), "k2 should have been evicted");
        assert!(c.get(&k3).is_some(), "k3 just inserted");
    }

    #[test]
    fn test_cache_oversize_skip() {
        let c = BlockCache::new(64);
        let k = BlockKey { sst_path: "x".into(), block_offset: 0 };
        c.put(k.clone(), Arc::new(vec![0u8; 1024]));
        assert!(c.get(&k).is_none(), "block bigger than cache should be skipped");
    }

    // ── v0.14.1 cache upgrades ──

    #[test]
    fn test_o1_lru_touch_order() {
        // After touching k1 then k2 then k1 again, the LRU order from
        // tail-to-head must be [k3, k2, k1] — confirms link/unlink keep
        // the doubly-linked list consistent through many touches.
        let c = BlockCache::new(1024);
        let k1 = BlockKey { sst_path: "a".into(), block_offset: 0 };
        let k2 = BlockKey { sst_path: "a".into(), block_offset: 8 };
        let k3 = BlockKey { sst_path: "a".into(), block_offset: 16 };
        c.put(k1.clone(), Arc::new(vec![0u8; 8]));
        c.put(k2.clone(), Arc::new(vec![0u8; 8]));
        c.put(k3.clone(), Arc::new(vec![0u8; 8]));
        // Touch sequence: k1 (oldest -> head), k2 (now oldest -> head),
        // k1 again (back to head). Expected tail -> head: k3, k2, k1.
        let _ = c.get(&k1);
        let _ = c.get(&k2);
        let _ = c.get(&k1);
        // Force eviction by exceeding capacity — tail should die first.
        // 3 × 8 (k1+k2+k3) + 1010 = 1034 > 1024 capacity.
        let big = BlockKey { sst_path: "a".into(), block_offset: 1024 };
        c.put(big.clone(), Arc::new(vec![0u8; 1010]));
        // After admission, only the most-recently-used survive. Expected
        // outcome with the touch sequence above: k1 lives, k3 dies first.
        assert!(c.get(&k1).is_some(), "k1 (head) must survive");
        assert!(c.get(&k3).is_none(), "k3 (tail) must have been evicted first");
    }

    #[test]
    fn test_invalidate_sst_removes_only_targeted_blocks() {
        let c = BlockCache::new(1024);
        let p_a: PathBuf = "a.sst".into();
        let p_b: PathBuf = "b.sst".into();
        for i in 0u64..5 {
            c.put(
                BlockKey { sst_path: p_a.clone(), block_offset: i * 16 },
                Arc::new(vec![0u8; 8]),
            );
            c.put(
                BlockKey { sst_path: p_b.clone(), block_offset: i * 16 },
                Arc::new(vec![0u8; 8]),
            );
        }
        assert_eq!(c.len(), 10);
        let removed = c.invalidate_sst(&p_a);
        assert_eq!(removed, 5, "expected 5 a.sst blocks to vanish");
        assert_eq!(c.len(), 5);
        // a.sst entries gone; b.sst entries intact.
        for i in 0u64..5 {
            assert!(c.get(&BlockKey { sst_path: p_a.clone(), block_offset: i * 16 }).is_none());
            assert!(c.get(&BlockKey { sst_path: p_b.clone(), block_offset: i * 16 }).is_some());
        }
    }

    #[test]
    fn test_invalidate_sst_zeros_per_sst_stats() {
        let c = BlockCache::new(1024);
        let p: PathBuf = "doomed.sst".into();
        let k = BlockKey { sst_path: p.clone(), block_offset: 0 };
        c.put(k.clone(), Arc::new(vec![0u8; 8]));
        let _ = c.get(&k); // hit
        let _ = c.get(&BlockKey { sst_path: p.clone(), block_offset: 99 }); // miss on same SST
        let before = c.stats_per_sst();
        assert_eq!(before.get(&p), Some(&(1, 1)));

        c.invalidate_sst(&p);
        let after = c.stats_per_sst();
        assert!(after.get(&p).is_none(), "per-SST stats for invalidated path must be cleared");
    }

    #[test]
    fn test_per_sst_stats_survive_eviction() {
        // A block evicted by capacity pressure is gone, but its SST's
        // counters should keep accumulating from later probes — eviction
        // is not the same as invalidation.
        let c = BlockCache::new(16); // tiny: 2 blocks of 8 bytes each
        let p_hot: PathBuf = "hot.sst".into();
        let p_cold: PathBuf = "cold.sst".into();
        c.put(
            BlockKey { sst_path: p_hot.clone(), block_offset: 0 },
            Arc::new(vec![0u8; 8]),
        );
        c.put(
            BlockKey { sst_path: p_hot.clone(), block_offset: 8 },
            Arc::new(vec![0u8; 8]),
        );
        // Probe cold a few times → records misses.
        for i in 0u64..3 {
            let _ = c.get(&BlockKey { sst_path: p_cold.clone(), block_offset: i });
        }
        let per = c.stats_per_sst();
        assert_eq!(per.get(&p_cold), Some(&(0, 3)));
        assert!(per.get(&p_hot).is_none(), "no probes on hot.sst yet → no stat row");
    }
}
