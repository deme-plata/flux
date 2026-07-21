// Flux DB — Embedded LSM-tree database for Rust
//
// RocksDB-inspired, Rust-native, BLAKE3-hashed, io_uring-powered.
// Designed as the primary storage backend for Wickes CMS/ERP.
//
// Architecture:
//   MemTable (SkipList) → WAL → SST files (levels) → Compaction
//   Snapshots for MVCC, Bloom filters for point lookups.
//
// ── v0.10.0 upgrade ──
//   * get() now reads through SSTs (was: memtable-only — broke after flush)
//   * Each SST persists its bloom filter so we can skip 99%+ of scans
//   * WAL entries carry a CRC32; replay stops cleanly at the first torn write
//   * SST files have a magic+version header for forward compatibility
//   * Streaming iter() API (lazy, no full materialization)
//   * Auto-compaction when sst_count > AUTO_COMPACT_THRESHOLD

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use parking_lot::RwLock;

// v0.37: background/async compaction (918a3b2e) renames a merge's outputs into
// place, invalidates the shared SST-path cache, THEN deletes the old inputs one
// at a time -- a reader whose pread() (index build or block read) lands on one
// of those inputs in the narrow window before its remove_file can race a
// concurrent compaction and get NotFound. This is EXPECTED and benign (the
// key is superseded by the just-renamed merge output) -- distinct from real
// corruption. Set by pread(), checked+cleared by Database::get() so a hit
// can trigger exactly one retry against a freshly-invalidated SST list instead
// of silently trusting a possibly-stale "not found". Thread-local: get() and
// every pread() it triggers run on the SAME calling thread; the async compactor
// runs on its own thread and never touches this cell.
thread_local! {
    static SST_VANISHED_RACE: std::cell::Cell<bool> = std::cell::Cell::new(false);
}

pub mod block;
pub mod cache;
pub mod cf;
pub mod filter;
pub mod ingest;
pub mod merge;
pub mod range_tomb;
pub mod skeleton;
pub mod ttl;
pub mod shard;

pub use skeleton::{SkeletonRecord, SkeletonStore};
pub use ingest::{build_sorted_sst_bytes, sst_ingest_enabled, BulkMode};

pub(crate) const SST_MAGIC: [u8; 4] = *b"FXDB";
/// SST version. v1 = monolithic LZ4 blob (pre-v0.12). v2 = block-based
/// format with index + footer; payload bytes start at the position right
/// after the bloom filter, but the payload is now `block::build_block_sst`
/// output rather than a single compressed buffer.
// v3 (chronos-v035 data-loss fix): body length is u64. v2 framed it as u32,
// so any SST body >= 4 GiB wrapped mod 2^32 -> reader saw a prefix -> footer
// parse failed silently -> gets returned None and compaction merged 'empty'
// inputs then deleted them (measured: 1.06 TB written, 4 GB left, 0.02% readable).
pub(crate) const SST_VERSION: u8 = 3;
const AUTO_COMPACT_THRESHOLD: usize = 4;
/// v0.36: default WAL auto-flush threshold. Once the approximate number of
/// bytes appended to the WAL since the last truncation exceeds this, the next
/// write triggers an automatic `flush()` (which persists the memtable to an
/// SST and truncates the WAL). Keeps WALs bounded for applications that never
/// call `flush()` themselves — a real store once reached a 689 MB WAL and a
/// 20.7 s replay at open. Configurable via `with_max_wal_bytes`; 0 disables.
const DEFAULT_MAX_WAL_BYTES: u64 = 64 * 1024 * 1024;
/// Default block cache size — small enough to be safe in a unit test,
/// configurable via `Database::with_block_cache_capacity`.
const DEFAULT_BLOCK_CACHE_BYTES: usize = 16 * 1024 * 1024;
/// v0.15: total number of levels in the LSM tree. L0 receives flushes;
/// Li (i > 0) receives compaction outputs from L(i-1).
pub const MAX_LEVEL: u8 = 7;
/// v0.15: number of L0 files allowed before we compact L0 into L1.
const L0_COMPACT_THRESHOLD: usize = 4;
/// v3 (LANE-C bulk-load): the L0 SST cap WHILE compaction is deferred. Fully skipping
/// compaction for a multi-million-block sync lets L0 grow unbounded, and every `get()` /
/// `has_height()` (the per-block linkage check) scans newest-to-oldest SSTs — so an
/// unbounded pile silently degrades the sync's own read path into a feedback stall (flagged
/// by the DeepSeek review). Bounding L0 at this (much higher than the eager `4`) cap keeps
/// the point-lookup fan-out sane while still cutting compaction frequency ~8×: the storm is
/// amortized, not eliminated, and the read path can't run away.
const BULK_L0_COMPACT_THRESHOLD: usize = 32;
/// v0.15: size pyramid factor — Li+1 target is `LEVEL_SIZE_RATIO ×` Li.
/// Smaller than RocksDB's default 10 to keep the test corpus snappy.
const LEVEL_SIZE_RATIO: usize = 4;
/// v0.37 (task #10): rolling size cap for streaming-compaction outputs. A
/// merge emits a new `flux_L{lvl}_{seq}.sst` every time the current output's
/// body crosses this, so no single SST grows unbounded (the 2 TB soak
/// produced ONE monolithic file whose whole-body reader was the RSS wall).
/// Override with FLUX_DB_COMPACT_OUTPUT_BYTES (>0).
const DEFAULT_COMPACT_OUTPUT_BYTES: u64 = 1024 * 1024 * 1024;

// ── LZ4 compression helpers ──

fn compress(data: &[u8]) -> Vec<u8> {
    // prepend_size=true so the decompressor doesn't need an out-of-band size.
    // Pre-v0.10.0 used prepend_size=false + decompress(None), which silently
    // round-trips garbage; that was latent because the old code never read
    // SSTs back. Now that get()/iter()/compact() all read SSTs, the bug
    // would corrupt every read — so we fix it here.
    lz4::block::compress(data, Some(lz4::block::CompressionMode::FAST(1)), true)
        .unwrap_or_else(|_| data.to_vec())
}

fn decompress(data: &[u8]) -> Vec<u8> {
    lz4::block::decompress(data, None).unwrap_or_else(|_| data.to_vec())
}

/// v3 (LANE 1): serialize one SST file image — header + bloom + block body — into a single
/// buffer, in the EXACT on-disk framing that [`SstReader::open`] parses:
///
/// ```text
///   magic(4) ‖ version(1) ‖ flags(1) ‖ key_count(u64 LE) ‖ num_hashes(u32 LE)
///     ‖ bloom_len(u32 LE) ‖ bloom_bytes ‖ block_body_len(u32 LE) ‖ block_body
/// ```
///
/// Factored out of `flush()` and `compact()` (which inlined byte-identical copies) so the
/// off-thread SST-ingest path (`ingest`) produces a bit-for-bit identical file and the
/// framing lives in ONE place. `block_body` is `block::build_block_sst` output (the v2
/// block-based payload). Changing these bytes is a read-format change — don't, casually.
pub(crate) fn encode_sst(key_count: u64, bloom: &BloomFilter, block_body: &[u8]) -> Vec<u8> {
    let bloom_bytes: Vec<u8> = bloom.bits.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut out = Vec::with_capacity(22 + bloom_bytes.len() + 4 + block_body.len());
    out.extend_from_slice(&SST_MAGIC);
    out.push(SST_VERSION);
    out.push(0u8); // flags
    out.extend_from_slice(&key_count.to_le_bytes());
    out.extend_from_slice(&(bloom.num_hashes as u32).to_le_bytes());
    out.extend_from_slice(&(bloom_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&bloom_bytes);
    out.extend_from_slice(&(block_body.len() as u64).to_le_bytes()); // v3: u64 (v2's u32 wrapped at 4 GiB)
    out.extend_from_slice(block_body);
    out
}

/// Main database handle — thread-safe, cloneable.
pub struct Database {
    inner: Arc<RwLock<DbInner>>,
    path: PathBuf,
    /// LRU cache of decompressed SST blocks. Shared across all `Database`
    /// clones (it's already `Arc<Mutex>` internally).
    block_cache: Arc<cache::BlockCache>,
    /// v0.14: registry of column families. Shared across clones so
    /// `db.cf("users")` returns the same handle for every clone of `db`.
    cfs: Arc<cf::CfRegistry>,
    /// v0.35: cache of opened SST readers, keyed by path, shared across clones.
    /// SST files are IMMUTABLE once written (LSM: flush creates, compaction
    /// replaces+deletes), so caching a reader per path is sound. Before this,
    /// `get()`/`iter()` called `SstReader::open` PER CALL per SST — with the old
    /// eager open that meant re-reading the entire store from disk on every
    /// lookup. Pruned against the live SST list after compaction.
    sst_readers: Arc<RwLock<std::collections::HashMap<PathBuf, Arc<SstReader>>>>,
    /// v3 (LANE-C, the 285µs/block fix): cached SST path list. `get()`/`iter*()` previously
    /// called `list_ssts()` — an `fs::read_dir` syscall + per-file name-parse — on EVERY
    /// memtable miss. During a forward chain sync every lookup is a guaranteed miss (brand-new
    /// keys), so the commit path paid ~2-3 `read_dir`s PER BLOCK (~285µs, batch- and fsync-
    /// independent — the real >20k wall). SSTs change ONLY on `flush()` (adds L0) and `compact()`
    /// (adds+removes); both invalidate this cache; between them the set is immutable, so caching
    /// is sound and negative lookups become O(bloom) with ZERO syscalls. Shared across clones.
    sst_paths: Arc<RwLock<Option<Vec<PathBuf>>>>,
    /// v0.37 (chronos-v035 finding): true while a background compaction thread is
    /// running. flush() spawns compaction on a side thread so a large merge no longer
    /// freezes ingestion; this flag admits only ONE at a time. Shared across clones.
    compacting: Arc<std::sync::atomic::AtomicBool>,
    /// v0.37: TRUE mutual exclusion for compact_inner() across every entry
    /// point (explicit compact(), compact_async(), the flush-spawned background
    /// pass, ingest bulk paths). `compacting` above is kept as a cheap HINT so
    /// flush() can skip spawning a redundant thread when one's already running,
    /// but it is NOT sufficient on its own: two compact_inner() calls racing each
    /// other's `fs::remove_file` can make one side's `SstReader::open()` hit a
    /// hard NotFound error (unlike the graceful per-file degrade `load_index()`
    /// does for a READER racing a compaction). A blocking lock -- not a no-op
    /// skip -- is required so every caller still observes compact()'s existing
    /// synchronous-completion contract (callers assume real work happened by the
    /// time compact() returns; see test_v15_compaction_promotes_to_l1).
    compact_lock: Arc<parking_lot::Mutex<()>>,
    /// v0.38: true while a background WAL-cap flush thread is running (single-flight).
    /// Shared across clones. See `flush_background` / `auto_flush_if_needed`.
    flushing: Arc<std::sync::atomic::AtomicBool>,
}

struct DbInner {
    memtable: BTreeMap<Vec<u8>, Vec<u8>>,
    wal_file: Option<fs::File>,
    /// v0.36: approximate bytes appended to the WAL since the last truncation.
    /// Initialized from the WAL file length at `open()`, incremented by entry
    /// size on every WAL write, reset to 0 when `flush()` truncates the WAL.
    wal_bytes: u64,
    /// v0.36: auto-flush threshold (see `DEFAULT_MAX_WAL_BYTES`). 0 = disabled.
    /// Lives in DbInner so every clone of this `Database` shares the setting.
    max_wal_bytes: u64,
    sequence: u64,
    mod_history: std::collections::HashMap<Vec<u8>, u64>,
    /// Active range tombstones (v0.13). Each masks any key in `[start, end)`
    /// from `get()`. They drop their target during the next `compact()`.
    range_tombs: Vec<range_tomb::RangeTombstone>,
    /// v0.16: optional merge operator.
    merge_op: RwLock<Option<Arc<dyn merge::MergeOperator>>>,
    /// v0.16: optional compaction filter.
    compaction_filter: RwLock<Option<Arc<dyn filter::CompactionFilter>>>,
    /// v3 (LANE-C bulk-load): when true, `flush()` persists the memtable to an SST
    /// but SKIPS the synchronous post-flush `compact()` call. A forward chain sync is
    /// append-mostly (unique hash/height keys, ~zero overwrites), so leveled compaction
    /// during the sync is almost pure write-amplification (full-level rewrite, ratio 4
    /// per level → 10-20× the ingest bytes) — the real >20k-blk/s wall, NOT fsync.
    /// Defer it: ingest fast into many L0 SSTs, then `compact()` ONCE at the tip.
    /// Default false (unchanged behavior for every existing caller).
    defer_compaction: bool,
    /// v0.37 (task #10): rolling size cap for streaming-compaction outputs
    /// (see DEFAULT_COMPACT_OUTPUT_BYTES). Lives in DbInner so every clone
    /// shares it; FLUX_DB_COMPACT_OUTPUT_BYTES overrides at merge time.
    compact_output_bytes: u64,
}

/// Snapshot of the database at a point in time (MVCC read).
pub struct Snapshot {
    data: BTreeMap<Vec<u8>, Vec<u8>>,
    seq: u64,
}

/// v0.50 (LANE-A): sliding-window size for streaming WAL replay. The replay holds at
/// most ~one window (plus a single oversized entry, if one exceeds it) in RAM at a time,
/// so ANY WAL size replays in bounded memory — the 1.6 GB WAL that ate 3 GB RSS via the
/// old `read_to_end` slurp now costs ~8 MiB for the reader. 8 MiB matches the chunk size
/// used elsewhere in the stack and amortizes syscalls without bloating the resident set.
const WAL_REPLAY_WINDOW: usize = 8 * 1024 * 1024;

/// Streaming WAL replay into `memtable`. Replaces the former whole-file `read_to_end`
/// slurp (which ballooned RAM to ~2× the WAL size — the OOM the 256 MiB quarantine guard
/// only papers over). Reads the WAL through a fixed sliding window so peak RSS is bounded
/// by `WAL_REPLAY_WINDOW` regardless of file size.
///
/// Wire format, CRC coverage, tombstone, and torn-write STOP semantics are BYTE-IDENTICAL
/// to the slurp it replaces:
///   entry = `[crc32: u32 LE][key_len: u32 LE][val_len: u32 LE][key][val]`
///   the CRC covers `key_len ++ val_len ++ key ++ val`; replay stops at the FIRST bad CRC
///   or truncated tail (pre-crash entries are kept, the partial post-crash entry dropped).
fn replay_wal_streaming(
    wal_path: &std::path::Path,
    memtable: &mut BTreeMap<Vec<u8>, Vec<u8>>,
) -> Result<(), String> {
    let mut f = fs::File::open(wal_path).map_err(|e| format!("open wal: {}", e))?;
    // `buf` is the live window; `pos` is the parse cursor — bytes [pos..buf.len()) are
    // unparsed. `pos` advances entry-by-entry WITHOUT copying; the window is only compacted
    // (the consumed prefix dropped) and refilled when the tail can't satisfy the next read.
    // `eof` latches once the file is drained so we stop refilling.
    let mut buf: Vec<u8> = Vec::with_capacity(WAL_REPLAY_WINDOW + 4096);
    let mut pos = 0usize;
    let mut eof = false;

    // Ensure at least `need` unparsed bytes are available at `buf[pos..]`, reading more from
    // the file as required. FAST PATH: when the tail already satisfies `need`, return without
    // copying or a syscall — this is the per-entry common case, so total memmove cost is
    // ~O(file), not O(file × window). Only when the tail is too short do we compact the
    // consumed prefix ONCE and refill a full window. Returns the unparsed-byte count from
    // `pos` (>= `need` unless EOF was hit first).
    fn ensure(
        f: &mut fs::File,
        buf: &mut Vec<u8>,
        pos: &mut usize,
        eof: &mut bool,
        need: usize,
    ) -> Result<usize, String> {
        if buf.len() - *pos >= need {
            return Ok(buf.len() - *pos); // fast path: nothing to do
        }
        if *pos > 0 {
            buf.drain(0..*pos); // slide the small leftover tail to the front (≈once per window)
            *pos = 0;
        }
        while buf.len() < need && !*eof {
            let old = buf.len();
            // Read at least a full window; grow to fit a single entry larger than the
            // window (rare but must be CRC-checked whole, so it can't be streamed in halves).
            let want = WAL_REPLAY_WINDOW.max(need - old);
            buf.resize(old + want, 0);
            let n = f.read(&mut buf[old..]).map_err(|e| format!("read wal: {}", e))?;
            buf.truncate(old + n);
            if n == 0 {
                *eof = true;
            }
        }
        Ok(buf.len() - *pos)
    }

    loop {
        // 12-byte header: crc + key_len + val_len.
        if ensure(&mut f, &mut buf, &mut pos, &mut eof, 12)? < 12 {
            break; // clean end (no more entries) or a header-sized torn tail — drop & stop
        }
        let crc = u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]);
        let key_len = u32::from_le_bytes([buf[pos + 4], buf[pos + 5], buf[pos + 6], buf[pos + 7]]) as usize;
        let val_len = u32::from_le_bytes([buf[pos + 8], buf[pos + 9], buf[pos + 10], buf[pos + 11]]) as usize;
        let total = 12 + key_len + val_len;
        if ensure(&mut f, &mut buf, &mut pos, &mut eof, total)? < total {
            break; // torn write: the entry body is truncated — drop & stop
        }
        let body = &buf[pos + 4..pos + total]; // key_len ++ val_len ++ key ++ val (CRC coverage)
        if crc32fast::hash(body) != crc {
            break; // torn write / corruption: stop at the first bad CRC
        }
        let key = buf[pos + 12..pos + 12 + key_len].to_vec();
        let val = buf[pos + 12 + key_len..pos + total].to_vec();
        if val_len == 0 {
            memtable.remove(&key); // tombstone
        } else {
            memtable.insert(key, val);
        }
        pos += total; // advance the cursor; the window slides only when the tail runs short
    }
    Ok(())
}

impl Database {
    /// Open or create a database at the given path.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        fs::create_dir_all(&path).map_err(|e| format!("create dir: {}", e))?;

        // v3 (LANE 1): sweep orphan `*.sst.tmp` left by a kill-9 mid-ingest (crash after
        // tmp write, before the atomic rename). They are never listed as live SSTs
        // (list_ssts matches only `.sst`/`.sst.lz4`) so they can't corrupt reads — this
        // just reclaims their disk. The ingested prefix is re-pulled by the caller because
        // the durable tip never advanced past an un-renamed SST (ingest-or-nothing).
        ingest::sweep_stale_tmp(&path);

