//! flux-history — an append-only, time-ordered history DB plugin with full-text
//! search and filesystem ingestion.
//!
//! Two layers, two jobs (the whole design rests on keeping them separate):
//!
//! 1. **flux-db = the durable source of truth + the ordering.** Every entry is
//!    written under a primary key `h:<ts_be><seq_be>`, so `flux_db`'s ascending
//!    `iter_from` yields entries in **chronological order with zero re-sorting**.
//!    Secondary index keys give O(range) filtering:
//!      - `k:<kind>\0<ts_be><seq_be>` → all entries of a kind, in time order
//!      - `g:<tagkey>=<tagval>\0<ts_be><seq_be>` → all entries with a tag
//!    "Fast sort and filter and retrieval" = range scans over sorted keys, not
//!    load-everything-then-sort.
//!
//! 2. **flux-search = the derived full-text index.** A [`flux_search::SearchEngine`]
//!    (TF-IDF + ranking + snippets) is the query path for "find entries mentioning
//!    X". It is NOT authoritative — [`HistoryStore::open`] rebuilds it from flux-db,
//!    so search survives process restarts without a separate index file.
//!
//! Plus **filesystem ingestion**: [`HistoryStore::ingest_file`] /
//! [`HistoryStore::ingest_dir`] turn files on disk into searchable history rows
//! (path → source, contents → searchable content, mtime → timestamp).
//!
//! ## Use in SIGIL
//! Wrap block/tx/event/swarm-message records as [`HistoryEntry`] and `append`
//! them; the chain gets a queryable, sortable, full-text-searchable history
//! without bolting an external search service onto the node. flux-db is already
//! SIGIL's storage layer, so this is one more column-family-shaped store.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::Path;

use flux_db::Database;
use flux_search::{Document, SearchEngine, SearchQuery};
use serde::{Deserialize, Serialize};

/// Primary-key prefix: time-ordered entries.
const P_PRIMARY: &[u8] = b"h:";
/// Secondary-key prefix: by-kind index.
const P_KIND: &[u8] = b"k:";
/// Secondary-key prefix: by-tag index.
const P_TAG: &[u8] = b"g:";
/// Bookkeeping key for the monotonic sequence counter.
const K_SEQ: &[u8] = b"meta:seq";

/// One history record. `content` is what gets full-text indexed; `tags` are the
/// filterable facets (e.g. `wallet=qnk…`, `pool=PACI/QUG`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Stable id (caller-supplied, or derived from content hash if empty).
    pub id: String,
    /// Unix milliseconds. Drives chronological ordering.
    pub ts_ms: u64,
    /// Record kind — the primary filter axis ("block", "tx", "swarm-msg", "file").
    pub kind: String,
    /// Where it came from: a path, url, wallet, or tx hash.
    pub source: String,
    /// Short human title.
    pub title: String,
    /// Full text — this is what `search` matches against.
    pub content: String,
    /// Filterable facets. Sorted (BTreeMap) so secondary keys are deterministic.
    pub tags: BTreeMap<String, String>,
}

impl HistoryEntry {
    /// A new entry; if `id` is empty it's filled with a blake3 of the content.
    pub fn new(kind: impl Into<String>, source: impl Into<String>, title: impl Into<String>, content: impl Into<String>, ts_ms: u64) -> Self {
        let content = content.into();
        let mut e = HistoryEntry {
            id: String::new(),
            ts_ms,
            kind: kind.into(),
            source: source.into(),
            title: title.into(),
            content,
            tags: BTreeMap::new(),
        };
        if e.id.is_empty() {
            e.id = hex(&blake3::hash(format!("{}:{}:{}", e.kind, e.source, e.content).as_bytes()).as_bytes()[..16]);
        }
        e
    }

    /// Builder: attach a filterable tag.
    pub fn with_tag(mut self, key: impl Into<String>, val: impl Into<String>) -> Self {
        self.tags.insert(key.into(), val.into());
        self
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

/// A stored record = the entry plus the monotonic seq it was written at (the
/// tie-breaker that keeps two same-millisecond entries strictly ordered).
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Stored {
    seq: u64,
    entry: HistoryEntry,
}

/// Errors surfaced by the store. flux-db returns `String` errors; we wrap them.
#[derive(Debug)]
pub enum HistoryError {
    Db(String),
    Codec(String),
    Io(String),
}

impl std::fmt::Display for HistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HistoryError::Db(s) => write!(f, "db error: {s}"),
            HistoryError::Codec(s) => write!(f, "codec error: {s}"),
            HistoryError::Io(s) => write!(f, "io error: {s}"),
        }
    }
}
impl std::error::Error for HistoryError {}

