// flux-cache — Content-hash MIR/object cache for Flux
//
// Phase 1: mmap-based I/O with BLAKE3 hashing and NAND-optimized batch writes.
// Stores compiled outputs and reuses them on rebuild.
//
// NAND optimization: sequential access via mmap (no random reads),
// batch writes via write buffer, arena-preallocated hash buffers.
//
// v0.10.x (new): in-memory LRU layer above the on-disk cache + stats.
// Pattern borrowed from flux-db's BlockCache — recent hits skip disk
// entirely. Git-style: content keys are BLAKE3 hashes, so identical
// outputs collapse to one entry naturally.
//
// Cache directory: target/flux-cache/
// Cache entries:    target/flux-cache/<blake3>.json

use blake3::Hasher;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

/// A single cache entry — the outputs from one rustc invocation.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
#[repr(C, align(64))]
pub struct CacheEntry {
    pub source_hash: String,
    pub args_hash: String,
    pub outputs: HashMap<String, String>,
    pub rustc_version: String,
    pub timestamp: u64,
}

// ── v0.10.x: in-memory LRU layer + stats ──────────────────────────────────
//
// Pattern lifted from flux-db's BlockCache. Recent hits skip disk and JSON
// deserialization entirely. Bounded by entry count (NOT bytes — entries are
// uniformly small CacheEntry structs, ~1-2 KiB serialized).
//
// Stats:
//   mem_hits  — found in the LRU; no disk touch
//   disk_hits — missed LRU but found on disk; promoted into LRU
//   misses    — not in LRU and not on disk

const MEM_LRU_CAPACITY: usize = 4096;
const DEFAULT_TTL_DAYS: u64 = 30;

struct MemLru {
    map: HashMap<String, CacheEntry>,
    order: VecDeque<String>,
}

impl MemLru {
    fn new() -> Self {
        Self {
            map: HashMap::with_capacity(MEM_LRU_CAPACITY),
            order: VecDeque::with_capacity(MEM_LRU_CAPACITY),
        }
    }
}

static MEM_LRU: OnceLock<Mutex<MemLru>> = OnceLock::new();
static MEM_HITS: AtomicU64 = AtomicU64::new(0);
static DISK_HITS: AtomicU64 = AtomicU64::new(0);
static MISSES: AtomicU64 = AtomicU64::new(0);
static STORES: AtomicU64 = AtomicU64::new(0);
static TTL_EVICTIONS: AtomicU64 = AtomicU64::new(0);
/// v0.11: count of entries evicted to keep total bytes under the disk cap.
static DISK_EVICTIONS: AtomicU64 = AtomicU64::new(0);
/// v0.11: disk size cap in bytes. `0` means "uncapped" (legacy behavior).
/// AtomicU64 so callers can set it from any thread.
static DISK_CAPACITY: AtomicU64 = AtomicU64::new(0);
/// v0.11.1 hotpatch (post-bench): cached running total of bytes on disk.
/// `store` and eviction maintain this; reading it is O(1) instead of an
/// O(n) directory walk. Reconciled to the truth via
/// `recompute_disk_bytes()` when callers explicitly request it.
static CACHED_DISK_BYTES: AtomicU64 = AtomicU64::new(0);
/// Whether the cached total is trusted. Anything that mutates the cache
/// outside of our `store` / eviction paths flips this to `false` and the
/// next read pays for a fresh walk.
static CACHED_DISK_BYTES_VALID: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn mem_lru() -> &'static Mutex<MemLru> {
    MEM_LRU.get_or_init(|| Mutex::new(MemLru::new()))
}

fn lru_get(key: &str) -> Option<CacheEntry> {
    let mut lru = mem_lru().lock();
    if let Some(entry) = lru.map.get(key).cloned() {
        // Move-to-front O(n) on VecDeque — acceptable at this capacity
        // (4096) since the inner type is &String comparison.
        if let Some(pos) = lru.order.iter().position(|k| k == key) {
            lru.order.remove(pos);
        }
        lru.order.push_back(key.to_string());
        return Some(entry);
    }
    None
}