        let wal_path = path.join("flux.wal");
        // WAL-bomb guard: a WAL beyond this cap is evidence of a broken truncation
        // epoch (the Windows append-mode bug let WALs grow to GBs); replaying one
        // slurps the whole file AND duplicates every entry into the memtable —
        // ballooning RAM by ~2x the file size in under a second and freezing the
        // host (proven on a real machine: a 1.6 GB WAL ate 3 GB RSS; a 4.1 GB one
        // ate 4 GB). Quarantine instead of replay: the bytes stay on disk for
        // manual recovery and the store resumes from its last checkpointed SSTs.
        //
        // v0.35 re-sizing: the original 256 MiB cap was an OOM guard for the old
        // whole-file slurp replay. Streaming replay (WAL_REPLAY_WINDOW) bounds RSS
        // at 8 MiB regardless of WAL size, so the cap's job is now only to stop
        // PATHOLOGICAL WALs (runaway/corrupt files), not normal large windows —
        // consumers like sigil-node legitimately run max_wal_bytes = 1 GiB (the
        // chronos-v035 matrix: +22% write throughput, ~20x lower read p99), and a
        // 256 MiB cap would quarantine (set aside!) their crash-recovery data.
        // Measured replay speed is ~330 MB/s, so the 4 GiB default is a ~13 s
        // worst-case open. FLUX_DB_WAL_QUARANTINE_BYTES overrides; 0 disables.
        let wal_quarantine_bytes: u64 = std::env::var("FLUX_DB_WAL_QUARANTINE_BYTES").ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4 * 1024 * 1024 * 1024);
        if wal_quarantine_bytes > 0 {
            if let Ok(m) = fs::metadata(&wal_path) {
                if m.len() > wal_quarantine_bytes {
                    let q = path.join(format!("flux.wal.quarantined-{}", m.len()));
                    eprintln!("[flux-db] WAL is {} bytes (cap {}) — quarantining to {:?}; resuming from last checkpoint",
                        m.len(), wal_quarantine_bytes, q);
                    let _ = fs::rename(&wal_path, &q);
                }
            }
        }
        let mut memtable = BTreeMap::new();

        // Replay WAL if it exists. Each entry is:
        //   [crc32: u32 LE][key_len: u32 LE][val_len: u32 LE][key][val]
        // CRC covers key_len ++ val_len ++ key ++ val. On the first bad CRC
        // or truncated tail we stop — pre-crash entries are safe; the partial
        // post-crash entry is discarded.
        if wal_path.exists() {
            // v0.50 (LANE-A): STREAMING replay — bounded RAM regardless of WAL size. The old
            // read_to_end slurp held the whole WAL (+ a ~equal-size memtable) resident, which
            // is the OOM the 256 MiB quarantine above only papers over. Semantics (CRC, torn
            // write, tombstone) are byte-identical; see `replay_wal_streaming`.
            replay_wal_streaming(&wal_path, &mut memtable)?;
        }

        let mut wal_file = fs::OpenOptions::new()
            .create(true).read(true).write(true)
            .open(&wal_path)
            .map_err(|e| format!("create wal: {}", e))?;
        // v0.36.1: NOT append-mode. Windows strips FILE_WRITE_DATA from append
        // handles, so set_len(0) (WAL truncate after flush) is denied (os err 5,
        // "Adgang naegtet"). A plain write handle seeked to EOF appends
        // identically AND can truncate. write_wal_entry only write_all's, so the
        // position advances naturally; truncate seeks back to 0.
        std::io::Seek::seek(&mut wal_file, std::io::SeekFrom::End(0))
            .map_err(|e| format!("wal seek end: {}", e))?;

        // v0.36: seed the auto-flush counter from the WAL bytes already on
        // disk, so a store reopened with a fat WAL flushes on its first write
        // instead of growing the backlog by another `max_wal_bytes`.
        let wal_bytes = wal_file.metadata().map(|m| m.len()).unwrap_or(0);

        // v0.37 (task #10): seed the in-memory sequence from the highest
        // sequence embedded in the existing SST filenames. It used to reset
        // to 0 on every open, while SST names DERIVE from it — so after a
        // crash-restart a deterministic workload reproduced the same sequence
        // numbers and compaction outputs RENAMED OVER the previous epoch's
        // SSTs. Measured in kill -9 chaos: two same-sized flux_L01_...7f82
        // files from different epochs, three whole deciles (0..255k of 850k
        // blocks) physically destroyed, presence 60.85%. Monotonic naming
        // across restarts kills the entire collision class. `+1` so even a
        // replay-only flush (no new puts) can never reuse the max name.
        let max_sst_seq = list_ssts_leveled(&path)
            .map(|hs| hs.iter().map(|h| h.sequence).max().unwrap_or(0))
            .unwrap_or(0);
        let seed_sequence = if max_sst_seq == 0 { 0 } else { max_sst_seq.saturating_add(1) };

        Ok(Database {
            inner: Arc::new(RwLock::new(DbInner {
                memtable,
                wal_file: Some(wal_file),
                wal_bytes,
                max_wal_bytes: DEFAULT_MAX_WAL_BYTES,
                sequence: seed_sequence,
                mod_history: std::collections::HashMap::new(),
                range_tombs: Vec::new(),
                merge_op: RwLock::new(None),
                compaction_filter: RwLock::new(None),
                defer_compaction: false,
                compact_output_bytes: DEFAULT_COMPACT_OUTPUT_BYTES,
            })),
            path: path.clone(),
            block_cache: Arc::new(cache::BlockCache::new(DEFAULT_BLOCK_CACHE_BYTES)),
            cfs: Arc::new(cf::CfRegistry::new(path)),
            sst_readers: Arc::new(RwLock::new(std::collections::HashMap::new())),
            sst_paths: Arc::new(RwLock::new(None)),
            compacting: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            compact_lock: Arc::new(parking_lot::Mutex::new(())),
            flushing: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    /// Configure the LRU block cache capacity. Default is 16 MiB. Returns
    /// `self` to enable builder-style chaining at open time:
    ///
    /// ```text
    /// let db = Database::open(p)?.with_block_cache_capacity(64 * 1024 * 1024);
    /// ```
    pub fn with_block_cache_capacity(self, bytes: usize) -> Self {
        Database {
            inner: self.inner,
            path: self.path,
            block_cache: Arc::new(cache::BlockCache::new(bytes)),
            cfs: self.cfs,
            sst_readers: self.sst_readers,
            sst_paths: self.sst_paths,
            compacting: self.compacting,
            compact_lock: self.compact_lock,
            flushing: self.flushing,
        }
    }

    /// v0.36: configure the WAL auto-flush threshold. When the approximate
    /// bytes appended to the WAL since the last truncation exceed this, the
    /// write that crossed the line triggers an automatic `flush()` (memtable →
    /// SST, then WAL truncate). Default is 64 MiB; `0` disables auto-flush
    /// entirely (the pre-v0.36 behavior — only explicit `flush()` truncates).
    /// Builder-style, for chaining at open time:
    ///
    /// ```text
    /// let db = Database::open(p)?.with_max_wal_bytes(8 * 1024 * 1024);
    /// ```
    pub fn with_max_wal_bytes(self, bytes: u64) -> Self {
        self.inner.write().max_wal_bytes = bytes;
        self
    }

    /// v3 (LANE-C): fsync ONLY the WAL — the cheap "one fsync per batch" durability
    /// primitive. Unlike `flush()` (memtable.clone → lz4 → SST sync_all → WAL truncate →
    /// maybe synchronous compaction), this just forces the already-appended WAL bytes
    /// durable via `fdatasync`. After it returns, every `put`/`batch_put` issued before it
    /// survives a power loss (open() replays the WAL into the memtable). This is what lets a
    /// batched commit be durable WITHOUT paying the SST-flush + compaction-storm cost on the
    /// hot path. `sync_data` (not `sync_all`): the WAL is a plain growing file with no
    /// out-of-band metadata that crash recovery depends on (replay reads by content+CRC, not
    /// by the inode-recorded length), so skipping the metadata flush is sound and ~2× cheaper.
    /// Holds the READ lock for the fsync, which EXCLUDES any concurrent `put`/`batch_put`
    /// (they take the write lock) — so no torn append can race the sync. fdatasync forcing
    /// MORE than the caller's own bytes durable is harmless: durability is monotonic, and the
    /// commit path's ordering invariant (blocks fsync'd before the tip) is already established
    /// by the earlier phase-2 sync, independent of whatever else the fsync happens to flush.
    /// (In sigil-top the BlockStore commit path is the SOLE writer — `&mut` — and BlockReader
    /// is read-only, so there is in fact no concurrent writer at all.)
    pub fn sync_wal(&self) -> Result<(), String> {
        let inner = self.inner.read();
        if let Some(w) = inner.wal_file.as_ref() {
            w.sync_data().map_err(|e| format!("wal sync_data: {}", e))?;
        }
        Ok(())
    }

    /// v3 (LANE-C): runtime setter for the WAL auto-flush threshold (builder `with_max_wal_bytes`
    /// only works at open time). Bulk-load raises this so the memtable grows larger and flushes
    /// to SSTs far less often during a high-rate sync; restore it for steady-state operation.
    pub fn set_max_wal_bytes(&self, bytes: u64) {
        self.inner.write().max_wal_bytes = bytes;
    }

    /// v3 (LANE-C bulk-load): toggle deferred compaction (see `DbInner::defer_compaction`).
    /// Set `true` before a high-rate sync so `flush()` never blocks the commit thread on a
    /// full-level rewrite; set `false` and call `compact()` once when the sync reaches tip.
    pub fn set_defer_compaction(&self, defer: bool) {
        self.inner.write().defer_compaction = defer;
    }

    /// v3 (LANE-C): is post-flush compaction currently deferred?
    pub fn is_compaction_deferred(&self) -> bool {
        self.inner.read().defer_compaction
    }

    /// v0.37 (task #10): configure the rolling size cap for streaming-
    /// compaction outputs (default ~1 GiB; see DEFAULT_COMPACT_OUTPUT_BYTES).
    /// Builder-style; shared by every clone of this Database. Values of 0 are
    /// ignored (keeps the default). FLUX_DB_COMPACT_OUTPUT_BYTES still wins
    /// at merge time when set.
    pub fn with_compact_output_bytes(self, bytes: u64) -> Self {
        if bytes > 0 {
            self.inner.write().compact_output_bytes = bytes;
        }
        self
    }

    /// v0.36: if the WAL has outgrown `max_wal_bytes`, flush. MUST be called
    /// only when the caller holds NO lock on `self.inner` — `flush()` takes
    /// the inner write lock and parking_lot locks are not reentrant, so
    /// calling this under a `put()`/`batch_put()` guard would deadlock.
    /// Auto-flush failure never fails the triggering write: the data is
    /// already durable in WAL + memtable, so we log and move on.
    /// v0.38: single-flight BACKGROUND flush. Returns true if a flush is running
    /// (just spawned or already in flight); false if we won the flag but the OS
    /// refused a thread (caller should flush inline). Same flush(), same durability —
    /// only the calling thread changes; flush() already snapshots under a read lock
    /// and writes the SST lock-free, so writers/readers proceed during it.
    pub fn flush_background(&self) -> bool {
        if self.flushing
            .compare_exchange(false, true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst).is_err()
        {
            return true; // already in flight
        }
        let db = self.clone();
        let spawned = std::thread::Builder::new()
            .name("flux-db-flush".into())
            .spawn(move || {
                struct Guard(Arc<std::sync::atomic::AtomicBool>);
                impl Drop for Guard {
                    fn drop(&mut self) { self.0.store(false, std::sync::atomic::Ordering::SeqCst); }
                }
                let _g = Guard(db.flushing.clone());
                let _ = db.flush();
            });
        if spawned.is_err() {
            self.flushing.store(false, std::sync::atomic::Ordering::SeqCst);
            return false;
        }
        true
    }

    fn auto_flush_if_needed(&self) {
        let (wal_bytes, max) = {
            let inner = self.inner.read();
            (inner.wal_bytes, inner.max_wal_bytes)
        };
        if max == 0 || wal_bytes <= max {
            return;
        }
        // v0.38 ASYNC AUTO-FLUSH (measured): this flush ran INLINE inside put()/batch_put()
        // on the writer thread — at a sustained ~10k blk/s chain sync that's a 1-4 s
        // memtable→SST write holding the apply loop every time the WAL cap trips (the
        // residual stall after compaction went background). Flush in the background,
        // single-flight. BACKPRESSURE: if the WAL outruns the background flush past 4×
        // the cap (writer faster than the disk), flush INLINE — bounded WAL growth =
        // bounded crash-replay time; the stall is the honest cost of a saturated disk.
        let inline = wal_bytes > max.saturating_mul(4) || !self.flush_background();
        if inline {
            if let Err(e) = self.flush() {
                eprintln!(
                    "flux-db: auto-flush at {} WAL bytes (threshold {}) failed: {} — \
                     continuing; data is safe in WAL+memtable",
                    wal_bytes, max, e
                );
            }
        }
    }

    /// v0.35: fetch (or open-and-cache) the reader for one SST. Fast path is a
    /// read-lock map hit; misses open the reader (header+bloom only — the body
    /// is lazy) and insert. SSTs are immutable, so entries never go stale —
    /// deleted files simply stop being asked for (the caller iterates the live
    /// `list_ssts` result) and are pruned after compaction.
    fn sst_cached(&self, path: &std::path::Path) -> Result<Arc<SstReader>, String> {
        if let Some(r) = self.sst_readers.read().get(path) {
            return Ok(Arc::clone(r));
        }
        let r = Arc::new(SstReader::open(path)?);
        self.sst_readers.write().insert(path.to_path_buf(), Arc::clone(&r));
        Ok(r)
    }

    /// v3 (LANE-C): the SST path list, from cache when warm. A cache miss does ONE `list_ssts`
    /// (`fs::read_dir` + name-parse) and memoizes it; subsequent reads (the hot negative-lookup
    /// path during sync) cost only a read-lock + clone — no syscall. Invalidated by `flush()` /
    /// `compact()`, the only operations that change the SST set. Returns oldest-data-first
    /// (deepest level first; see `list_ssts`), so callers that need newest-first still `.rev()`.
    fn cached_sst_paths(&self) -> Result<Vec<PathBuf>, String> {
        if let Some(paths) = self.sst_paths.read().as_ref() {
            return Ok(paths.clone());
        }
        let paths = list_ssts(&self.path)?;
        *self.sst_paths.write() = Some(paths.clone());
        Ok(paths)
    }

    /// v3 (LANE-C): drop the cached SST path list so the next read re-enumerates. MUST be called
    /// after any `flush()` (new L0 SST) or `compact()` (SSTs added+removed) — else a reader could
    /// miss a freshly-flushed SST and wrongly return `None` for a key that IS on disk.
    fn invalidate_sst_paths(&self) {
        *self.sst_paths.write() = None;
    }

    /// Cache stats as (hits, misses, current_bytes). Useful for tests and
    /// telemetry — a healthy workload should see hit-rate > 90% on repeat
    /// gets to the same SST.
    pub fn block_cache_stats(&self) -> (u64, u64, usize) {
        self.block_cache.stats()
    }

    /// v0.13: delete every key in `[start, end)` with a single range tombstone.
    /// Cheaper than N individual deletes, and `get()` honors the tombstone
    /// immediately. Compaction later drops the covered keys for good.
    pub fn delete_range(&self, start: &[u8], end: &[u8]) -> Result<(), String> {
        if start >= end {
            return Ok(());
        }
        let mut inner = self.inner.write();
        inner.sequence += 1;
        let seq = inner.sequence;
        inner.range_tombs.push(range_tomb::RangeTombstone {
            start: start.to_vec(),
            end: end.to_vec(),
            seq,
        });
        // Also drop in-memtable matches now so iter() / scans skip them.
        let covered: Vec<Vec<u8>> = inner.memtable
            .range(start.to_vec()..end.to_vec())
            .map(|(k, _)| k.clone())
            .collect();
        for k in covered {
            inner.memtable.insert(k.clone(), Vec::new());
            inner.mod_history.insert(k, seq);
        }
        Ok(())
    }

    // ── v0.16: TTL ──

    /// Insert a key with an expiry time (seconds since UNIX epoch). Stores
    /// `[u64 expiry_unix][value]`; `get()` returns the RAW wrapped bytes —
    /// readers enforce expiry with `ttl::unwrap(raw, now)` (get() cannot
    /// unwrap unconditionally without corrupting ordinary >=8-byte values).
    /// Compaction drops expired keys for good.
    pub fn put_with_ttl(&self, key: &[u8], value: &[u8], expiry_unix: u64) -> Result<(), String> {
        let wrapped = ttl::wrap(value, expiry_unix);
        self.put(key, &wrapped)
    }

    /// Convenience: live for `seconds` from now.
    pub fn put_ttl_seconds(&self, key: &[u8], value: &[u8], seconds: u64) -> Result<(), String> {
        self.put_with_ttl(key, value, ttl::now_unix().saturating_add(seconds))
    }

    // ── v0.16: merge operator ──

    /// Install (or replace) the merge operator. `db.merge(k, delta)` will
    /// call `op.merge(existing, delta)` to compute the new value, all under
    /// the writer lock.
    pub fn set_merge_operator(&self, op: Arc<dyn merge::MergeOperator>) {
        *self.inner.write().merge_op.write() = Some(op);
    }

    /// Merge a delta into a key. The current value (or None if absent) and
    /// the delta are passed to the installed `MergeOperator`. Without an
    /// operator installed, returns an error.
    pub fn merge(&self, key: &[u8], delta: &[u8]) -> Result<(), String> {
        let op = self
            .inner
            .read()
            .merge_op
            .read()
            .clone()
            .ok_or("no merge operator installed — call db.set_merge_operator() first")?;
        let existing = self.get(key)?;
        let combined = op.merge(existing.as_deref(), delta);
        self.put(key, &combined)
    }

    // ── v0.16: compaction filter ──

    /// Install (or replace) the compaction filter. Runs on every key/value
    /// during `compact()`. Use it to drop expired records, evict tombstones,
    /// or transform values forward to a new schema.
    pub fn set_compaction_filter(&self, filter: Arc<dyn filter::CompactionFilter>) {
        *self.inner.write().compaction_filter.write() = Some(filter);
    }

    // ── v0.14: column families ──

    /// Create or open a column family. The returned `Database` handle is
    /// a fully-featured DB rooted at `<db_path>/cf_<name>/` and shares the
    /// parent's block cache. Calling `create_cf("users")` twice returns the
    /// same handle.
    pub fn create_cf(&self, name: &str) -> Result<Database, String> {
        self.cfs.create_cf(name)
    }

    /// Look up an already-created CF by name. None if it doesn't exist.
    pub fn cf(&self, name: &str) -> Option<Database> {
        self.cfs.cf(name)
    }

    /// List currently-open CF names. Does NOT include the implicit default
    /// (which is just `self`).
    pub fn list_cfs(&self) -> Vec<String> {
        self.cfs.list()
    }

    /// Drop a CF: removes the handle and recursively deletes its on-disk
    /// directory. Idempotent on missing CFs.
    pub fn drop_cf(&self, name: &str) -> Result<(), String> {
        self.cfs.drop_cf(name)
    }

    /// v0.13: kick off compaction on a background thread. Returns a handle
    /// the caller can `.join()` to wait for completion. The compaction runs
    /// the same logic as the synchronous `compact()`, just off the writer
    /// thread so puts don't stall.
    pub fn compact_async(&self) -> std::thread::JoinHandle<Result<(), String>> {
        let db = self.clone();
        std::thread::spawn(move || db.compact())
    }

    /// v0.13: iterator builder that supports `seek(key)` for O(log n) jumps
    /// into the merged view (memtable + SSTs).
    pub fn iter_from(&self, start: &[u8]) -> DbIterator {
        // Build the merged view exactly as iter() does, then skip past start.
        let mut merged: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        if let Ok(ssts) = self.cached_sst_paths() {
            for sst in &ssts {
                if let Ok(table) = self.sst_cached(sst) {
                    for (k, v) in table.pairs() {
                        if v.is_empty() { merged.remove(&k); } else { merged.insert(k, v); }
                    }
                }
            }
        }
        let inner = self.inner.read();
        for (k, v) in inner.memtable.iter() {
            if v.is_empty() { merged.remove(k); } else { merged.insert(k.clone(), v.clone()); }
        }
        for tomb in &inner.range_tombs {
            let to_drop: Vec<Vec<u8>> = merged
                .range(tomb.start.clone()..tomb.end.clone())
                .map(|(k, _)| k.clone())
                .collect();
            for k in to_drop { merged.remove(&k); }
        }
        // Skip past start.
        let tail: BTreeMap<Vec<u8>, Vec<u8>> = merged
            .into_iter()
            .filter(|(k, _)| k.as_slice() >= start)
            .collect();
        DbIterator { inner: tail.into_iter(), descending: false }
    }

    /// Insert a key-value pair.
    pub fn put(&self, key: &[u8], value: &[u8]) -> Result<(), String> {
        // v0.36: the auto-flush check-and-call lives AFTER this lock scope —
        // flush() takes the same (non-reentrant) write lock, so calling it
        // while holding the guard would deadlock.
        let need_flush = {
            let mut inner = self.inner.write();
            inner.sequence += 1;
            let seq = inner.sequence;
            let n = write_wal_entry(inner.wal_file.as_mut(), key, value)?;
            inner.wal_bytes += n;
            inner.memtable.insert(key.to_vec(), value.to_vec());
            inner.mod_history.insert(key.to_vec(), seq);
            inner.max_wal_bytes > 0 && inner.wal_bytes > inner.max_wal_bytes
        };
        if need_flush {
            self.auto_flush_if_needed();
        }
        Ok(())
    }

    /// v0.51 (LANE-B bulk-ingest): insert MANY key/value pairs under a SINGLE
    /// write-lock acquisition, coalescing every WAL record into ONE buffer that
    /// is written with ONE `write_all` + ONE `flush` syscall for the whole batch.
    ///
    /// Why this exists (measured on 8 KB chronos blocks, defer_compaction, 4 GiB WAL):
    ///   * `put()` per entry:            ~168 MB/s  (lock + 3 syscalls PER 8 KB block)
    ///   * `write(WriteBatch=64)`:       ~346 MB/s  (lock once, but still 3 syscalls/entry
    ///                                                and a double key/val copy into BatchOp)
    ///   * `put_many(batch=256)`:        see SHARDED-WRITER.md — one syscall per batch,
    ///                                    no BatchOp staging, direct memtable insert.
    ///
    /// The per-entry WAL framing is IDENTICAL to `write_wal_entry` — each record is
    /// `[crc32(le)][key_len(le u32)][val_len(le u32)][key][val]`, independently CRC-
    /// covered — so `replay_wal_streaming` is unchanged and torn-write recovery still
    /// stops cleanly at the first bad record. We simply concatenate the framed records
    /// in memory and flush them in one shot.
    ///
    /// Durability is UNCHANGED from `put()`: entries are written to the WAL and
    /// `flush()`ed to the OS (one syscall), but NOT fsync'd here — callers batch the
    /// durability barrier via `sync_wal()` exactly as they do for `put()` (chronos_scale
    /// fsyncs before advancing its height marker). An empty `value` is a tombstone,
    /// matching `delete()`.
    ///
    /// Auto-flush (memtable -> SST when the WAL outgrows `max_wal_bytes`) is honored
    /// once, AFTER the lock is released — same non-reentrancy rule as `put()`.
    pub fn put_many<K: AsRef<[u8]>, V: AsRef<[u8]>>(&self, entries: &[(K, V)]) -> Result<(), String> {
        if entries.is_empty() {
            return Ok(());
        }
        // Pre-size the coalesced WAL buffer: 12-byte header per entry + payloads.
        let approx: usize = entries.iter()
            .map(|(k, v)| 12 + k.as_ref().len() + v.as_ref().len())
            .sum();
        let mut wal_buf: Vec<u8> = Vec::with_capacity(approx);
        for (k, v) in entries {
            let (key, val) = (k.as_ref(), v.as_ref());
            // header: key_len ++ val_len (the CRC covers header ++ key ++ val,
            // i.e. everything except the leading crc field itself).
            let mut header = [0u8; 8];
            header[0..4].copy_from_slice(&(key.len() as u32).to_le_bytes());
            header[4..8].copy_from_slice(&(val.len() as u32).to_le_bytes());
            let mut crc = crc32fast::Hasher::new();
            crc.update(&header);
            crc.update(key);
            crc.update(val);
            let crc = crc.finalize();
            wal_buf.extend_from_slice(&crc.to_le_bytes());
            wal_buf.extend_from_slice(&header);
            wal_buf.extend_from_slice(key);
            wal_buf.extend_from_slice(val);
        }

        let need_flush = {
            let mut inner = self.inner.write();
            // ONE write + ONE flush for the entire batch (vs 3 syscalls/entry).
            if let Some(w) = inner.wal_file.as_mut() {
                w.write_all(&wal_buf).map_err(|e| format!("wal write batch: {}", e))?;
                w.flush().map_err(|e| format!("wal flush batch: {}", e))?;
            }
            inner.wal_bytes += wal_buf.len() as u64;
            // Bulk-apply to the memtable + mod_history. One sequence stamp per entry
            // keeps mod_history / snapshot semantics identical to N separate put()s.
            for (k, v) in entries {
                inner.sequence += 1;
                let seq = inner.sequence;
                let key = k.as_ref().to_vec();
                inner.memtable.insert(key.clone(), v.as_ref().to_vec());
                inner.mod_history.insert(key, seq);
            }
            inner.max_wal_bytes > 0 && inner.wal_bytes > inner.max_wal_bytes
        };
        if need_flush {
            self.auto_flush_if_needed();
        }
        Ok(())
    }

    /// Get a value by key. Searches memtable first, then the newest-to-oldest
    /// SSTs (skipping any whose persisted Bloom filter says "definitely not").
    /// A tombstone in either layer (empty value) means "deleted" and overrides
    /// any older value in a deeper SST.
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
        {
            let inner = self.inner.read();
            // v0.13: range tombstones shadow any older data for keys they cover.
            if range_tomb::is_covered(&inner.range_tombs, key) {
                return Ok(None);
            }
            if let Some(v) = inner.memtable.get(key) {
                return Ok(if v.is_empty() { None } else { Some(v.clone()) });
            }
        }
        // v0.37: bounded retry against the compact_async() vanished-file race (see
        // SST_VANISHED_RACE) -- one extra attempt with a forced-fresh SST list is
        // enough because a fresh list always reflects an internally-consistent
        // post-rename filesystem state (compact() renames outputs in BEFORE it
        // invalidates the cache or removes any input, see compact()).
        for attempt in 0..2 {
            SST_VANISHED_RACE.with(|c| c.set(false));
            // Read-through SSTs. Newest first: `.rev()` of the read-merge order =
            // L0 by descending sequence, THEN deeper levels — a fresh L0 copy must
            // shadow the stale copy a compaction pushed to L1+.
            // v0.12: lookups consult the LRU block cache to avoid re-decompressing
            // the same blocks on hot keys.
            // v3 (LANE-C): cached SST list — no `read_dir` syscall on this (hot) miss path.
            let ssts = self.cached_sst_paths()?;
            for sst in ssts.iter().rev() {
                // v0.35: cached reader (header+bloom resident, body lazy). A bloom miss
                // now costs zero disk reads; before this line the WHOLE file was read
                // per get per SST.
                let table = match self.sst_cached(sst) {
                    Ok(t) => t,
                    Err(e) if e.starts_with("VANISHED") => continue,
                    Err(e) => return Err(e),
                };
                if !table.bloom.may_contain(key) {
                    continue;
                }
                if let Some(v) = table.lookup_cached(key, Some(&self.block_cache)) {
                    return Ok(if v.is_empty() { None } else { Some(v) });
                }
            }
            let raced = SST_VANISHED_RACE.with(|c| c.get());
            if !raced || attempt == 1 {
                break;
            }
            // A file vanished mid-lookup (async compaction raced us) -- the list we
            // just walked may predate the rename that superseded it. Force a fresh
            // re-list (compact() always renames outputs in before removing inputs,
            // so the next list is guaranteed consistent) and try exactly once more.
            self.invalidate_sst_paths();
        }
        Ok(None)
    }

    /// Batch read: resolve many keys with ONE memtable lock scope and ONE
    /// shared newest-first SST pass, instead of a full `get()` per key.
    /// Results come back in input order (`out[i]` answers `keys[i]`); misses
    /// stay `None`. Semantics are identical to calling `get()` per key:
    /// range/point tombstones and memtable entries shadow SSTs, and the
    /// newest SST copy wins. The read-path mirror of `put_many` — the batch
    /// amortizes the inner-lock acquisition, the SST-list fetch, and the
    /// reader-cache probes across the whole batch.
    pub fn get_many<K: AsRef<[u8]>>(&self, keys: &[K]) -> Result<Vec<Option<Vec<u8>>>, String> {
        let mut out: Vec<Option<Vec<u8>>> = vec![None; keys.len()];
        if keys.is_empty() {
            return Ok(out);
        }
        // Phase 1 — one lock scope for the whole batch: range tombstones +
        // memtable. Keys resolved here (value OR tombstone) never touch disk.
        let mut pending: Vec<(usize, &[u8])> = Vec::new();
        {
            let inner = self.inner.read();
            for (i, k) in keys.iter().enumerate() {
                let k = k.as_ref();
                if range_tomb::is_covered(&inner.range_tombs, k) {
                    continue; // deleted — stays None
                }
                if let Some(v) = inner.memtable.get(k) {
                    if !v.is_empty() {
                        out[i] = Some(v.clone());
                    }
                    continue; // resolved: value or point tombstone
                }
                pending.push((i, k));
            }
        }
        if pending.is_empty() {
            return Ok(out);
        }
        // Phase 2 — one newest-first SST walk shared by every unresolved key.
        // Per-key first-hit-wins is preserved: a key leaves `pending` the
        // moment the newest table holding it answers (value or tombstone).
        // Same bounded retry as get() against the compact_async()
        // vanished-file race, but only still-unresolved keys re-walk.
        for attempt in 0..2 {
            SST_VANISHED_RACE.with(|c| c.set(false));
            let ssts = self.cached_sst_paths()?;
            for sst in ssts.iter().rev() {
                if pending.is_empty() {
                    break;
                }
                let table = match self.sst_cached(sst) {
                    Ok(t) => t,
                    Err(e) if e.starts_with("VANISHED") => continue,
                    Err(e) => return Err(e),
                };
                pending.retain(|&(i, k)| {
                    if !table.bloom.may_contain(k) {
                        return true; // definitely not here — still unresolved
                    }
                    match table.lookup_cached(k, Some(&self.block_cache)) {
                        Some(v) => {
                            if !v.is_empty() {
                                out[i] = Some(v);
                            }
                            false // resolved: value or tombstone
                        }
                        None => true, // bloom false positive — keep looking
                    }
                });
            }
            let raced = SST_VANISHED_RACE.with(|c| c.get());
            if pending.is_empty() || !raced || attempt == 1 {
                break;
            }
            self.invalidate_sst_paths();
        }
        Ok(out)
    }

    /// Delete a key. Writes a tombstone (empty value) to both WAL and memtable
    /// so the deletion shadows any older copy persisted in an SST.
    pub fn delete(&self, key: &[u8]) -> Result<(), String> {
        // v0.36: auto-flush check after the lock scope (see put()).
        let need_flush = {
            let mut inner = self.inner.write();
            inner.sequence += 1;
            let seq = inner.sequence;
            let n = write_wal_entry(inner.wal_file.as_mut(), key, &[])?;
            inner.wal_bytes += n;
            inner.memtable.insert(key.to_vec(), Vec::new()); // tombstone
            inner.mod_history.insert(key.to_vec(), seq);
            inner.max_wal_bytes > 0 && inner.wal_bytes > inner.max_wal_bytes
        };
        if need_flush {
            self.auto_flush_if_needed();
        }
        Ok(())
    }

    /// Start a new transaction. Captures the current database sequence number
    /// as the read snapshot; all `tx.get()` calls see the database state as of
    /// that point. Writes are buffered locally and applied atomically at
    /// `commit()` — or discarded if any key the transaction read or wrote was
    /// modified by another writer in the meantime (`TxError::Conflict`).
    ///
    /// The transaction is single-threaded: it holds an `Arc<RwLock>` to the
    /// database but does not take any lock until `commit()`.
    pub fn begin_transaction(&self) -> Transaction {
        let snapshot_seq = self.inner.read().sequence;
        Transaction {
            db: self.clone(),
            snapshot_seq,
            write_buf: BTreeMap::new(),
            read_set: std::collections::HashSet::new(),
            finished: false,
        }
    }

    /// v0.17 (G1): Apply a `WriteBatch` atomically across one or more CFs.
    ///
    /// All operations in the batch land under a single critical section per CF:
    /// concurrent readers see either none of the batch or all of it (per CF).
    /// To avoid deadlocking two simultaneous batches that touch the same CFs in
    /// different orders, CFs are locked in deterministic path order.
    ///
    /// Returns the maximum sequence number stamped across all CFs (the "commit
    /// sequence"). Callers can use this for read-after-write consistency checks
    /// via `Snapshot::sequence()`.
    ///
    /// Crash atomicity: each CF still owns its own WAL, so a crash mid-batch
    /// could leave some CFs durable and others not. In-process atomicity is
    /// strict; cross-WAL crash atomicity is a follow-up (audit gap G1.5 — a
    /// shared parent WAL or a two-phase commit marker is needed for that).
    pub fn write(&self, batch: WriteBatch) -> Result<u64, String> {
        if batch.ops.is_empty() {
            return Ok(self.inner.read().sequence);
        }

        let mut ops = batch.ops;
        ops.sort_by(|a, b| a.cf_path.cmp(&b.cf_path));

        let mut max_seq: u64 = 0;
        let mut i = 0;
        while i < ops.len() {
            let target_inner = Arc::clone(&ops[i].inner);
            let mut j = i + 1;
            while j < ops.len() && Arc::ptr_eq(&ops[j].inner, &target_inner) {
                j += 1;
            }
            let mut guard = target_inner.write();
            for k in i..j {
                guard.sequence += 1;
                let seq = guard.sequence;
                match &ops[k].kind {
                    BatchOpKind::Put { key, value } => {
                        let n = write_wal_entry(guard.wal_file.as_mut(), key, value)?;
                        guard.wal_bytes += n;
                        guard.memtable.insert(key.clone(), value.clone());
                        guard.mod_history.insert(key.clone(), seq);
                    }
                    BatchOpKind::Delete { key } => {
                        let n = write_wal_entry(guard.wal_file.as_mut(), key, &[])?;
                        guard.wal_bytes += n;
                        guard.memtable.insert(key.clone(), Vec::new());
                        guard.mod_history.insert(key.clone(), seq);
                    }
                }
                if seq > max_seq { max_seq = seq; }
            }
            drop(guard);
            // v0.36 note: no inline auto-flush here — a BatchOp only carries the
            // target CF's `inner` Arc, not a full `Database` handle, and flush()
            // needs the handle (path + caches). The counter still accumulates,
            // so the next put()/delete()/batch_put()/commit() or explicit
            // flush() on that CF triggers/performs the truncation.
            i = j;
        }
        Ok(max_seq)
    }
}