/// Compose `prefix || ts_be(8) || seq_be(8)` — the time-sorted key shape.
fn time_key(prefix: &[u8], ts_ms: u64, seq: u64) -> Vec<u8> {
    let mut k = Vec::with_capacity(prefix.len() + 16);
    k.extend_from_slice(prefix);
    k.extend_from_slice(&ts_ms.to_be_bytes());
    k.extend_from_slice(&seq.to_be_bytes());
    k
}

/// `k:<kind>\0<ts_be><seq_be>` — by-kind index key.
fn kind_key(kind: &str, ts_ms: u64, seq: u64) -> Vec<u8> {
    let mut k = Vec::new();
    k.extend_from_slice(P_KIND);
    k.extend_from_slice(kind.as_bytes());
    k.push(0);
    k.extend_from_slice(&ts_ms.to_be_bytes());
    k.extend_from_slice(&seq.to_be_bytes());
    k
}

/// `g:<key>=<val>\0<ts_be><seq_be>` — by-tag index key.
fn tag_key(tk: &str, tv: &str, ts_ms: u64, seq: u64) -> Vec<u8> {
    let mut k = Vec::new();
    k.extend_from_slice(P_TAG);
    k.extend_from_slice(tk.as_bytes());
    k.push(b'=');
    k.extend_from_slice(tv.as_bytes());
    k.push(0);
    k.extend_from_slice(&ts_ms.to_be_bytes());
    k.extend_from_slice(&seq.to_be_bytes());
    k
}

/// The append-only history store.
pub struct HistoryStore {
    db: Database,
    search: SearchEngine,
    next_seq: u64,
}