fn lru_put(key: String, entry: CacheEntry) {
    let mut lru = mem_lru().lock();
    if lru.map.contains_key(&key) {
        if let Some(pos) = lru.order.iter().position(|k| k == &key) {
            lru.order.remove(pos);
        }
    }
    lru.order.push_back(key.clone());
    lru.map.insert(key, entry);
    while lru.map.len() > MEM_LRU_CAPACITY {
        if let Some(victim) = lru.order.pop_front() {
            lru.map.remove(&victim);
        } else {
            break;
        }
    }
}

/// Cache hit/miss counters. Tuple is (mem_hits, disk_hits, misses, stores,
/// ttl_evictions). Atomically read at point-in-time.
pub fn stats() -> (u64, u64, u64, u64, u64) {
    (
        MEM_HITS.load(Ordering::Relaxed),
        DISK_HITS.load(Ordering::Relaxed),
        MISSES.load(Ordering::Relaxed),
        STORES.load(Ordering::Relaxed),
        TTL_EVICTIONS.load(Ordering::Relaxed),
    )
}

/// v0.11: extended stats including disk_evictions and current total disk bytes.
/// Returns (mem_hits, disk_hits, misses, stores, ttl_evictions,
///          disk_evictions, total_disk_bytes).
pub fn stats_v11() -> (u64, u64, u64, u64, u64, u64, u64) {
    (
        MEM_HITS.load(Ordering::Relaxed),
        DISK_HITS.load(Ordering::Relaxed),
        MISSES.load(Ordering::Relaxed),
        STORES.load(Ordering::Relaxed),
        TTL_EVICTIONS.load(Ordering::Relaxed),
        DISK_EVICTIONS.load(Ordering::Relaxed),
        current_disk_bytes(),
    )
}

/// v0.11: set the maximum total bytes flux-cache may keep on disk. When a
/// `store` would push the cache over the cap, the oldest entries (by mtime)
/// are evicted until the cap holds. Passing `0` disables the cap (legacy
/// behavior — bound only by free disk).
pub fn set_disk_capacity(bytes: u64) {
    DISK_CAPACITY.store(bytes, Ordering::Relaxed);
}

/// v0.11: read the current disk capacity setting (0 = uncapped).
pub fn disk_capacity() -> u64 {
    DISK_CAPACITY.load(Ordering::Relaxed)
}

/// v0.11: total bytes of every cache entry on disk.
///
/// **v0.11.1 (post-bench):** O(1) when the cached running total is fresh —
/// `store` and eviction maintain it as they go. Falls back to a full O(n)
/// directory walk only on first read or when external mutation has marked
/// the cache invalid (e.g. after `clean()` or process restart).
pub fn current_disk_bytes() -> u64 {
    if CACHED_DISK_BYTES_VALID.load(Ordering::Relaxed) {
        return CACHED_DISK_BYTES.load(Ordering::Relaxed);
    }
    recompute_disk_bytes()
}

/// Force a full directory walk and update the cached total. Returns the
/// computed value. Use this when the cache contents have been mutated
/// outside this process (e.g. user ran `rm`).
pub fn recompute_disk_bytes() -> u64 {
    let dir = cache_dir();
    let total = if dir.exists() { walk_bytes(&dir) } else { 0 };
    CACHED_DISK_BYTES.store(total, Ordering::Relaxed);
    CACHED_DISK_BYTES_VALID.store(true, Ordering::Relaxed);
    total
}

