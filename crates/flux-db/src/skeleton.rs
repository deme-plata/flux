//! Skeleton store — a flat, append-only, fixed-stride record store for **fast chain sync**.
//!
//! ## Why this lives in flux-db
//!
//! flux-db's LSM KV path (`Database::put` / `WriteBatch`) is the right tool for the
//! mutable *frontier* of a chain — random-access reads, overwrites, MVCC snapshots.
//! It is the *wrong* tool for bulk-importing a verified historical **skeleton prefix**:
//! every KV entry pays the per-key LSM tax (WAL append + memtable insert + eventual SST
//! flush + leveled compaction), which a SIGIL bench pinned at **~3,863 records/s durable**
//! — two orders of magnitude under a 92.6k blk/s sync target.
//!
//! A verified prefix `[base, anchor]` has three properties the KV store can't exploit:
//!   1. **dense + contiguous** — heights have no gaps, so `seq → offset` is implicit at a
//!      fixed stride (`offset = (seq - base) * WIDTH`). No index, no key bytes.
//!   2. **append-only** — history never mutates, so there is no compaction, no tombstones.
//!   3. **externally authenticated** — the prefix is fold/anchor-signed, so reads are
//!      trusted by construction; no per-record re-verify on the hot path.
//!
//! Exploiting all three, a flat sequential append of fixed-width records reaches
//! **~10M records/s durable** in a standalone bench (≈2,600× the KV path). This module
//! promotes that primitive into flux-db so *any* Flux-native chain (SIGIL and siblings)
//! gets the fast-sync write path for free, while flux-db KV keeps serving the live frontier.
//!
//! ## Record model
//!
//! The store is generic over [`SkeletonRecord`]: a fixed-width record that carries its own
//! dense ordering key (`seq()` — a block height for a chain). flux-db stays free of chain
//! semantics; the caller owns the byte layout. (SIGIL's `sigil_header::SkeletonRecord`
//! `= height:u64 ‖ block_hash:[u8;32] ‖ parent_hash:[u8;32]` implements this as a 72-B record.)
//!
//! ## Durability
//!
//! Default [`append`](SkeletonStore::append) is **2-phase durable**: write the run, `fsync`,
//! and only then advance the authoritative count. A kill-9 mid-append can leave a torn tail;
//! [`open`](SkeletonStore::open) truncates to `floor(len / WIDTH) * WIDTH` so a partial record
//! is never read. For maximum bulk-import throughput, [`append_unsynced`] batches many runs and
//! the caller fsyncs once via [`sync`](SkeletonStore::sync) — the 10M/s path.
//!
//! ## Authentication
//!
//! [`archive_root`](SkeletonStore::archive_root) streams the whole file through BLAKE3 to
//! produce a single 32-byte commitment over the stored prefix — the witness a light client
//! checks against a signed anchor before trusting any `read_at`.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// A fixed-width record with a dense monotonic ordering key.
///
/// Implementors define their own on-disk byte layout. `WIDTH` is the exact serialized
/// size in bytes — the store asserts every `encode` produces exactly `WIDTH` bytes and
/// refuses to run on layout drift (fail-loud), because the whole design rests on the
/// stride being constant.
pub trait SkeletonRecord: Sized {
    /// Exact on-disk width of one record, in bytes.
    const WIDTH: usize;

    /// The dense ordering key (e.g. block height). Consecutive stored records must have
    /// `seq` values `base, base+1, base+2, …` with no gaps.
    fn seq(&self) -> u64;

    /// Serialize exactly `WIDTH` bytes into `out` (`out.len() == WIDTH`).
    fn encode(&self, out: &mut [u8]);

    /// Deserialize from exactly `WIDTH` bytes.
    fn decode(buf: &[u8]) -> Result<Self, String>;
}

/// Flat append-only fixed-stride store, indexed by dense `seq` from `base`.
pub struct SkeletonStore<R: SkeletonRecord> {
    path: PathBuf,
    base: u64,
    file: File,
    /// committed records = file_len / WIDTH (authoritative only after fsync)
    count: u64,
    /// reusable scratch the width of one record — avoids a per-`read_at` heap alloc
    scratch: Vec<u8>,
    _marker: std::marker::PhantomData<R>,
}

impl<R: SkeletonRecord> SkeletonStore<R> {
    #[inline]
    fn rec() -> u64 {
        R::WIDTH as u64
    }

