//! The sync core.
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

const CHUNK: usize = 1 << 20; // 1 MB

/// Result of a sync run.
#[derive(Debug, Default, Clone)]
pub struct SyncReport {
    /// Files considered.
    pub files: u64,
    /// Files copied (content differed / missing).
    pub copied: u64,
    /// Files skipped (dest already had matching content).
    pub skipped: u64,
    /// Total logical bytes.
    pub bytes_total: u64,
    /// Bytes actually written.
    pub bytes_copied: u64,
    /// Copied files that verified (hash matched after write).
    pub verify_pass: u64,
    /// Wall-clock.
    pub elapsed_ms: u128,
    /// Throughput of copied data.
    pub mbps: f64,
}

/// BLAKE3 a file by streaming (handles arbitrarily large files).
fn hash_file(p: &Path) -> io::Result<[u8; 32]> {
    let mut f = fs::File::open(p)?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 { break; }
        h.update(&buf[..n]);
    }
    Ok(*h.finalize().as_bytes())
}

/// Streaming copy that also hashes the bytes written, so the copy is verified
/// against `expect` in the same pass.
fn copy_verify(src: &Path, dst: &Path, expect: &[u8; 32]) -> io::Result<(u64, bool)> {
    if let Some(parent) = dst.parent() { fs::create_dir_all(parent)?; }
    let mut r = fs::File::open(src)?;
    let mut w = fs::File::create(dst)?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 { break; }
        w.write_all(&buf[..n])?;
        h.update(&buf[..n]);
        total += n as u64;
    }
    w.flush()?;
    Ok((total, h.finalize().as_bytes() == expect))
}

/// Recursively collect (relative path, size) for every file under `root`.
fn walk(root: &Path) -> io::Result<Vec<(PathBuf, u64)>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d)? {
            let p = e?.path();
            if p.is_dir() { stack.push(p); }
            else if p.is_file() {
                let size = p.metadata()?.len();
                let rel = p.strip_prefix(root).unwrap().to_path_buf();
                out.push((rel, size));
            }
        }
    }
    Ok(out)
}

/// Sync `src` → `dst` content-addressed, verified, parallel over `threads`.
pub fn sync(src: &Path, dst: &Path, threads: usize) -> io::Result<SyncReport> {
    let files = walk(src)?;
    fs::create_dir_all(dst)?;
    let threads = threads.max(1);
    let (files_n, bytes_total): (u64, u64) = (files.len() as u64, files.iter().map(|(_, s)| *s).sum());

    let copied = Arc::new(AtomicU64::new(0));
    let skipped = Arc::new(AtomicU64::new(0));
    let bytes_copied = Arc::new(AtomicU64::new(0));
    let verify_pass = Arc::new(AtomicU64::new(0));
    let files = Arc::new(files);
    let t0 = Instant::now();

    std::thread::scope(|s| {
        for t in 0..threads {
            let (files, copied, skipped, bytes_copied, verify_pass) =
                (files.clone(), copied.clone(), skipped.clone(), bytes_copied.clone(), verify_pass.clone());
            let (src, dst) = (src.to_path_buf(), dst.to_path_buf());
            s.spawn(move || {
                let mut i = t;
                while i < files.len() {
                    let (rel, _size) = &files[i];
                    let (sp, dp) = (src.join(rel), dst.join(rel));
                    if let Ok(scid) = hash_file(&sp) {
                        // skip iff dest content already matches (true dedup).
                        let same = dp.exists() && hash_file(&dp).map(|d| d == scid).unwrap_or(false);
                        if same {
                            skipped.fetch_add(1, Ordering::Relaxed);
                        } else if let Ok((n, ok)) = copy_verify(&sp, &dp, &scid) {
                            copied.fetch_add(1, Ordering::Relaxed);
                            bytes_copied.fetch_add(n, Ordering::Relaxed);
                            if ok { verify_pass.fetch_add(1, Ordering::Relaxed); }
                        }
                    }
                    i += threads;
                }
            });
        }
    });

    let elapsed_ms = t0.elapsed().as_millis();
    let bc = bytes_copied.load(Ordering::Relaxed);
    Ok(SyncReport {
        files: files_n,
        copied: copied.load(Ordering::Relaxed),
        skipped: skipped.load(Ordering::Relaxed),
        bytes_total,
        bytes_copied: bc,
        verify_pass: verify_pass.load(Ordering::Relaxed),
        elapsed_ms,
        mbps: (bc as f64 / 1e6) / (elapsed_ms as f64 / 1000.0).max(1e-3),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copy_then_skip_then_repair() {
        let base = std::env::temp_dir().join(format!("flux-rsync-{}", std::process::id()));
        let (src, dst) = (base.join("src"), base.join("dst"));
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.bin"), vec![1u8; 3 << 20]).unwrap(); // 3MB (multi-chunk)
        fs::write(src.join("sub/b.bin"), b"hello").unwrap();
        // first sync: both copied + verified.
        let r1 = sync(&src, &dst, 4).unwrap();
        assert_eq!(r1.copied, 2);
        assert_eq!(r1.verify_pass, 2);
        assert_eq!(fs::read(dst.join("a.bin")).unwrap().len(), 3 << 20);
        // second sync: both skipped (content matches — the dedup win).
        let r2 = sync(&src, &dst, 4).unwrap();
        assert_eq!(r2.skipped, 2);
        assert_eq!(r2.copied, 0);
        // corrupt a dest file → next sync re-copies just that one.
        fs::write(dst.join("a.bin"), b"corrupt").unwrap();
        let r3 = sync(&src, &dst, 4).unwrap();
        assert_eq!(r3.copied, 1);
        assert_eq!(r3.skipped, 1);
        let _ = fs::remove_dir_all(&base);
    }
}