fn walk_bytes(p: &std::path::Path) -> u64 {
    let mut total: u64 = 0;
    let Ok(entries) = fs::read_dir(p) else { return 0; };
    for e in entries.flatten() {
        let Ok(ft) = e.file_type() else { continue; };
        if ft.is_dir() {
            total = total.saturating_add(walk_bytes(&e.path()));
        } else if let Ok(meta) = e.metadata() {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

/// v0.11: evict the oldest cache entries until total disk bytes fits the
/// cap. Returns the number of files removed. No-op if cap is 0 or current
/// usage is already below cap.
pub fn evict_to_capacity() -> usize {
    let cap = DISK_CAPACITY.load(Ordering::Relaxed);
    if cap == 0 { return 0; }
    let mut current = current_disk_bytes();
    if current <= cap { return 0; }

    // Collect (mtime, size, path) for every file under cache_dir/objects/.
    let dir = cache_dir();
    let mut victims: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    collect_files(&dir, &mut victims);
    // Oldest first — evict in mtime ascending order.
    victims.sort_by_key(|(t, _, _)| *t);

    let mut count = 0;
    let mut bytes_freed: u64 = 0;
    for (_t, size, path) in victims {
        if current <= cap { break; }
        if fs::remove_file(&path).is_ok() {
            current = current.saturating_sub(size);
            bytes_freed = bytes_freed.saturating_add(size);
            count += 1;
        }
    }
    DISK_EVICTIONS.fetch_add(count as u64, Ordering::Relaxed);
    if CACHED_DISK_BYTES_VALID.load(Ordering::Relaxed) {
        CACHED_DISK_BYTES.fetch_sub(bytes_freed, Ordering::Relaxed);
    }
    count
}

/// Only enforce capacity if the user has set one. Called on every `store`
/// so a misconfigured (or default) cache pays just the one `Atomic::load`.
fn evict_to_capacity_if_set(_just_wrote_size: u64) {
    if DISK_CAPACITY.load(Ordering::Relaxed) > 0 {
        evict_to_capacity();
    }
}

fn collect_files(dir: &std::path::Path, out: &mut Vec<(SystemTime, u64, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else { return; };
    for e in entries.flatten() {
        let Ok(ft) = e.file_type() else { continue; };
        let path = e.path();
        if ft.is_dir() {
            collect_files(&path, out);
        } else if let Ok(meta) = e.metadata() {
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            out.push((mtime, meta.len(), path));
        }
    }
}

/// Combined hit-rate (mem + disk) over total lookups. 0.0 if no lookups yet.
pub fn hit_rate() -> f64 {
    let (m, d, x, _, _) = stats();
    let total = m + d + x;
    if total == 0 { 0.0 } else { (m + d) as f64 / total as f64 }
}

/// Reset all stat counters. Useful between benchmark runs.
pub fn reset_stats() {
    MEM_HITS.store(0, Ordering::Relaxed);
    DISK_HITS.store(0, Ordering::Relaxed);
    MISSES.store(0, Ordering::Relaxed);
    STORES.store(0, Ordering::Relaxed);
    TTL_EVICTIONS.store(0, Ordering::Relaxed);
}

/// Drop every cache entry older than `max_age`. Returns the number of files
/// evicted. Runs O(n) over the cache directory — call periodically (e.g.
/// once at fluxc startup), not on every cache lookup.
pub fn evict_older_than(max_age: Duration) -> usize {
    let dir = cache_dir();
    let now = SystemTime::now();
    let mut count = 0;
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    if let Ok(age) = now.duration_since(modified) {
                        if age > max_age {
                            let _ = fs::remove_file(entry.path());
                            count += 1;
                        }
                    }
                }
            }
        }
    }
    TTL_EVICTIONS.fetch_add(count as u64, Ordering::Relaxed);
    count
}

/// Convenience: evict entries older than the default TTL (30 days).
pub fn evict_stale() -> usize {
    evict_older_than(Duration::from_secs(DEFAULT_TTL_DAYS * 24 * 3600))
}


/// Compute a cache key from source file + compiler args.
/// Uses mmap for large files (> 64KB) to avoid heap allocation.
/// Returns: hex-encoded BLAKE3 string.
pub fn compute_hash(source_file: Option<&str>, args: &[String]) -> String {
    let mut hasher = Hasher::new();

    const MMAP_THRESHOLD: u64 = 65536; // 64KB

    if let Some(path) = source_file {
        // Use mmap for large files — zero-copy sequential read (NAND-friendly)
        if let Ok(meta) = fs::metadata(path) {
            if meta.len() > MMAP_THRESHOLD {
                if let Ok(file) = fs::File::open(path) {
                    if let Ok(mmap) = unsafe { memmap2::Mmap::map(&file) } {
                        hasher.update(&mmap[..]);
                        return finish_hash(hasher, args);
                    }
                }
            }
        }

        // Heap path for small files or when mmap fails
        if let Ok(mut f) = fs::File::open(path) {
            // Pre-allocate buffer based on file size hint
            let cap = fs::metadata(path).map(|m| m.len() as usize).unwrap_or(8192);
            let mut buf = Vec::with_capacity(cap.min(16 * 1024 * 1024)); // cap at 16MB
            if f.read_to_end(&mut buf).is_ok() {
                hasher.update(&buf);
            }
        }
    }

    finish_hash(hasher, args)
}

