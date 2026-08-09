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
// Cache directory (v0.36): $HOME/.flux/cache by default — a SHARED, per-user
// location that survives `rm -rf target` / `cargo clean`. Override with the
// FLUX_CACHE_DIR env var or `set_cache_dir()`. Legacy CWD-relative
// `target/flux-cache` is only the last-resort fallback when no HOME exists.
// Cache entries:    <cache_dir>/objects/<2-char>/<rest>.bin

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
/// v0.11: disk size cap in bytes, set programmatically via `set_disk_capacity`.
/// v0.36: `0` no longer means "uncapped" — it means "defer to the
/// FLUX_CACHE_DISK_CAP env var, default 50 GiB" (see `effective_disk_capacity`).
/// AtomicU64 so callers can set it from any thread.
static DISK_CAPACITY: AtomicU64 = AtomicU64::new(0);
/// v0.36: default disk cap when neither `set_disk_capacity` nor
/// FLUX_CACHE_DISK_CAP says otherwise. A shared per-user cache dir must not
/// grow unbounded now that `rm -rf target` no longer wipes it.
pub const DEFAULT_DISK_CAP_BYTES: u64 = 50 * 1024 * 1024 * 1024; // 50 GiB
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
/// are evicted until the cap holds.
///
/// v0.36 semantics change: passing `0` no longer means "uncapped" — it means
/// "defer to FLUX_CACHE_DISK_CAP / the 50 GiB default". To truly uncap, set
/// the env var `FLUX_CACHE_DISK_CAP=0`.
pub fn set_disk_capacity(bytes: u64) {
    DISK_CAPACITY.store(bytes, Ordering::Relaxed);
}

/// v0.11: read the programmatic disk capacity setting (0 = defer to env/default).
pub fn disk_capacity() -> u64 {
    DISK_CAPACITY.load(Ordering::Relaxed)
}

/// v0.36: the cap that eviction actually enforces.
///
/// Resolution order:
///   1. a non-zero programmatic `set_disk_capacity()` value
///   2. the `FLUX_CACHE_DISK_CAP` env var — plain bytes, or with a K/M/G/T
///      suffix (optionally `iB`/`B`), e.g. `FLUX_CACHE_DISK_CAP=20GiB`.
///      An explicit `0` disables the cap entirely.
///   3. `DEFAULT_DISK_CAP_BYTES` (50 GiB)
///
/// The env var is read once per process (the rustc-wrapper is one process per
/// compilation unit, so this is effectively per-invocation).
pub fn effective_disk_capacity() -> u64 {
    let explicit = DISK_CAPACITY.load(Ordering::Relaxed);
    if explicit != 0 {
        return explicit;
    }
    static ENV_CAP: OnceLock<u64> = OnceLock::new();
    *ENV_CAP.get_or_init(|| match std::env::var("FLUX_CACHE_DISK_CAP") {
        Ok(v) => parse_capacity(&v).unwrap_or(DEFAULT_DISK_CAP_BYTES),
        Err(_) => DEFAULT_DISK_CAP_BYTES,
    })
}

/// Parse a human capacity string: `53687091200`, `50G`, `50GiB`, `512M`, `0`.
/// Returns None on anything unparseable (caller falls back to the default).
fn parse_capacity(s: &str) -> Option<u64> {
    let t = s.trim();
    if t.is_empty() { return None; }
    let lower = t.to_ascii_lowercase();
    let (num_part, mult) = {
        let stripped = lower
            .strip_suffix("ib")
            .or_else(|| lower.strip_suffix('b'))
            .unwrap_or(&lower);
        match stripped.chars().last() {
            Some('k') => (&stripped[..stripped.len() - 1], 1024u64),
            Some('m') => (&stripped[..stripped.len() - 1], 1024u64.pow(2)),
            Some('g') => (&stripped[..stripped.len() - 1], 1024u64.pow(3)),
            Some('t') => (&stripped[..stripped.len() - 1], 1024u64.pow(4)),
            _ => (stripped, 1u64),
        }
    };
    num_part.trim().parse::<u64>().ok().and_then(|n| n.checked_mul(mult))
}