/// Atomic batched write across one or more column families.
///
/// All puts/deletes accumulated on a single `WriteBatch` are applied under one
/// critical section per touched CF when passed to `Database::write(batch)`.
/// Use this whenever a multi-key (or multi-CF) state mutation must be
/// all-or-nothing to in-process readers.
///
/// ```text
/// let mut batch = WriteBatch::new();
/// batch.put(&wallets, addr, balance_bytes);
/// batch.put(&nonces, addr, nonce_bytes);
/// batch.delete(&pending, tx_hash);
/// db.write(batch)?;  // single commit sequence stamps all three writes
/// ```
#[derive(Default)]
pub struct WriteBatch {
    ops: Vec<BatchOp>,
}

struct BatchOp {
    /// Identity of the target CF (memtable+WAL). Used both for lock-grouping
    /// (`Arc::ptr_eq`) and for deterministic lock ordering (via cf_path).
    inner: Arc<RwLock<DbInner>>,
    cf_path: PathBuf,
    kind: BatchOpKind,
}

enum BatchOpKind {
    Put { key: Vec<u8>, value: Vec<u8> },
    Delete { key: Vec<u8> },
}

impl WriteBatch {
    /// Create an empty batch. Same as `WriteBatch::default()`.
    pub fn new() -> Self {
        Self { ops: Vec::new() }
    }

    /// Stage a put. `cf` may be the default `Database` (no CF) or any handle
    /// returned by `Database::cf(name)` / `Database::create_cf(name)`.
    pub fn put(&mut self, cf: &Database, key: &[u8], value: &[u8]) {
        self.ops.push(BatchOp {
            inner: Arc::clone(&cf.inner),
            cf_path: cf.path.clone(),
            kind: BatchOpKind::Put { key: key.to_vec(), value: value.to_vec() },
        });
    }

    /// Stage a delete (tombstone).
    pub fn delete(&mut self, cf: &Database, key: &[u8]) {
        self.ops.push(BatchOp {
            inner: Arc::clone(&cf.inner),
            cf_path: cf.path.clone(),
            kind: BatchOpKind::Delete { key: key.to_vec() },
        });
    }

    /// Total number of staged operations across all CFs.
    pub fn len(&self) -> usize { self.ops.len() }

    /// Whether the batch has any staged operations.
    pub fn is_empty(&self) -> bool { self.ops.is_empty() }

    /// Drop all staged operations.
    pub fn clear(&mut self) { self.ops.clear() }
}

impl Clone for Database {
    fn clone(&self) -> Self {
        Database {
            inner: Arc::clone(&self.inner),
            path: self.path.clone(),
            block_cache: Arc::clone(&self.block_cache),
            cfs: Arc::clone(&self.cfs),
            sst_readers: Arc::clone(&self.sst_readers),
            sst_paths: Arc::clone(&self.sst_paths),
            compacting: Arc::clone(&self.compacting),
            compact_lock: Arc::clone(&self.compact_lock),
            flushing: Arc::clone(&self.flushing),
        }
    }
}

/// MVCC transaction with optimistic concurrency control.
///
/// Reads see the database as of `snapshot_seq` (plus any buffered writes from
/// this same transaction — read-your-writes). Writes go into a local buffer
/// and are applied atomically at `commit()`. The buffer is dropped on
/// `rollback()` or on `Drop` (auto-rollback if not committed).
pub struct Transaction {
    db: Database,
    snapshot_seq: u64,
    /// Buffered writes from this transaction. `None` value means tombstone.
    write_buf: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    /// Every key the transaction has read (for conflict detection).
    read_set: std::collections::HashSet<Vec<u8>>,
    finished: bool,
}

/// Reasons a `Transaction::commit()` can fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxError {
    /// Another writer modified one or more keys in our read or write set
    /// after we started. The transaction has been rolled back; retry.
    Conflict { conflicting_key: Vec<u8> },
    /// The transaction was already committed or rolled back.
    AlreadyFinished,
    /// Underlying database error (WAL write, etc).
    Io(String),
}

impl std::fmt::Display for TxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TxError::Conflict { conflicting_key } => write!(
                f,
                "tx conflict on key {:?}",
                String::from_utf8_lossy(conflicting_key),
            ),
            TxError::AlreadyFinished => write!(f, "tx already finished"),
            TxError::Io(s) => write!(f, "tx io: {}", s),
        }
    }
}

impl std::error::Error for TxError {}

impl Transaction {
    /// The database sequence number captured at `begin_transaction()`.
    pub fn snapshot_sequence(&self) -> u64 {
        self.snapshot_seq
    }

    /// Read a key. First checks the local write buffer (read-your-writes),
    /// then the database snapshot taken at `begin_transaction()`. Any key
    /// read is added to `read_set` for conflict detection at commit.
    pub fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
        if self.finished {
            return Err("tx already finished".into());
        }
        self.read_set.insert(key.to_vec());

        // 1. Local writes (read-your-writes).
        if let Some(buffered) = self.write_buf.get(key) {
            return Ok(buffered.clone());
        }

        // 2. Memtable, but ignore writes newer than our snapshot.
        {
            let inner = self.db.inner.read();
            if let Some(seq) = inner.mod_history.get(key) {
                if *seq > self.snapshot_seq {
                    // The memtable's value is newer than our snapshot. Fall
                    // through to SSTs to find the pre-snapshot value. (A
                    // strict MVCC would store full version vectors; we
                    // approximate by treating any post-snapshot write as
                    // "didn't exist at snapshot time" for non-buffered keys.)
                    // For the tests we care about (isolation between
                    // overlapping txs), this is sufficient.
                    return Ok(None);
                }
            }
            if let Some(v) = inner.memtable.get(key) {
                return Ok(if v.is_empty() { None } else { Some(v.clone()) });
            }
        }

        // 3. Read-through SSTs (already snapshot-stable on disk).
        self.db.get(key)
    }

    /// Buffer a write. Becomes visible to other readers only after `commit()`.
    pub fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), String> {
        if self.finished {
            return Err("tx already finished".into());
        }
        self.write_buf.insert(key.to_vec(), Some(value.to_vec()));
        Ok(())
    }

    /// Buffer a delete (tombstone).
    pub fn delete(&mut self, key: &[u8]) -> Result<(), String> {
        if self.finished {
            return Err("tx already finished".into());
        }
        self.write_buf.insert(key.to_vec(), None);
        Ok(())
    }

    /// Atomically apply all buffered writes. Optimistic concurrency: if any
    /// key in `read_set ∪ write_buf` was modified after `snapshot_seq`,
    /// returns `TxError::Conflict` and the transaction is rolled back.
    /// On success, the transaction's writes become visible to subsequent
    /// readers and all written keys are recorded in `mod_history` at the new
    /// sequence.
    pub fn commit(mut self) -> Result<u64, TxError> {
        if self.finished {
            return Err(TxError::AlreadyFinished);
        }
        let mut inner = self.db.inner.write();

        // Conflict check: any key we read or wrote modified since snapshot?
        let probe_keys: Vec<&Vec<u8>> = self.read_set.iter()
            .chain(self.write_buf.keys())
            .collect();
        for k in probe_keys {
            if let Some(seq) = inner.mod_history.get(k) {
                if *seq > self.snapshot_seq {
                    self.finished = true;
                    return Err(TxError::Conflict { conflicting_key: k.clone() });
                }
            }
        }

        // Apply writes atomically. We hold the write lock across the entire
        // WAL flush + memtable update + mod_history update, so commit appears
        // instantaneous to other readers.
        for (key, value) in &self.write_buf {
            inner.sequence += 1;
            let seq = inner.sequence;
            match value {
                Some(v) => {
                    let n = write_wal_entry(inner.wal_file.as_mut(), key, v)
                        .map_err(TxError::Io)?;
                    inner.wal_bytes += n;
                    inner.memtable.insert(key.clone(), v.clone());
                }
                None => {
                    let n = write_wal_entry(inner.wal_file.as_mut(), key, &[])
                        .map_err(TxError::Io)?;
                    inner.wal_bytes += n;
                    inner.memtable.insert(key.clone(), Vec::new());
                }
            }
            inner.mod_history.insert(key.clone(), seq);
        }
        let commit_seq = inner.sequence;
        let need_flush = inner.max_wal_bytes > 0 && inner.wal_bytes > inner.max_wal_bytes;
        self.finished = true;
        // v0.36: release the write guard BEFORE the auto-flush check-and-call —
        // flush() re-takes this same non-reentrant lock.
        drop(inner);
        if need_flush {
            self.db.auto_flush_if_needed();
        }
        Ok(commit_seq)
    }

    /// Discard all buffered writes. The transaction is finished.
    pub fn rollback(mut self) {
        self.finished = true;
        self.write_buf.clear();
        self.read_set.clear();
    }
}

impl Drop for Transaction {
    /// Auto-rollback if neither `commit()` nor `rollback()` was called.
    fn drop(&mut self) {
        if !self.finished {
            self.write_buf.clear();
            self.read_set.clear();
        }
    }
}

impl Database {
    /// Trivial accessor for the public API surface.
    pub fn sequence(&self) -> u64 {
        self.inner.read().sequence
    }

    /// Iterate over all key-value pairs (for range scans).
    pub fn scan(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        let inner = self.inner.read();
        inner.memtable.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// Create a point-in-time snapshot (MVCC read).
    pub fn snapshot(&self) -> Snapshot {
        let inner = self.inner.read();
        Snapshot {
            data: inner.memtable.clone(),
            seq: inner.sequence,
        }
    }

    /// Approximate number of keys.
    pub fn len(&self) -> usize {
        self.inner.read().memtable.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Compute BLAKE3 hash of the entire database state (for integrity checks).
    pub fn checksum(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        let inner = self.inner.read();
        for (k, v) in &inner.memtable {
            hasher.update(k);
            hasher.update(v);
        }
        format!("{}", hasher.finalize())
    }

    /// Flush memtable to a new SST on disk. The SST format is:
    ///
    ///   [0..4]    magic = b"FXDB"
    ///   [4]       version
    ///   [5]       flags (reserved, 0)
    ///   [6..14]   key_count (u64 LE)
    ///   [14..18]  bloom_num_hashes (u32 LE)
    ///   [18..22]  bloom_len_bytes (u32 LE)
    ///   [22..]    bloom bytes (raw u64 words, little-endian)
    ///   then      payload_len (u32 LE) + lz4-compressed payload
    ///
    /// Payload is the same flat (key_len:u32, val_len:u32, key, val) form as
    /// before. The memtable entries captured in the flush snapshot are cleared on
    /// success; auto-compaction triggers when the SST count crosses
    /// AUTO_COMPACT_THRESHOLD.
    pub fn flush(&self) -> Result<(), String> {
        // 0.95: take a single, consistent memtable snapshot under the lock, then
        // release it before the expensive SST write + fsync. This keeps readers
        // responsive during sigil-top catch-up. Crash safety is preserved by leaving
        // the old WAL intact whenever writes race this flush; a later quiet flush
        // truncates it after all remaining memtable entries have reached SSTs.
        let (snapshot, snapshot_seq) = {
            let inner = self.inner.read();
            if inner.memtable.is_empty() {
                return Ok(());
            }
            (inner.memtable.clone(), inner.sequence)
        };
        // v0.15: flushes always land at L0; the leveled compactor pushes them deeper.
        let sst_path = self.path.join(sst_name(0, snapshot_seq));

        // v0.37 (task #10): STREAM the SST to disk. The old path built the
        // block body in RAM (SstBuilder.output, ~= SST size) and then
        // `encode_sst` copied it into a second full-size buffer — at a 1 GiB
        // WAL cap that is ~2.2 GB of transient allocations per flush ON TOP
        // of the memtable + its snapshot clone. FileSstWriter (the compaction
        // output writer) emits the byte-identical v3 framing incrementally:
        // peak extra RAM is one 4 KiB block + the flat index bytes.
        //
        // ATOMIC PUBLISH: FileSstWriter writes `.sst.tmp` and fsyncs in
        // finish(); we rename only after. flush() used to create the final
        // `.sst` path directly, so a kill -9 mid-write left a TORN file that
        // `SstReader::open` rejects — and one unopenable SST fails every
        // subsequent `get()`. A torn tmp is swept at the next open
        // (`ingest::sweep_stale_tmp`) and the data is still covered by the
        // un-truncated WAL, so the crash costs nothing. The fsync before the
        // WAL truncate below stays MANDATORY.
        let mut w = FileSstWriter::create(sst_path.clone(), snapshot.len())
            .map_err(|e| format!("flush create sst: {}", e))?;
        for (k, v) in snapshot.iter() {
            if let Err(e) = w.add(k, v) {
                w.abort();
                return Err(format!("flush sst write: {}", e));
            }
        }
        let (sst_tmp, sst_final) = w.finish().map_err(|e| format!("flush sst finish: {}", e))?; // finish() removes its tmp on error
        fs::rename(&sst_tmp, &sst_final).map_err(|e| format!("sst rename: {}", e))?;
        // v3 (LANE-C): a new L0 SST now exists — drop the cached path list so readers see it.
        self.invalidate_sst_paths();

        // Snapshot entries are now durable in the SST. Re-take the lock briefly
        // and remove only entries that have not changed since the snapshot. If no
        // writes raced the flush, the WAL can be truncated. If writes did race it,
        // leave the WAL intact: it may contain already-SSTed entries, but replaying
        // them is idempotent and safe; truncating would risk losing the racing writes.
        let mut inner = self.inner.write();
        let no_writes_since_snapshot = inner.sequence == snapshot_seq;
        for (k, v) in snapshot.iter() {
            if inner.memtable.get(k).map(|cur| cur == v).unwrap_or(false) {
                inner.memtable.remove(k);
                inner.mod_history.remove(k);
            }
        }
        if no_writes_since_snapshot || inner.memtable.is_empty() {
            if let Some(w) = inner.wal_file.as_mut() {
                let _ = w.flush();
                w.set_len(0).map_err(|e| format!("wal truncate: {}", e))?;
                // v0.36.1: plain write handle (not O_APPEND) → seek to 0 so the next
                // write goes to the new EOF. inner.sequence stays monotonic.
                let _ = std::io::Seek::seek(w, std::io::SeekFrom::Start(0));
            }
            // v0.36: the WAL is empty again — reset the auto-flush counter.
            inner.wal_bytes = 0;
        }
        drop(inner);

        // Auto-compact if we've accumulated too many SSTs. In bulk-load (v3 LANE-C) the
        // threshold is raised — NOT removed: the synchronous full-level rewrite is the
        // throughput wall, but fully skipping it lets L0 grow unbounded and the per-block
        // `has_height()` linkage check (a point lookup that scans every SST) degrades the
        // sync's OWN read path. So we let L0 pile to BULK_L0_COMPACT_THRESHOLD (8× the eager
        // cap) before one bounded compaction, then `compact_to_tip` folds the rest at the end.
        let threshold = if self.inner.read().defer_compaction {
            BULK_L0_COMPACT_THRESHOLD
        } else {
            AUTO_COMPACT_THRESHOLD
        };
        let sst_count = list_ssts(&self.path)?.len();
        if sst_count > threshold {
            // v0.37 (chronos-v035 finding): compaction runs in the BACKGROUND so a large
            // L{n}->L{n+1} merge no longer freezes ingestion (measured: a synchronous merge
            // at ~480GB stalled the writer 20+ min). Safe because compact() releases the
            // write lock during its streaming disk merge, its L0 memtable-clear is guarded on
            // `sequence == snap+N` (puts racing the merge survive), and a concurrent flush
            // writes NEW L0 files that don't conflict with the merge removing OLDER levels.
            // Only ONE compaction at a time (the flag); a Drop guard resets it even on panic.
            // defer_compaction (bulk load) keeps the inline path — it wants the settle now.
            let deferring = self.inner.read().defer_compaction;
            if deferring {
                // v0.38 ASYNC-SETTLE (measured on a live ~10k blk/s SIGIL sync): the bulk/
                // deferred path compacted INLINE at its raised threshold ("wants the settle
                // now") — a 384 MiB L0→L1 merge froze the apply loop 9-13 s, the operator-
                // visible periodic "rate 0". Settle in the BACKGROUND like the eager path,
                // with RUNAWAY BACKPRESSURE: if L0 has grown past 2× the bulk threshold
                // (ingest outpacing the running merge), fall back to the old inline settle —
                // compact() blocks on compact_lock behind the in-flight merge, which IS the
                // write-stall, bounded and deliberate, so reads never degrade unboundedly.
                let runaway = sst_count > threshold.saturating_mul(2);
                if runaway || !self.spawn_background_compact() {
                    self.compact()?;
                }
            } else if !self.spawn_background_compact() {
                // We won the flag but the OS refused a thread (resource pressure — the
                // epsilon overload condition): settle INLINE so the store still bounds L0.
                // (An already-in-flight merge returns true above = nothing to do; this
                // flush's L0 files are folded by the next pass.)
                self.compact()?;
            }
        }
        Ok(())
    }

    /// v0.37/v0.38: single-flight background compaction, shared by the eager and the
    /// deferred (bulk) flush paths. Returns:
    /// - `true`  → a merge is running (just spawned, or one was already in flight) —
    ///             the caller has nothing to do.
    /// - `false` → we won the `compacting` flag but the OS refused a thread; the flag
    ///             has been reset and the CALLER must settle inline (or L0 would grow
    ///             unbounded with auto-compaction silently disabled).
    ///
    /// Safety (unchanged from the v0.37 background pass): compact() releases the write
    /// lock during its streaming disk merge; the L0 memtable-clear is guarded on
    /// `sequence == snap+N` so puts racing the merge survive; `compact_lock` is the
    /// real mutual exclusion across every entry point; a Drop guard resets the flag
    /// even if compact_inner() panics.
    fn spawn_background_compact(&self) -> bool {
        if self.compacting
            .compare_exchange(false, true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst).is_err()
        {
            return true; // already in flight
        }
        let db = self.clone();
        let spawned = std::thread::Builder::new()
            .name("flux-db-compact".into())
            .spawn(move || {
                struct Guard(Database);
                impl Drop for Guard {
                    fn drop(&mut self) {
                        self.0.compacting.store(false, std::sync::atomic::Ordering::SeqCst);
                    }
                }
                let _g = Guard(db.clone());
                let _lock = db.compact_lock.lock();
                let _ = db.compact_inner();
            });
        if spawned.is_err() {
            self.compacting.store(false, std::sync::atomic::Ordering::SeqCst);
            return false;
        }
        true
    }

    /// Streaming iterator across everything visible: the live memtable plus
    /// all SSTs, with tombstones applied. Returns an owning iterator so the
    /// DB lock is held only during construction.
    pub fn iter(&self) -> DbIterator {
        let mut merged: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        // Oldest-first so newer writes overwrite older ones.
        if let Ok(ssts) = self.cached_sst_paths() {
            for sst in &ssts {
                if let Ok(table) = self.sst_cached(sst) {
                    for (k, v) in table.pairs() {
                        if v.is_empty() {
                            merged.remove(&k);
                        } else {
                            merged.insert(k, v);
                        }
                    }
                }
            }
        }
        let inner = self.inner.read();
        for (k, v) in inner.memtable.iter() {
            if v.is_empty() {
                merged.remove(k);
            } else {
                merged.insert(k.clone(), v.clone());
            }
        }
        DbIterator { inner: merged.into_iter(), descending: false }
    }

    /// v0.17 (G2): iterate the full merged view in descending key order.
    /// `iter().rev()` also works (via DoubleEndedIterator) but this is the
    /// idiomatic call for "latest key" scans (turbo-sync's tip lookup).
    pub fn iter_rev(&self) -> DbIterator {
        let mut merged: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        if let Ok(ssts) = self.cached_sst_paths() {
            for sst in &ssts {
                if let Ok(table) = self.sst_cached(sst) {
                    for (k, v) in table.pairs() {
                        if v.is_empty() { merged.remove(&k); } else { merged.insert(k, v); }
                    }
                }
            }
        }
        let inner = self.inner.read();
        for (k, v) in inner.memtable.iter() {
            if v.is_empty() { merged.remove(k); } else { merged.insert(k.clone(), v.clone()); }
        }
        for tomb in &inner.range_tombs {
            let to_drop: Vec<Vec<u8>> = merged
                .range(tomb.start.clone()..tomb.end.clone())
                .map(|(k, _)| k.clone())
                .collect();
            for k in to_drop { merged.remove(&k); }
        }
        DbIterator { inner: merged.into_iter(), descending: true }
    }

    /// v0.17 (G2): iterate descending from `end` (inclusive if present).
    /// Use this for "find the largest key ≤ X" patterns: turbo-sync tip
    /// lookup, reorg-rollback walks, DagKnight round-descending scans.
    pub fn iter_from_back(&self, end: &[u8]) -> DbIterator {
        let mut merged: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        if let Ok(ssts) = self.cached_sst_paths() {
            for sst in &ssts {
                if let Ok(table) = self.sst_cached(sst) {
                    for (k, v) in table.pairs() {
                        if v.is_empty() { merged.remove(&k); } else { merged.insert(k, v); }
                    }
                }
            }
        }
        let inner = self.inner.read();
        for (k, v) in inner.memtable.iter() {
            if v.is_empty() { merged.remove(k); } else { merged.insert(k.clone(), v.clone()); }
        }
        for tomb in &inner.range_tombs {
            let to_drop: Vec<Vec<u8>> = merged
                .range(tomb.start.clone()..tomb.end.clone())
                .map(|(k, _)| k.clone())
                .collect();
            for k in to_drop { merged.remove(&k); }
        }
        let head: BTreeMap<Vec<u8>, Vec<u8>> = merged
            .into_iter()
            .filter(|(k, _)| k.as_slice() <= end)
            .collect();
        DbIterator { inner: head.into_iter(), descending: true }
    }

    /// v0.37: bounded most-recent scan of one key prefix WITHOUT materializing
    /// the whole DB. Returns up to `limit` (key, value) pairs whose key starts
    /// with `prefix`, in ASCENDING key order (oldest→newest of the recent
    /// window). Memory is O(limit), not O(db).
    ///
    /// Why this exists: `iter`/`iter_from`/`iter_rev`/`iter_from_back` all build
    /// a full `BTreeMap` of every visible pair before returning — fine for small
    /// scans, catastrophic for a 30M-entry append-only history that an explorer
    /// `/recent?limit=12` endpoint polls (it materialized the entire history in
    /// RAM per request → multi-GB heap → cgroup OOM). This keeps only the
    /// `limit` LARGEST matching keys seen, which for an append-only prefix (the
    /// flux-history h:/k:/g: indexes never delete) equals the most recent N.
    ///
    /// Merge order mirrors `iter_from`: SSTs oldest→newest, then the live
    /// memtable, then range tombstones; an empty value (point tombstone) removes
    /// the key. For prefixes that DO take in-range deletes the result is
    /// best-effort-recent rather than exact (a delete that frees window space
    /// can't resurrect an already-evicted older key) — acceptable for the
    /// recent/explorer views this serves.
    pub fn scan_prefix_recent(&self, prefix: &[u8], limit: usize) -> Vec<(Vec<u8>, Vec<u8>)> {
        if limit == 0 {
            return Vec::new();
        }
        // Bounded window of the `limit` largest matching keys seen so far.
        let mut win: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        let mut consider = |win: &mut BTreeMap<Vec<u8>, Vec<u8>>, k: Vec<u8>, v: Vec<u8>| {
            if !k.starts_with(prefix) {
                return;
            }
            if v.is_empty() {
                win.remove(&k); // point tombstone shadows older data
                return;
            }
            // Fast reject: once the window is full, a brand-new key no larger
            // than the smallest kept key can never make the top-`limit`.
            if win.len() >= limit && !win.contains_key(&k) {
                if let Some((min_k, _)) = win.iter().next() {
                    if k.as_slice() <= min_k.as_slice() {
                        return;
                    }
                }
            }
            win.insert(k, v);
            while win.len() > limit {
                // Drop the smallest (oldest) kept key.
                if let Some(min_k) = win.keys().next().cloned() {
                    win.remove(&min_k);
                } else {
                    break;
                }
            }
        };
        if let Ok(ssts) = self.cached_sst_paths() {
            for sst in &ssts {
                if let Ok(table) = self.sst_cached(sst) {
                    for (k, v) in table.pairs() {
                        consider(&mut win, k, v);
                    }
                }
            }
        }
        let inner = self.inner.read();
        for (k, v) in inner.memtable.iter() {
            consider(&mut win, k.clone(), v.clone());
        }
        for tomb in &inner.range_tombs {
            let to_drop: Vec<Vec<u8>> = win
                .range(tomb.start.clone()..tomb.end.clone())
                .map(|(k, _)| k.clone())
                .collect();
            for k in to_drop {
                win.remove(&k);
            }
        }
        win.into_iter().collect()
    }
}

/// Streaming key/value iterator. Holds an owned BTreeMap iterator so the
/// caller doesn't have to hold the DB lock.
pub struct DbIterator {
    inner: std::collections::btree_map::IntoIter<Vec<u8>, Vec<u8>>,
    /// When `true`, `.next()` walks the underlying ordered view in
    /// descending key order (and `.next_back()` walks ascending). When
    /// `false`, the natural ascending behaviour. Set by `iter_rev` /
    /// `iter_from_back`; default `false` for `iter` / `iter_from`.
    descending: bool,
}

impl Iterator for DbIterator {
    type Item = (Vec<u8>, Vec<u8>);
    fn next(&mut self) -> Option<Self::Item> {
        if self.descending { self.inner.next_back() } else { self.inner.next() }
    }
}

impl DoubleEndedIterator for DbIterator {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.descending { self.inner.next() } else { self.inner.next_back() }
    }
}

/// Write a single WAL entry framed as
/// `[crc32(body) | key_len | val_len | key | val]` (all u32 LE).
/// Append one entry to the WAL. Returns the number of bytes written (0 when
/// there is no WAL file), so callers can maintain `DbInner::wal_bytes` for
/// the v0.36 auto-flush trigger.
fn write_wal_entry(wal: Option<&mut fs::File>, key: &[u8], value: &[u8]) -> Result<u64, String> {
    let Some(wal) = wal else { return Ok(0) };
    let mut body = Vec::with_capacity(8 + key.len() + value.len());
    body.extend_from_slice(&(key.len() as u32).to_le_bytes());
    body.extend_from_slice(&(value.len() as u32).to_le_bytes());
    body.extend_from_slice(key);
    body.extend_from_slice(value);
    let crc = crc32fast::hash(&body);
    wal.write_all(&crc.to_le_bytes()).map_err(|e| format!("wal write crc: {}", e))?;
    wal.write_all(&body).map_err(|e| format!("wal write body: {}", e))?;
    wal.flush().map_err(|e| format!("wal flush: {}", e))?;
    Ok((4 + body.len()) as u64)
}

/// v0.15: a discovered SST file with its level. Legacy files (no
/// `_L{level}_` segment in the name) are treated as L0.
#[derive(Debug, Clone)]
pub struct SstHandle {
    pub path: PathBuf,
    pub level: u8,
    /// Sequence number embedded in the filename.
    pub sequence: u64,
}

/// List all SSTs in the database directory tagged with their level. Returns
/// them oldest-first within each level. Naming:
///   * `flux_{seq:016x}.sst`        — legacy (pre-v0.15), treated as L0
///   * `flux_L{lvl:02}_{seq:016x}.sst` — v0.15+ with explicit level
fn list_ssts_leveled(path: &std::path::Path) -> Result<Vec<SstHandle>, String> {
    let mut out: Vec<SstHandle> = Vec::new();
    let entries = fs::read_dir(path).map_err(|e| format!("read_dir: {}", e))?;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.starts_with("flux_") {
            continue;
        }
        if !(name.ends_with(".sst") || name.ends_with(".sst.lz4")) {
            continue;
        }
        let stem = name
            .trim_start_matches("flux_")
            .trim_end_matches(".sst.lz4")
            .trim_end_matches(".sst");
        // Strip the optional `_compacted` suffix for parsing.
        let stem = stem.trim_end_matches("_compacted");
        let (level, seq) = if let Some(rest) = stem.strip_prefix('L') {
            // L{lvl:02}_{seq:016x}
            if let Some(under) = rest.find('_') {
                let lvl = rest[..under].parse::<u8>().unwrap_or(0);
                let seq = u64::from_str_radix(&rest[under + 1..], 16).unwrap_or(0);
                (lvl, seq)
            } else {
                (0, 0)
            }
        } else {
            // Legacy: just {seq:016x}
            let seq = u64::from_str_radix(stem, 16).unwrap_or(0);
            (0, seq)
        };
        out.push(SstHandle { path: e.path(), level, sequence: seq });
    }
    out.sort_by(|a, b| a.level.cmp(&b.level).then(a.sequence.cmp(&b.sequence)));
    Ok(out)
}