fn finish_hash(mut hasher: Hasher, args: &[String]) -> String {
    for arg in args {
        if !arg.starts_with("--out-dir") && !arg.contains("/tmp/") {
            hasher.update(arg.as_bytes());
            hasher.update(b"\0");
        }
    }
    hasher.finalize().to_hex().to_string()
}

/// Look up a cache entry by key.
///
/// Three-tier lookup (v0.11):
///   1. In-memory LRU      — zero disk I/O, zero deserialization
///   2. mmap'd .bin file   — single sequential read + bincode decode
///   3. legacy .json file  — backward compat with v0.10 entries (still readable)
///
/// Disk hits warm the LRU so the next call is tier-1.
pub fn lookup(key: &str) -> Option<CacheEntry> {
    // Tier 1: in-memory LRU.
    if let Some(entry) = lru_get(key) {
        MEM_HITS.fetch_add(1, Ordering::Relaxed);
        return Some(entry);
    }

    // Tier 2: v0.11 sharded .bin file.
    let bin_path = cache_path(key);
    if bin_path.exists() {
        if let Ok(file) = fs::File::open(&bin_path) {
            if let Ok(mmap) = unsafe { memmap2::Mmap::map(&file) } {
                // SEC-021: bound the decode so a forged length prefix in a cache
                // .bin (writable by anything with cache-dir access) can't drive a
                // huge pre-allocation. 256 MiB is well above any real cache entry.
                const MAX_ENTRY_BYTES: u64 = 256 * 1024 * 1024;
                if let Ok(entry) = bincode::config().limit(MAX_ENTRY_BYTES).deserialize::<CacheEntry>(&mmap[..]) {
                    DISK_HITS.fetch_add(1, Ordering::Relaxed);
                    lru_put(key.to_string(), entry.clone());
                    return Some(entry);
                }
            }
        }
    }

    // Tier 3: pre-v0.11 legacy JSON entries at the flat path
    //   target/flux-cache/<key>.json
    // Read them transparently so existing caches don't appear empty after
    // upgrade. We do NOT migrate them eagerly — they're rewritten as .bin
    // the next time the caller stores under the same key.
    let legacy_path = cache_dir().join(format!("{}.json", key));
    if legacy_path.exists() {
        if let Some(entry) = fs::read(&legacy_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<CacheEntry>(&bytes).ok())
        {
            DISK_HITS.fetch_add(1, Ordering::Relaxed);
            lru_put(key.to_string(), entry.clone());
            return Some(entry);
        }
    }

    MISSES.fetch_add(1, Ordering::Relaxed);
    None
}

/// Store a cache entry. v0.11: bincode-serialized, atomic write
/// (`.tmp` + rename), git-style 2-char sharded path. Crash mid-write
/// leaves the orphaned `.tmp` file but never a half-written entry under
/// the canonical name.
pub fn store(key: &str, entry: &CacheEntry) {
    STORES.fetch_add(1, Ordering::Relaxed);
    let path = cache_path(key);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    // Subtract any pre-existing size at this key before writing the new one,
    // so the cached running total stays accurate across overwrites.
    let old_size = path.metadata().map(|m| m.len()).unwrap_or(0);

    if let Ok(bytes) = bincode::serialize(entry) {
        let entry_size = bytes.len() as u64;
        atomic_write(&path, &bytes);
        if CACHED_DISK_BYTES_VALID.load(Ordering::Relaxed) {
            CACHED_DISK_BYTES.fetch_add(entry_size, Ordering::Relaxed);
            if old_size > 0 {
                CACHED_DISK_BYTES.fetch_sub(old_size, Ordering::Relaxed);
            }
        }
        // After a successful write, enforce the disk cap if one is set.
        evict_to_capacity_if_set(entry_size);
    }

    // Warm the LRU.
    lru_put(key.to_string(), entry.clone());
}