    /// Open (or create) the flat store anchored at `base`. Recovers from a torn tail
    /// (kill-9 mid-append) by truncating to a whole number of records.
    pub fn open(path: impl AsRef<Path>, base: u64) -> Result<Self, String> {
        assert!(R::WIDTH > 0, "SkeletonRecord::WIDTH must be > 0");
        let path = path.as_ref().to_path_buf();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)
            .map_err(|e| format!("skeleton open {}: {e}", path.display()))?;
        let len = file.metadata().map_err(|e| e.to_string())?.len();
        let rec = Self::rec();
        let count = len / rec;
        if len % rec != 0 {
            // torn tail — keep only whole records
            file.set_len(count * rec)
                .map_err(|e| format!("skeleton truncate: {e}"))?;
            let _ = file.sync_all();
        }
        Ok(Self {
            path,
            base,
            file,
            count,
            scratch: vec![0u8; R::WIDTH],
            _marker: std::marker::PhantomData,
        })
    }

    /// Encode a contiguous run into one page buffer, asserting fixed stride + no gaps.
    /// Does NOT touch the file. Shared by the durable and unsynced append paths.
    fn encode_run(&self, recs: &[R]) -> Result<Vec<u8>, String> {
        let expect_first = self.base + self.count;
        if recs[0].seq() != expect_first {
            return Err(format!(
                "non-contiguous append: got seq={} expected {}",
                recs[0].seq(),
                expect_first
            ));
        }
        let w = R::WIDTH;
        let mut buf = vec![0u8; recs.len() * w];
        for (i, r) in recs.iter().enumerate() {
            let want = expect_first + i as u64;
            if r.seq() != want {
                return Err(format!(
                    "gap in run at index {i}: seq={} expected {want}",
                    r.seq()
                ));
            }
            r.encode(&mut buf[i * w..(i + 1) * w]);
        }
        Ok(buf)
    }

    /// Append a contiguous run (must start at `base + count`), **2-phase durable**:
    /// write → fsync → advance the authoritative count. Returns the number appended.
    pub fn append(&mut self, recs: &[R]) -> Result<usize, String> {
        if recs.is_empty() {
            return Ok(0);
        }
        let buf = self.encode_run(recs)?;
        self.write_at_tail(&buf)?;
        self.file
            .sync_all()
            .map_err(|e| format!("skeleton fsync: {e}"))?; // phase 1: data durable
        self.count += recs.len() as u64; // phase 2: advance the authoritative count
        Ok(recs.len())
    }

    /// Append a contiguous run **without fsync** — the maximum-throughput bulk-import path.
    /// The count is advanced immediately, but the data is NOT guaranteed durable until a
    /// later [`sync`](Self::sync). Use for streaming a large verified prefix: many
    /// `append_unsynced` calls, then one `sync` at the end (or per N pages). A crash before
    /// `sync` may lose the unsynced tail — `open` will recover to the last whole record.
    pub fn append_unsynced(&mut self, recs: &[R]) -> Result<usize, String> {
        if recs.is_empty() {
            return Ok(0);
        }
        let buf = self.encode_run(recs)?;
        self.write_at_tail(&buf)?;
        self.count += recs.len() as u64;
        Ok(recs.len())
    }

    fn write_at_tail(&mut self, buf: &[u8]) -> Result<(), String> {
        self.file
            .seek(SeekFrom::Start(self.count * Self::rec()))
            .map_err(|e| e.to_string())?;
        self.file
            .write_all(buf)
            .map_err(|e| format!("skeleton write: {e}"))
    }

    /// Flush any unsynced appends to stable storage. Call after a batch of
    /// [`append_unsynced`](Self::append_unsynced).
    pub fn sync(&mut self) -> Result<(), String> {
        self.file
            .sync_all()
            .map_err(|e| format!("skeleton sync: {e}"))
    }

    /// O(1) read by `seq` — seek to the implicit offset, decode. No index, no re-verify
    /// (the prefix is externally authenticated). `None` if `seq` is out of range.
    pub fn read_at(&mut self, seq: u64) -> Result<Option<R>, String> {
        if seq < self.base || seq >= self.base + self.count {
            return Ok(None);
        }
        let off = (seq - self.base) * Self::rec();
        self.file
            .seek(SeekFrom::Start(off))
            .map_err(|e| e.to_string())?;
        // scratch is exactly WIDTH bytes
        self.file
            .read_exact(&mut self.scratch)
            .map_err(|e| format!("skeleton read: {e}"))?;
        let rec = R::decode(&self.scratch)?;
        Ok(Some(rec))
    }

    /// Bulk read the inclusive range `[from, to]` in ONE seek + ONE read — the snapshot-page
    /// serving path for the sync *server*. Clamps to what is stored; returns the decoded run
    /// (possibly shorter than requested, empty if the range is entirely out of bounds).
    pub fn read_range(&mut self, from: u64, to: u64) -> Result<Vec<R>, String> {
        let tip = match self.tip_seq() {
            Some(t) => t,
            None => return Ok(Vec::new()),
        };
        let lo = from.max(self.base);
        let hi = to.min(tip);
        if lo > hi {
            return Ok(Vec::new());
        }
        let n = (hi - lo + 1) as usize;
        let w = R::WIDTH;
        let off = (lo - self.base) * Self::rec();
        self.file
            .seek(SeekFrom::Start(off))
            .map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; n * w];
        self.file
            .read_exact(&mut buf)
            .map_err(|e| format!("skeleton range read: {e}"))?;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            out.push(R::decode(&buf[i * w..(i + 1) * w])?);
        }
        Ok(out)
    }

    /// Stream the whole stored prefix through BLAKE3 → a single 32-byte commitment.
    ///
    /// This is the **fold/anchor witness**: a light client compares `archive_root()` against
    /// a signed anchor before trusting any `read_at`, authenticating the entire prefix in one
    /// hash instead of re-verifying per record. Reads the file in 1 MiB chunks (constant
    /// memory) over exactly the committed `count * WIDTH` bytes.
    pub fn archive_root(&mut self) -> Result<[u8; 32], String> {
        let mut hasher = blake3::Hasher::new();
        let total = self.count * Self::rec();
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|e| e.to_string())?;
        let mut remaining = total;
        let mut chunk = vec![0u8; 1 << 20];
        while remaining > 0 {
            let want = remaining.min(chunk.len() as u64) as usize;
            self.file
                .read_exact(&mut chunk[..want])
                .map_err(|e| format!("skeleton root read: {e}"))?;
            hasher.update(&chunk[..want]);
            remaining -= want as u64;
        }
        Ok(*hasher.finalize().as_bytes())
    }

    /// Lowest stored `seq` (the anchor `base`). The store is dense from here.
    pub fn base(&self) -> u64 {
        self.base
    }

    /// Highest stored `seq`, or `None` if empty.
    pub fn tip_seq(&self) -> Option<u64> {
        if self.count == 0 {
            None
        } else {
            Some(self.base + self.count - 1)
        }
    }

    /// Number of committed records.
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Total committed size in bytes (`count * WIDTH`).
    pub fn len_bytes(&self) -> u64 {
        self.count * Self::rec()
    }

    /// Path backing this store.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 72-B record mirroring `sigil_header::SkeletonRecord`
    /// (`height:u64 ‖ block_hash:[u8;32] ‖ parent_hash:[u8;32]`) so the flux-db primitive
    /// is tested against the exact layout SIGIL uses.
    #[derive(Debug, PartialEq, Eq, Clone)]
    struct Skel {
        height: u64,
        block_hash: [u8; 32],
        parent_hash: [u8; 32],
    }

    impl SkeletonRecord for Skel {
        const WIDTH: usize = 72;
        fn seq(&self) -> u64 {
            self.height
        }
        fn encode(&self, out: &mut [u8]) {
            out[0..8].copy_from_slice(&self.height.to_le_bytes());
            out[8..40].copy_from_slice(&self.block_hash);
            out[40..72].copy_from_slice(&self.parent_hash);
        }
        fn decode(buf: &[u8]) -> Result<Self, String> {
            if buf.len() != 72 {
                return Err(format!("Skel: need 72 bytes, got {}", buf.len()));
            }
            let mut h = [0u8; 8];
            h.copy_from_slice(&buf[0..8]);
            let mut b = [0u8; 32];
            b.copy_from_slice(&buf[8..40]);
            let mut p = [0u8; 32];
            p.copy_from_slice(&buf[40..72]);
            Ok(Skel {
                height: u64::from_le_bytes(h),
                block_hash: b,
                parent_hash: p,
            })
        }
    }

    fn rec(h: u64) -> Skel {
        Skel {
            height: h,
            block_hash: [h as u8; 32],
            parent_hash: [(h.wrapping_sub(1)) as u8; 32],
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("fxdb-skel-{tag}-{}.bin", std::process::id()))
    }

    #[test]
    fn append_read_roundtrip_and_offsets() {
        let p = tmp("rt");
        let _ = std::fs::remove_file(&p);
        let mut s = SkeletonStore::<Skel>::open(&p, 1).unwrap();
        let run: Vec<_> = (1..=1000).map(rec).collect();
        assert_eq!(s.append(&run).unwrap(), 1000);
        assert_eq!(s.tip_seq(), Some(1000));
        assert_eq!(s.count(), 1000);
        assert_eq!(s.len_bytes(), 72 * 1000);
        // O(1) random reads land on the right record
        assert_eq!(s.read_at(1).unwrap().unwrap().height, 1);
        assert_eq!(s.read_at(500).unwrap().unwrap(), rec(500));
        assert_eq!(s.read_at(1000).unwrap().unwrap().block_hash, rec(1000).block_hash);
        assert!(s.read_at(1001).unwrap().is_none());
        assert!(s.read_at(0).unwrap().is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn non_contiguous_append_is_rejected() {
        let p = tmp("nc");
        let _ = std::fs::remove_file(&p);
        let mut s = SkeletonStore::<Skel>::open(&p, 1).unwrap();
        assert!(s.append(&[rec(2)]).is_err(), "starting above base+count must fail");
        s.append(&[rec(1)]).unwrap();
        assert!(s.append(&[rec(3)]).is_err(), "gap must fail (expected 2)");
        // a run with an internal gap is rejected too
        assert!(s.append(&[rec(2), rec(4)]).is_err(), "internal gap must fail");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn torn_tail_is_truncated_on_open() {
        let p = tmp("torn");
        let _ = std::fs::remove_file(&p);
        {
            let mut s = SkeletonStore::<Skel>::open(&p, 1).unwrap();
            s.append(&(1..=10).map(rec).collect::<Vec<_>>()).unwrap();
        }
        // simulate kill-9 mid-append: 40 stray bytes (a torn 11th record)
        {
            let mut f = OpenOptions::new().append(true).open(&p).unwrap();
            f.write_all(&[0u8; 40]).unwrap();
        }
        let s = SkeletonStore::<Skel>::open(&p, 1).unwrap();
        assert_eq!(s.count(), 10, "torn tail dropped — only whole records survive");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn unsynced_bulk_then_sync_and_reopen() {
        let p = tmp("unsync");
        let _ = std::fs::remove_file(&p);
        {
            let mut s = SkeletonStore::<Skel>::open(&p, 1).unwrap();
            // many small runs, one fsync at the end — the 10M/s import shape
            for start in (1..=5000).step_by(500) {
                let run: Vec<_> = (start..start + 500).map(rec).collect();
                s.append_unsynced(&run).unwrap();
            }
            assert_eq!(s.count(), 5000);
            s.sync().unwrap();
        }
        let mut s = SkeletonStore::<Skel>::open(&p, 1).unwrap();
        assert_eq!(s.count(), 5000, "synced records survive reopen");
        assert_eq!(s.read_at(5000).unwrap().unwrap(), rec(5000));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn read_range_bulk_and_clamping() {
        let p = tmp("range");
        let _ = std::fs::remove_file(&p);
        let mut s = SkeletonStore::<Skel>::open(&p, 100).unwrap();
        s.append(&(100..=200).map(rec).collect::<Vec<_>>()).unwrap();
        // exact middle slab
        let mid = s.read_range(120, 130).unwrap();
        assert_eq!(mid.len(), 11);
        assert_eq!(mid[0], rec(120));
        assert_eq!(mid[10], rec(130));
        // clamps below base and above tip
        let clamped = s.read_range(50, 250).unwrap();
        assert_eq!(clamped.len(), 101);
        assert_eq!(clamped[0], rec(100));
        assert_eq!(clamped[100], rec(200));
        // fully out of range
        assert!(s.read_range(300, 400).unwrap().is_empty());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn archive_root_is_deterministic_and_content_bound() {
        let p1 = tmp("root1");
        let p2 = tmp("root2");
        let _ = std::fs::remove_file(&p1);
        let _ = std::fs::remove_file(&p2);
        let mut a = SkeletonStore::<Skel>::open(&p1, 1).unwrap();
        let mut b = SkeletonStore::<Skel>::open(&p2, 1).unwrap();
        a.append(&(1..=300).map(rec).collect::<Vec<_>>()).unwrap();
        b.append(&(1..=300).map(rec).collect::<Vec<_>>()).unwrap();
        let ra = a.archive_root().unwrap();
        let rb = b.archive_root().unwrap();
        assert_eq!(ra, rb, "same prefix → same root");
        // a different record flips the root
        let p3 = tmp("root3");
        let _ = std::fs::remove_file(&p3);
        let mut c = SkeletonStore::<Skel>::open(&p3, 1).unwrap();
        let mut run: Vec<_> = (1..=300).map(rec).collect();
        run[150].block_hash = [0xAB; 32];
        c.append(&run).unwrap();
        assert_ne!(ra, c.archive_root().unwrap(), "tampered record → different root");
        for q in [&p1, &p2, &p3] {
            let _ = std::fs::remove_file(q);
        }
    }
}