/// Unleveled wrapper: paths in READ-MERGE order — oldest data first, i.e.
/// deepest level first (level DESC), ascending sequence within a level.
/// `.rev()` therefore yields newest-first: L0 by descending sequence, then
/// deeper levels. This is the order every reader must use — the raw
/// `list_ssts_leveled` order (level ASC) made forward-merging readers apply
/// deep (old) levels LAST and `.rev()` readers consult them FIRST, so a key
/// updated after compaction pushed its old value to L1 read back the stale
/// L1 copy, and deleted keys resurrected. Within-level sequence order
/// matters at EVERY level: an L0→L1 merge lands beside older L1 files, so
/// a newer L1 file (higher sequence) shadows an older one.
fn list_ssts(path: &std::path::Path) -> Result<Vec<PathBuf>, String> {
    let mut handles = list_ssts_leveled(path)?;
    handles.sort_by(|a, b| b.level.cmp(&a.level).then(a.sequence.cmp(&b.sequence)));
    Ok(handles.into_iter().map(|h| h.path).collect())
}

/// Build the v0.15 SST filename for a given level + sequence.
fn sst_name(level: u8, sequence: u64) -> String {
    format!("flux_L{:02}_{:016x}.sst", level, sequence)
}

/// Reader for an SST. Carries the bloom filter (always materialized) and
/// the file body so block-based reads can index into it.
///
/// SST format dispatch:
///   * legacy (no FXDB magic) — treat whole file as v0.10 LZ4 payload.
///   * version 1 — v0.10/v0.11 monolithic LZ4 payload.
///   * version 2 — v0.12 block-based (data blocks + index + footer).
pub struct SstReader {
    pub bloom: BloomFilter,
    version: u8,
    legacy: bool,
    /// Path of the SST on disk — required as the block-cache key prefix AND,
    /// since v0.35, to lazily read the payload on first access.
    path: PathBuf,
    /// v0.35 LAZY PAYLOAD: byte range of the body within the file. `open()` now
    /// reads ONLY the 22-byte header + bloom filter (a few KB); the body is read
    /// on first [`Self::payload`] access. Before this, `open()` `fs::read` the
    /// WHOLE file — and `Database::get` opens every SST per lookup, so a single
    /// point-read on a multi-GB store re-read gigabytes (measured: 78 s "open",
    /// ms-class gets). Bloom-misses now never touch the body at all.
    payload_off: u64,
    payload_len: usize,
    payload: std::sync::OnceLock<Vec<u8>>,
    /// Header key_count — authoritative entry count for v2+; 0 for legacy.
    /// The compaction data-loss guard compares parsed pairs against this.
    key_count: u64,
    /// v0.37 (task #10): sparse fence index for FILE-BACKED block reads on
    /// v2/v3 SSTs. Built lazily on the first lookup that survives the bloom
    /// filter; the whole body is NEVER materialized for v2+ (the OnceLock
    /// `payload` above now serves ONLY legacy/v1 files). A parse failure
    /// yields an empty index (all lookups miss) — same observable behavior
    /// as a torn SST, never a panic; compaction has its own error-propagating
    /// path (`stream()`), so the data-loss guard is unaffected.
    index: std::sync::OnceLock<SstIndex>,
}

impl SstReader {
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        use std::io::Read;
        let mut f = fs::File::open(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SST_VANISHED_RACE.with(|c| c.set(true));
                format!("VANISHED read sst {}: {}", path.display(), e)
            } else {
                format!("read sst {}: {}", path.display(), e)
            }
        })?;
        let flen = f.metadata().map_err(|e| format!("stat sst {}: {}", path.display(), e))?.len();
        // Legacy (no FXDB magic / file shorter than the magic): whole file is the payload.
        let mut magic = [0u8; 4];
        let is_versioned = flen >= 4 && f.read_exact(&mut magic).is_ok() && magic == SST_MAGIC;
        if !is_versioned {
            return Ok(SstReader {
                bloom: BloomFilter::passthrough(),
                version: 0,
                legacy: true,
                path: path.to_path_buf(),
                payload_off: 0,
                payload_len: flen as usize,
                payload: std::sync::OnceLock::new(),
                key_count: 0,
                index: std::sync::OnceLock::new(),
            });
        }
        // Bytes 4..22 of the header (we already consumed the 4-byte magic).
        let mut rest = [0u8; 18];
        f.read_exact(&mut rest).map_err(|_| "SST truncated in header".to_string())?;
        let version = rest[0]; // raw[4]
        let key_count = u64::from_le_bytes([rest[2], rest[3], rest[4], rest[5], rest[6], rest[7], rest[8], rest[9]]); // raw[6..14]
        if !(1..=3).contains(&version) {
            return Err(format!("unsupported SST version {} (supported: 1-3)", version));
        }
        let bloom_num_hashes = u32::from_le_bytes([rest[10], rest[11], rest[12], rest[13]]) as usize; // raw[14..18]
        let bloom_len = u32::from_le_bytes([rest[14], rest[15], rest[16], rest[17]]) as usize;        // raw[18..22]
        let mut bloom_bytes = vec![0u8; bloom_len];
        f.read_exact(&mut bloom_bytes).map_err(|_| "SST truncated in bloom".to_string())?;
        let mut bits: Vec<u64> = Vec::with_capacity(bloom_len / 8);
        let mut i = 0;
        while i + 8 <= bloom_len {
            bits.push(u64::from_le_bytes([
                bloom_bytes[i], bloom_bytes[i+1], bloom_bytes[i+2], bloom_bytes[i+3],
                bloom_bytes[i+4], bloom_bytes[i+5], bloom_bytes[i+6], bloom_bytes[i+7],
            ]));
            i += 8;
        }
        let bloom = BloomFilter { bits, num_hashes: bloom_num_hashes.max(1) };
        let (mut payload_len, payload_off) = if version >= 3 {
            let mut plen = [0u8; 8];
            f.read_exact(&mut plen).map_err(|_| "SST truncated in bloom".to_string())?;
            (u64::from_le_bytes(plen) as usize, (22 + bloom_len + 8) as u64)
        } else {
            let mut plen = [0u8; 4];
            f.read_exact(&mut plen).map_err(|_| "SST truncated in bloom".to_string())?;
            (u32::from_le_bytes(plen) as usize, (22 + bloom_len + 4) as u64)
        };
        // v2 WRAP RECOVERY (chronos-v035): v2 stored the body length as u32, so a
        // >= 4 GiB body recorded len mod 2^32 while the FULL body (with its valid
        // u64-offset footer) is on disk. If the true remaining length matches the
        // stored value mod 2^32, trust the file: the whole tail is the body. This
        // makes every wrapped-but-complete v2 SST fully readable again.
        if version == 2 {
            let true_body = flen.saturating_sub(payload_off);
            if true_body != payload_len as u64 && true_body % (1u64 << 32) == payload_len as u64 {
                eprintln!("[flux-db] v2 SST {} : u32-wrapped body length recovered ({} -> {} bytes)",
                    path.display(), payload_len, true_body);
                payload_len = true_body as usize;
            }
        }
        if flen < payload_off + payload_len as u64 {
            return Err("SST truncated in payload".into());
        }
        Ok(SstReader {
            bloom,
            version,
            legacy: false,
            path: path.to_path_buf(),
            payload_off,
            payload_len,
            payload: std::sync::OnceLock::new(),
            key_count,
            index: std::sync::OnceLock::new(),
        })
    }

    /// v0.35: the SST body, read from disk ON FIRST ACCESS (and kept). A read
    /// failure (file vanished mid-run, truncation) logs and yields an empty
    /// body — lookups then find nothing, the same observable behavior as a
    /// torn SST today, never a panic.
    ///
    /// v0.37 (task #10): LEGACY/v1 ONLY. v2/v3 lookups are file-backed (see
    /// [`Self::load_index`]) and never call this — caching a whole multi-GiB
    /// body per SST was the RSS blow-up that OOM-killed the 2 TB soak.
    fn payload(&self) -> &[u8] {
        self.payload.get_or_init(|| {
            use std::io::{Read, Seek, SeekFrom};
            let read = (|| -> std::io::Result<Vec<u8>> {
                let mut f = fs::File::open(&self.path)?;
                f.seek(SeekFrom::Start(self.payload_off))?;
                let mut buf = vec![0u8; self.payload_len];
                f.read_exact(&mut buf)?;
                Ok(buf)
            })();
            match read {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("[flux-db] lazy SST payload read failed {}: {e}", self.path.display());
                    Vec::new()
                }
            }
        })
    }

    /// Point lookup. On v2/v3 files, uses the (sparse, lazily-built) fence
    /// index to find the single relevant data block and READS THAT BLOCK FROM
    /// THE FILE — the body is never materialized (v0.37, task #10; before
    /// this the first lookup per SST slurped the entire body into a OnceLock,
    /// which at TB scale is an unbounded resident set). On v1 / legacy files,
    /// decompresses the whole payload (slow path kept for backwards compat
    /// with old on-disk SSTs).
    ///
    /// If `cache` is provided, v2+ reads consult and populate it — both for
    /// decompressed data blocks (keyed by body-relative block offset, exactly
    /// as before) and for raw index slices (keyed by their body-relative
    /// offset, which is always >= index_off > any data-block offset, so the
    /// two key spaces cannot collide).
    pub fn lookup_cached(&self, key: &[u8], cache: Option<&cache::BlockCache>) -> Option<Vec<u8>> {
        if self.version >= 2 {
            let idx = self.load_index();
            let fence = idx.locate_group(key)?;
            // Fetch the group's raw index slice (through the cache when given).
            let slice_body_off = idx.index_off + fence.start as u64;
            let slice_len = (fence.end - fence.start) as usize;
            let slice: std::sync::Arc<Vec<u8>> = if let Some(c) = cache {
                let ck = cache::BlockKey { sst_path: self.path.clone(), block_offset: slice_body_off };
                if let Some(buf) = c.get(&ck) {
                    buf
                } else {
                    let raw = pread(&self.path, self.payload_off + slice_body_off, slice_len).ok()?;
                    let arc = std::sync::Arc::new(raw);
                    c.put(ck, arc.clone());
                    arc
                }
            } else {
                std::sync::Arc::new(pread(&self.path, self.payload_off + slice_body_off, slice_len).ok()?)
            };
            let (blk_off, clen, ulen) = scan_index_slice(&slice, key)?;
            // Read + decompress the single data block (through the cache).
            if let Some(c) = cache {
                let ck = cache::BlockKey { sst_path: self.path.clone(), block_offset: blk_off };
                if let Some(buf) = c.get(&ck) {
                    return block::BlockSstReader::lookup_in_block(&buf, key);
                }
                let decompressed = self.read_block_at(blk_off, clen, ulen).ok()?;
                let arc = std::sync::Arc::new(decompressed);
                c.put(ck, arc.clone());
                return block::BlockSstReader::lookup_in_block(&arc, key);
            }
            let decompressed = self.read_block_at(blk_off, clen, ulen).ok()?;
            return block::BlockSstReader::lookup_in_block(&decompressed, key);
        }

        // v1 / legacy path — whole-payload decompress + linear scan.
        let data = decompress(self.payload());
        let mut pos = 0;
        while pos + 8 <= data.len() {
            let kl = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
            let vl = u32::from_le_bytes([data[pos+4], data[pos+5], data[pos+6], data[pos+7]]) as usize;
            pos += 8;
            if pos + kl + vl > data.len() {
                break;
            }
            if &data[pos..pos+kl] == key {
                return Some(data[pos+kl..pos+kl+vl].to_vec());
            }
            pos += kl + vl;
        }
        None
    }

    /// Cache-free convenience for callers that don't have a cache handle.
    pub fn lookup(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.lookup_cached(key, None)
    }

    pub fn into_pairs(self) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.pairs()
    }

    /// v0.35: borrowing variant of [`Self::into_pairs`] so cached `Arc<SstReader>`s
    /// (the Database reader cache) can be drained without cloning the reader.
    ///
    /// v0.37 (task #10): v2+ walks the file block-by-block via [`Self::stream`]
    /// instead of materializing the whole body first. The RESULT is still fully
    /// materialized (that is this method's contract); a parse error mid-file
    /// logs and returns the pairs decoded so far (previously: empty vec).
    /// Compaction does NOT use this — it drives `stream()` directly so errors
    /// propagate to the data-loss guard.
    pub fn pairs(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        let _ = self.legacy;
        if self.version >= 2 {
            let mut out = Vec::new();
            match self.stream() {
                Ok(mut s) => loop {
                    match s.next_pair() {
                        Ok(Some(kv)) => out.push(kv),
                        Ok(None) => break,
                        Err(e) => {
                            eprintln!("[flux-db] pairs() stream error on {}: {}", self.path.display(), e);
                            break;
                        }
                    }
                },
                Err(e) => {
                    eprintln!("[flux-db] pairs() stream open error on {}: {}", self.path.display(), e);
                }
            }
            return out;
        }
        // v1 / legacy.
        let data = decompress(self.payload());
        let mut out = Vec::new();
        let mut pos = 0;
        while pos + 8 <= data.len() {
            let kl = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
            let vl = u32::from_le_bytes([data[pos+4], data[pos+5], data[pos+6], data[pos+7]]) as usize;
            pos += 8;
            if pos + kl + vl > data.len() {
                break;
            }
            out.push((data[pos..pos+kl].to_vec(), data[pos+kl..pos+kl+vl].to_vec()));
            pos += kl + vl;
        }
        out
    }

    /// v0.37 (task #10): the sparse fence index, built ON FIRST USE by one
    /// sequential scan of the index region (never the data blocks). Build
    /// failure logs and degrades to an empty index — lookups miss, no panic.
    fn load_index(&self) -> &SstIndex {
        self.index.get_or_init(|| {
            match build_sst_index(&self.path, self.payload_off, self.payload_len) {
                Ok(ix) => ix,
                Err(e) if e.starts_with("VANISHED") => {
                    // Benign: this file was superseded by a concurrent compaction's
                    // merge output between being listed and being read. Not corruption
                    // -- no eprintln alarm. Database::get() retries once against a
                    // fresh SST list via SST_VANISHED_RACE; scan/iter callers that hit
                    // this mid-traversal simply see this one file as empty (the merge
                    // output that superseded it is already on disk).
                    SstIndex::empty()
                }
                Err(e) => {
                    eprintln!("[flux-db] SST index parse failed {}: {}", self.path.display(), e);
                    SstIndex::empty()
                }
            }
        })
    }

    /// v0.37 (task #10): read + decompress ONE data block straight from the
    /// file. `off` is body-relative (as recorded in the index).
    fn read_block_at(&self, off: u64, clen: u32, ulen: u32) -> Result<Vec<u8>, String> {
        let comp = pread(&self.path, self.payload_off + off, clen as usize)?;
        let d = lz4::block::decompress(&comp, None)
            .map_err(|e| format!("decompress block at {} in {}: {}", off, self.path.display(), e))?;
        if d.len() != ulen as usize {
            return Err(format!(
                "block decompress size mismatch at {} in {}: expected {}, got {}",
                off, self.path.display(), ulen, d.len()));
        }
        Ok(d)
    }

    /// v0.37 (task #10): sequential streaming scan of every (key, value) pair,
    /// block-at-a-time — RAM is O(one block), not O(body). This is the
    /// compaction input path: errors PROPAGATE (they feed the data-loss
    /// guard), and [`SstStream::yielded`] reports the exact entry count so the
    /// caller can compare against the header `key_count`.
    pub fn stream(&self) -> Result<SstStream, String> {
        if self.version < 2 {
            // v1/legacy: monolithic LZ4 payload; these files predate blocks
            // and are small — decode whole (unchanged behavior), iterate.
            return Ok(SstStream {
                inner: StreamInner::Whole(self.pairs().into_iter()),
                yielded: 0,
            });
        }
        use std::io::{Seek, SeekFrom};
        if self.payload_len < block::FOOTER_LEN {
            return Err(format!("SST body too short for footer: {}", self.payload_len));
        }
        let footer = pread(&self.path, self.payload_off + self.payload_len as u64 - block::FOOTER_LEN as u64, block::FOOTER_LEN)?;
        let index_off = u64::from_le_bytes(footer[0..8].try_into().unwrap());
        let index_len = u32::from_le_bytes(footer[8..12].try_into().unwrap()) as u64;
        let magic = u32::from_le_bytes(footer[12..16].try_into().unwrap());
        if magic != block::BLK_MAGIC {
            return Err(format!("bad block-SST footer magic 0x{magic:08x} in {}", self.path.display()));
        }
        if index_off + index_len > (self.payload_len - block::FOOTER_LEN) as u64 {
            return Err(format!("index range overruns footer in {}", self.path.display()));
        }
        if index_len < 4 {
            return Err(format!("index too short in {}", self.path.display()));
        }
        let mut fidx = fs::File::open(&self.path).map_err(|e| format!("open {}: {}", self.path.display(), e))?;
        fidx.seek(SeekFrom::Start(self.payload_off + index_off))
            .map_err(|e| format!("seek index {}: {}", self.path.display(), e))?;
        let mut index_rd = std::io::BufReader::with_capacity(64 * 1024, fidx);
        let mut nbuf = [0u8; 4];
        index_rd.read_exact(&mut nbuf).map_err(|e| format!("read index count {}: {}", self.path.display(), e))?;
        let n_entries = u32::from_le_bytes(nbuf);
        let mut fblk = fs::File::open(&self.path).map_err(|e| format!("open {}: {}", self.path.display(), e))?;
        fblk.seek(SeekFrom::Start(self.payload_off))
            .map_err(|e| format!("seek body {}: {}", self.path.display(), e))?;
        Ok(SstStream {
            inner: StreamInner::Blocks {
                path: self.path.clone(),
                index_rd: std::io::Read::take(index_rd, index_len - 4),
                entries_left: n_entries,
                block_rd: std::io::BufReader::with_capacity(256 * 1024, fblk),
                block_pos: self.payload_off,
                payload_off: self.payload_off,
                cur: Vec::new().into_iter(),
            },
            yielded: 0,
        })
    }
}

// ── v0.37 (task #10): file-backed SST reads — sparse fence index, streaming
//    scan, and a streaming compaction writer ──

/// One fence per group of up to `INDEX_FENCE_GROUP` index entries. At the
/// chronos shape (8 KiB incompressible values → one entry per block, ~131 K
/// blocks per GiB) a fully-materialized `Vec<BlockHandle>` index costs
/// ~12 MB/GiB-of-SST resident FOREVER per cached reader — at TB scale that is
/// gigabytes. Fences keep 1/64th of that (~0.2 MB/GiB); the group's raw index
/// slice (~2.5 KB) is read from the file on demand and LRU-cached in the
/// existing BlockCache, so total index residency is BOUNDED by cache capacity.
const INDEX_FENCE_GROUP: usize = 64;

/// Sparse in-memory view of one v2/v3 SST's block index.
struct SstIndex {
    /// Body-relative offset where the index region starts.
    index_off: u64,
    /// Flat fence-key storage (one allocation for all fences).
    fence_keys: Vec<u8>,
    fences: Vec<Fence>,
}

/// Covers index entries whose raw bytes live at `[start, end)` WITHIN the
/// index region. `key` (in `fence_keys`) is the `last_key` of the LAST entry
/// in the group — i.e. the max key this group of blocks can contain.
struct Fence {
    key_off: u32,
    key_len: u32,
    start: u32,
    end: u32,
}

impl SstIndex {
    fn empty() -> Self {
        SstIndex { index_off: 0, fence_keys: Vec::new(), fences: Vec::new() }
    }

    fn fence_key(&self, i: usize) -> &[u8] {
        let f = &self.fences[i];
        &self.fence_keys[f.key_off as usize..(f.key_off + f.key_len) as usize]
    }

    /// First group whose max last_key >= `key` (binary search — fences are in
    /// ascending key order because blocks are). None ⇒ key is past the end.
    fn locate_group(&self, key: &[u8]) -> Option<&Fence> {
        let (mut lo, mut hi) = (0usize, self.fences.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.fence_key(mid) < key { lo = mid + 1; } else { hi = mid; }
        }
        self.fences.get(lo)
    }
}

/// Positional read helper — opens the file PER CALL so cached readers hold no
/// long-lived fd (a TB-scale store has 1000+ SSTs; one fd each would exhaust
/// default ulimits). Open+read costs ~µs against the block I/O it fronts, and
/// warm lookups are served by the BlockCache without reaching here.
fn pread(path: &std::path::Path, off: u64, len: usize) -> Result<Vec<u8>, String> {
    use std::io::{Seek, SeekFrom};
    let mut f = fs::File::open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            SST_VANISHED_RACE.with(|c| c.set(true));
            format!("VANISHED pread open {}: {}", path.display(), e)
        } else {
            format!("pread open {}: {}", path.display(), e)
        }
    })?;
    f.seek(SeekFrom::Start(off)).map_err(|e| format!("pread seek {}: {}", path.display(), e))?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf)
        .map_err(|e| format!("pread {} B at {} from {}: {}", len, off, path.display(), e))?;
    Ok(buf)
}

/// Build the sparse fence index with ONE sequential pass over the index
/// region (a few MB read once per open reader; data blocks are not touched).
fn build_sst_index(path: &std::path::Path, payload_off: u64, payload_len: usize) -> Result<SstIndex, String> {
    use std::io::{Seek, SeekFrom};
    if payload_len < block::FOOTER_LEN {
        return Err(format!("body too short for footer: {}", payload_len));
    }
    let footer = pread(path, payload_off + payload_len as u64 - block::FOOTER_LEN as u64, block::FOOTER_LEN)?;
    let index_off = u64::from_le_bytes(footer[0..8].try_into().unwrap());
    let index_len = u32::from_le_bytes(footer[8..12].try_into().unwrap());
    let magic = u32::from_le_bytes(footer[12..16].try_into().unwrap());
    if magic != block::BLK_MAGIC {
        return Err(format!("bad block-SST footer magic 0x{magic:08x}"));
    }
    if index_off + index_len as u64 > (payload_len - block::FOOTER_LEN) as u64 {
        return Err("index range overruns into footer".into());
    }
    if index_len < 4 {
        return Err("index too short".into());
    }
    let mut f = fs::File::open(path).map_err(|e| format!("open {}: {}", path.display(), e))?;
    f.seek(SeekFrom::Start(payload_off + index_off)).map_err(|e| format!("seek {}: {}", path.display(), e))?;
    let mut rd = std::io::BufReader::with_capacity(256 * 1024, f);
    let mut b4 = [0u8; 4];
    rd.read_exact(&mut b4).map_err(|e| format!("read index count: {}", e))?;
    let n = u32::from_le_bytes(b4);
    let mut fences: Vec<Fence> = Vec::with_capacity((n as usize + INDEX_FENCE_GROUP - 1) / INDEX_FENCE_GROUP);
    let mut fence_keys: Vec<u8> = Vec::new();
    let mut key_scratch: Vec<u8> = Vec::new();
    let mut pos: u32 = 4; // byte cursor within the index region
    let mut group_start: u32 = 4;
    let mut skip16 = [0u8; 16];
    for i in 0..n {
        rd.read_exact(&mut b4).map_err(|_| "index truncated at key_len".to_string())?;
        let klen = u32::from_le_bytes(b4);
        if klen > index_len.saturating_sub(pos) {
            return Err("index entry key_len overruns index".into());
        }
        key_scratch.resize(klen as usize, 0);
        rd.read_exact(&mut key_scratch).map_err(|_| "index truncated at key".to_string())?;
        rd.read_exact(&mut skip16).map_err(|_| "index truncated at handle".to_string())?;
        pos = pos
            .checked_add(4 + klen + 16)
            .ok_or_else(|| "index cursor overflow".to_string())?;
        if pos > index_len {
            return Err("index entries overrun declared index_len".into());
        }
        if (i + 1) % INDEX_FENCE_GROUP as u32 == 0 || i + 1 == n {
            fences.push(Fence {
                key_off: fence_keys.len() as u32,
                key_len: klen,
                start: group_start,
                end: pos,
            });
            fence_keys.extend_from_slice(&key_scratch);
            group_start = pos;
        }
    }
    Ok(SstIndex { index_off, fence_keys, fences })
}