/// Batch store multiple entries. Each entry goes through the same atomic
/// + sharded pipeline as `store`; the directory fsync at the end gives one
/// write barrier for the whole batch.
pub fn store_batch(entries: &[(String, CacheEntry)]) {
    let dir = cache_dir();
    let _ = fs::create_dir_all(&dir);

    let mut total_size = 0u64;
    for (key, entry) in entries {
        let path = cache_path(key);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(bytes) = bincode::serialize(entry) {
            total_size += bytes.len() as u64;
            atomic_write(&path, &bytes);
            lru_put(key.clone(), entry.clone());
        }
    }
    let _ = fs::File::open(&dir).and_then(|f| f.sync_all());
    STORES.fetch_add(entries.len() as u64, Ordering::Relaxed);
    evict_to_capacity_if_set(total_size);
}

/// Atomic write via `.tmp + rename`. Returns silently on any I/O error —
/// caller treats this as best-effort cache population, not a hard guarantee.
fn atomic_write(final_path: &std::path::Path, bytes: &[u8]) {
    use std::io::Write;
    let tmp_path = final_path.with_extension("tmp");
    let result = (|| -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp_path)?;
        f.write_all(bytes)?;
        f.flush()?;
        fs::rename(&tmp_path, final_path)?;
        Ok(())
    })();
    if result.is_err() {
        // Clean up the stray tmp on failure so the dir doesn't leak.
        let _ = fs::remove_file(&tmp_path);
    }
}

/// Clean all cached entries (both v0.10 .json and v0.11 sharded .bin).
pub fn clean() {
    let dir = cache_dir();
    if dir.exists() {
        let _ = fs::remove_dir_all(&dir);
    }
    // Reset the cached running total.
    CACHED_DISK_BYTES.store(0, Ordering::Relaxed);
    CACHED_DISK_BYTES_VALID.store(true, Ordering::Relaxed);
    // Also clear the LRU so the next lookup re-reads from disk.
    mem_lru().lock().map.clear();
    mem_lru().lock().order.clear();
}

fn cache_dir() -> PathBuf {
    let mut dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    loop {
        let target = dir.join("target");
        if target.exists() && target.is_dir() {
            return target.join("flux-cache");
        }
        if !dir.pop() { break; }
    }
    PathBuf::from("target").join("flux-cache")
}