impl HistoryStore {
    /// Open (or create) a history store at `path`. Rebuilds the in-memory search
    /// index from the persisted entries so search works immediately after a
    /// restart — flux-db is the source of truth, the index is derived.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, HistoryError> {
        let db = Database::open(path.as_ref().to_path_buf()).map_err(HistoryError::Db)?;
        let next_seq = match db.get(K_SEQ).map_err(HistoryError::Db)? {
            Some(v) if v.len() == 8 => u64::from_be_bytes(v.try_into().unwrap()),
            _ => 0,
        };
        let mut store = HistoryStore { db, search: SearchEngine::new(), next_seq };
        store.rebuild_search_index()?;
        Ok(store)
    }

    /// Open WITHOUT building the search index — returns instantly, search starts
    /// empty. The caller is expected to build the index off the hot path (e.g. a
    /// background thread via `build_detached_index` + `install_index`) so a
    /// service can bind its port immediately instead of blocking on a large
    /// re-tokenize. flux-db is still fully open: append/get/by_tag work at once;
    /// only full-text `search` is empty until the index is installed.
    pub fn open_fast(path: impl AsRef<Path>) -> Result<Self, HistoryError> {
        let db = Database::open(path.as_ref().to_path_buf()).map_err(HistoryError::Db)?;
        let next_seq = match db.get(K_SEQ).map_err(HistoryError::Db)? {
            Some(v) if v.len() == 8 => u64::from_be_bytes(v.try_into().unwrap()),
            _ => 0,
        };
        Ok(HistoryStore { db, search: SearchEngine::new(), next_seq })
    }

    /// Build a fresh search index from the persisted entries WITHOUT mutating
    /// self — takes `&self`, so it can run while readers hold a shared lock on
    /// the store (only writers are blocked). Pair with `install_index` to swap
    /// the result in under a brief exclusive lock. O(n) (bulk_load).
    pub fn build_detached_index(&self) -> Result<SearchEngine, HistoryError> {
        let mut engine = SearchEngine::new();
        let mut docs = Vec::new();
        for (_k, v) in self.db.iter_from(P_PRIMARY) {
            if !_k.starts_with(P_PRIMARY) {
                break;
            }
            let stored: Stored = serde_json::from_slice(&v).map_err(|e| HistoryError::Codec(e.to_string()))?;
            docs.push(to_document(&stored.entry));
        }
        engine.bulk_load(docs);
        Ok(engine)
    }

    /// Swap in a pre-built search index (from `build_detached_index`).
    pub fn install_index(&mut self, engine: SearchEngine) {
        self.search = engine;
    }

    /// Rebuild the flux-search index from every persisted primary entry. Called
    /// on `open`; also exposed for explicit re-index. O(n) over stored entries.
    pub fn rebuild_search_index(&mut self) -> Result<(), HistoryError> {
        let mut engine = SearchEngine::new();
        // Collect first, then bulk_load — per-doc index_document() degrades to
        // O(n²) when many stored entries share a url (e.g. millions of mining
        // events keyed by the same wallet/tag), which on a large store makes a
        // restart never finish. bulk_load dedups + rebuilds the index once = O(n).
        let mut docs = Vec::new();
        for (_k, v) in self.db.iter_from(P_PRIMARY) {
            // stop once we leave the primary keyspace
            if !_k.starts_with(P_PRIMARY) {
                break;
            }
            let stored: Stored = serde_json::from_slice(&v).map_err(|e| HistoryError::Codec(e.to_string()))?;
            docs.push(to_document(&stored.entry));
        }
        engine.bulk_load(docs);
        self.search = engine;
        Ok(())
    }

    /// Append an entry. Writes the primary (time-ordered) record + the by-kind and
    /// by-tag secondary index keys + indexes the content for full-text search.
    /// Returns the seq it was stored at.
    pub fn append(&mut self, entry: HistoryEntry) -> Result<u64, HistoryError> {
        let seq = self.next_seq;
        self.next_seq += 1;

        let stored = Stored { seq, entry: entry.clone() };
        let blob = serde_json::to_vec(&stored).map_err(|e| HistoryError::Codec(e.to_string()))?;

        // primary, time-ordered
        self.db
            .put(&time_key(P_PRIMARY, entry.ts_ms, seq), &blob)
            .map_err(HistoryError::Db)?;
        // by-kind index → points back to the primary blob (store the id, cheap)
        self.db
            .put(&kind_key(&entry.kind, entry.ts_ms, seq), entry.id.as_bytes())
            .map_err(HistoryError::Db)?;
        // by-tag indexes
        for (tk, tv) in &entry.tags {
            self.db
                .put(&tag_key(tk, tv, entry.ts_ms, seq), entry.id.as_bytes())
                .map_err(HistoryError::Db)?;
        }
        // persist seq counter
        self.db.put(K_SEQ, &self.next_seq.to_be_bytes()).map_err(HistoryError::Db)?;

        // derived full-text index
        self.search.index_document(to_document(&entry));
        Ok(seq)
    }

    /// All entries in `[start_ms, end_ms)`, chronological. O(range) — a single
    /// sorted scan, no full-table load.
    pub fn by_time_range(&self, start_ms: u64, end_ms: u64) -> Result<Vec<HistoryEntry>, HistoryError> {
        let mut out = Vec::new();
        let start = time_key(P_PRIMARY, start_ms, 0);
        for (k, v) in self.db.iter_from(&start) {
            if !k.starts_with(P_PRIMARY) {
                break;
            }
            let stored: Stored = serde_json::from_slice(&v).map_err(|e| HistoryError::Codec(e.to_string()))?;
            if stored.entry.ts_ms >= end_ms {
                break; // keys are time-sorted, so we're past the window
            }
            out.push(stored.entry);
        }
        Ok(out)
    }

    /// The most recent `limit` entries, newest first. Bounded memory: pulls only
    /// the last `limit` primary keys via [`Database::scan_prefix_recent`] instead
    /// of materializing the whole primary range (the pre-fix `iter_from(P_PRIMARY)`
    /// loaded every entry into RAM just to keep the tail — fine for a small store,
    /// an OOM for a multi-million-entry history that a `/recent` endpoint polls).
    pub fn recent(&self, limit: usize) -> Result<Vec<HistoryEntry>, HistoryError> {
        let mut out = Vec::new();
        // scan_prefix_recent returns the `limit` largest keys ascending; reverse
        // for newest-first.
        for (_k, v) in self.db.scan_prefix_recent(P_PRIMARY, limit) {
            let stored: Stored = serde_json::from_slice(&v).map_err(|e| HistoryError::Codec(e.to_string()))?;
            out.push(stored.entry);
        }
        out.reverse();
        Ok(out)
    }

    /// The most recent `limit` entries of one `kind`, chronological (oldest→newest
    /// of the recent window). Bounded memory: scans the by-kind index for only the
    /// last `limit` ids, then fetches those primaries — unlike [`by_kind`], which
    /// materializes EVERY entry of the kind (millions of mining events) regardless
    /// of how few the caller wants.
    pub fn by_kind_recent(&self, kind: &str, limit: usize) -> Result<Vec<HistoryEntry>, HistoryError> {
        let mut prefix = Vec::new();
        prefix.extend_from_slice(P_KIND);
        prefix.extend_from_slice(kind.as_bytes());
        prefix.push(0);
        let mut ids = Vec::new();
        for (k, _v) in self.db.scan_prefix_recent(&prefix, limit) {
            // recover (ts, seq) from the tail of the key → fetch the primary blob
            let tail = &k[k.len() - 16..];
            let ts = u64::from_be_bytes(tail[..8].try_into().unwrap());
            let seq = u64::from_be_bytes(tail[8..].try_into().unwrap());
            ids.push((ts, seq));
        }
        self.fetch_primaries(&ids)
    }

    /// All entries of one `kind`, chronological. O(range) over the by-kind index.
    /// WARNING: unbounded — materializes every entry of the kind. For polled
    /// "recent" views use [`by_kind_recent`] instead (this loaded millions of
    /// mining events per `/recent` poll → OOM).
    pub fn by_kind(&self, kind: &str) -> Result<Vec<HistoryEntry>, HistoryError> {
        let mut ids = Vec::new();
        let mut prefix = Vec::new();
        prefix.extend_from_slice(P_KIND);
        prefix.extend_from_slice(kind.as_bytes());
        prefix.push(0);
        for (k, _id) in self.db.iter_from(&prefix) {
            if !k.starts_with(&prefix) {
                break;
            }
            // recover (ts, seq) from the tail of the key → fetch the primary blob
            let tail = &k[k.len() - 16..];
            let ts = u64::from_be_bytes(tail[..8].try_into().unwrap());
            let seq = u64::from_be_bytes(tail[8..].try_into().unwrap());
            ids.push((ts, seq));
        }
        self.fetch_primaries(&ids)
    }

    /// All entries carrying tag `key=val`, chronological. O(range) over the tag index.
    pub fn by_tag(&self, key: &str, val: &str) -> Result<Vec<HistoryEntry>, HistoryError> {
        let mut prefix = Vec::new();
        prefix.extend_from_slice(P_TAG);
        prefix.extend_from_slice(key.as_bytes());
        prefix.push(b'=');
        prefix.extend_from_slice(val.as_bytes());
        prefix.push(0);
        let mut ids = Vec::new();
        for (k, _id) in self.db.iter_from(&prefix) {
            if !k.starts_with(&prefix) {
                break;
            }
            let tail = &k[k.len() - 16..];
            let ts = u64::from_be_bytes(tail[..8].try_into().unwrap());
            let seq = u64::from_be_bytes(tail[8..].try_into().unwrap());
            ids.push((ts, seq));
        }
        self.fetch_primaries(&ids)
    }

    fn fetch_primaries(&self, ids: &[(u64, u64)]) -> Result<Vec<HistoryEntry>, HistoryError> {
        let mut out = Vec::with_capacity(ids.len());
        for (ts, seq) in ids {
            if let Some(v) = self.db.get(&time_key(P_PRIMARY, *ts, *seq)).map_err(HistoryError::Db)? {
                let stored: Stored = serde_json::from_slice(&v).map_err(|e| HistoryError::Codec(e.to_string()))?;
                out.push(stored.entry);
            }
        }
        Ok(out)
    }

    /// Full-text search over entry content (TF-IDF + ranking via flux-search).
    /// Returns up to `limit` results, best-scoring first.
    pub fn search(&mut self, query: &str, limit: usize) -> Vec<flux_search::SearchResult> {
        let q = SearchQuery { q: query.to_string(), page: 1, per_page: limit.max(1), ..Default::default() };
        self.search.search(q).results
    }

    /// Number of persisted primary entries.
    pub fn len(&self) -> usize {
        self.db.iter_from(P_PRIMARY).take_while(|(k, _)| k.starts_with(P_PRIMARY)).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    // ── filesystem ingestion ──────────────────────────────────────────────────

    /// Ingest a single file as a `kind="file"` history entry: path → source,
    /// contents → searchable content, mtime → ts. Non-UTF8 files are skipped.
    pub fn ingest_file(&mut self, path: impl AsRef<Path>) -> Result<Option<u64>, HistoryError> {
        let path = path.as_ref();
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Ok(None), // skip binary / unreadable
        };
        let ts_ms = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let title = path.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_string();
        let entry = HistoryEntry::new("file", path.to_string_lossy(), title, content, ts_ms)
            .with_tag("ext", ext);
        Ok(Some(self.append(entry)?))
    }

    /// Ingest every readable file under `dir` (recursive). Returns how many were
    /// indexed. Skips unreadable/binary files. The whole tree becomes searchable
    /// + filterable history — "integrate it with the filesystem for fast sort
    /// and filter and retrieval."
    pub fn ingest_dir(&mut self, dir: impl AsRef<Path>) -> Result<usize, HistoryError> {
        let mut count = 0;
        let mut stack = vec![dir.as_ref().to_path_buf()];
        while let Some(d) = stack.pop() {
            let rd = match std::fs::read_dir(&d) {
                Ok(r) => r,
                Err(e) => return Err(HistoryError::Io(e.to_string())),
            };
            for ent in rd.flatten() {
                let p = ent.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.is_file() && self.ingest_file(&p)?.is_some() {
                    count += 1;
                }
            }
        }
        Ok(count)
    }
}

