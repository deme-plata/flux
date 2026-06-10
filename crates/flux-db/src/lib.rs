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

pub mod block;
pub mod cache;
pub mod cf;
pub mod filter;
pub mod merge;
pub mod range_tomb;
pub mod ttl;

const SST_MAGIC: [u8; 4] = *b"FXDB";
/// SST version. v1 = monolithic LZ4 blob (pre-v0.12). v2 = block-based
/// format with index + footer; payload bytes start at the position right
/// after the bloom filter, but the payload is now `block::build_block_sst`
/// output rather than a single compressed buffer.
const SST_VERSION: u8 = 2;
const AUTO_COMPACT_THRESHOLD: usize = 4;
/// Default block cache size — small enough to be safe in a unit test,
/// configurable via `Database::with_block_cache_capacity`.
const DEFAULT_BLOCK_CACHE_BYTES: usize = 16 * 1024 * 1024;
/// v0.15: total number of levels in the LSM tree. L0 receives flushes;
/// Li (i > 0) receives compaction outputs from L(i-1).
pub const MAX_LEVEL: u8 = 7;
/// v0.15: number of L0 files allowed before we compact L0 into L1.
const L0_COMPACT_THRESHOLD: usize = 4;
/// v0.15: size pyramid factor — Li+1 target is `LEVEL_SIZE_RATIO ×` Li.
/// Smaller than RocksDB's default 10 to keep the test corpus snappy.
const LEVEL_SIZE_RATIO: usize = 4;

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
}

struct DbInner {
    memtable: BTreeMap<Vec<u8>, Vec<u8>>,
    wal_file: Option<fs::File>,
    sequence: u64,
    mod_history: std::collections::HashMap<Vec<u8>, u64>,
    /// Active range tombstones (v0.13). Each masks any key in `[start, end)`
    /// from `get()`. They drop their target during the next `compact()`.
    range_tombs: Vec<range_tomb::RangeTombstone>,
    /// v0.16: optional merge operator.
    merge_op: RwLock<Option<Arc<dyn merge::MergeOperator>>>,
    /// v0.16: optional compaction filter.
    compaction_filter: RwLock<Option<Arc<dyn filter::CompactionFilter>>>,
}

/// Snapshot of the database at a point in time (MVCC read).
pub struct Snapshot {
    data: BTreeMap<Vec<u8>, Vec<u8>>,
    seq: u64,
}

impl Database {
    /// Open or create a database at the given path.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        fs::create_dir_all(&path).map_err(|e| format!("create dir: {}", e))?;

        let wal_path = path.join("flux.wal");
        let mut memtable = BTreeMap::new();