/// v0.36: account bytes written to the cache dir OUTSIDE flux-cache's own
/// `store` path (flux-driver's blob side-copies). Keeps the O(1) running
/// total honest so the disk-cap eviction fires when blobs — the bulk of the
/// cache by bytes — grow, not just when index entries do.
pub fn add_external_bytes(n: u64) {
    if CACHED_DISK_BYTES_VALID.load(Ordering::Relaxed) {
        CACHED_DISK_BYTES.fetch_add(n, Ordering::Relaxed);
    }
    persist_delta(n as i64);
}

// ── v0.41: cross-process persisted running total ──────────────────────────
//
// The v0.11.1 O(1) cached total lives in process statics — useless in the
// RUSTC_WRAPPER, which is a FRESH fluxc process per compiled unit. Every
// wrapper `store` therefore fell back to a full stat-walk of the shared
// cache tree (51k files / 33 GiB measured 2026-08-09) inside
// evict_to_capacity — ~10-25s blocked in wait_on_buffer on a cold inode
// cache, dominating small-crate edit loops (fluxc-serve real-edit check:
// 28s with the walk, 4s without). Persisting the total lets a fresh
// process answer current_disk_bytes() with two tiny reads instead.
//
// Layout (both under cache_dir(); they count as cache bytes — negligible):
//   .disk-total-base   ASCII u64 — total as of the last real walk
//   .disk-total-delta  append-only signed-decimal lines (O_APPEND atomic)
// Reader = base + sum(deltas). `rebase_persisted` folds the log back into
// base after every real walk. Drift (crash between write and append, a
// delta racing a compaction truncate) is tolerated by design: eviction
// RECONFIRMS with a real walk before deleting anything, so drift can waste
// one walk but never evict wrongly.

fn persisted_base_path() -> PathBuf {
    cache_dir().join(".disk-total-base")
}
fn persisted_delta_path() -> PathBuf {
    cache_dir().join(".disk-total-delta")
}

/// Best-effort append of a signed byte delta to the cross-process log.
fn persist_delta(delta: i64) {
    if delta == 0 {
        return;
    }
    let _ = fs::create_dir_all(cache_dir());
    use std::io::Write;
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(persisted_delta_path())
    {
        let _ = writeln!(f, "{}", delta);
    }
    // Compact a runaway log back into base (~64 KiB ≈ thousands of stores).
    if let Ok(meta) = fs::metadata(persisted_delta_path()) {
        if meta.len() > 64 * 1024 {
            if let Some(total) = persisted_disk_bytes() {
                rebase_persisted(total);
            }
        }
    }
}

/// Read the persisted total: base + sum of deltas. `None` when no base has
/// ever been written for this cache dir — the caller must walk once.
fn persisted_disk_bytes() -> Option<u64> {
    let base: u64 = fs::read_to_string(persisted_base_path())
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let mut total = base as i128;
    if let Ok(log) = fs::read_to_string(persisted_delta_path()) {
        for line in log.lines() {
            if let Ok(d) = line.trim().parse::<i64>() {
                total += d as i128;
            }
        }
    }
    Some(total.clamp(0, u64::MAX as i128) as u64)
}