/// Map a history entry to a flux-search `Document`. `url` carries the unique id
/// (flux-search keys documents by url), `category` carries the kind so search
/// callers can post-filter, `last_crawled` carries the timestamp.
fn to_document(e: &HistoryEntry) -> Document {
    Document {
        id: e.id.clone(),
        url: format!("hist://{}/{}", e.kind, e.id),
        title: e.title.clone(),
        content: e.content.clone(),
        meta_description: Some(e.source.clone()),
        language: None,
        category: Some(e.kind.clone()),
        page_rank: 0.0,
        readability_score: 0.0,
        word_count: e.content.split_whitespace().count(),
        last_crawled: Some(e.ts_ms),
        content_hash: hex(&blake3::hash(e.content.as_bytes()).as_bytes()[..16]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (HistoryStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let s = HistoryStore::open(dir.path().join("hist")).unwrap();
        (s, dir)
    }

    fn entry(kind: &str, title: &str, content: &str, ts: u64) -> HistoryEntry {
        HistoryEntry::new(kind, "test", title, content, ts)
    }

    #[test]
    fn append_and_recent_is_newest_first() {
        let (mut s, _d) = store();
        s.append(entry("block", "b1", "first block mined", 1000)).unwrap();
        s.append(entry("block", "b2", "second block mined", 2000)).unwrap();
        s.append(entry("tx", "t1", "a payment tx", 3000)).unwrap();
        let recent = s.recent(2).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].title, "t1", "newest first");
        assert_eq!(recent[1].title, "b2");
    }

    #[test]
    fn time_range_is_a_sorted_window() {
        let (mut s, _d) = store();
        for ts in [100u64, 200, 300, 400, 500] {
            s.append(entry("e", &format!("e{ts}"), "x", ts)).unwrap();
        }
        let win = s.by_time_range(200, 400).unwrap();
        assert_eq!(win.len(), 2, "[200,400) excludes 400");
        assert_eq!(win[0].ts_ms, 200);
        assert_eq!(win[1].ts_ms, 300);
    }

    #[test]
    fn filter_by_kind() {
        let (mut s, _d) = store();
        s.append(entry("block", "b", "blk", 1)).unwrap();
        s.append(entry("tx", "t", "txn", 2)).unwrap();
        s.append(entry("block", "b2", "blk2", 3)).unwrap();
        let blocks = s.by_kind("block").unwrap();
        assert_eq!(blocks.len(), 2);
        assert!(blocks.iter().all(|e| e.kind == "block"));
        assert_eq!(s.by_kind("tx").unwrap().len(), 1);
    }

    #[test]
    fn filter_by_tag() {
        let (mut s, _d) = store();
        s.append(entry("tx", "t1", "pay", 1).with_tag("wallet", "qnkAAA")).unwrap();
        s.append(entry("tx", "t2", "pay", 2).with_tag("wallet", "qnkBBB")).unwrap();
        s.append(entry("tx", "t3", "pay", 3).with_tag("wallet", "qnkAAA")).unwrap();
        let aaa = s.by_tag("wallet", "qnkAAA").unwrap();
        assert_eq!(aaa.len(), 2);
        assert!(aaa.iter().all(|e| e.tags.get("wallet").map(|v| v == "qnkAAA").unwrap_or(false)));
    }

    #[test]
    fn full_text_search_finds_content() {
        let (mut s, _d) = store();
        s.append(entry("note", "a", "the quick brown fox jumps", 1)).unwrap();
        s.append(entry("note", "b", "lazy dog sleeping", 2)).unwrap();
        let hits = s.search("fox", 10);
        assert!(!hits.is_empty(), "should find the fox note");
        assert!(hits[0].title == "a", "fox note ranks first");
    }

    #[test]
    fn search_index_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hist");
        {
            let mut s = HistoryStore::open(&path).unwrap();
            s.append(entry("note", "persisted", "searchable after restart", 1)).unwrap();
        }
        // reopen: index must be rebuilt from flux-db
        let mut s2 = HistoryStore::open(&path).unwrap();
        assert_eq!(s2.len(), 1, "entry persisted");
        let hits = s2.search("searchable", 10);
        assert!(!hits.is_empty(), "search works after reopen (index rebuilt from db)");
    }

    #[test]
    fn seq_counter_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hist");
        {
            let mut s = HistoryStore::open(&path).unwrap();
            s.append(entry("e", "a", "x", 1)).unwrap();
            s.append(entry("e", "b", "x", 1)).unwrap(); // same ts → seq breaks tie
        }
        let mut s2 = HistoryStore::open(&path).unwrap();
        let seq = s2.append(entry("e", "c", "x", 1)).unwrap();
        assert_eq!(seq, 2, "seq continues from persisted counter, no key collision");
        assert_eq!(s2.len(), 3, "all three distinct despite identical ts");
    }

    #[test]
    fn ingest_dir_indexes_files() {
        let (mut s, _d) = store();
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("a.txt"), "alpha content here").unwrap();
        std::fs::write(src.path().join("b.md"), "beta markdown doc").unwrap();
        std::fs::create_dir(src.path().join("sub")).unwrap();
        std::fs::write(src.path().join("sub/c.rs"), "fn gamma() {}").unwrap();
        let n = s.ingest_dir(src.path()).unwrap();
        assert_eq!(n, 3, "recursive: 3 files indexed");
        assert_eq!(s.by_kind("file").unwrap().len(), 3);
        // searchable by content
        assert!(!s.search("markdown", 10).is_empty());
        // filterable by extension tag
        assert_eq!(s.by_tag("ext", "rs").unwrap().len(), 1);
    }

    #[test]
    fn same_ts_entries_stay_distinct_and_ordered() {
        let (mut s, _d) = store();
        // three entries at the identical millisecond
        s.append(entry("e", "first", "x", 5000)).unwrap();
        s.append(entry("e", "second", "x", 5000)).unwrap();
        s.append(entry("e", "third", "x", 5000)).unwrap();
        let all = s.by_time_range(5000, 5001).unwrap();
        assert_eq!(all.len(), 3, "seq tie-breaker keeps all three");
        assert_eq!(all[0].title, "first");
        assert_eq!(all[2].title, "third");
    }

    #[test]
    fn empty_store_is_empty() {
        let (s, _d) = store();
        assert!(s.is_empty());
        assert_eq!(s.recent(10).unwrap().len(), 0);
    }
}