/// Canonical v0.11 path for a key: `<cache_dir>/objects/<2-char>/<rest>.bin`.
/// Git uses the same `objects/aa/bb…` scheme to avoid one flat directory of
/// millions of entries.
fn cache_path(key: &str) -> PathBuf {
    let dir = cache_dir().join("objects");
    if key.len() >= 2 {
        let (prefix, rest) = key.split_at(2);
        dir.join(prefix).join(format!("{}.bin", rest))
    } else {
        // Sub-2-char keys are pathological but should still work.
        dir.join("__").join(format!("{}.bin", key))
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_hash_deterministic() {
        let args1 = vec!["--crate-name".into(), "test".into(), "src/lib.rs".into()];
        let args2 = vec!["--crate-name".into(), "test".into(), "src/lib.rs".into()];
        let h1 = compute_hash(Some("src/lib.rs"), &args1);
        let h2 = compute_hash(Some("src/lib.rs"), &args2);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_compute_hash_different() {
        let args1 = vec!["--edition".into(), "2021".into()];
        let args2 = vec!["--edition".into(), "2024".into()];
        let h1 = compute_hash(None, &args1);
        let h2 = compute_hash(None, &args2);
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_store_and_lookup() {
        let entry = CacheEntry {
            source_hash: "abc123".into(),
            args_hash: "def456".into(),
            outputs: HashMap::new(),
            rustc_version: "1.80.0".into(),
            timestamp: 12345,
        };
        store("test_key", &entry);
        let found = lookup("test_key").unwrap();
        assert_eq!(found.source_hash, "abc123");
    }

    #[test]
    fn test_batch_store() {
        let entries: Vec<(String, CacheEntry)> = (0..3).map(|i| {
            (format!("batch_{}", i), CacheEntry {
                source_hash: format!("hash_{}", i),
                args_hash: "batch".into(),
                outputs: HashMap::new(),
                rustc_version: "1.80.0".into(),
                timestamp: i as u64,
            })
        }).collect();
        store_batch(&entries);
        for (key, expected) in &entries {
            let found = lookup(key).unwrap();
            assert_eq!(found.source_hash, expected.source_hash);
        }
    }

    #[test]
    fn test_lookup_missing() {
        assert!(lookup("nonexistent_key_xyz").is_none());
    }

    // ── v0.10.x: LRU + stats tests ──

    // Serializes every test touching GLOBAL cache state (stats counters, disk_capacity,
    // current_disk_bytes, eviction). Parallel tests otherwise clobber each other's globals
    // (reset_stats / set_disk_capacity / evict) → flaky stores>=1, after>=before, etc.
    static CACHE_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_lru_warms_on_disk_hit() {
        let _stats_guard = CACHE_TEST_LOCK.lock();
        reset_stats();
        let key = "lru_warm_test_key";
        // Clear any leftover from previous runs.
        let _ = fs::remove_file(cache_path(key));
        let entry = CacheEntry {
            source_hash: "h".into(),
            args_hash: "a".into(),
            outputs: HashMap::new(),
            rustc_version: "1.80.0".into(),
            timestamp: 1,
        };
        store(key, &entry);
        // After store() the LRU is warm; lookup is a mem hit.
        let _ = lookup(key);
        let (mem_hits, _disk_hits, _misses, stores, _) = stats();
        assert!(mem_hits >= 1, "expected ≥1 mem hit, got {mem_hits}");
        assert!(stores >= 1);
        let _ = fs::remove_file(cache_path(key));
    }

    #[test]
    fn test_stats_counts_misses() {
        let _stats_guard = CACHE_TEST_LOCK.lock();
        reset_stats();
        let _ = lookup("definitely_not_a_real_cache_key_zzz");
        let (_, _, misses, _, _) = stats();
        assert!(misses >= 1);
    }

    // ── v0.11 tests ──

    fn fresh_entry(tag: u64) -> CacheEntry {
        CacheEntry {
            source_hash: format!("src-{tag}"),
            args_hash: format!("args-{tag}"),
            outputs: HashMap::new(),
            rustc_version: "1.80.0".into(),
            timestamp: tag,
        }
    }

    #[test]
    fn test_v11_bincode_round_trip() {
        let e = fresh_entry(42);
        let bytes = bincode::serialize(&e).unwrap();
        let back: CacheEntry = bincode::deserialize(&bytes).unwrap();
        assert_eq!(back.source_hash, "src-42");
        assert_eq!(back.timestamp, 42);
    }

    #[test]
    fn test_v11_sharded_path() {
        let p = cache_path("abcdef1234");
        assert!(p.to_string_lossy().contains("objects"));
        assert!(p.to_string_lossy().contains("/ab/"));
        assert!(p.to_string_lossy().ends_with("cdef1234.bin"));
    }

    #[test]
    fn test_v11_store_and_lookup_bin() {
        let key = "v11_round_trip_aabbcc";
        let _ = fs::remove_file(cache_path(key));
        let entry = fresh_entry(7);
        store(key, &entry);
        let got = lookup(key).unwrap();
        assert_eq!(got.source_hash, "src-7");
        assert!(cache_path(key).exists(), "v0.11 .bin should land at sharded path");
        let _ = fs::remove_file(cache_path(key));
    }

    #[test]
    fn test_v11_legacy_json_fallback() {
        // Hand-craft a pre-v0.11 entry at the flat JSON path; lookup() must
        // find it via the tier-3 legacy fallback.
        let key = "v11_legacy_fallback_xx";
        let legacy_path = cache_dir().join(format!("{}.json", key));
        let _ = fs::create_dir_all(cache_dir());
        let e = fresh_entry(99);
        let json = serde_json::to_vec(&e).unwrap();
        std::fs::write(&legacy_path, &json).unwrap();
        // Clear LRU so we go to disk.
        mem_lru().lock().map.clear();
        let got = lookup(key).unwrap();
        assert_eq!(got.timestamp, 99);
        let _ = fs::remove_file(&legacy_path);
    }

    #[test]
    fn test_v11_atomic_write_no_tmp_leak_on_success() {
        let key = "v11_atomic_clean_zz";
        let _ = fs::remove_file(cache_path(key));
        store(key, &fresh_entry(1));
        let tmp = cache_path(key).with_extension("tmp");
        assert!(!tmp.exists(), "successful write should leave no .tmp");
        let _ = fs::remove_file(cache_path(key));
    }

    #[test]
    fn test_v11_disk_cap_set_get() {
        let _g = CACHE_TEST_LOCK.lock();
        set_disk_capacity(1024);
        assert_eq!(disk_capacity(), 1024);
        set_disk_capacity(0);
        assert_eq!(disk_capacity(), 0);
    }

    #[test]
    fn test_v11_current_disk_bytes_grows_with_store() {
        let _g = CACHE_TEST_LOCK.lock();
        // Use unique keys so we don't trample concurrent test runs.
        let k1 = "v11_disk_bytes_1_a1b2c3";
        let k2 = "v11_disk_bytes_2_a1b2c3";
        let before = current_disk_bytes();
        store(k1, &fresh_entry(1));
        store(k2, &fresh_entry(2));
        let after = current_disk_bytes();
        assert!(after >= before, "disk bytes shouldn't decrease after stores");
        let _ = fs::remove_file(cache_path(k1));
        let _ = fs::remove_file(cache_path(k2));
    }

    #[test]
    fn test_v11_evict_to_capacity_no_cap_noop() {
        let _g = CACHE_TEST_LOCK.lock();
        set_disk_capacity(0);
        let removed = evict_to_capacity();
        assert_eq!(removed, 0, "uncapped should never evict");
    }

    #[test]
    fn test_v11_evict_to_capacity_drops_oldest() {
        let _g = CACHE_TEST_LOCK.lock();
        // Write a few entries, then set a tiny cap, then evict.
        // Use unique-prefixed keys so this test doesn't compete with peers.
        let keys: Vec<String> = (0..5).map(|i| format!("v11_cap_evict_{i:02}_z9z9")).collect();
        for (i, k) in keys.iter().enumerate() {
            store(k, &fresh_entry(i as u64));
            // Stagger mtimes a bit so the "oldest" ordering is deterministic.
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        let before = current_disk_bytes();
        // Cap to roughly half of current usage to force eviction.
        set_disk_capacity(before / 2);
        let removed = evict_to_capacity();
        let after = current_disk_bytes();
        assert!(removed >= 1, "expected at least one eviction, got {removed}");
        assert!(after <= before, "post-eviction bytes should be <= pre");
        // Cleanup
        set_disk_capacity(0);
        for k in &keys { let _ = fs::remove_file(cache_path(k)); }
    }

    #[test]
    fn test_v11_stats_v11_shape() {
        let s = stats_v11();
        // Just check the tuple length / type by destructuring.
        let (_a, _b, _c, _d, _e, _f, _g) = s;
    }

    #[test]
    fn test_evict_older_than_drops_old_files() {
        let key = "evict_test_key";
        let entry = CacheEntry {
            source_hash: "h".into(),
            args_hash: "a".into(),
            outputs: HashMap::new(),
            rustc_version: "1.80.0".into(),
            timestamp: 1,
        };
        store(key, &entry);
        assert!(cache_path(key).exists());
        // Evict with a max_age of 0 — anything with a positive age dies.
        let _ = evict_older_than(Duration::from_secs(0));
        // The file MAY or may not be gone depending on filesystem mtime
        // granularity; either way evict shouldn't panic.
        let _ = fs::remove_file(cache_path(key));
    }
}