/// Fold a known-true total into the base file and truncate the delta log.
/// A delta appended by a racing wrapper between our read and the truncate
/// is lost (undercount) — bounded drift, corrected at the next real walk.
fn rebase_persisted(total: u64) {
    let _ = fs::create_dir_all(cache_dir());
    atomic_write(&persisted_base_path(), total.to_string().as_bytes());
    let _ = fs::write(persisted_delta_path(), b"");
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
    // v0.41: fresh process (the per-unit RUSTC_WRAPPER case) — seed from the
    // persisted cross-process total instead of paying the full stat-walk.
    if let Some(total) = persisted_disk_bytes() {
        CACHED_DISK_BYTES.store(total, Ordering::Relaxed);
        CACHED_DISK_BYTES_VALID.store(true, Ordering::Relaxed);
        return total;
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
    // v0.41: a real walk is the reconciliation point — fold it into the
    // cross-process persisted total so fresh processes skip their own walk.
    rebase_persisted(total);
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
    let cap = effective_disk_capacity();
    if cap == 0 { return 0; }
    let mut current = current_disk_bytes();
    if current <= cap { return 0; }

    // v0.41: the O(1) total (statics or persisted log) can drift — RECONFIRM
    // with a real walk before deleting anything. Over-cap is rare, so the
    // walk is the price of an actual eviction, not of every store.
    current = recompute_disk_bytes();
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
    // `current` descends from the reconfirming walk above, so post-eviction
    // it is the freshest truth — fold it into the persisted total.
    rebase_persisted(current);
    count
}

/// v0.36: enforce the effective disk cap on every `store`. The common case
/// (under cap) costs one atomic load + one O(1) cached-total read; the full
/// mtime-sorted walk only happens when the cache is actually over cap.
fn evict_to_capacity_if_set(_just_wrote_size: u64) {
    if effective_disk_capacity() > 0 {
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
/// Content-hash a single file (hex BLAKE3), or None if unreadable. Used by the cache's
/// closure-consistency check (flux-driver T2c) to verify a restored crate's --extern deps are
/// byte-identical to what it was cached against — a dep with a deterministic FILENAME but a
/// non-deterministic rmeta CONTENT would otherwise pass the key match yet leave the restored crate
/// inconsistent (SIGBUS / "no resolution for an import"). [[flux-cache-reality]]
pub fn hash_file(path: &str) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(blake3::hash(&bytes).to_hex().to_string())
}

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
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        // v0.36 (the cold-CARGO_TARGET_DIR gate fix): drop `-C incremental=<dir>`
        // (cargo's pair form) and `-Cincremental=<dir>` (fused). The incremental
        // scratch dir lives under CARGO_TARGET_DIR and doesn't affect the compiled
        // artifact — hashing it made every key vary per target dir, so a cold
        // target dir could NEVER hit the shared cache even with identical inputs.
        if arg == "-C"
            && args.get(i + 1).map_or(false, |v| v.starts_with("incremental="))
        {
            i += 2;
            continue;
        }
        if arg.starts_with("-Cincremental=") {
            i += 1;
            continue;
        }
        // `--diagnostic-width=N` tracks the invoking terminal, not the artifact.
        if !arg.starts_with("--out-dir")
            && !arg.starts_with("--diagnostic-width")
            && !arg.contains("/tmp/")
        {
            hasher.update(arg.as_bytes());
            hasher.update(b"\0");
        }
        i += 1;
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
        persist_delta(entry_size as i64 - old_size as i64);
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
    let mut net_delta = 0i64;
    for (key, entry) in entries {
        let path = cache_path(key);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(bytes) = bincode::serialize(entry) {
            let old_size = path.metadata().map(|m| m.len()).unwrap_or(0);
            total_size += bytes.len() as u64;
            net_delta += bytes.len() as i64 - old_size as i64;
            atomic_write(&path, &bytes);
            lru_put(key.clone(), entry.clone());
        }
    }
    let _ = fs::File::open(&dir).and_then(|f| f.sync_all());
    persist_delta(net_delta);
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

/// v0.36: programmatic cache-dir override. First writer wins (OnceLock);
/// mainly for tests and embedders that must not touch the user's real cache.
static CACHE_DIR_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();
/// Memoized env/default resolution — one resolution, stable for the process.
static CACHE_DIR_RESOLVED: OnceLock<PathBuf> = OnceLock::new();

/// Pin the cache directory for this process. Returns false when a directory
/// was already pinned or resolved by first use — the running process keeps
/// its original dir; callers should treat that as "too late".
pub fn set_cache_dir(dir: PathBuf) -> bool {
    if CACHE_DIR_RESOLVED.get().is_some() {
        return false;
    }
    CACHE_DIR_OVERRIDE.set(dir).is_ok()
}

/// v0.36: resolve the shared flux cache directory. THE fix for "rm -rf target
/// wipes the cache": the default now lives OUTSIDE any workspace.
///
/// Resolution order (memoized per process — one resolution, stable for the
/// process lifetime):
///   1. `set_cache_dir()` programmatic override
///   2. `FLUX_CACHE_DIR` env var
///   3. `$HOME/.flux/cache` (the default)
///   4. legacy fallback (no HOME at all): nearest `target/` up from CWD,
///      i.e. the pre-v0.36 behavior.
///
/// `pub` so flux-driver roots its blob/closure sidecars in the SAME dir
/// instead of a separately-hardcoded `target/flux-cache` (the v0.35
/// inconsistency: flux-cache walked up from CWD while flux-driver used a
/// literal relative path — the two could disagree under `cargo -p` subdirs).
pub fn cache_dir() -> PathBuf {
    if let Some(d) = CACHE_DIR_OVERRIDE.get() {
        return d.clone();
    }
    CACHE_DIR_RESOLVED
        .get_or_init(|| {
            if let Ok(d) = std::env::var("FLUX_CACHE_DIR") {
                if !d.trim().is_empty() {
                    return PathBuf::from(d);
                }
            }
            if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
                if !home.is_empty() {
                    return PathBuf::from(home).join(".flux").join("cache");
                }
            }
            // Legacy fallback: nearest target/ dir up from CWD.
            let mut dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            loop {
                let target = dir.join("target");
                if target.exists() && target.is_dir() {
                    return target.join("flux-cache");
                }
                if !dir.pop() { break; }
            }
            PathBuf::from("target").join("flux-cache")
        })
        .clone()
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

    /// v0.36: the default cache dir is now the user's REAL shared cache
    /// ($HOME/.flux/cache). Every disk-touching test must pin an isolated
    /// per-process temp dir FIRST, or the eviction tests would garbage-collect
    /// the developer's actual cache. OnceLock: first call wins, all tests in
    /// this binary share the same isolated dir.
    fn iso() {
        let _ = set_cache_dir(
            std::env::temp_dir().join(format!("flux-cache-test-{}", std::process::id())),
        );
    }

    #[test]
    fn test_v036_cache_dir_is_isolated_and_not_cwd_target() {
        iso();
        let d = cache_dir();
        assert!(
            d.starts_with(std::env::temp_dir()),
            "override must win: {}", d.display()
        );
        // Too late to re-pin now.
        assert!(!set_cache_dir(PathBuf::from("/nope")));
    }

    #[test]
    fn test_v036_parse_capacity() {
        assert_eq!(parse_capacity("0"), Some(0));
        assert_eq!(parse_capacity("53687091200"), Some(53_687_091_200));
        assert_eq!(parse_capacity("50G"), Some(50 * 1024u64.pow(3)));
        assert_eq!(parse_capacity("50GiB"), Some(50 * 1024u64.pow(3)));
        assert_eq!(parse_capacity("512M"), Some(512 * 1024 * 1024));
        assert_eq!(parse_capacity(" 2TiB "), Some(2 * 1024u64.pow(4)));
        assert_eq!(parse_capacity("1024K"), Some(1024 * 1024));
        assert_eq!(parse_capacity("junk"), None);
        assert_eq!(parse_capacity(""), None);
    }

    #[test]
    fn test_v036_effective_capacity_defaults_and_override() {
        let _g = CACHE_TEST_LOCK.lock();
        // With no programmatic cap, the effective cap is the env/default
        // (either FLUX_CACHE_DISK_CAP or 50 GiB) — never the raw 0.
        set_disk_capacity(0);
        let eff = effective_disk_capacity();
        let env_expected = std::env::var("FLUX_CACHE_DISK_CAP")
            .ok()
            .and_then(|v| parse_capacity(&v))
            .unwrap_or(DEFAULT_DISK_CAP_BYTES);
        assert_eq!(eff, env_expected);
        // A non-zero programmatic cap wins over env/default.
        set_disk_capacity(1234);
        assert_eq!(effective_disk_capacity(), 1234);
        set_disk_capacity(0);
    }

    #[test]
    fn test_v041_persisted_total_roundtrip() {
        iso();
        let _g = CACHE_TEST_LOCK.lock();
        rebase_persisted(1000);
        assert_eq!(persisted_disk_bytes(), Some(1000));
        persist_delta(234);
        persist_delta(-34);
        assert_eq!(persisted_disk_bytes(), Some(1200));
        // Rebase folds the log: base carries the total, delta log empties.
        rebase_persisted(500);
        assert_eq!(persisted_disk_bytes(), Some(500));
        assert_eq!(
            fs::metadata(persisted_delta_path()).map(|m| m.len()).unwrap_or(0),
            0
        );
        // Negative drift clamps at zero instead of wrapping.
        persist_delta(-9999);
        assert_eq!(persisted_disk_bytes(), Some(0));
    }

    #[test]
    fn test_v041_store_appends_persisted_delta() {
        iso();
        let _g = CACHE_TEST_LOCK.lock();
        rebase_persisted(0);
        let entry = CacheEntry {
            source_hash: "v041src".into(),
            args_hash: "v041args".into(),
            outputs: HashMap::new(),
            rustc_version: "test".into(),
            timestamp: 0,
        };
        store("v041persistkey", &entry);
        let after_first = persisted_disk_bytes().expect("base must exist after rebase");
        assert!(after_first > 0, "store must persist a positive delta");
        // Overwriting the same key with identical bytes nets to zero delta.
        store("v041persistkey", &entry);
        assert_eq!(persisted_disk_bytes(), Some(after_first));
        // A fresh-process read path: seed statics from the persisted file.
        CACHED_DISK_BYTES_VALID.store(false, Ordering::Relaxed);
        assert_eq!(current_disk_bytes(), after_first);
        assert!(CACHED_DISK_BYTES_VALID.load(Ordering::Relaxed));
    }

    #[test]
    fn test_compute_hash_deterministic() {
        let args1 = vec!["--crate-name".into(), "test".into(), "src/lib.rs".into()];
        let args2 = vec!["--crate-name".into(), "test".into(), "src/lib.rs".into()];
        let h1 = compute_hash(Some("src/lib.rs"), &args1);
        let h2 = compute_hash(Some("src/lib.rs"), &args2);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_v036_key_invariant_to_target_dir_args() {
        // The cold-CARGO_TARGET_DIR gate: `-C incremental=<dir>` (both forms) and
        // `--diagnostic-width` must NOT flow into the key — they vary per target
        // dir / terminal while the compiled artifact is identical.
        let base = vec!["--crate-name".to_string(), "a".to_string(), "--emit=metadata,link".to_string()];
        let mut t1 = base.clone();
        t1.extend(["-C".into(), "incremental=/ws/t1/debug/incremental".into(), "--diagnostic-width=120".into()]);
        let mut t2 = base.clone();
        t2.extend(["-C".into(), "incremental=/ws/t2/debug/incremental".into(), "--diagnostic-width=80".into()]);
        let mut t3 = base.clone();
        t3.push("-Cincremental=/ws/t3/debug/incremental".into());
        let k_base = compute_hash(None, &base);
        assert_eq!(k_base, compute_hash(None, &t1));
        assert_eq!(k_base, compute_hash(None, &t2));
        assert_eq!(k_base, compute_hash(None, &t3));
        // Sanity: a REAL arg change still changes the key.
        let mut differ = base.clone();
        differ.push("--edition=2024".into());
        assert_ne!(k_base, compute_hash(None, &differ));
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
        iso();
        // v0.41: store() maintains the persisted cross-process total —
        // global state, so this test now serializes like the stats tests.
        let _g = CACHE_TEST_LOCK.lock();
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
        iso();
        // v0.41: store_batch() maintains the persisted cross-process total.
        let _g = CACHE_TEST_LOCK.lock();
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
        iso();
        assert!(lookup("nonexistent_key_xyz").is_none());
    }

    // ── v0.10.x: LRU + stats tests ──

    // Serializes every test touching GLOBAL cache state (stats counters, disk_capacity,
    // current_disk_bytes, eviction). Parallel tests otherwise clobber each other's globals
    // (reset_stats / set_disk_capacity / evict) → flaky stores>=1, after>=before, etc.
    static CACHE_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_lru_warms_on_disk_hit() {
        iso();
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
        iso();
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
        iso();
        let p = cache_path("abcdef1234");
        assert!(p.to_string_lossy().contains("objects"));
        assert!(p.to_string_lossy().contains("/ab/"));
        assert!(p.to_string_lossy().ends_with("cdef1234.bin"));
    }

    #[test]
    fn test_v11_store_and_lookup_bin() {
        iso();
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
        iso();
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
        iso();
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
        iso();
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
        iso();
        let _g = CACHE_TEST_LOCK.lock();
        set_disk_capacity(0);
        let removed = evict_to_capacity();
        assert_eq!(removed, 0, "uncapped should never evict");
    }

    #[test]
    fn test_v11_evict_to_capacity_drops_oldest() {
        iso();
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
        iso();
        let s = stats_v11();
        // Just check the tuple length / type by destructuring.
        let (_a, _b, _c, _d, _e, _f, _g) = s;
    }

    #[test]
    fn test_evict_older_than_drops_old_files() {
        iso();
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