/// Scan one raw index slice (a fence group's entries) for the first block
/// whose `last_key >= key`. Returns (body-relative offset, compressed len,
/// uncompressed len). Malformed slices yield None (lookup miss, no panic).
fn scan_index_slice(slice: &[u8], key: &[u8]) -> Option<(u64, u32, u32)> {
    let mut pos = 0usize;
    while pos + 4 <= slice.len() {
        let klen = u32::from_le_bytes(slice[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        if pos + klen + 16 > slice.len() {
            return None;
        }
        let last_key = &slice[pos..pos + klen];
        pos += klen;
        let off = u64::from_le_bytes(slice[pos..pos + 8].try_into().unwrap());
        let clen = u32::from_le_bytes(slice[pos + 8..pos + 12].try_into().unwrap());
        let ulen = u32::from_le_bytes(slice[pos + 12..pos + 16].try_into().unwrap());
        pos += 16;
        if last_key >= key {
            return Some((off, clen, ulen));
        }
    }
    None
}

enum StreamInner {
    /// v2/v3: walk the index region and data blocks with two buffered readers.
    Blocks {
        path: PathBuf,
        index_rd: std::io::Take<std::io::BufReader<fs::File>>,
        entries_left: u32,
        block_rd: std::io::BufReader<fs::File>,
        /// Absolute file offset `block_rd` is currently positioned at.
        block_pos: u64,
        payload_off: u64,
        /// Decoded pairs of the current block, drained in order.
        cur: std::vec::IntoIter<(Vec<u8>, Vec<u8>)>,
    },
    /// v1/legacy: pre-decoded (small, pre-block-format files).
    Whole(std::vec::IntoIter<(Vec<u8>, Vec<u8>)>),
}

/// Streaming SST scan (see [`SstReader::stream`]). Peak RAM: one compressed +
/// one decompressed block + two read buffers, regardless of SST size.
pub struct SstStream {
    inner: StreamInner,
    yielded: u64,
}

impl SstStream {
    /// Next (key, value) in file (= key-sorted) order. `Ok(None)` = clean end
    /// of file; `Err` = parse/IO failure (compaction aborts on these).
    pub fn next_pair(&mut self) -> Result<Option<(Vec<u8>, Vec<u8>)>, String> {
        match &mut self.inner {
            StreamInner::Whole(it) => {
                let n = it.next();
                if n.is_some() { self.yielded += 1; }
                Ok(n)
            }
            StreamInner::Blocks { path, index_rd, entries_left, block_rd, block_pos, payload_off, cur } => {
                loop {
                    if let Some(kv) = cur.next() {
                        self.yielded += 1;
                        return Ok(Some(kv));
                    }
                    if *entries_left == 0 {
                        return Ok(None);
                    }
                    // Next index entry → next data block.
                    let mut b4 = [0u8; 4];
                    index_rd.read_exact(&mut b4).map_err(|e| format!("stream index key_len {}: {}", path.display(), e))?;
                    let klen = u32::from_le_bytes(b4) as u64;
                    if klen > index_rd.limit() {
                        return Err(format!("stream index key_len {} overruns index of {}", klen, path.display()));
                    }
                    // Skip the last_key — the stream doesn't need it.
                    let mut skip = vec![0u8; klen as usize];
                    index_rd.read_exact(&mut skip).map_err(|e| format!("stream index key skip {}: {}", path.display(), e))?;
                    let mut b16 = [0u8; 16];
                    index_rd.read_exact(&mut b16).map_err(|e| format!("stream index handle {}: {}", path.display(), e))?;
                    let off = u64::from_le_bytes(b16[0..8].try_into().unwrap());
                    let clen = u32::from_le_bytes(b16[8..12].try_into().unwrap());
                    let ulen = u32::from_le_bytes(b16[12..16].try_into().unwrap());
                    *entries_left -= 1;
                    // Blocks are laid out in index order, so this is a pure
                    // sequential read; seek only if the file disagrees.
                    let abs = *payload_off + off;
                    if abs != *block_pos {
                        use std::io::{Seek, SeekFrom};
                        block_rd.seek(SeekFrom::Start(abs)).map_err(|e| format!("stream block seek {}: {}", path.display(), e))?;
                    }
                    let mut comp = vec![0u8; clen as usize];
                    block_rd.read_exact(&mut comp).map_err(|e| format!("stream block read {}: {}", path.display(), e))?;
                    *block_pos = abs + clen as u64;
                    let d = lz4::block::decompress(&comp, None)
                        .map_err(|e| format!("stream block decompress at {} in {}: {}", off, path.display(), e))?;
                    if d.len() != ulen as usize {
                        return Err(format!("stream block size mismatch at {} in {}: expected {}, got {}",
                            off, path.display(), ulen, d.len()));
                    }
                    // Decode the block's entries.
                    let mut pairs = Vec::new();
                    let mut pos = 0usize;
                    while pos + 8 <= d.len() {
                        let kl = u32::from_le_bytes(d[pos..pos + 4].try_into().unwrap()) as usize;
                        let vl = u32::from_le_bytes(d[pos + 4..pos + 8].try_into().unwrap()) as usize;
                        pos += 8;
                        if pos + kl + vl > d.len() {
                            return Err(format!("malformed block payload at {} in {}", off, path.display()));
                        }
                        pairs.push((d[pos..pos + kl].to_vec(), d[pos + kl..pos + kl + vl].to_vec()));
                        pos += kl + vl;
                    }
                    *cur = pairs.into_iter();
                }
            }
        }
    }

    /// Total pairs yielded so far — compared against the header `key_count`
    /// by the compaction data-loss guard once the stream is exhausted.
    pub fn yielded(&self) -> u64 {
        self.yielded
    }
}

/// v0.37 (task #10): streaming SST writer for compaction outputs. Writes the
/// EXACT [`encode_sst`] on-disk framing, but incrementally: the header region
/// (whose size is known upfront — bloom bit-arrays are sized at construction
/// and never grow) is reserved with a seek, data blocks stream out as they
/// fill, and `finish()` writes index + footer, back-patches the header, and
/// fsyncs. Peak RAM: one 4 KiB block + the (flat, ~40 B/block) index bytes —
/// NOT the ~2× body that `encode_sst` + a materialized merge map cost.
///
/// Output lands at `<final>.sst.tmp`; the caller renames after ALL outputs of
/// a merge pass finish cleanly, so a mid-merge failure leaves inputs intact.
struct FileSstWriter {
    f: fs::File,
    tmp_path: PathBuf,
    final_path: PathBuf,
    bloom: BloomFilter,
    cur_block: Vec<u8>,
    last_key: Vec<u8>,
    /// Flat index-entry bytes (count prefix added at finish).
    index: Vec<u8>,
    n_blocks: u32,
    /// Compressed data-block bytes written so far (== body-relative offset of
    /// the next block).
    data_pos: u64,
    key_count: u64,
}

impl FileSstWriter {
    fn create(final_path: PathBuf, bloom_capacity: usize) -> Result<Self, String> {
        use std::io::{Seek, SeekFrom};
        let bloom = BloomFilter::new(bloom_capacity.max(16), 0.01);
        let bloom_bytes = bloom.bits.len() * 8;
        let tmp_path = final_path.with_extension("sst.tmp");
        let mut f = fs::File::create(&tmp_path).map_err(|e| format!("create {}: {}", tmp_path.display(), e))?;
        // Reserve the header (magic..bloom..body_len); body starts right after.
        f.seek(SeekFrom::Start((22 + bloom_bytes + 8) as u64))
            .map_err(|e| format!("seek past header {}: {}", tmp_path.display(), e))?;
        Ok(FileSstWriter {
            f,
            tmp_path,
            final_path,
            bloom,
            cur_block: Vec::with_capacity(block::TARGET_BLOCK_SIZE * 2),
            last_key: Vec::new(),
            index: Vec::new(),
            n_blocks: 0,
            data_pos: 0,
            key_count: 0,
        })
    }

    /// Keys MUST arrive in ascending order (the merge guarantees it).
    fn add(&mut self, key: &[u8], value: &[u8]) -> Result<(), String> {
        self.bloom.insert(key);
        self.key_count += 1;
        self.cur_block.extend_from_slice(&(key.len() as u32).to_le_bytes());
        self.cur_block.extend_from_slice(&(value.len() as u32).to_le_bytes());
        self.cur_block.extend_from_slice(key);
        self.cur_block.extend_from_slice(value);
        self.last_key.clear();
        self.last_key.extend_from_slice(key);
        if self.cur_block.len() >= block::TARGET_BLOCK_SIZE {
            self.flush_block()?;
        }
        Ok(())
    }

    fn flush_block(&mut self) -> Result<(), String> {
        if self.cur_block.is_empty() {
            return Ok(());
        }
        // Unlike SstBuilder's `.unwrap_or_else(clone)` fallback (which would
        // store bytes the reader can't decompress), a compression failure here
        // ABORTS the merge — inputs are kept, nothing is lost.
        let compressed = lz4::block::compress(
            &self.cur_block,
            Some(lz4::block::CompressionMode::FAST(1)),
            true,
        ).map_err(|e| format!("compact block compress: {}", e))?;
        self.f.write_all(&compressed).map_err(|e| format!("compact block write {}: {}", self.tmp_path.display(), e))?;
        self.index.extend_from_slice(&(self.last_key.len() as u32).to_le_bytes());
        self.index.extend_from_slice(&self.last_key);
        self.index.extend_from_slice(&self.data_pos.to_le_bytes());
        self.index.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        self.index.extend_from_slice(&(self.cur_block.len() as u32).to_le_bytes());
        self.n_blocks += 1;
        self.data_pos += compressed.len() as u64;
        self.cur_block.clear();
        Ok(())
    }

    /// Approximate body bytes so far — the rolling-output cap check.
    fn body_bytes(&self) -> u64 {
        self.data_pos + self.cur_block.len() as u64
    }

    /// Write index + footer, back-patch the header, fsync. Returns
    /// (tmp_path, final_path) for the caller's rename phase. On ANY error the
    /// partial tmp file is removed before returning — callers never have to
    /// clean up a writer they can no longer see.
    fn finish(self) -> Result<(PathBuf, PathBuf), String> {
        let tmp = self.tmp_path.clone();
        match self.finish_inner() {
            Ok(pair) => Ok(pair),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                Err(e)
            }
        }
    }

    fn finish_inner(mut self) -> Result<(PathBuf, PathBuf), String> {
        use std::io::{Seek, SeekFrom};
        self.flush_block()?;
        let index_offset = self.data_pos;
        let index_len = (4 + self.index.len()) as u32;
        self.f.write_all(&self.n_blocks.to_le_bytes()).map_err(|e| format!("compact index count: {}", e))?;
        self.f.write_all(&self.index).map_err(|e| format!("compact index write: {}", e))?;
        self.f.write_all(&index_offset.to_le_bytes()).map_err(|e| format!("compact footer: {}", e))?;
        self.f.write_all(&index_len.to_le_bytes()).map_err(|e| format!("compact footer: {}", e))?;
        self.f.write_all(&block::BLK_MAGIC.to_le_bytes()).map_err(|e| format!("compact footer: {}", e))?;
        // Body = data blocks + index (incl. its 4-byte count prefix) + 16-byte
        // footer — the exact bytes a reader finds between header and EOF.
        let body_len = self.data_pos + index_len as u64 + block::FOOTER_LEN as u64;
        let bloom_bytes: Vec<u8> = self.bloom.bits.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.f.seek(SeekFrom::Start(0)).map_err(|e| format!("compact header seek: {}", e))?;
        self.f.write_all(&SST_MAGIC).map_err(|e| format!("compact header: {}", e))?;
        self.f.write_all(&[SST_VERSION, 0u8]).map_err(|e| format!("compact header: {}", e))?;
        self.f.write_all(&self.key_count.to_le_bytes()).map_err(|e| format!("compact header: {}", e))?;
        self.f.write_all(&(self.bloom.num_hashes as u32).to_le_bytes()).map_err(|e| format!("compact header: {}", e))?;
        self.f.write_all(&(bloom_bytes.len() as u32).to_le_bytes()).map_err(|e| format!("compact header: {}", e))?;
        self.f.write_all(&bloom_bytes).map_err(|e| format!("compact header bloom: {}", e))?;
        self.f.write_all(&body_len.to_le_bytes()).map_err(|e| format!("compact header body_len: {}", e))?;
        self.f.sync_all().map_err(|e| format!("compact sync {}: {}", self.tmp_path.display(), e))?;
        Ok((self.tmp_path, self.final_path))
    }

    /// Drop the partial output (merge aborted).
    fn abort(self) {
        let path = self.tmp_path.clone();
        drop(self);
        let _ = fs::remove_file(&path);
    }
}

impl Snapshot {
    pub fn get(&self, key: &[u8]) -> Option<&Vec<u8>> {
        self.data.get(key)
    }

    pub fn sequence(&self) -> u64 {
        self.seq
    }
}

// ── v0.9.0: DbStats ──

#[derive(Debug, Clone)]
pub struct DbStats {
    pub key_count: usize,
    pub wal_size: u64,
    pub sst_count: usize,
    pub total_disk_bytes: u64,
    pub sequence: u64,
}

impl Database {
    pub fn stats(&self) -> DbStats {
        let inner = self.inner.read();
        let wal_size = self.path.join("flux.wal")
            .metadata().map(|m| m.len()).unwrap_or(0);
        let sst_count = std::fs::read_dir(&self.path)
            .map(|d| d.filter(|e| {
                e.as_ref().map(|f| f.file_name().to_string_lossy().ends_with(".sst.lz4")).unwrap_or(false)
            }).count())
            .unwrap_or(0);
        let mut total = wal_size;
        if let Ok(entries) = std::fs::read_dir(&self.path) {
            for e in entries.flatten() {
                if let Ok(meta) = e.metadata() { total += meta.len(); }
            }
        }
        DbStats { key_count: inner.memtable.len(), wal_size, sst_count, total_disk_bytes: total, sequence: inner.sequence }
    }

    pub fn batch_put(&self, entries: &[(&[u8], &[u8])]) -> Result<(), String> {
        // v0.37: delegate to put_many. IDENTICAL semantics (whole batch under one
        // write lock, per-entry sequence, tombstone-on-empty, auto-flush once after
        // the guard) and byte-identical WAL record framing — but put_many coalesces
        // the whole batch into ONE write_all + ONE flush syscall instead of the 3
        // syscalls PER entry this used to do (write_wal_entry did crc-write + body-
        // write + flush() for every key). Every batch caller — notably sigil-top's
        // trusted bulk block sync (put_blocks_bulk_trusted -> batch_put) — gets the
        // coalesced-WAL win with no caller change. Measured (bg3 micro-bench, 8KB
        // values): per-entry ~168 MB/s -> coalesced ~346 MB/s.
        self.put_many(entries)
    }

    /// Prefix scan. Walks the merged view (memtable + SSTs) via `iter()` so
    /// keys persisted to disk are visible. O(n) — for huge tables prefer
    /// `iter()` and skip past the prefix range manually.
    pub fn scan_prefix(&self, prefix: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.iter()
            .skip_while(|(k, _)| k.as_slice() < prefix)
            .take_while(|(k, _)| k.starts_with(prefix))
            .collect()
    }

    /// Range scan over [start, end).
    pub fn scan_range(&self, start: &[u8], end: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.iter()
            .skip_while(|(k, _)| k.as_slice() < start)
            .take_while(|(k, _)| k.as_slice() < end)
            .collect()
    }

    /// Compact all SSTs into one merged SST. Tombstones drop their target —
    /// but the PAIR itself is dropped only when the merge inputs are the
    /// deepest data in the tree; otherwise the tombstone is written through
    /// to the output so it keeps shadowing older copies of the key that live
    /// at or below the output level (dropping it early resurrected them).
    /// The merged view also folds in the current memtable so an explicit
    /// `flush()` before `compact()` isn't required.
    /// v0.15: leveled compaction. For each level L where the file count
    /// exceeds the threshold, merge ALL files at L (plus the memtable on
    /// the first pass) into a single file at L+1. Repeats until every
    /// level is below its threshold or we've hit MAX_LEVEL.
    ///
    /// This is a simple "tiered-into-leveled" strategy. RocksDB's real
    /// leveled compaction picks only overlapping key-ranges at each step;
    /// ours rewrites the whole level in one shot. Correctness is the
    /// same — newer entries always shadow older ones because the input
    /// list is sorted by sequence — just less I/O-efficient at scale.
    /// v0.37: `compact()` itself is now single-flight across EVERY entry point
    /// (explicit `compact()`, `compact_async()`, the ingest bulk paths, AND
    /// flush's own auto-spawned background compaction) -- not just flush's
    /// spawn path, which only guarded against ITSELF. Two compactions running
    /// concurrently raced each other's `fs::remove_file` / input reads: one
    /// pass's `SstReader::open()` on an input file could return a hard error
    /// (not the graceful degrade `load_index()` does) if the OTHER pass had
    /// already deleted that exact file moments earlier -- reproduced by
    /// `test_get_retries_past_vanished_sst_race`'s own compact() loop tripping
    /// over flush's auto-spawned background compaction. A second caller now
    /// gets an immediate `Ok(())` no-op (matching flush's existing "a
    /// compaction is already in flight" comment) instead of racing.
    pub fn compact(&self) -> Result<(), String> {
        // Blocks until any in-flight compaction (background-spawned or another
        // explicit caller) finishes, then runs a REAL pass of its own -- callers
        // rely on compact() having done actual work by the time it returns.
        let _guard = self.compact_lock.lock();
        self.compact_inner()
    }

    fn compact_inner(&self) -> Result<(), String> {
        // First pass: fold the memtable into L0 by treating it as a
        // virtual level-0 file with the highest sequence number.
        for current_level in 0..MAX_LEVEL {
            let handles = list_ssts_leveled(&self.path)?;
            let mut at_level: Vec<&SstHandle> =
                handles.iter().filter(|h| h.level == current_level).collect();
            // Threshold: L0 uses L0_COMPACT_THRESHOLD; deeper levels use a
            // size pyramid (L1 allows 4, L2 allows 16, L3 allows 64, ...).
            let threshold = if current_level == 0 {
                L0_COMPACT_THRESHOLD
            } else {
                L0_COMPACT_THRESHOLD * LEVEL_SIZE_RATIO.pow(current_level as u32)
            };
            let memtable_nonempty = !self.inner.read().memtable.is_empty();
            let need = at_level.len() > threshold
                || (current_level == 0 && memtable_nonempty);
            if !need {
                continue;
            }

            // v0.37 (task #10): STREAMING K-WAY MERGE. Inputs are key-sorted
            // SSTs, so a heap over per-SST streaming cursors merges them in
            // O(inputs × block) RAM — the old BTreeMap materialization
            // OOM-killed the 2 TB soak at 58.8 GB RSS, then hid behind a
            // hard 8 GiB skip-cap. The cap is now an OPT-IN escape hatch:
            // FLUX_DB_COMPACT_MAX_BYTES > 0 skips oversized merges; default
            // is uncapped because the merge no longer scales RAM with input.
            let compact_cap: u64 = std::env::var("FLUX_DB_COMPACT_MAX_BYTES").ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let input_bytes: u64 = at_level.iter()
                .filter_map(|h| fs::metadata(&h.path).ok().map(|m| m.len()))
                .sum();
            if compact_cap > 0 && input_bytes > compact_cap {
                eprintln!("[flux-db] compact SKIP at L{}: {} input bytes > FLUX_DB_COMPACT_MAX_BYTES {}",
                    current_level, input_bytes, compact_cap);
                break;
            }
            at_level.sort_by_key(|h| h.sequence);

            // Observability (chronos-v035 5TB finding): a large L{n}->L{n+1} merge runs
            // SYNCHRONOUSLY in the put()->flush()->compact() path and can block ingestion
            // for MINUTES at deep levels (measured: ~480GB / 300+ L3 tables -> a 10min+
            // write stall that looks like a hang). Log the merge so it is diagnosable as
            // progress, not a freeze. (Async/background compaction is the roadmap fix.)
            let t_compact = std::time::Instant::now();
            // FLUX_DB_QUIET=1 suppresses progress chatter (NOT errors): a fullscreen-TUI
            // host (sigil-top) sets it — raw stderr prints corrupt the alternate screen.
            if std::env::var_os("FLUX_DB_QUIET").is_none() {
                eprintln!("[flux-db] compact L{}->L{}: merging {} tables ({} MiB) ...",
                    current_level, current_level + 1, at_level.len(), input_bytes / (1024*1024));
            }

            // Memtable snapshot rides along on the L0 pass as the newest
            // source (rank above every SST — same shadowing as before).
            let (mem_snapshot, mem_snap_seq) = if current_level == 0 {
                let inner = self.inner.read();
                (Some(inner.memtable.clone()), inner.sequence)
            } else {
                (None, 0)
            };

            let out_level = current_level + 1;
            // A tombstone's PAIR may be dropped only when no level deeper
            // than the inputs holds data — every older copy of the key is
            // then an input to THIS merge and dies with it. Merging above
            // deeper data must write tombstones through, or the older copy
            // at/below out_level resurrects. New L0 flushes racing in are
            // strictly newer data, so they can't be resurrected by this.
            let deepest_populated = handles.iter().map(|h| h.level).max().unwrap_or(0);
            let drop_tombstones = current_level >= deepest_populated;
            let mut outputs: Vec<(PathBuf, PathBuf)> = Vec::new(); // (tmp, final)
            let merge_res = self.merge_level_streaming(&at_level, mem_snapshot, input_bytes, out_level, drop_tombstones, &mut outputs);
            if let Err(e) = merge_res {
                // Data-loss guard semantics: ANY parse mismatch or IO failure
                // aborts the whole pass — partial outputs are dropped, every
                // input stays on disk.
                for (tmp, _) in &outputs {
                    let _ = fs::remove_file(tmp);
                }
                return Err(e);
            }

            // All outputs are written + fsynced — atomically publish them.
            for (tmp, fin) in &outputs {
                fs::rename(tmp, fin).map_err(|e| format!("compact rename {}: {}", fin.display(), e))?;
            }
            // v3 (LANE-C): the merged outputs now exist alongside the inputs (both still valid —
            // newer seq shadows older). Invalidate so a reader re-lists BOTH before we remove the
            // inputs; a second invalidate follows the removals so the next read sees outputs-only.
            self.invalidate_sst_paths();
            for h in &at_level {
                // Drop cached blocks for the file we're about to remove —
                // otherwise they sit as zombies until evicted by capacity.
                self.block_cache.invalidate_sst(&h.path);
                // v0.35: drop the cached reader too; the path will never be
                // listed again.
                self.sst_readers.write().remove(&h.path);
                let _ = fs::remove_file(&h.path);
            }
            // v3 (LANE-C): inputs removed — re-list so the next read sees the merged outputs only.
            self.invalidate_sst_paths();
            if current_level == 0 {
                // Memtable content is durable in the outputs. Clear it ONLY if
                // no write raced the merge — the old blind `clear()` could drop
                // entries that were put after the snapshot (they lived on in
                // the WAL but vanished from live reads, and a later truncation
                // made the loss permanent). If writes raced, keep the memtable
                // intact: already-merged entries are shadow-equal duplicates,
                // nothing is lost, and the next flush folds the rest.
                //
                // The merge itself advances `sequence` once per output file it
                // names, so the no-race fingerprint is snapshot seq + output
                // count — plain equality never held once the merge wrote an
                // output, so the memtable was NEVER cleared and its whole
                // content (tombstones included) re-folded into a fresh L1
                // file on every compact.
                let mut inner = self.inner.write();
                if inner.sequence == mem_snap_seq + outputs.len() as u64 {
                    inner.memtable.clear();
                }
            }
            if std::env::var_os("FLUX_DB_QUIET").is_none() {
                eprintln!("[flux-db] compact L{}->L{}: done in {:.1}s",
                    current_level, current_level + 1, t_compact.elapsed().as_secs_f64());
            }
            // Keep iterating — out_level may now exceed its own threshold.
        }
        Ok(())
    }

    /// v0.37 (task #10): heap-based k-way merge of one level's SSTs (plus an
    /// optional memtable snapshot) into rolling ~1 GiB outputs at `out_level`.
    ///
    ///   * Newer source wins on duplicate keys (sources are ranked oldest →
    ///     newest; the memtable outranks every SST).
    ///   * Tombstones (empty values) drop the pair from the output only when
    ///     `drop_tombstones` is set (inputs are the deepest data); otherwise
    ///     the tombstone is written through so it keeps shadowing older
    ///     copies at/below the output level.
    ///   * DATA-LOSS GUARD: for v2+ inputs the header `key_count` is
    ///     authoritative — when a stream ends, its yielded count must match,
    ///     and any block/index parse error is fatal. The caller keeps all
    ///     inputs and removes partial outputs on ANY `Err`.
    ///   * Outputs land as `flux_L{lvl}_{seq}.sst.tmp` (fresh sequence each,
    ///     fsynced in `finish`) and are pushed onto `outputs`; the caller
    ///     renames them into place only after the whole pass succeeds.
    ///
    /// Peak RAM: one block + read buffers per input stream (~0.5 MB each),
    /// the current output block + its flat index, and the heap (one head
    /// entry per source) — independent of input bytes.
    fn merge_level_streaming(
        &self,
        at_level: &[&SstHandle],
        mem_snapshot: Option<BTreeMap<Vec<u8>, Vec<u8>>>,
        input_bytes: u64,
        out_level: u8,
        drop_tombstones: bool,
        outputs: &mut Vec<(PathBuf, PathBuf)>,
    ) -> Result<(), String> {
        use std::cmp::Ordering;

        enum Source {
            Sst { stream: SstStream, expect: u64, vers: u8, path: PathBuf },
            Mem(std::collections::btree_map::IntoIter<Vec<u8>, Vec<u8>>),
        }
        impl Source {
            fn next(&mut self) -> Result<Option<(Vec<u8>, Vec<u8>)>, String> {
                match self {
                    Source::Mem(it) => Ok(it.next()),
                    Source::Sst { stream, expect, vers, path } => {
                        let n = stream.next_pair()?;
                        if n.is_none() && *vers >= 2 && stream.yielded() != *expect {
                            return Err(format!(
                                "compact ABORT (data-loss guard): {:?} streamed {} of {} entries — inputs kept",
                                path, stream.yielded(), expect));
                        }
                        Ok(n)
                    }
                }
            }
        }

        struct HeapEntry {
            key: Vec<u8>,
            value: Vec<u8>,
            rank: usize,
        }
        // BinaryHeap is a max-heap → invert key order (smallest key pops
        // first); among equal keys the HIGHEST rank (newest source) pops first.
        impl Ord for HeapEntry {
            fn cmp(&self, o: &Self) -> Ordering {
                o.key.cmp(&self.key).then(self.rank.cmp(&o.rank))
            }
        }
        impl PartialOrd for HeapEntry {
            fn partial_cmp(&self, o: &Self) -> Option<Ordering> { Some(self.cmp(o)) }
        }
        impl PartialEq for HeapEntry {
            fn eq(&self, o: &Self) -> bool { self.key == o.key && self.rank == o.rank }
        }
        impl Eq for HeapEntry {}

        // Open sources oldest-first: index in this vec == rank.
        let mut sources: Vec<Source> = Vec::with_capacity(at_level.len() + 1);
        let mut total_keys: u64 = 0;
        for h in at_level {
            let table = SstReader::open(&h.path)?;
            total_keys += table.key_count;
            sources.push(Source::Sst {
                expect: table.key_count,
                vers: table.version,
                path: h.path.clone(),
                stream: table.stream()?,
            });
        }
        if let Some(mem) = mem_snapshot {
            total_keys += mem.len() as u64;
            sources.push(Source::Mem(mem.into_iter()));
        }

        // Rolling output cap (~1 GiB) + bloom sizing per output: estimate the
        // keys landing in one output by scaling total input keys to the cap,
        // with 25% headroom (an undersized bloom only raises the FP rate).
        let out_cap: u64 = std::env::var("FLUX_DB_COMPACT_OUTPUT_BYTES").ok()
            .and_then(|v| v.parse().ok())
            .filter(|v| *v > 0)
            .unwrap_or_else(|| self.inner.read().compact_output_bytes);
        let bloom_capacity: usize = if input_bytes <= out_cap {
            total_keys.max(16) as usize
        } else {
            (((total_keys as u128 * out_cap as u128 / input_bytes.max(1) as u128) as u64)
                .saturating_mul(5) / 4).max(16) as usize
        };

        let mut heap: std::collections::BinaryHeap<HeapEntry> = std::collections::BinaryHeap::new();
        for (rank, src) in sources.iter_mut().enumerate() {
            if let Some((key, value)) = src.next()? {
                heap.push(HeapEntry { key, value, rank });
            }
        }

        let mut writer: Option<FileSstWriter> = None;
        let fail = |w: Option<FileSstWriter>, e: String| -> Result<(), String> {
            if let Some(w) = w { w.abort(); }
            Err(e)
        };

        while let Some(winner) = heap.pop() {
            // Advance the winner's source.
            match sources[winner.rank].next() {
                Ok(Some((key, value))) => heap.push(HeapEntry { key, value, rank: winner.rank }),
                Ok(None) => {}
                Err(e) => return fail(writer.take(), e),
            }
            // Drain + discard older duplicates of this key.
            while heap.peek().map(|h| h.key == winner.key).unwrap_or(false) {
                let dup = heap.pop().unwrap();
                match sources[dup.rank].next() {
                    Ok(Some((key, value))) => heap.push(HeapEntry { key, value, rank: dup.rank }),
                    Ok(None) => {}
                    Err(e) => return fail(writer.take(), e),
                }
            }
            // Tombstone: drop the pair entirely — but ONLY when this merge's
            // inputs are the deepest data (see compact()). Otherwise write it
            // through; it dies for good once it reaches the bottom level.
            if winner.value.is_empty() && drop_tombstones {
                continue;
            }
            if writer.is_none() {
                let seq = {
                    let mut inner = self.inner.write();
                    inner.sequence += 1;
                    inner.sequence
                };
                let final_path = self.path.join(sst_name(out_level, seq));
                match FileSstWriter::create(final_path, bloom_capacity) {
                    Ok(w) => writer = Some(w),
                    Err(e) => return fail(None, e),
                }
            }
            let w = writer.as_mut().unwrap();
            if let Err(e) = w.add(&winner.key, &winner.value) {
                return fail(writer.take(), e);
            }
            if w.body_bytes() >= out_cap {
                let full = writer.take().unwrap();
                match full.finish() {
                    Ok(pair) => outputs.push(pair),
                    Err(e) => return fail(None, e),
                }
            }
        }
        if let Some(w) = writer.take() {
            match w.finish() {
                Ok(pair) => outputs.push(pair),
                Err(e) => return fail(None, e),
            }
        }
        Ok(())
    }
}

// ── v0.9.0: Bloom Filter ──

pub struct BloomFilter {
    pub bits: Vec<u64>,
    pub num_hashes: usize,
}

impl BloomFilter {
    pub fn new(capacity: usize, false_positive_rate: f64) -> Self {
        let num_bits = (-(capacity as f64) * false_positive_rate.ln() / (2.0_f64.ln().powi(2))).ceil() as usize;
        let num_hashes = ((num_bits as f64 / capacity as f64) * 2.0_f64.ln()).ceil() as usize;
        BloomFilter { bits: vec![0; ((num_bits + 63) / 64).max(1)], num_hashes: num_hashes.max(1) }
    }

    /// A bloom filter that says "yes" to every key. Used when reading legacy
    /// (pre-v0.10.0) SSTs that don't have a persisted bloom — we fall back to
    /// a full scan in that case.
    pub fn passthrough() -> Self {
        BloomFilter { bits: vec![u64::MAX], num_hashes: 1 }
    }

    pub fn insert(&mut self, key: &[u8]) {
        let (h1, h2) = Self::hash_pair(key);
        let m = self.bits.len() as u64 * 64;
        for i in 0..self.num_hashes as u64 {
            let bit = (h1.wrapping_add(i.wrapping_mul(h2))) % m;
            self.bits[(bit / 64) as usize] |= 1 << (bit % 64);
        }
    }

    pub fn may_contain(&self, key: &[u8]) -> bool {
        let (h1, h2) = Self::hash_pair(key);
        let m = self.bits.len() as u64 * 64;
        for i in 0..self.num_hashes as u64 {
            let bit = (h1.wrapping_add(i.wrapping_mul(h2))) % m;
            if self.bits[(bit / 64) as usize] & (1 << (bit % 64)) == 0 { return false; }
        }
        true
    }

    fn hash_pair(key: &[u8]) -> (u64, u64) {
        let hash = blake3::hash(key);
        let b = hash.as_bytes();
        (u64::from_le_bytes([b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7]]),
         u64::from_le_bytes([b[8],b[9],b[10],b[11],b[12],b[13],b[14],b[15]]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_put_get() {
        let tmp = std::env::temp_dir().join("flux-db-test-putget");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.put(b"hello", b"world").unwrap();
        assert_eq!(db.get(b"hello").unwrap(), Some(b"world".to_vec()));
        assert_eq!(db.get(b"nope").unwrap(), None);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_delete() {
        let tmp = std::env::temp_dir().join("flux-db-test-delete");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.put(b"key", b"val").unwrap();
        db.delete(b"key").unwrap();
        assert_eq!(db.get(b"key").unwrap(), None);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_get_many_parity_with_get() {
        // get_many must be byte-identical to per-key get() across every layer
        // a key can live in: memtable, flushed SSTs, overwrites (newest copy
        // wins), point tombstones, range tombstones, and plain misses.
        let tmp = std::env::temp_dir().join("flux-db-test-getmany");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();

        // Layer 1: flushed to SST.
        for i in 0..50u32 {
            db.put(format!("sst-{:03}", i).as_bytes(), format!("old-{}", i).as_bytes()).unwrap();
        }
        db.flush().unwrap();
        // Layer 2: newer SST shadowing some of layer 1, plus point deletes.
        for i in 0..20u32 {
            db.put(format!("sst-{:03}", i).as_bytes(), format!("new-{}", i).as_bytes()).unwrap();
        }
        db.delete(b"sst-030").unwrap();
        db.flush().unwrap();
        // Layer 3: memtable-only entries + a memtable overwrite + range delete.
        db.put(b"mem-a", b"in-memtable").unwrap();
        db.put(b"sst-040", b"memtable-wins").unwrap();
        db.delete_range(b"sst-045", b"sst-048").unwrap();

        let mut keys: Vec<Vec<u8>> = (0..50u32).map(|i| format!("sst-{:03}", i).into_bytes()).collect();
        keys.push(b"mem-a".to_vec());
        keys.push(b"missing-1".to_vec());
        keys.insert(7, b"missing-0".to_vec()); // miss in the middle, order must hold

        let batch = db.get_many(&keys).unwrap();
        assert_eq!(batch.len(), keys.len());
        for (k, got) in keys.iter().zip(batch.iter()) {
            let single = db.get(k).unwrap();
            assert_eq!(got, &single, "get_many diverged from get() on key {:?}", String::from_utf8_lossy(k));
        }
        // Spot-check the interesting layers directly.
        let idx = |k: &[u8]| keys.iter().position(|x| x.as_slice() == k).unwrap();
        assert_eq!(batch[idx(b"sst-005")].as_deref(), Some(b"new-5" as &[u8]), "newer SST must shadow older");
        assert_eq!(batch[idx(b"sst-025")].as_deref(), Some(b"old-25" as &[u8]), "unshadowed key reads old SST");
        assert_eq!(batch[idx(b"sst-030")], None, "point tombstone must hide SST copy");
        assert_eq!(batch[idx(b"sst-040")].as_deref(), Some(b"memtable-wins" as &[u8]), "memtable shadows SSTs");
        assert_eq!(batch[idx(b"sst-045")], None, "range tombstone start covered");
        assert_eq!(batch[idx(b"sst-047")], None, "range tombstone interior covered");
        assert_eq!(batch[idx(b"mem-a")].as_deref(), Some(b"in-memtable" as &[u8]));
        assert_eq!(batch[idx(b"missing-0")], None);
        assert_eq!(batch[idx(b"missing-1")], None);
        // Empty batch is a clean no-op.
        assert!(db.get_many(&Vec::<Vec<u8>>::new()).unwrap().is_empty());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_snapshot() {
        let tmp = std::env::temp_dir().join("flux-db-test-snap");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.put(b"a", b"1").unwrap();
        let snap = db.snapshot();
        db.put(b"b", b"2").unwrap();
        assert_eq!(snap.get(b"a"), Some(&b"1".to_vec()));
        assert_eq!(snap.get(b"b"), None); // snapshot is before b was inserted
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_wal_replay() {
        let tmp = std::env::temp_dir().join("flux-db-test-wal");
        let _ = fs::remove_dir_all(&tmp);
        {
            let db = Database::open(&tmp).unwrap();
            db.put(b"persist", b"me").unwrap();
        }
        {
            let db = Database::open(&tmp).unwrap();
            assert_eq!(db.get(b"persist").unwrap(), Some(b"me".to_vec()));
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_checksum() {
        let tmp = std::env::temp_dir().join("flux-db-test-checksum");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.put(b"a", b"1").unwrap();
        let cs1 = db.checksum();
        db.put(b"b", b"2").unwrap();
        let cs2 = db.checksum();
        assert_ne!(cs1, cs2);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_batch_put() {
        let tmp = std::env::temp_dir().join("flux-db-test-batch");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.batch_put(&[(b"a", b"1"), (b"b", b"2"), (b"c", b"3")]).unwrap();
        assert_eq!(db.get(b"a").unwrap(), Some(b"1".to_vec()));
        assert_eq!(db.get(b"b").unwrap(), Some(b"2".to_vec()));
        assert_eq!(db.get(b"c").unwrap(), Some(b"3".to_vec()));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_scan_prefix() {
        let tmp = std::env::temp_dir().join("flux-db-test-prefix");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.batch_put(&[(b"usr:1", b"alice"), (b"usr:2", b"bob"), (b"ord:1", b"foo")]).unwrap();
        let users = db.scan_prefix(b"usr:");
        assert_eq!(users.len(), 2);
        let orders = db.scan_prefix(b"ord:");
        assert_eq!(orders.len(), 1);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_scan_range() {
        let tmp = std::env::temp_dir().join("flux-db-test-range");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.batch_put(&[(b"a", b"1"), (b"b", b"2"), (b"c", b"3"), (b"d", b"4")]).unwrap();
        let r = db.scan_range(b"b", b"d");
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].0, b"b"); assert_eq!(r[1].0, b"c");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_stats() {
        let tmp = std::env::temp_dir().join("flux-db-test-stats");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.batch_put(&[(b"x", b"1"), (b"y", b"2")]).unwrap();
        let s = db.stats();
        assert_eq!(s.key_count, 2);
        assert!(s.wal_size > 0);
        assert!(s.sequence >= 2);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_bloom_filter() {
        let mut bf = BloomFilter::new(1000, 0.01);
        bf.insert(b"hello");
        bf.insert(b"world");
        assert!(bf.may_contain(b"hello"));
        assert!(bf.may_contain(b"world"));
        assert!(!bf.may_contain(b"nope"));
        // Test many inserts
        for i in 0u32..500 {
            bf.insert(&i.to_le_bytes());
        }
        let mut fp = 0;
        for i in 500u32..1500 {
            if bf.may_contain(&i.to_le_bytes()) { fp += 1; }
        }
        let fpr = fp as f64 / 1000.0;
        assert!(fpr < 0.05, "false positive rate {} exceeds 5%", fpr);
    }

    #[test]
    fn test_compaction() {
        let tmp = std::env::temp_dir().join("flux-db-test-compact");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.put(b"keep", b"me").unwrap();
        db.put(b"del", b"me").unwrap();
        db.flush().unwrap();
        db.delete(b"del").unwrap();
        db.flush().unwrap();
        db.compact().unwrap();
        // After compaction, deleted key should be gone from SST
        assert!(db.get(b"keep").unwrap().is_some());
        assert!(db.get(b"del").unwrap().is_none());
        let _ = fs::remove_dir_all(&tmp);
    }

    // ── v0.10.0 upgrade tests ──

    #[test]
    fn test_get_reads_through_sst() {
        // The bug v0.10.0 fixes: pre-v0.10 get() only looked at the memtable.
        // After flush(), the memtable was cleared and get() returned None.
        let tmp = std::env::temp_dir().join("flux-db-test-read-through");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.put(b"persisted", b"hello").unwrap();
        db.flush().unwrap();
        // Memtable is now empty — pre-upgrade this would return None.
        assert_eq!(db.get(b"persisted").unwrap(), Some(b"hello".to_vec()));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_get_tombstone_shadows_sst() {
        // Delete after flush must shadow the older SST copy.
        let tmp = std::env::temp_dir().join("flux-db-test-tombstone");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.put(b"k", b"v1").unwrap();
        db.flush().unwrap();
        db.delete(b"k").unwrap();
        // Tombstone in memtable, value still in SST. get() must return None.
        assert_eq!(db.get(b"k").unwrap(), None);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_wal_torn_write_recovery() {
        // Simulate: a clean session writes A and B, then a crash mid-write
        // appends garbage to the WAL. On reopen, A and B must survive and the
        // garbage must be discarded silently — not crash the loader, not get
        // applied as a key.
        let tmp = std::env::temp_dir().join("flux-db-test-torn");
        let _ = fs::remove_dir_all(&tmp);
        {
            let db = Database::open(&tmp).unwrap();
            db.put(b"A", b"1").unwrap();
            db.put(b"B", b"2").unwrap();
        }
        // Append torn bytes to the WAL.
        let wal_path = tmp.join("flux.wal");
        {
            let mut f = fs::OpenOptions::new().append(true).open(&wal_path).unwrap();
            // 12 bytes of header that would claim a 1KB payload, plus a few
            // garbage body bytes — guaranteed CRC mismatch + truncated body.
            f.write_all(&[0xDE, 0xAD, 0xBE, 0xEF, 0, 4, 0, 0, 0, 4, 0, 0]).unwrap();
            f.write_all(b"GARBAGE").unwrap();
        }
        let db = Database::open(&tmp).unwrap();
        assert_eq!(db.get(b"A").unwrap(), Some(b"1".to_vec()));
        assert_eq!(db.get(b"B").unwrap(), Some(b"2".to_vec()));
        // The garbage key (whatever it would decode to) must NOT be there.
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_bloom_skip_in_persisted_sst() {
        // After flush, the SST carries a bloom filter. A key never seen by
        // the SST must be rejected by the bloom (or at worst a false-positive
        // at ~1%). Probe many random misses and assert the bloom rejects
        // most of them.
        let tmp = std::env::temp_dir().join("flux-db-test-bloom-skip");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        for i in 0u32..200 {
            db.put(&i.to_le_bytes(), b"v").unwrap();
        }
        db.flush().unwrap();

        let ssts = list_ssts(&tmp).unwrap();
        assert_eq!(ssts.len(), 1);
        let reader = SstReader::open(&ssts[0]).unwrap();
        let mut may = 0;
        for i in 1000u32..2000 {
            if reader.bloom.may_contain(&i.to_le_bytes()) { may += 1; }
        }
        // 1% FPR target -> expect <50 false-positives out of 1000.
        assert!(may < 50, "bloom too lossy: {may} false positives / 1000");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_iter_streams_memtable_and_sst() {
        let tmp = std::env::temp_dir().join("flux-db-test-iter");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.put(b"a", b"1").unwrap();
        db.put(b"b", b"2").unwrap();
        db.flush().unwrap();
        db.put(b"c", b"3").unwrap();
        db.delete(b"a").unwrap();
        // iter() must see b + c, not a (tombstone shadows SST copy).
        let pairs: Vec<(Vec<u8>, Vec<u8>)> = db.iter().collect();
        assert_eq!(pairs, vec![
            (b"b".to_vec(), b"2".to_vec()),
            (b"c".to_vec(), b"3".to_vec()),
        ]);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_auto_compaction_triggers() {
        // With AUTO_COMPACT_THRESHOLD = 4, the 5th flush should leave us with
        // a single compacted SST instead of five small ones.
        let tmp = std::env::temp_dir().join("flux-db-test-autocompact");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        for i in 0u8..5 {
            db.put(&[i], &[i * 10]).unwrap();
            db.flush().unwrap();
        }
        // v0.37: compaction is now BACKGROUND (async) for non-defer stores, so it
        // settles shortly after the triggering flush rather than inline. Poll for the
        // eventual result instead of asserting synchronously (still verifies compaction
        // fires -- just allows it to be async).
        let mut ssts = list_ssts(&tmp).unwrap();
        for _ in 0..200 {
            if ssts.len() <= 2 { break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
            ssts = list_ssts(&tmp).unwrap();
        }
        assert!(ssts.len() <= 2, "auto-compact didn't fire (async): {} ssts left", ssts.len());
        // All keys still visible.
        for i in 0u8..5 {
            assert_eq!(db.get(&[i]).unwrap(), Some(vec![i * 10]));
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    // ── v0.11.0 transaction tests ──

    #[test]
    fn test_tx_basic_commit() {
        let tmp = std::env::temp_dir().join("flux-db-test-tx-basic");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        let mut tx = db.begin_transaction();
        tx.put(b"a", b"1").unwrap();
        tx.put(b"b", b"2").unwrap();
        let _ = tx.commit().unwrap();
        // Both keys visible after commit.
        assert_eq!(db.get(b"a").unwrap(), Some(b"1".to_vec()));
        assert_eq!(db.get(b"b").unwrap(), Some(b"2".to_vec()));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_tx_rollback() {
        let tmp = std::env::temp_dir().join("flux-db-test-tx-rollback");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        let mut tx = db.begin_transaction();
        tx.put(b"x", b"never").unwrap();
        tx.rollback();
        assert_eq!(db.get(b"x").unwrap(), None);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_tx_auto_rollback_on_drop() {
        let tmp = std::env::temp_dir().join("flux-db-test-tx-drop");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        {
            let mut tx = db.begin_transaction();
            tx.put(b"dropped", b"v").unwrap();
            // tx falls out of scope — implicit rollback
        }
        assert_eq!(db.get(b"dropped").unwrap(), None);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_tx_read_your_writes() {
        // Within one transaction, put then get must see the just-put value
        // even though commit hasn't happened.
        let tmp = std::env::temp_dir().join("flux-db-test-tx-ryw");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        let mut tx = db.begin_transaction();
        tx.put(b"k", b"v1").unwrap();
        assert_eq!(tx.get(b"k").unwrap(), Some(b"v1".to_vec()));
        tx.put(b"k", b"v2").unwrap();
        assert_eq!(tx.get(b"k").unwrap(), Some(b"v2".to_vec()));
        tx.delete(b"k").unwrap();
        assert_eq!(tx.get(b"k").unwrap(), None);
        // Outside the tx the DB is still empty.
        assert_eq!(db.get(b"k").unwrap(), None);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_tx_isolation_other_writes_invisible() {
        // tx1 begins; tx2 puts a key and commits. tx1.get for that key must
        // NOT see tx2's write — tx1 is snapshot-isolated at its begin_seq.
        let tmp = std::env::temp_dir().join("flux-db-test-tx-iso");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.put(b"before", b"yes").unwrap();
        let mut tx1 = db.begin_transaction();
        // Now tx2 writes a brand-new key.
        let mut tx2 = db.begin_transaction();
        tx2.put(b"new_in_tx2", b"v").unwrap();
        let _ = tx2.commit().unwrap();
        // tx1 sees the pre-existing key but NOT the post-snapshot write.
        assert_eq!(tx1.get(b"before").unwrap(), Some(b"yes".to_vec()));
        assert_eq!(tx1.get(b"new_in_tx2").unwrap(), None);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_tx_conflict_on_overlapping_write() {
        // tx1 reads K, tx2 writes K and commits, tx1 commit (also touching K)
        // must fail with Conflict.
        let tmp = std::env::temp_dir().join("flux-db-test-tx-conflict");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.put(b"k", b"v0").unwrap();
        let mut tx1 = db.begin_transaction();
        let _ = tx1.get(b"k").unwrap(); // tx1 read-sets k
        let mut tx2 = db.begin_transaction();
        tx2.put(b"k", b"tx2-wins").unwrap();
        let _ = tx2.commit().unwrap();
        tx1.put(b"k", b"tx1-late").unwrap();
        let err = tx1.commit().unwrap_err();
        assert!(matches!(err, TxError::Conflict { .. }), "expected Conflict, got {:?}", err);
        // The winning value survived.
        assert_eq!(db.get(b"k").unwrap(), Some(b"tx2-wins".to_vec()));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_tx_atomic_multikey() {
        // Mid-commit must be invisible. Commit either all writes or none.
        let tmp = std::env::temp_dir().join("flux-db-test-tx-atomic");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        let mut tx = db.begin_transaction();
        tx.put(b"a", b"1").unwrap();
        tx.put(b"b", b"2").unwrap();
        tx.put(b"c", b"3").unwrap();
        // From a concurrent reader's perspective right before commit, none of
        // these keys exist yet.
        assert_eq!(db.get(b"a").unwrap(), None);
        assert_eq!(db.get(b"b").unwrap(), None);
        assert_eq!(db.get(b"c").unwrap(), None);
        let _ = tx.commit().unwrap();
        // Now all three visible.
        assert_eq!(db.get(b"a").unwrap(), Some(b"1".to_vec()));
        assert_eq!(db.get(b"b").unwrap(), Some(b"2".to_vec()));
        assert_eq!(db.get(b"c").unwrap(), Some(b"3".to_vec()));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_tx_no_conflict_on_disjoint_keys() {
        // Two transactions touching different keys both succeed.
        let tmp = std::env::temp_dir().join("flux-db-test-tx-disjoint");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        let mut tx1 = db.begin_transaction();
        let mut tx2 = db.begin_transaction();
        tx1.put(b"alpha", b"1").unwrap();
        tx2.put(b"beta", b"2").unwrap();
        let _ = tx1.commit().unwrap();
        let _ = tx2.commit().unwrap(); // disjoint — no conflict
        assert_eq!(db.get(b"alpha").unwrap(), Some(b"1".to_vec()));
        assert_eq!(db.get(b"beta").unwrap(), Some(b"2".to_vec()));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_tx_after_finished_errors() {
        let tmp = std::env::temp_dir().join("flux-db-test-tx-finished");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        let mut tx = db.begin_transaction();
        tx.put(b"k", b"v").unwrap();
        let _ = tx.commit().unwrap();
        // Note: commit consumed self, so we can't error-test on the same tx.
        // Instead, test that a tx whose commit returned Err can't be reused.
        db.put(b"k", b"v0").unwrap();
        let mut tx1 = db.begin_transaction();
        let _ = tx1.get(b"k").unwrap();
        let mut tx2 = db.begin_transaction();
        tx2.put(b"k", b"contention").unwrap();
        let _ = tx2.commit().unwrap();
        tx1.put(b"k", b"loser").unwrap();
        let _ = tx1.commit().unwrap_err();
        let _ = fs::remove_dir_all(&tmp);
    }

    // ── v0.12 block-based SST + LRU block cache tests ──

    #[test]
    fn test_block_sst_round_trip() {
        // After flush, every put-then-flushed key still reads correctly via
        // the block-based reader path.
        let tmp = std::env::temp_dir().join("flux-db-test-block-rt");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        for i in 0u32..500 {
            db.put(&i.to_le_bytes(), &i.to_le_bytes()).unwrap();
        }
        db.flush().unwrap();
        for i in 0u32..500 {
            assert_eq!(
                db.get(&i.to_le_bytes()).unwrap(),
                Some(i.to_le_bytes().to_vec()),
                "round-trip failed at i={i}",
            );
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_block_cache_warms() {
        // The block cache should have non-zero hits after repeated lookups
        // to the same key — proving we're not re-decompressing every time.
        let tmp = std::env::temp_dir().join("flux-db-test-cache-warm");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        for i in 0u32..200 {
            db.put(&i.to_le_bytes(), b"v").unwrap();
        }
        db.flush().unwrap();
        // Hit the same key 50 times.
        for _ in 0..50 {
            let _ = db.get(&100u32.to_le_bytes()).unwrap();
        }
        let (hits, misses, _) = db.block_cache_stats();
        assert!(hits >= 49, "expected ≥49 cache hits, got {hits}");
        assert!(misses >= 1, "expected ≥1 miss (cold start), got {misses}");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_block_sst_v2_format_marker() {
        // SSTs we write should be tagged as v2; readers should still accept
        // v1 / legacy via the dispatcher.
        let tmp = std::env::temp_dir().join("flux-db-test-v2-marker");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.put(b"k", b"v").unwrap();
        db.flush().unwrap();
        let ssts = list_ssts(&tmp).unwrap();
        let raw = std::fs::read(&ssts[0]).unwrap();
        assert_eq!(&raw[0..4], &SST_MAGIC);
        assert_eq!(raw[4], SST_VERSION);
        assert_eq!(SST_VERSION, 3);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_block_index_locates_correctly() {
        // Each data block records its last key; the index must locate the
        // right block for an arbitrary lookup including misses.
        let mut builder = block::SstBuilder::new();
        for i in 0u32..2000 {
            builder.add(&i.to_be_bytes(), &i.to_le_bytes());
        }
        let body = builder.finish();
        let reader = block::BlockSstReader::new(&body).unwrap();
        // Multiple blocks present (we filled 2000 keys so should be > 1).
        assert!(reader.index.len() > 1, "expected multiple data blocks, got {}", reader.index.len());
        // Probe several keys.
        for probe in [0u32, 500, 1000, 1999] {
            let handle = reader.locate_block(&probe.to_be_bytes()).unwrap();
            let decompressed = reader.read_block(handle).unwrap();
            let got = block::BlockSstReader::lookup_in_block(&decompressed, &probe.to_be_bytes());
            assert_eq!(got, Some(probe.to_le_bytes().to_vec()), "probe failed at {probe}");
        }
    }

    #[test]
    fn test_block_cache_eviction_under_pressure() {
        // Use a 4 KiB cache and write enough data that it can't all fit.
        // Hits should still be positive but bounded by capacity.
        let tmp = std::env::temp_dir().join("flux-db-test-cache-evict");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap().with_block_cache_capacity(4096);
        // Write a lot of keys so the SST has many blocks.
        for i in 0u32..5000 {
            db.put(&i.to_be_bytes(), &i.to_le_bytes()).unwrap();
        }
        db.flush().unwrap();
        // Probe across the entire keyspace so we miss blocks evicted earlier.
        for i in 0u32..5000 {
            let _ = db.get(&i.to_be_bytes()).unwrap();
        }
        let (_hits, _misses, cur_bytes) = db.block_cache_stats();
        assert!(cur_bytes <= 4096, "cache overshot capacity: {cur_bytes}");
        let _ = fs::remove_dir_all(&tmp);
    }

    // ── v0.16 tests (TTL + merge + filter) ──

    #[test]
    fn test_v16_ttl_expires() {
        let tmp = std::env::temp_dir().join("flux-db-test-v16-ttl");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        // Wrap with an already-expired expiry so we don't depend on sleep.
        let value = ttl::wrap(b"old", 100); // expires at unix 100 — long past
        db.put(b"k", &value).unwrap();
        // Raw get returns the wrapped bytes; the user-facing put_with_ttl /
        // unwrap pair is what handles expiry.
        let raw = db.get(b"k").unwrap().unwrap();
        assert_eq!(ttl::unwrap(&raw, ttl::now_unix()), None,
                   "expired ttl should unwrap to None");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_v16_ttl_live_value() {
        let tmp = std::env::temp_dir().join("flux-db-test-v16-ttl-live");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.put_ttl_seconds(b"k", b"hello", 3600).unwrap();
        let raw = db.get(b"k").unwrap().unwrap();
        let live = ttl::unwrap(&raw, ttl::now_unix()).unwrap();
        assert_eq!(live, b"hello".to_vec());
        let _ = fs::remove_dir_all(&tmp);
    }

    struct AddInt;
    impl merge::MergeOperator for AddInt {
        fn merge(&self, existing: Option<&[u8]>, delta: &[u8]) -> Vec<u8> {
            let cur = existing
                .and_then(|e| e.try_into().ok())
                .map(i64::from_le_bytes)
                .unwrap_or(0);
            let d = i64::from_le_bytes(delta.try_into().unwrap_or([0; 8]));
            (cur + d).to_le_bytes().to_vec()
        }
    }

    #[test]
    fn test_v16_merge_counter() {
        let tmp = std::env::temp_dir().join("flux-db-test-v16-merge");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.set_merge_operator(Arc::new(AddInt));
        db.put(b"counter", &0i64.to_le_bytes()).unwrap();
        db.merge(b"counter", &5i64.to_le_bytes()).unwrap();
        db.merge(b"counter", &3i64.to_le_bytes()).unwrap();
        db.merge(b"counter", &(-2i64).to_le_bytes()).unwrap();
        let raw = db.get(b"counter").unwrap().unwrap();
        let val = i64::from_le_bytes(raw.try_into().unwrap());
        assert_eq!(val, 6);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_v16_merge_without_operator_fails() {
        let tmp = std::env::temp_dir().join("flux-db-test-v16-no-op");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        let err = db.merge(b"k", b"d").unwrap_err();
        assert!(err.contains("no merge operator"));
        let _ = fs::remove_dir_all(&tmp);
    }

    // ── v0.15 multi-level LSM tests ──

    #[test]
    fn test_v15_flushes_land_at_l0() {
        let tmp = std::env::temp_dir().join("flux-db-test-v15-l0-flush");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.put(b"k", b"v").unwrap();
        db.flush().unwrap();
        let handles = list_ssts_leveled(&tmp).unwrap();
        assert_eq!(handles.len(), 1);
        assert_eq!(handles[0].level, 0, "fresh flush should be L0");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_v15_compaction_promotes_to_l1() {
        let tmp = std::env::temp_dir().join("flux-db-test-v15-l0-to-l1");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        // Five L0 flushes — above L0_COMPACT_THRESHOLD.
        for i in 0u32..5 {
            db.put(&i.to_be_bytes(), b"v").unwrap();
            db.flush().unwrap();
        }
        db.compact().unwrap();
        let handles = list_ssts_leveled(&tmp).unwrap();
        let l1: Vec<_> = handles.iter().filter(|h| h.level == 1).collect();
        assert!(!l1.is_empty(), "compact() should have created at least one L1 file");
        // All five values still readable.
        for i in 0u32..5 {
            assert_eq!(db.get(&i.to_be_bytes()).unwrap(), Some(b"v".to_vec()));
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_v15_legacy_files_treated_as_l0() {
        // Hand-craft a legacy-named SST file (no _L{lvl}_ segment) and make
        // sure list_ssts_leveled reports level 0.
        let tmp = std::env::temp_dir().join("flux-db-test-v15-legacy");
        let _ = fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // Touch a fake legacy file. It's empty so SstReader::open would fail,
        // but list_ssts_leveled only cares about the name.
        std::fs::write(tmp.join("flux_0000000000000005.sst"), b"").unwrap();
        let handles = list_ssts_leveled(&tmp).unwrap();
        assert_eq!(handles.len(), 1);
        assert_eq!(handles[0].level, 0);
        assert_eq!(handles[0].sequence, 5);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_v15_sst_name_format() {
        assert_eq!(sst_name(0, 5),   "flux_L00_0000000000000005.sst");
        assert_eq!(sst_name(2, 100), "flux_L02_0000000000000064.sst");
    }

    // ── v0.14 column-family tests ──

    #[test]
    fn test_cf_isolation_same_key_different_values() {
        let tmp = std::env::temp_dir().join("flux-db-test-cf-iso");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        let users = db.create_cf("users").unwrap();
        let orders = db.create_cf("orders").unwrap();
        users.put(b"id1", b"alice").unwrap();
        orders.put(b"id1", b"order-xyz").unwrap();
        // Same key, two CFs, two values — fully isolated.
        assert_eq!(users.get(b"id1").unwrap(),  Some(b"alice".to_vec()));
        assert_eq!(orders.get(b"id1").unwrap(), Some(b"order-xyz".to_vec()));
        // Default CF (parent db) is independent of both.
        assert_eq!(db.get(b"id1").unwrap(), None);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_cf_handle_dedup() {
        let tmp = std::env::temp_dir().join("flux-db-test-cf-dedup");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        let h1 = db.create_cf("logs").unwrap();
        let h2 = db.cf("logs").unwrap();
        h1.put(b"k", b"via h1").unwrap();
        // h2 sees the write because they share inner state.
        assert_eq!(h2.get(b"k").unwrap(), Some(b"via h1".to_vec()));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_cf_drop_removes_files_and_handle() {
        let tmp = std::env::temp_dir().join("flux-db-test-cf-drop");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        let logs = db.create_cf("logs").unwrap();
        logs.put(b"k", b"v").unwrap();
        logs.flush().unwrap();
        let logs_dir = tmp.join("cf_logs");
        assert!(logs_dir.exists());
        drop(logs);
        db.drop_cf("logs").unwrap();
        assert!(!logs_dir.exists(), "drop_cf should remove the directory");
        assert!(db.cf("logs").is_none(), "drop_cf should remove the handle");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_cf_default_rejected() {
        let tmp = std::env::temp_dir().join("flux-db-test-cf-default");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        assert!(db.create_cf("default").is_err());
        assert!(db.drop_cf("default").is_err());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_cf_invalid_names() {
        let tmp = std::env::temp_dir().join("flux-db-test-cf-invalid");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        assert!(db.create_cf("").is_err());
        assert!(db.create_cf("path/with/slash").is_err());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_cf_drop_doesnt_affect_others() {
        // Dropping one CF must leave sibling CFs and their on-disk state intact.
        let tmp = std::env::temp_dir().join("flux-db-test-cf-drop-siblings");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        let users = db.create_cf("users").unwrap();
        let orders = db.create_cf("orders").unwrap();
        users.put(b"u1", b"alice").unwrap();
        orders.put(b"o1", b"order123").unwrap();
        // Flush both so each CF has at least one SST that drop must skip.
        users.flush().unwrap();
        orders.flush().unwrap();

        drop(users);
        db.drop_cf("users").unwrap();

        // orders is untouched — both memtable and on-disk content survive.
        assert!(db.cf("users").is_none(), "users handle gone after drop");
        let orders_again = db.cf("orders").unwrap();
        assert_eq!(orders_again.get(b"o1").unwrap(), Some(b"order123".to_vec()));
        assert!(tmp.join("cf_orders").exists(), "orders directory destroyed by sibling drop");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_cf_wal_recovery() {
        // Write to a non-default CF, close the parent Database, reopen it,
        // re-attach the CF handle and verify the WAL replayed cleanly.
        let tmp = std::env::temp_dir().join("flux-db-test-cf-wal");
        let _ = fs::remove_dir_all(&tmp);
        {
            let db = Database::open(&tmp).unwrap();
            let inv = db.create_cf("inventory").unwrap();
            inv.put(b"sku-42", b"hammer").unwrap();
            inv.put(b"sku-43", b"nail").unwrap();
            inv.delete(b"sku-42").unwrap();
            // Parent + CF drop here — WAL must persist the writes.
        }
        let db = Database::open(&tmp).unwrap();
        // create_cf is idempotent (returns the existing handle/data on second open).
        let inv = db.create_cf("inventory").unwrap();
        assert_eq!(inv.get(b"sku-42").unwrap(), None, "tombstone survived WAL replay");
        assert_eq!(inv.get(b"sku-43").unwrap(), Some(b"nail".to_vec()));
        // Default CF is independent.
        assert_eq!(db.get(b"sku-43").unwrap(), None);
        let _ = fs::remove_dir_all(&tmp);
    }

    // ── v0.13 tests ──

    #[test]
    fn test_delete_range_basic() {
        let tmp = std::env::temp_dir().join("flux-db-test-dr-basic");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        for i in 0u32..20 {
            db.put(&i.to_be_bytes(), b"v").unwrap();
        }
        db.delete_range(&5u32.to_be_bytes(), &10u32.to_be_bytes()).unwrap();
        for i in 0u32..20 {
            let got = db.get(&i.to_be_bytes()).unwrap();
            if (5..10).contains(&i) {
                assert_eq!(got, None, "{i} should be tombstoned");
            } else {
                assert_eq!(got, Some(b"v".to_vec()), "{i} should survive");
            }
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_delete_range_shadows_sst() {
        let tmp = std::env::temp_dir().join("flux-db-test-dr-sst");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        for i in 0u32..10 {
            db.put(&i.to_be_bytes(), b"v").unwrap();
        }
        db.flush().unwrap();
        db.delete_range(&3u32.to_be_bytes(), &7u32.to_be_bytes()).unwrap();
        for i in 0u32..10 {
            let got = db.get(&i.to_be_bytes()).unwrap();
            if (3..7).contains(&i) {
                assert_eq!(got, None, "{i} should be tombstoned over SST");
            } else {
                assert_eq!(got, Some(b"v".to_vec()));
            }
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_iter_from_seeks() {
        let tmp = std::env::temp_dir().join("flux-db-test-iter-from");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        for c in [b"a", b"c", b"e", b"g"].iter() {
            db.put(c.as_ref(), b"v").unwrap();
        }
        db.flush().unwrap();
        let after_c: Vec<Vec<u8>> = db.iter_from(b"c").map(|(k, _)| k).collect();
        assert_eq!(after_c, vec![b"c".to_vec(), b"e".to_vec(), b"g".to_vec()]);
        let after_d: Vec<Vec<u8>> = db.iter_from(b"d").map(|(k, _)| k).collect();
        assert_eq!(after_d, vec![b"e".to_vec(), b"g".to_vec()]);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_compact_async_completes() {
        let tmp = std::env::temp_dir().join("flux-db-test-compact-async");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        for i in 0u32..200 {
            db.put(&i.to_be_bytes(), b"v").unwrap();
        }
        db.flush().unwrap();
        let handle = db.compact_async();
        let res = handle.join().expect("worker panicked");
        assert!(res.is_ok(), "compact_async returned err: {:?}", res);
        for i in 0u32..200 {
            assert!(db.get(&i.to_be_bytes()).unwrap().is_some(), "{i} lost after async compact");
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_scan_prefix_sees_persisted() {
        let tmp = std::env::temp_dir().join("flux-db-test-scan-prefix-sst");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.batch_put(&[(b"usr:1", b"alice"), (b"usr:2", b"bob")]).unwrap();
        db.flush().unwrap();
        db.put(b"usr:3", b"carol").unwrap();
        let users = db.scan_prefix(b"usr:");
        assert_eq!(users.len(), 3, "scan_prefix didn't merge SST + memtable: {users:?}");
        let _ = fs::remove_dir_all(&tmp);
    }

    // ── v0.17 (G1) WriteBatch tests ──

    #[test]
    fn test_writebatch_empty_is_noop() {
        let tmp = std::env::temp_dir().join("flux-db-test-wb-empty");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.put(b"pre", b"v").unwrap();
        let before = db.inner.read().sequence;
        let batch = WriteBatch::new();
        assert!(batch.is_empty());
        assert_eq!(batch.len(), 0);
        let committed = db.write(batch).unwrap();
        assert_eq!(committed, before, "empty batch must not bump sequence");
        assert_eq!(db.get(b"pre").unwrap(), Some(b"v".to_vec()));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_writebatch_single_cf_put_delete() {
        let tmp = std::env::temp_dir().join("flux-db-test-wb-single-cf");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.put(b"keep", b"old").unwrap();
        db.put(b"gone", b"dying").unwrap();

        let mut batch = WriteBatch::new();
        batch.put(&db, b"new1", b"alpha");
        batch.put(&db, b"new2", b"beta");
        batch.delete(&db, b"gone");
        batch.put(&db, b"keep", b"updated");
        assert_eq!(batch.len(), 4);

        let commit_seq = db.write(batch).unwrap();
        assert!(commit_seq >= 6); // 2 pre-batch puts + 4 batch ops

        assert_eq!(db.get(b"new1").unwrap(), Some(b"alpha".to_vec()));
        assert_eq!(db.get(b"new2").unwrap(), Some(b"beta".to_vec()));
        assert_eq!(db.get(b"keep").unwrap(), Some(b"updated".to_vec()));
        assert_eq!(db.get(b"gone").unwrap(), None);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_writebatch_multi_cf_atomic() {
        let tmp = std::env::temp_dir().join("flux-db-test-wb-multi-cf");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        let wallets = db.create_cf("wallets").unwrap();
        let nonces  = db.create_cf("nonces").unwrap();
        let events  = db.create_cf("events").unwrap();

        let mut batch = WriteBatch::new();
        // Simulate SIGIL state-transition: balance write + nonce bump + event log.
        batch.put(&wallets, b"alice", &100u64.to_be_bytes());
        batch.put(&wallets, b"bob",   &50u64.to_be_bytes());
        batch.put(&nonces,  b"alice", &7u64.to_be_bytes());
        batch.put(&events,  b"evt-0", b"send alice->bob 50");
        batch.delete(&db,   b"pending-tx-deadbeef"); // also touches default CF
        let _ = db.write(batch).unwrap();

        assert_eq!(wallets.get(b"alice").unwrap(), Some(100u64.to_be_bytes().to_vec()));
        assert_eq!(wallets.get(b"bob").unwrap(),   Some(50u64.to_be_bytes().to_vec()));
        assert_eq!(nonces.get(b"alice").unwrap(),  Some(7u64.to_be_bytes().to_vec()));
        assert_eq!(events.get(b"evt-0").unwrap(),  Some(b"send alice->bob 50".to_vec()));
        // Cross-CF isolation: alice in nonces is not the wallets value.
        assert_ne!(nonces.get(b"alice").unwrap(), wallets.get(b"alice").unwrap());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_writebatch_sequence_monotonic_within_cf() {
        let tmp = std::env::temp_dir().join("flux-db-test-wb-seq-mono");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        let before = db.inner.read().sequence;
        let mut batch = WriteBatch::new();
        for i in 0u32..8 {
            batch.put(&db, &i.to_be_bytes(), b"v");
        }
        let committed = db.write(batch).unwrap();
        assert_eq!(committed, before + 8, "each op must bump sequence exactly once");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_writebatch_clear_resets_ops() {
        let tmp = std::env::temp_dir().join("flux-db-test-wb-clear");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        let mut batch = WriteBatch::new();
        batch.put(&db, b"a", b"1");
        batch.put(&db, b"b", b"2");
        assert_eq!(batch.len(), 2);
        batch.clear();
        assert!(batch.is_empty());
        let committed = db.write(batch).unwrap();
        // cleared batch = no-op, so sequence is still 0
        assert_eq!(committed, 0);
        assert_eq!(db.get(b"a").unwrap(), None);
        assert_eq!(db.get(b"b").unwrap(), None);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_writebatch_concurrent_reader_sees_all_or_none() {
        // The atomicity we promise: a reader holding a Snapshot taken BEFORE
        // the batch commits sees zero batch keys; a reader taken AFTER sees
        // all of them. There's no in-between snapshot.
        let tmp = std::env::temp_dir().join("flux-db-test-wb-atomic");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();

        let before = db.snapshot();
        let mut batch = WriteBatch::new();
        for i in 0u32..32 {
            let k = format!("k{i:04}");
            batch.put(&db, k.as_bytes(), b"v");
        }
        let _ = db.write(batch).unwrap();
        let after = db.snapshot();

        // before-snapshot sees none
        let before_count = (0u32..32).filter(|i| {
            before.get(format!("k{i:04}").as_bytes()).is_some()
        }).count();
        assert_eq!(before_count, 0, "pre-batch snapshot leaked batch writes");

        // after-snapshot sees all
        let after_count = (0u32..32).filter(|i| {
            after.get(format!("k{i:04}").as_bytes()).is_some()
        }).count();
        assert_eq!(after_count, 32, "post-batch snapshot missed batch writes");
        let _ = fs::remove_dir_all(&tmp);
    }

    // ── v0.17 (G2) reverse iteration tests ──

    #[test]
    fn test_iter_rev_walks_descending() {
        let tmp = std::env::temp_dir().join("flux-db-test-iter-rev");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        for c in [b"a", b"c", b"e", b"g"].iter() {
            db.put(c.as_ref(), b"v").unwrap();
        }
        db.flush().unwrap();
        let desc: Vec<Vec<u8>> = db.iter_rev().map(|(k, _)| k).collect();
        assert_eq!(desc, vec![b"g".to_vec(), b"e".to_vec(), b"c".to_vec(), b"a".to_vec()]);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_iter_rev_via_double_ended() {
        // .rev() on forward iter() must produce the same sequence as iter_rev().
        let tmp = std::env::temp_dir().join("flux-db-test-iter-rev-de");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        for c in [b"a", b"c", b"e", b"g"].iter() {
            db.put(c.as_ref(), b"v").unwrap();
        }
        let from_iter_rev: Vec<Vec<u8>> = db.iter_rev().map(|(k, _)| k).collect();
        let from_rev: Vec<Vec<u8>> = db.iter().rev().map(|(k, _)| k).collect();
        assert_eq!(from_iter_rev, from_rev);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_iter_from_back_includes_end_if_present() {
        let tmp = std::env::temp_dir().join("flux-db-test-iter-from-back");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        for c in [b"a", b"c", b"e", b"g"].iter() {
            db.put(c.as_ref(), b"v").unwrap();
        }
        let back_from_e: Vec<Vec<u8>> = db.iter_from_back(b"e").map(|(k, _)| k).collect();
        assert_eq!(back_from_e, vec![b"e".to_vec(), b"c".to_vec(), b"a".to_vec()]);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_iter_from_back_excludes_keys_above_end() {
        let tmp = std::env::temp_dir().join("flux-db-test-iter-from-back-exclude");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        for c in [b"a", b"c", b"e", b"g"].iter() {
            db.put(c.as_ref(), b"v").unwrap();
        }
        // "d" is between c and e — should include c and a only.
        let back_from_d: Vec<Vec<u8>> = db.iter_from_back(b"d").map(|(k, _)| k).collect();
        assert_eq!(back_from_d, vec![b"c".to_vec(), b"a".to_vec()]);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_iter_from_back_empty_below_first_key() {
        let tmp = std::env::temp_dir().join("flux-db-test-iter-from-back-empty");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        for c in [b"c", b"e", b"g"].iter() {
            db.put(c.as_ref(), b"v").unwrap();
        }
        let nothing: Vec<Vec<u8>> = db.iter_from_back(b"a").map(|(k, _)| k).collect();
        assert!(nothing.is_empty());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_turbo_sync_tip_lookup_pattern() {
        // The "latest applied height" pattern that motivates G2: 8-byte BE
        // heights as keys, find the largest height present.
        let tmp = std::env::temp_dir().join("flux-db-test-tip-lookup");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        for h in [10u64, 11, 12, 15, 18, 22, 30].iter() {
            db.put(&h.to_be_bytes(), b"block-bytes").unwrap();
        }
        db.flush().unwrap();
        let latest = db.iter_rev().next().unwrap();
        let latest_height = u64::from_be_bytes(latest.0.as_slice().try_into().unwrap());
        assert_eq!(latest_height, 30);

        // "Highest height ≤ 20" — reorg-rollback pattern.
        let largest_le_20 = db.iter_from_back(&20u64.to_be_bytes()).next().unwrap();
        let h = u64::from_be_bytes(largest_le_20.0.as_slice().try_into().unwrap());
        assert_eq!(h, 18);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn background_compaction_during_writes_loses_nothing() {
        // v0.38 ASYNC-SETTLE presence audit (the chronos-v035 lesson: never trust a
        // storage change without asserting every byte comes BACK). Writes race a
        // background L0 merge in BOTH flush modes (eager + deferred/bulk); afterwards
        // every key must read back with its exact value.
        for deferred in [false, true] {
            let tmp = std::env::temp_dir().join(format!("flux-db-test-bg-compact-{deferred}"));
            let _ = fs::remove_dir_all(&tmp);
            let db = Database::open(&tmp).unwrap().with_max_wal_bytes(32 * 1024);
            db.set_defer_compaction(deferred);
            let val = |i: u32| format!("value-{i:08}").into_bytes();
            let mut written = 0u32;
            let mut saw_background = false;
            // Enough flush rounds to trip the threshold (AUTO=4 / BULK=8×) several times.
            for round in 0..40u32 {
                for _ in 0..25 {
                    db.put(format!("k{written:08}").as_bytes(), &val(written)).unwrap();
                    written += 1;
                }
                db.flush().unwrap();
                if db.compacting.load(std::sync::atomic::Ordering::SeqCst) {
                    saw_background = true;
                    // keep writing WHILE the merge runs — the race under test
                    for _ in 0..25 {
                        db.put(format!("k{written:08}").as_bytes(), &val(written)).unwrap();
                        written += 1;
                    }
                }
                let _ = round;
            }
            assert!(saw_background, "vacuous test: no background compaction observed (deferred={deferred})");
            // Wait for the in-flight merge to finish, then settle once.
            let t0 = std::time::Instant::now();
            while db.compacting.load(std::sync::atomic::Ordering::SeqCst) {
                assert!(t0.elapsed().as_secs() < 30, "background compaction hung");
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            db.flush().unwrap();
            // PRESENCE: every key readable with its exact value.
            for i in 0..written {
                let got = db.get(format!("k{i:08}").as_bytes()).unwrap();
                assert_eq!(got.as_deref(), Some(val(i).as_slice()),
                    "key k{i:08} lost/corrupt after background compaction (deferred={deferred})");
            }
            let _ = fs::remove_dir_all(&tmp);
        }
    }

    #[test]
    fn test_iter_rev_honors_range_tombstones() {
        // Range tombstones must shadow keys in reverse iteration too.
        let tmp = std::env::temp_dir().join("flux-db-test-iter-rev-tomb");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        for c in [b"a", b"c", b"e", b"g"].iter() {
            db.put(c.as_ref(), b"v").unwrap();
        }
        db.delete_range(b"c", b"f").unwrap();   // removes c, e
        let desc: Vec<Vec<u8>> = db.iter_rev().map(|(k, _)| k).collect();
        assert_eq!(desc, vec![b"g".to_vec(), b"a".to_vec()]);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_writebatch_path_sorted_lock_order_no_deadlock() {
        // Two batches touching the same two CFs in reverse order must both
        // succeed without deadlock. Path-sort enforces a stable lock order.
        let tmp = std::env::temp_dir().join("flux-db-test-wb-deadlock-free");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        let cf_a = db.create_cf("alpha").unwrap();
        let cf_b = db.create_cf("zeta").unwrap();
        let db_t1 = db.clone();
        let a_t1 = cf_a.clone();
        let b_t1 = cf_b.clone();
        let db_t2 = db.clone();
        let a_t2 = cf_a.clone();
        let b_t2 = cf_b.clone();
        let t1 = std::thread::spawn(move || {
            for i in 0u32..50 {
                let mut batch = WriteBatch::new();
                batch.put(&a_t1, &i.to_be_bytes(), b"alpha-side");
                batch.put(&b_t1, &i.to_be_bytes(), b"zeta-side");
                db_t1.write(batch).unwrap();
            }
        });
        let t2 = std::thread::spawn(move || {
            for i in 0u32..50 {
                // Stage in REVERSE order — same CFs, opposite insertion.
                let mut batch = WriteBatch::new();
                batch.put(&b_t2, &(i + 1000).to_be_bytes(), b"zeta-side-2");
                batch.put(&a_t2, &(i + 1000).to_be_bytes(), b"alpha-side-2");
                db_t2.write(batch).unwrap();
            }
        });
        t1.join().unwrap();
        t2.join().unwrap();
        // 50 keys from t1 + 50 keys from t2 = 100 in each CF.
        let alpha_count = (0u32..1050).filter(|i| cf_a.get(&i.to_be_bytes()).unwrap().is_some()).count();
        let zeta_count  = (0u32..1050).filter(|i| cf_b.get(&i.to_be_bytes()).unwrap().is_some()).count();
        assert_eq!(alpha_count, 100);
        assert_eq!(zeta_count, 100);
        let _ = fs::remove_dir_all(&tmp);
    }

    // ── v0.36: WAL auto-flush ──

    fn count_ssts(dir: &std::path::Path) -> usize {
        fs::read_dir(dir).map(|d| {
            d.flatten()
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    n.starts_with("flux_") && n.ends_with(".sst")
                })
                .count()
        }).unwrap_or(0)
    }

    #[test]
    fn test_auto_flush_truncates_wal() {
        let tmp = std::env::temp_dir().join("flux-db-test-auto-flush");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap().with_max_wal_bytes(4096);
        // 100 entries × (12B header + ~7B key + 128B value) ≈ 14.7 KB —
        // crosses the 4 KB threshold several times. Each crossing must
        // auto-flush (memtable → SST) and truncate the WAL.
        let value = [0xABu8; 128];
        for i in 0u32..100 {
            db.put(format!("key{:04}", i).as_bytes(), &value).unwrap();
        }
        // v0.38: the WAL-cap auto-flush is BACKGROUND now — wait for the in-flight
        // flush, then flush once more quiescently so the truncation is observable
        // (truncation needs a flush with no racing writes; the loop above raced them).
        let t0 = std::time::Instant::now();
        while db.flushing.load(std::sync::atomic::Ordering::SeqCst) {
            assert!(t0.elapsed().as_secs() < 30, "background auto-flush hung");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        db.flush().unwrap();
        let wal_len = tmp.join("flux.wal").metadata().unwrap().len();
        assert!(
            wal_len < 4096,
            "WAL should have been auto-truncated, but is {} bytes",
            wal_len
        );
        assert!(
            count_ssts(&tmp) >= 1,
            "auto-flush should have produced at least one SST"
        );
        // Every key must still be readable (memtable + SST read-through).
        for i in 0u32..100 {
            assert_eq!(
                db.get(format!("key{:04}", i).as_bytes()).unwrap(),
                Some(value.to_vec()),
                "key{:04} lost after auto-flush",
                i
            );
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_default_threshold_no_spurious_flush() {
        // With the 64 MB default, a few KB of writes must NOT trigger any
        // auto-flush: the WAL keeps growing and no SST appears.
        let tmp = std::env::temp_dir().join("flux-db-test-no-spurious-flush");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        let value = [0xCDu8; 128];
        for i in 0u32..100 {
            db.put(format!("key{:04}", i).as_bytes(), &value).unwrap();
        }
        let wal_len = tmp.join("flux.wal").metadata().unwrap().len();
        assert!(
            wal_len > 4096,
            "WAL should have grown past 4 KB without flushing (got {})",
            wal_len
        );
        assert_eq!(count_ssts(&tmp), 0, "no SST expected below the 64 MB default");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_auto_flush_disabled_with_zero() {
        // max_wal_bytes = 0 disables auto-flush entirely (pre-v0.36 behavior).
        let tmp = std::env::temp_dir().join("flux-db-test-auto-flush-off");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap().with_max_wal_bytes(0);
        let value = [0xEFu8; 128];
        for i in 0u32..100 {
            db.put(format!("key{:04}", i).as_bytes(), &value).unwrap();
        }
        let wal_len = tmp.join("flux.wal").metadata().unwrap().len();
        assert!(wal_len > 4096, "WAL should grow unbounded when disabled");
        assert_eq!(count_ssts(&tmp), 0);
        let _ = fs::remove_dir_all(&tmp);
    }

    // ── v0.50 (LANE-A): streaming WAL replay ────────────────────────────────────────────
    /// Encode one WAL entry exactly as the writer does:
    /// `[crc32 LE][key_len LE][val_len LE][key][val]`, CRC over `key_len++val_len++key++val`.
    fn encode_wal_entry(key: &[u8], val: &[u8]) -> Vec<u8> {
        let mut body = Vec::with_capacity(8 + key.len() + val.len());
        body.extend_from_slice(&(key.len() as u32).to_le_bytes());
        body.extend_from_slice(&(val.len() as u32).to_le_bytes());
        body.extend_from_slice(key);
        body.extend_from_slice(val);
        let crc = crc32fast::hash(&body);
        let mut out = Vec::with_capacity(4 + body.len());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn streaming_replay_matches_semantics_and_stops_on_torn_tail() {
        // last-write-wins, tombstones, an entry that straddles the 8 MiB window boundary,
        // and a torn (truncated) final entry that MUST be dropped — all byte-identical to
        // the former read_to_end slurp.
        let tmp = std::env::temp_dir().join("flux-db-test-stream-replay-semantics");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let wal = tmp.join("flux.wal");
        let mut bytes = Vec::new();
        bytes.extend(encode_wal_entry(b"a", b"1"));
        bytes.extend(encode_wal_entry(b"b", b"22"));
        bytes.extend(encode_wal_entry(b"a", b"333")); // overwrite a
        // a value that forces the window to grow past WAL_REPLAY_WINDOW for one entry.
        let big = vec![0x5Au8; WAL_REPLAY_WINDOW + 1024];
        bytes.extend(encode_wal_entry(b"big", &big));
        bytes.extend(encode_wal_entry(b"b", b"")); // tombstone b
        bytes.extend(encode_wal_entry(b"c", b"keep"));
        // torn tail: a header that claims a body longer than what follows.
        let mut torn = encode_wal_entry(b"torn", b"xxxxxxxx");
        torn.truncate(torn.len() - 4); // chop the last 4 bytes of val → truncated body
        bytes.extend(torn);
        fs::write(&wal, &bytes).unwrap();

        let mut mt: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        replay_wal_streaming(&wal, &mut mt).unwrap();

        assert_eq!(mt.get(b"a".as_slice()).map(|v| v.as_slice()), Some(b"333".as_slice()));
        assert_eq!(mt.get(b"big".as_slice()).map(|v| v.len()), Some(big.len()));
        assert!(!mt.contains_key(b"b".as_slice()), "b tombstoned");
        assert_eq!(mt.get(b"c".as_slice()).map(|v| v.as_slice()), Some(b"keep".as_slice()));
        assert!(!mt.contains_key(b"torn".as_slice()), "torn tail dropped");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[cfg(target_os = "linux")]
    fn vm_rss_kb() -> u64 {
        let s = fs::read_to_string("/proc/self/status").unwrap_or_default();
        for line in s.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                return rest.trim().trim_end_matches("kB").trim().parse().unwrap_or(0);
            }
        }
        0
    }

    /// ACCEPTANCE GATE (LANE-A.2): a synthetic ~1 GB WAL replays in bounded memory. Keys
    /// cycle over a small space so the memtable stays tiny — this isolates the READER's
    /// memory (the slurp was the OOM: it held the whole file + a ~equal memtable). A
    /// sampler thread polls VmRSS during replay; the resident GROWTH must stay < 64 MiB.
    /// The old read_to_end would have grown RSS by ~1 GB here. `#[ignore]` because it
    /// writes 1 GB to TMPDIR (set SIGIL_WAL_TEST_GB to scale); run with `--ignored`.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "writes ~1GB to TMPDIR; run explicitly with --ignored"]
    fn streaming_replay_1gb_bounded_rss() {
        use std::io::{BufWriter, Write as _};
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::Arc;

        let gb: u64 = std::env::var("SIGIL_WAL_TEST_GB").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
        let target_bytes: u64 = gb * 1024 * 1024 * 1024;
        let tmp = std::env::temp_dir().join("flux-db-test-stream-replay-1gb");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let wal = tmp.join("flux.wal");

        // ~1 KB values, keys cycling over 256 slots (last-write-wins keeps the memtable ~256 entries).
        let val = vec![0xA5u8; 1024];
        {
            let f = fs::File::create(&wal).unwrap();
            let mut w = BufWriter::with_capacity(8 * 1024 * 1024, f);
            let mut written: u64 = 0;
            let mut i: u64 = 0;
            while written < target_bytes {
                let key = format!("k{:03}", i % 256);
                let entry = encode_wal_entry(key.as_bytes(), &val);
                w.write_all(&entry).unwrap();
                written += entry.len() as u64;
                i += 1;
            }
            w.flush().unwrap();
        }
        let file_len = wal.metadata().unwrap().len();
        assert!(file_len >= target_bytes, "synthetic WAL is {} bytes", file_len);

        let stop = Arc::new(AtomicBool::new(false));
        let peak = Arc::new(AtomicU64::new(0));
        let sampler = {
            let stop = stop.clone();
            let peak = peak.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    peak.fetch_max(vm_rss_kb(), Ordering::Relaxed);
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            })
        };

        let rss_before = vm_rss_kb();
        peak.fetch_max(rss_before, Ordering::Relaxed);
        let mut mt: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        replay_wal_streaming(&wal, &mut mt).unwrap();
        stop.store(true, Ordering::Relaxed);
        sampler.join().unwrap();

        let peak_delta_kb = peak.load(Ordering::Relaxed).saturating_sub(rss_before);
        eprintln!(
            "[1gb-wal] file={} MiB  rss_before={} MiB  peak_delta={} MiB  memtable={} keys",
            file_len / 1048576, rss_before / 1024, peak_delta_kb / 1024, mt.len()
        );
        assert_eq!(mt.len(), 256, "cycling keys collapse to 256 live entries");
        assert!(
            peak_delta_kb < 64 * 1024,
            "streaming replay of a {} MiB WAL grew RSS by {} MiB (budget 64 MiB) — slurp regressed?",
            file_len / 1048576, peak_delta_kb / 1024
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    // ── v0.37 (task #10): file-backed reads + streaming k-way merge ──

    /// Deterministic incompressible value: blake3 keystream over (key, round).
    /// Incompressibility matters — lz4 collapses repetitive test values so a
    /// tiny rolling-output cap would never trip.
    fn v37_keystream(key: &str, round: u32, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
        let seed = blake3::hash(format!("{key}|{round}").as_bytes());
        let mut ctr = 0u64;
        while out.len() < len {
            let mut h = blake3::Hasher::new();
            h.update(seed.as_bytes());
            h.update(&ctr.to_le_bytes());
            out.extend_from_slice(h.finalize().as_bytes());
            ctr += 1;
        }
        out.truncate(len);
        out
    }

    /// >64 data blocks forces multiple fence groups (INDEX_FENCE_GROUP = 64),
    /// so lookups cross the sparse-index → slice-read → block-read path for
    /// every group, through the real get() (bloom + block cache).
    #[test]
    fn test_v37_fence_index_multiblock_lookup() {
        let tmp = std::env::temp_dir().join("flux-db-test-v37-fence");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        // 4 KiB values → every add crosses TARGET_BLOCK_SIZE → one block per
        // entry → 200 blocks (4 fence groups).
        let val = |i: u32| vec![(i % 251) as u8; 4096];
        for i in 0..200u32 {
            db.put(format!("key{:05}", i).as_bytes(), &val(i)).unwrap();
        }
        db.flush().unwrap();
        assert_eq!(db.len(), 0, "memtable flushed");
        for i in 0..200u32 {
            let got = db.get(format!("key{:05}", i).as_bytes()).unwrap();
            assert_eq!(got, Some(val(i)), "key{:05} must read back through the fence index", i);
        }
        assert_eq!(db.get(b"key99999").unwrap(), None);
        assert_eq!(db.get(b"aaa").unwrap(), None);
        // Cache must have warmed (index slices + data blocks).
        let (hits, misses, bytes) = db.block_cache_stats();
        assert!(bytes > 0, "block cache should hold blocks (hits={hits} misses={misses})");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Rolling outputs: a tiny output cap must split one merge into MULTIPLE
    /// key-disjoint L1 SSTs; every live key must survive, the newest write
    /// must win on duplicate keys, and tombstones must drop pairs.
    #[test]
    fn test_v37_streaming_compact_rolls_outputs() {
        let tmp = std::env::temp_dir().join("flux-db-test-v37-roll");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap().with_compact_output_bytes(16 * 1024);
        // 4 L0 SSTs (flush count stays ≤ AUTO_COMPACT_THRESHOLD — no auto-
        // compaction before the deletes land). Even rounds write keys i*5,
        // odd rounds i*5+1, so rounds 0/2 and 1/3 overlap → duplicate-key
        // resolution is exercised: round 2 must beat round 0, 3 must beat 1.
        for round in 0..4u32 {
            for i in 0..40u32 {
                let k = format!("k{:04}", i * 5 + (round % 2));
                db.put(k.as_bytes(), &v37_keystream(&k, round, 1024)).unwrap();
            }
            db.flush().unwrap();
        }
        // Tombstones ride the memtable into the L0 merge pass.
        db.delete(b"k0005").unwrap();
        db.delete(b"k0100").unwrap();
        db.compact().unwrap();

        let handles = list_ssts_leveled(&tmp).unwrap();
        let l1plus: Vec<_> = handles.iter().filter(|h| h.level >= 1).collect();
        assert!(l1plus.len() >= 2,
            "16 KiB cap over ~160 KiB of incompressible input must roll multiple outputs, got {}",
            l1plus.len());
        for h in &l1plus {
            let sz = fs::metadata(&h.path).unwrap().len();
            assert!(sz <= 64 * 1024, "output {} is {} B — cap plus one block max", h.path.display(), sz);
        }
        assert_eq!(db.get(b"k0005").unwrap(), None, "tombstone dropped the pair");
        assert_eq!(db.get(b"k0100").unwrap(), None);
        // Newest-wins across the merged outputs.
        assert_eq!(db.get(b"k0000").unwrap(), Some(v37_keystream("k0000", 2, 1024)),
            "round 2 must shadow round 0");
        assert_eq!(db.get(b"k0021").unwrap(), Some(v37_keystream("k0021", 3, 1024)),
            "round 3 must shadow round 1");
        // 80 distinct keys - 2 tombstoned.
        assert_eq!(db.iter().count(), 78, "merged view retains exactly the live key space");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// DATA-LOSS GUARD (streaming edition): an input whose stream yields a
    /// different entry count than its header key_count must ABORT the merge,
    /// keep every input file on disk, and leave no partial .sst.tmp behind.
    #[test]
    fn test_v37_streaming_guard_aborts_and_keeps_inputs() {
        let tmp = std::env::temp_dir().join("flux-db-test-v37-guard");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        for round in 0..4u32 {
            for i in 0..20u32 {
                db.put(format!("g{:03}-{}", i, round).as_bytes(), &[round as u8; 512]).unwrap();
            }
            db.flush().unwrap();
        }
        drop(db);
        // Doctor one input's header key_count (bytes 6..14) — the stream
        // parses fine but yields a different count → the guard must trip.
        let victim = list_ssts_leveled(&tmp).unwrap()
            .into_iter().find(|h| h.level == 0).expect("an L0 SST").path;
        let mut raw = fs::read(&victim).unwrap();
        raw[6..14].copy_from_slice(&999_999u64.to_le_bytes());
        fs::write(&victim, &raw).unwrap();

        let before: Vec<PathBuf> = list_ssts(&tmp).unwrap();
        let db = Database::open(&tmp).unwrap();
        // A sentinel write makes the L0 pass run (4 files ≤ threshold alone).
        db.put(b"sentinel", b"s").unwrap();
        let err = db.compact().expect_err("guard must abort the merge");
        assert!(err.contains("data-loss guard"), "unexpected error: {err}");
        let after: Vec<PathBuf> = list_ssts(&tmp).unwrap();
        assert_eq!(before, after, "every input must remain on disk after an aborted merge");
        let tmps = fs::read_dir(&tmp).unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".sst.tmp"))
            .count();
        assert_eq!(tmps, 0, "aborted merge must clean its partial outputs");
        // The sentinel survived the abort (memtable untouched on Err).
        assert_eq!(db.get(b"sentinel").unwrap(), Some(b"s".to_vec()));
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Cross-restart SST name collisions: `sequence` used to reset to 0 at
    /// open while SST filenames derive from it, so a restarted deterministic
    /// workload reproduced the same names and flush/compact RENAMED OVER the
    /// previous epoch's SSTs (kill -9 chaos measured 3 deciles destroyed).
    /// The sequence must now seed past every on-disk SST name.
    #[test]
    fn test_v37_sequence_seeds_from_ssts_across_reopen() {
        let tmp = std::env::temp_dir().join("flux-db-test-v37-seqseed");
        let _ = fs::remove_dir_all(&tmp);
        {
            let db = Database::open(&tmp).unwrap();
            for i in 0..50u32 {
                db.put(format!("e1-{i:03}").as_bytes(), &[1u8; 64]).unwrap();
            }
            db.flush().unwrap();
        }
        assert_eq!(list_ssts(&tmp).unwrap().len(), 1);
        {
            let db = Database::open(&tmp).unwrap();
            assert!(db.sequence() > 50,
                "sequence must seed past on-disk SST names, got {}", db.sequence());
            // Same put-count as epoch 1 — without seeding this flush reused
            // the exact filename and silently destroyed epoch 1's SST.
            for i in 0..50u32 {
                db.put(format!("e2-{i:03}").as_bytes(), &[2u8; 64]).unwrap();
            }
            db.flush().unwrap();
            assert_eq!(list_ssts(&tmp).unwrap().len(), 2,
                "epoch-2 flush must NOT rename over epoch-1's SST");
            for i in 0..50u32 {
                assert!(db.get(format!("e1-{i:03}").as_bytes()).unwrap().is_some(),
                    "epoch-1 key e1-{i:03} must survive the second epoch");
                assert!(db.get(format!("e2-{i:03}").as_bytes()).unwrap().is_some());
            }
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    /// READ-ORDER regression (task 717647ec): a key updated AFTER a
    /// compaction pushed its old value to L1 must read back the new L0
    /// value. The old read order (level ASC, seq ASC — `.rev()` = deepest
    /// level first) made get()/scan_prefix_recent consult L1 before L0,
    /// and the iter family applied L1 LAST in their merge — all of them
    /// returned the stale L1 copy.
    #[test]
    fn test_get_sees_l0_update_over_compacted_l1() {
        let tmp = std::env::temp_dir().join("flux-db-test-l0-over-l1");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.put(b"k:upd", b"old").unwrap();
        db.compact().unwrap(); // memtable folds into an L1 SST
        assert!(list_ssts_leveled(&tmp).unwrap().iter().any(|h| h.level == 1),
            "setup: compact must land the old value at L1");
        db.put(b"k:upd", b"new").unwrap();
        db.flush().unwrap(); // new value in a fresh L0 SST (higher sequence)

        assert_eq!(db.get(b"k:upd").unwrap(), Some(b"new".to_vec()),
            "get() must consult L0 before deeper levels");
        assert_eq!(db.scan_prefix_recent(b"k:", 10),
            vec![(b"k:upd".to_vec(), b"new".to_vec())],
            "scan_prefix_recent must apply L0 over deeper levels");
        let all: Vec<(Vec<u8>, Vec<u8>)> = db.iter().collect();
        assert_eq!(all, vec![(b"k:upd".to_vec(), b"new".to_vec())],
            "iter() must apply newest data last in its merge");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// TOMBSTONE-RESURRECTION regression: deleting a key whose live copy
    /// was compacted to L1 used to lose the delete at the next compact() —
    /// the L0 merge dropped the tombstone pair while the older L1 copy
    /// (NOT an input to that merge) stayed on disk and became visible
    /// again. The pair may only drop when the inputs are the deepest data.
    #[test]
    fn test_delete_survives_compaction_above_deeper_levels() {
        let tmp = std::env::temp_dir().join("flux-db-test-tomb-resurrect");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.put(b"zombie", b"v1").unwrap();
        db.compact().unwrap(); // "zombie" now lives in an L1 SST
        db.delete(b"zombie").unwrap();
        db.compact().unwrap(); // tombstone merges to L1 BESIDE the old copy
        assert_eq!(db.get(b"zombie").unwrap(), None,
            "tombstone must survive a merge above deeper data");
        assert_eq!(db.iter().count(), 0, "iter must not resurrect the deleted key");

        // Push the tree deep enough that the tombstone meets the old copy
        // in a deepest-level merge — there (and only there) the pair drops.
        for i in 0..20u32 {
            db.put(format!("fill{:03}", i).as_bytes(), b"x").unwrap();
            db.compact().unwrap();
        }
        assert_eq!(db.get(b"zombie").unwrap(), None, "still deleted after deep merges");
        let gone = list_ssts(&tmp).unwrap().iter().all(|p| {
            SstReader::open(p).unwrap().pairs().into_iter()
                .all(|(k, _)| k.as_slice() != b"zombie")
        });
        assert!(gone, "deepest-level merge must drop the tombstone pair entirely");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// v0.37: VANISHED-SST RACE regression (the bug behind the "SST index parse
    /// failed" alarm seen live on a Windows sigil-top node). Simulates a get()
    /// call whose cached SST-path snapshot names a file that a LATER, real
    /// compact() has since deleted (the exact interleaving async compaction
    /// makes possible: a reader's list can predate the compaction that
    /// supersedes it). Without the SST_VANISHED_RACE retry in get(), this would
    /// silently return None for a key that is very much still on disk (in the
    /// compaction's real output) -- the false negative the fix closes.
    #[test]
    fn test_get_retries_past_vanished_sst_race() {
        let tmp = std::env::temp_dir().join("flux-db-test-vanished-race");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.put(b"racy", b"v1").unwrap();
        db.compact().unwrap(); // "racy" now lives alone in one L1 SST (file A)

        let stale_list = db.cached_sst_paths().unwrap();
        assert_eq!(stale_list.len(), 1, "setup: exactly one L1 SST expected");
        let old_file = stale_list[0].clone();
        assert!(old_file.exists(), "setup: file A must exist before the race");

        // A second compaction (triggered by enough L1 growth) folds file A's
        // data into a NEW L1 output alongside fresh data, then deletes file A
        // -- this is the real, non-simulated compact() codepath.
        for i in 0..(L0_COMPACT_THRESHOLD * LEVEL_SIZE_RATIO + 1) {
            db.put(format!("fill{:04}", i).as_bytes(), b"x").unwrap();
            db.compact().unwrap();
        }
        assert!(!old_file.exists(), "setup: file A must be gone after the deeper merge");

        // Re-poison the cache with the STALE (pre-merge) list, as if THIS
        // get() call's snapshot were taken before the compaction above ran --
        // exactly the interleaving a concurrent async compact() can produce.
        *db.sst_paths.write() = Some(stale_list);

        assert_eq!(db.get(b"racy").unwrap(), Some(b"v1".to_vec()),
            "get() must not lose a key to a stale SST-list snapshot racing compaction's file removal");
    }

    #[test]
    fn test_put_many_basic_and_equivalence() {
        // put_many must be observably identical to N separate put()s.
        let tmp = std::env::temp_dir().join("flux-db-test-putmany-basic");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        let entries: Vec<(Vec<u8>, Vec<u8>)> = (0..500u32)
            .map(|i| (format!("k{:05}", i).into_bytes(), format!("v{}", i).into_bytes()))
            .collect();
        db.put_many(&entries).unwrap();
        for (k, v) in &entries {
            assert_eq!(db.get(k).unwrap(), Some(v.clone()), "put_many key {:?} missing", k);
        }
        assert_eq!(db.get(b"nope").unwrap(), None);
        // empty batch is a no-op, not an error
        db.put_many::<&[u8], &[u8]>(&[]).unwrap();
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_put_many_wal_replay() {
        // The coalesced WAL buffer must replay byte-for-byte like per-entry puts:
        // reopen from the WAL and every key survives.
        let tmp = std::env::temp_dir().join("flux-db-test-putmany-wal");
        let _ = fs::remove_dir_all(&tmp);
        {
            let db = Database::open(&tmp).unwrap();
            let entries: Vec<(Vec<u8>, Vec<u8>)> = (0..300u32)
                .map(|i| (format!("blk{:05}", i).into_bytes(), vec![i as u8; 100]))
                .collect();
            db.put_many(&entries).unwrap();
            db.sync_wal().unwrap();
        } // drop WITHOUT flush() — force recovery from the WAL alone
        {
            let db = Database::open(&tmp).unwrap();
            for i in 0..300u32 {
                let k = format!("blk{:05}", i).into_bytes();
                assert_eq!(db.get(&k).unwrap(), Some(vec![i as u8; 100]),
                    "put_many entry {} lost across WAL replay", i);
            }
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_put_many_tombstone() {
        // An empty value in put_many is a tombstone, exactly like delete().
        let tmp = std::env::temp_dir().join("flux-db-test-putmany-tomb");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        db.put(b"live", b"1").unwrap();
        db.put(b"doomed", b"2").unwrap();
        db.flush().unwrap(); // push to SST so the tombstone must shadow disk data
        let batch: Vec<(&[u8], &[u8])> = vec![(b"doomed", b""), (b"fresh", b"3")];
        db.put_many(&batch).unwrap();
        assert_eq!(db.get(b"doomed").unwrap(), None, "put_many empty value must tombstone");
        assert_eq!(db.get(b"live").unwrap(), Some(b"1".to_vec()));
        assert_eq!(db.get(b"fresh").unwrap(), Some(b"3".to_vec()));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_put_many_survives_flush() {
        // put_many entries must survive a memtable -> SST flush (read-through).
        let tmp = std::env::temp_dir().join("flux-db-test-putmany-flush");
        let _ = fs::remove_dir_all(&tmp);
        let db = Database::open(&tmp).unwrap();
        let entries: Vec<(Vec<u8>, Vec<u8>)> = (0..400u32)
            .map(|i| (format!("f{:05}", i).into_bytes(), vec![7u8; 64]))
            .collect();
        db.put_many(&entries).unwrap();
        db.flush().unwrap();
        assert_eq!(db.len(), 0, "memtable flushed");
        for i in 0..400u32 {
            let k = format!("f{:05}", i).into_bytes();
            assert_eq!(db.get(&k).unwrap(), Some(vec![7u8; 64]), "flushed put_many key {} lost", i);
        }
        let _ = fs::remove_dir_all(&tmp);
    }
}