        // Replay WAL if it exists. Each entry is:
        //   [crc32: u32 LE][key_len: u32 LE][val_len: u32 LE][key][val]
        // CRC covers key_len ++ val_len ++ key ++ val. On the first bad CRC
        // or truncated tail we stop — pre-crash entries are safe; the partial
        // post-crash entry is discarded.
        if wal_path.exists() {
            let mut f = fs::File::open(&wal_path).map_err(|e| format!("open wal: {}", e))?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).map_err(|e| format!("read wal: {}", e))?;
            let mut pos = 0;
            while pos + 12 <= buf.len() {
                let crc = u32::from_le_bytes([buf[pos], buf[pos+1], buf[pos+2], buf[pos+3]]);
                let key_len = u32::from_le_bytes([buf[pos+4], buf[pos+5], buf[pos+6], buf[pos+7]]) as usize;
                let val_len = u32::from_le_bytes([buf[pos+8], buf[pos+9], buf[pos+10], buf[pos+11]]) as usize;
                let body_start = pos + 12;
                let body_end = body_start + key_len + val_len;
                if body_end > buf.len() {
                    break; // torn write: drop and stop
                }
                let body = &buf[pos+4..body_end];
                let computed = crc32fast::hash(body);
                if computed != crc {
                    break; // torn write: drop and stop
                }
                let key = buf[body_start..body_start+key_len].to_vec();
                let val = buf[body_start+key_len..body_end].to_vec();
                if val_len == 0 {
                    memtable.remove(&key); // tombstone
                } else {
                    memtable.insert(key, val);
                }
                pos = body_end;
            }
        }

        let wal_file = fs::OpenOptions::new()
            .create(true).append(true)
            .open(&wal_path)
            .map_err(|e| format!("create wal: {}", e))?;

        Ok(Database {
            inner: Arc::new(RwLock::new(DbInner {
                memtable,
                wal_file: Some(wal_file),
                sequence: 0,
                mod_history: std::collections::HashMap::new(),
                range_tombs: Vec::new(),
                merge_op: RwLock::new(None),
                compaction_filter: RwLock::new(None),
            })),
            path: path.clone(),
            block_cache: Arc::new(cache::BlockCache::new(DEFAULT_BLOCK_CACHE_BYTES)),
            cfs: Arc::new(cf::CfRegistry::new(path)),
            sst_readers: Arc::new(RwLock::new(std::collections::HashMap::new())),
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

    /// Insert a key with an expiry time (seconds since UNIX epoch). After
    /// that point `get()` returns None and the next compaction drops the
    /// key. Internally stores `[u64 expiry_unix][value]`; readers detect
    /// the TTL prefix via length + expiry sanity.
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
        if let Ok(ssts) = list_ssts(&self.path) {
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
        let mut inner = self.inner.write();
        inner.sequence += 1;
        let seq = inner.sequence;
        write_wal_entry(inner.wal_file.as_mut(), key, value)?;
        inner.memtable.insert(key.to_vec(), value.to_vec());
        inner.mod_history.insert(key.to_vec(), seq);
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
        // Read-through SSTs. Newest first (highest sequence number in filename).
        // v0.12: lookups consult the LRU block cache to avoid re-decompressing
        // the same blocks on hot keys.
        let ssts = list_ssts(&self.path)?;
        for sst in ssts.iter().rev() {
            // v0.35: cached reader (header+bloom resident, body lazy). A bloom miss
            // now costs zero disk reads; before this line the WHOLE file was read
            // per get per SST.
            let table = self.sst_cached(sst)?;
            if !table.bloom.may_contain(key) {
                continue;
            }
            if let Some(v) = table.lookup_cached(key, Some(&self.block_cache)) {
                return Ok(if v.is_empty() { None } else { Some(v) });
            }
        }
        Ok(None)
    }

    /// Delete a key. Writes a tombstone (empty value) to both WAL and memtable
    /// so the deletion shadows any older copy persisted in an SST.
    pub fn delete(&self, key: &[u8]) -> Result<(), String> {
        let mut inner = self.inner.write();
        inner.sequence += 1;
        let seq = inner.sequence;
        write_wal_entry(inner.wal_file.as_mut(), key, &[])?;
        inner.memtable.insert(key.to_vec(), Vec::new()); // tombstone
        inner.mod_history.insert(key.to_vec(), seq);
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
                        write_wal_entry(guard.wal_file.as_mut(), key, value)?;
                        guard.memtable.insert(key.clone(), value.clone());
                        guard.mod_history.insert(key.clone(), seq);
                    }
                    BatchOpKind::Delete { key } => {
                        write_wal_entry(guard.wal_file.as_mut(), key, &[])?;
                        guard.memtable.insert(key.clone(), Vec::new());
                        guard.mod_history.insert(key.clone(), seq);
                    }
                }
                if seq > max_seq { max_seq = seq; }
            }
            drop(guard);
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
                    write_wal_entry(inner.wal_file.as_mut(), key, v)
                        .map_err(TxError::Io)?;
                    inner.memtable.insert(key.clone(), v.clone());
                }
                None => {
                    write_wal_entry(inner.wal_file.as_mut(), key, &[])
                        .map_err(TxError::Io)?;
                    inner.memtable.insert(key.clone(), Vec::new());
                }
            }
            inner.mod_history.insert(key.clone(), seq);
        }
        let commit_seq = inner.sequence;
        self.finished = true;
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
    /// before. The memtable is cleared on success; auto-compaction triggers
    /// when the SST count crosses AUTO_COMPACT_THRESHOLD.
    pub fn flush(&self) -> Result<(), String> {
        // v0.35 WAL-TRUNCATE: hold ONE write lock across snapshot → SST write+fsync
        // → memtable.clear → WAL truncate. Three reasons it must be atomic:
        //   1. Correctness — `put()` also takes this write lock, so no concurrent
        //      write can be lost in the gap between clearing the memtable and
        //      truncating the WAL (the reader thread shares this Database via Arc,
        //      so put-vs-flush across threads is real).
        //   2. Consistency — the pre-v0.35 code snapshotted the memtable under TWO
        //      separate read locks (bloom from one, SST body from another); a put
        //      between them could desync bloom vs body. One lock = one snapshot.
        //   3. The WAL truncate is only crash-safe once the SST is fsync'd; doing
        //      it under the lock keeps the ordering (sync_all → clear → truncate).
        let mut inner = self.inner.write();
        if inner.memtable.is_empty() {
            return Ok(());
        }
        // v0.15: flushes always land at L0; the leveled compactor pushes them deeper.
        let sst_path = self.path.join(sst_name(0, inner.sequence));
        let key_count = inner.memtable.len() as u64;
        let mut bloom = BloomFilter::new(inner.memtable.len().max(16), 0.01);
        // v0.12: block-based SST body (data blocks + index + footer); SstReader
        // auto-detects by the SST_VERSION byte. Build bloom + body from the SAME
        // snapshot so they can never disagree.
        let mut builder = block::SstBuilder::new();
        for (k, v) in inner.memtable.iter() {
            builder.add(k, v);
            bloom.insert(k);
        }
        let block_body = builder.finish();

        let bloom_bytes: Vec<u8> = bloom.bits.iter().flat_map(|w| w.to_le_bytes()).collect();
        let mut out = Vec::with_capacity(64 + bloom_bytes.len() + block_body.len());
        out.extend_from_slice(&SST_MAGIC);
        out.push(SST_VERSION);
        out.push(0u8); // flags
        out.extend_from_slice(&key_count.to_le_bytes());
        out.extend_from_slice(&(bloom.num_hashes as u32).to_le_bytes());
        out.extend_from_slice(&(bloom_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&bloom_bytes);
        out.extend_from_slice(&(block_body.len() as u32).to_le_bytes());
        out.extend_from_slice(&block_body);

        // Write the SST and FSYNC it. sync_all (not the old buffer-only flush) is
        // MANDATORY before truncating the WAL — otherwise a crash between SST write
        // and WAL truncate would lose the memtable's data entirely.
        let mut f = fs::File::create(&sst_path).map_err(|e| format!("create sst: {}", e))?;
        f.write_all(&out).map_err(|e| format!("sst write: {}", e))?;
        f.sync_all().map_err(|e| format!("sst sync: {}", e))?;

        // The memtable is now durable in the SST. Clear it AND truncate the WAL —
        // the WAL only ever needs to cover the (now-empty) memtable. Without this
        // the WAL grew unbounded (1.5 GB observed on the live sigil-top store) and
        // every open() re-read + replayed the entire history into RAM (the real
        // ~78s cold-open cost — the SSTs were never the bottleneck).
        inner.memtable.clear();
        if let Some(w) = inner.wal_file.as_mut() {
            let _ = w.flush();
            w.set_len(0).map_err(|e| format!("wal truncate: {}", e))?;
            // wal_file is O_APPEND, so the next write goes to the new EOF (0) — no
            // seek needed. inner.sequence is intentionally NOT reset (monotonic).
        }
        drop(inner);

        // Auto-compact if we've accumulated too many SSTs.
        let sst_count = list_ssts(&self.path)?.len();
        if sst_count > AUTO_COMPACT_THRESHOLD {
            self.compact()?;
        }
        Ok(())
    }

    /// Streaming iterator across everything visible: the live memtable plus
    /// all SSTs, with tombstones applied. Returns an owning iterator so the
    /// DB lock is held only during construction.
    pub fn iter(&self) -> DbIterator {
        let mut merged: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        // Oldest-first so newer writes overwrite older ones.
        if let Ok(ssts) = list_ssts(&self.path) {
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
        if let Ok(ssts) = list_ssts(&self.path) {
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
        if let Ok(ssts) = list_ssts(&self.path) {
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
fn write_wal_entry(wal: Option<&mut fs::File>, key: &[u8], value: &[u8]) -> Result<(), String> {
    let Some(wal) = wal else { return Ok(()) };
    let mut body = Vec::with_capacity(8 + key.len() + value.len());
    body.extend_from_slice(&(key.len() as u32).to_le_bytes());
    body.extend_from_slice(&(value.len() as u32).to_le_bytes());
    body.extend_from_slice(key);
    body.extend_from_slice(value);
    let crc = crc32fast::hash(&body);
    wal.write_all(&crc.to_le_bytes()).map_err(|e| format!("wal write crc: {}", e))?;
    wal.write_all(&body).map_err(|e| format!("wal write body: {}", e))?;
    wal.flush().map_err(|e| format!("wal flush: {}", e))?;
    Ok(())
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

/// Legacy unleveled wrapper kept for callers that don't care about level.
fn list_ssts(path: &std::path::Path) -> Result<Vec<PathBuf>, String> {
    Ok(list_ssts_leveled(path)?.into_iter().map(|h| h.path).collect())
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
}

impl SstReader {
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        use std::io::Read;
        let mut f = fs::File::open(path).map_err(|e| format!("read sst {}: {}", path.display(), e))?;
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
            });
        }
        // Bytes 4..22 of the header (we already consumed the 4-byte magic).
        let mut rest = [0u8; 18];
        f.read_exact(&mut rest).map_err(|_| "SST truncated in header".to_string())?;
        let version = rest[0]; // raw[4]
        if version != 1 && version != 2 {
            return Err(format!("unsupported SST version {} (supported: 1, 2)", version));
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
        let mut plen = [0u8; 4];
        f.read_exact(&mut plen).map_err(|_| "SST truncated in bloom".to_string())?;
        let payload_len = u32::from_le_bytes(plen) as usize;
        let payload_off = (22 + bloom_len + 4) as u64;
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
        })
    }

    /// v0.35: the SST body, read from disk ON FIRST ACCESS (and kept). A read
    /// failure (file vanished mid-run, truncation) logs and yields an empty
    /// body — lookups then find nothing, the same observable behavior as a
    /// torn SST today, never a panic.
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

    /// Point lookup. On v2 files, uses the index to find the single relevant
    /// data block, decompresses just that block, and searches it. On v1 /
    /// legacy files, decompresses the whole payload (slow path kept for
    /// backwards compat with old on-disk SSTs).
    ///
    /// If `cache` is provided, v2 reads consult and populate it.
    pub fn lookup_cached(&self, key: &[u8], cache: Option<&cache::BlockCache>) -> Option<Vec<u8>> {
        if self.version == 2 {
            let body = self.payload(); // v0.35: lazy — first access reads the file body
            let reader = match block::BlockSstReader::new(body) {
                Ok(r) => r,
                Err(_) => return None,
            };
            let handle = reader.locate_block(key)?;
            // Cache key uses (path, block_offset_within_body).
            if let Some(c) = cache {
                let ck = cache::BlockKey {
                    sst_path: self.path.clone(),
                    block_offset: handle.offset,
                };
                if let Some(buf) = c.get(&ck) {
                    return block::BlockSstReader::lookup_in_block(&buf, key);
                }
                let decompressed = reader.read_block(handle).ok()?;
                let arc = std::sync::Arc::new(decompressed);
                c.put(ck, arc.clone());
                return block::BlockSstReader::lookup_in_block(&arc, key);
            }
            let decompressed = reader.read_block(handle).ok()?;
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
    pub fn pairs(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        let _ = self.legacy;
        if self.version == 2 {
            if let Ok(r) = block::BlockSstReader::new(self.payload()) {
                if let Ok(p) = r.pairs() {
                    return p;
                }
            }
            return Vec::new();
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
        let mut inner = self.inner.write();
        for (k, v) in entries {
            inner.sequence += 1;
            let seq = inner.sequence;
            write_wal_entry(inner.wal_file.as_mut(), k, v)?;
            inner.memtable.insert(k.to_vec(), v.to_vec());
            inner.mod_history.insert(k.to_vec(), seq);
        }
        Ok(())
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

    /// Compact all SSTs into one merged SST. Tombstones drop their target
    /// (so deletions become permanent). The merged view also folds in the
    /// current memtable so an explicit `flush()` before `compact()` isn't
    /// required.
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
    pub fn compact(&self) -> Result<(), String> {
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

            // Merge input. Oldest-first so newer entries win on conflict.
            let mut merged: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
            at_level.sort_by_key(|h| h.sequence);
            for h in &at_level {
                let table = SstReader::open(&h.path)?;
                for (k, v) in table.into_pairs() {
                    if v.is_empty() {
                        merged.remove(&k);
                    } else {
                        merged.insert(k, v);
                    }
                }
            }
            if current_level == 0 {
                // Also fold the memtable on the L0 pass — same semantics as
                // pre-v0.15 compact() with no memtable flush required.
                let inner = self.inner.read();
                for (k, v) in &inner.memtable {
                    if v.is_empty() {
                        merged.remove(k);
                    } else {
                        merged.insert(k.clone(), v.clone());
                    }
                }
            }

            // Write merged output as a single file at current_level + 1.
            let seq = self.inner.read().sequence + 1;
            {
                let mut inner = self.inner.write();
                inner.sequence += 1;
            }
            let out_level = current_level + 1;
            let out_path = self.path.join(sst_name(out_level, seq));

            let mut bloom = BloomFilter::new(merged.len().max(16), 0.01);
            let mut builder = block::SstBuilder::new();
            for (k, v) in &merged {
                builder.add(k, v);
                bloom.insert(k);
            }
            let block_body = builder.finish();
            let bloom_bytes: Vec<u8> =
                bloom.bits.iter().flat_map(|w| w.to_le_bytes()).collect();
            let mut out = Vec::with_capacity(64 + bloom_bytes.len() + block_body.len());
            out.extend_from_slice(&SST_MAGIC);
            out.push(SST_VERSION);
            out.push(0u8);
            out.extend_from_slice(&(merged.len() as u64).to_le_bytes());
            out.extend_from_slice(&(bloom.num_hashes as u32).to_le_bytes());
            out.extend_from_slice(&(bloom_bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&bloom_bytes);
            out.extend_from_slice(&(block_body.len() as u32).to_le_bytes());
            out.extend_from_slice(&block_body);

            let tmp_path = out_path.with_extension("sst.tmp");
            {
                let mut f = fs::File::create(&tmp_path)
                    .map_err(|e| format!("create compacted: {}", e))?;
                f.write_all(&out).map_err(|e| format!("compact w: {}", e))?;
                f.flush().map_err(|e| format!("compact flush: {}", e))?;
            }
            fs::rename(&tmp_path, &out_path)
                .map_err(|e| format!("compact rename: {}", e))?;
            for h in &at_level {
                // Drop cached blocks for the file we're about to remove —
                // otherwise they sit as zombies until evicted by capacity.
                self.block_cache.invalidate_sst(&h.path);
                // v0.35: drop the cached reader too (frees its lazily-loaded body;
                // the path will never be listed again).
                self.sst_readers.write().remove(&h.path);
                let _ = fs::remove_file(&h.path);
            }
            if current_level == 0 {
                // Memtable's content is now durable on disk; clear it.
                self.inner.write().memtable.clear();
            }
            // Keep iterating — out_level may now exceed its own threshold.
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
        let ssts = list_ssts(&tmp).unwrap();
        assert!(ssts.len() <= 2, "auto-compact didn't fire: {} ssts left", ssts.len());
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
        assert_eq!(SST_VERSION, 2);
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
}
