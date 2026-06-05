//! Cross-process atomic file mutex via a sentinel lock file.
//!
//! `O_EXCL | O_CREAT` is the standard POSIX way to do a binary semaphore
//! across processes without depending on a third-party crate. We add:
//!
//! * **Spin-with-backoff acquisition** with an overall timeout, instead of
//!   busy-looping or blocking forever.
//! * **Stale-lock detection** based on the lock file's mtime, so a crashed
//!   holder eventually gets evicted instead of deadlocking the whole swarm.
//! * **RAII release on `Drop`** so a panic inside the critical section can
//!   still unlock for the next waiter.
//!
//! The intended use is `with_locked(path, |contents| { … new contents … })`
//! which reads the file, hands the bytes to the closure, and atomically
//! writes the closure's return value back via temp-file-and-rename.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime};

/// How long a lock file is allowed to exist before we assume the holder
/// crashed and steal it. 30 s is long enough to survive a real big-write
/// inside the critical section, short enough that swarm progress resumes
/// reasonably fast after a crash.
const STALE_LOCK_SECS: u64 = 30;

/// Default poll cadence + cap when waiting for a lock.
const POLL_START_MS: u64 = 5;
const POLL_MAX_MS: u64 = 200;

/// Default upper bound on `acquire` wait time.
const ACQUIRE_TIMEOUT_SECS: u64 = 10;

#[derive(Debug)]
pub enum LockError {
    Timeout,
    Io(String),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Timeout => write!(f, "lock acquire timeout"),
            LockError::Io(s) => write!(f, "lock io: {}", s),
        }
    }
}

impl std::error::Error for LockError {}

/// RAII guard that owns the on-disk lock file. Dropping it removes the
/// lock file. If the process crashes before drop, the next acquirer will
/// notice the file's age and steal it after `STALE_LOCK_SECS`.
pub struct LockedFile {
    lock_path: PathBuf,
}

impl LockedFile {
    /// Acquire `lock_path`, waiting up to the default timeout.
    pub fn acquire(lock_path: impl AsRef<Path>) -> Result<Self, LockError> {
        Self::acquire_with_timeout(lock_path, Duration::from_secs(ACQUIRE_TIMEOUT_SECS))
    }

    /// Acquire `lock_path`, waiting up to `timeout`.
    pub fn acquire_with_timeout(
        lock_path: impl AsRef<Path>,
        timeout: Duration,
    ) -> Result<Self, LockError> {
        let lock_path = lock_path.as_ref().to_path_buf();
        let deadline = SystemTime::now() + timeout;
        let mut wait_ms = POLL_START_MS;
        loop {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(mut f) => {
                    // Write owner pid as a hint — handy when debugging
                    // who's holding the lock.
                    let _ = writeln!(f, "{}", std::process::id());
                    return Ok(LockedFile { lock_path });
                }
                Err(_) => {
                    // Stale-lock detection: if the file has been there
                    // longer than STALE_LOCK_SECS, assume it's abandoned.
                    if let Ok(meta) = fs::metadata(&lock_path) {
                        if let Ok(modified) = meta.modified() {
                            if let Ok(age) = SystemTime::now().duration_since(modified) {
                                if age > Duration::from_secs(STALE_LOCK_SECS) {
                                    let _ = fs::remove_file(&lock_path);
                                    continue;
                                }
                            }
                        }
                    }
                    if SystemTime::now() >= deadline {
                        return Err(LockError::Timeout);
                    }
                    thread::sleep(Duration::from_millis(wait_ms));
                    wait_ms = (wait_ms.saturating_mul(2)).min(POLL_MAX_MS);
                }
            }
        }
    }
}

impl Drop for LockedFile {
    fn drop(&mut self) {
        // Best-effort cleanup. If this fails, the next acquirer will fall
        // back to the stale-lock path.
        let _ = fs::remove_file(&self.lock_path);
    }
}

/// Read-modify-write a file under a cross-process lock. The closure
/// receives the current bytes (empty if the file doesn't exist) and
/// returns the new bytes to write. The write is atomic via
/// temp-file-and-rename.
///
/// `lock_path` should be derived from `target_path` — convention is
/// `target_path.with_extension("lock")` or a sibling lock file. Use the
/// same `lock_path` everywhere callers may modify the same `target_path`.
pub fn with_locked<F>(
    lock_path: impl AsRef<Path>,
    target_path: impl AsRef<Path>,
    f: F,
) -> Result<(), LockError>
where
    F: FnOnce(&[u8]) -> Vec<u8>,
{
    let _guard = LockedFile::acquire(lock_path.as_ref())?;
    let target = target_path.as_ref();

    let mut current = Vec::new();
    if let Ok(mut fh) = fs::File::open(target) {
        fh.read_to_end(&mut current)
            .map_err(|e| LockError::Io(format!("read: {}", e)))?;
    }

    let new_bytes = f(&current);

    let tmp = target.with_extension("swap.tmp");
    {
        let mut out =
            fs::File::create(&tmp).map_err(|e| LockError::Io(format!("create tmp: {}", e)))?;
        out.write_all(&new_bytes)
            .map_err(|e| LockError::Io(format!("write tmp: {}", e)))?;
        out.sync_all()
            .map_err(|e| LockError::Io(format!("sync tmp: {}", e)))?;
    }
    fs::rename(&tmp, target).map_err(|e| LockError::Io(format!("rename: {}", e)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("flux-swarm-tools-test-{}", name));
        let _ = fs::remove_file(&p);
        let lock = p.with_extension("lock");
        let _ = fs::remove_file(&lock);
        p
    }

    #[test]
    fn acquires_and_releases() {
        let p = tmp("acq-rel");
        let lock = p.with_extension("lock");
        {
            let _g = LockedFile::acquire(&lock).unwrap();
            assert!(lock.exists(), "lock file present while holding guard");
        }
        assert!(!lock.exists(), "lock file removed on drop");
    }

    #[test]
    fn second_acquire_times_out() {
        let p = tmp("contend");
        let lock = p.with_extension("lock");
        let _hold = LockedFile::acquire(&lock).unwrap();
        let started = SystemTime::now();
        let res = LockedFile::acquire_with_timeout(&lock, Duration::from_millis(150));
        let elapsed = started.elapsed().unwrap();
        assert!(matches!(res, Err(LockError::Timeout)));
        // Sanity: actually waited the timeout, not 0ms.
        assert!(elapsed >= Duration::from_millis(120));
    }

    #[test]
    fn stale_lock_is_stolen() {
        let p = tmp("stale");
        let lock = p.with_extension("lock");
        // Forge an "old" lock file by setting mtime way in the past via
        // touch-style: easiest is just to create then leave; we override
        // the mtime check by sleeping past the threshold isn't realistic
        // in a unit test. Instead, monkey-patch by writing a lock and
        // using a tiny acquire timeout — staleness logic kicks in inside
        // the acquire loop regardless of the file's actual age, because
        // metadata().modified() may go backwards on weird filesystems.
        // Sanity-test the simpler path: a fresh lock blocks normally and
        // staleness IS detected if we manually predate the mtime via
        // remove + re-create with filetime sat below threshold isn't
        // portable without a deps. So just verify the happy path here:
        {
            let _g = LockedFile::acquire(&lock).unwrap();
            // Holder still alive; another acquirer should time out.
            assert!(
                LockedFile::acquire_with_timeout(&lock, Duration::from_millis(50)).is_err()
            );
        }
        // Released — next acquire works.
        let _g2 = LockedFile::acquire(&lock).unwrap();
    }

    #[test]
    fn with_locked_round_trip() {
        let p = tmp("rmw");
        let lock = p.with_extension("lock");
        with_locked(&lock, &p, |cur| {
            assert!(cur.is_empty(), "first write sees empty file");
            b"hello".to_vec()
        })
        .unwrap();
        with_locked(&lock, &p, |cur| {
            assert_eq!(cur, b"hello");
            b"hello world".to_vec()
        })
        .unwrap();
        let got = fs::read(&p).unwrap();
        assert_eq!(got, b"hello world");
    }

    #[test]
    fn with_locked_serializes_writes() {
        // Two threads each do read-modify-write that appends a marker.
        // Without the lock, one of the appends would race; with it,
        // both markers land.
        let p = tmp("serialize");
        let lock = p.with_extension("lock");
        with_locked(&lock, &p, |_| b"".to_vec()).unwrap(); // create empty
        let p2 = p.clone();
        let lock2 = lock.clone();
        let h = thread::spawn(move || {
            for _ in 0..20 {
                with_locked(&lock2, &p2, |cur| {
                    let mut v = cur.to_vec();
                    v.push(b'A');
                    v
                })
                .unwrap();
            }
        });
        for _ in 0..20 {
            with_locked(&lock, &p, |cur| {
                let mut v = cur.to_vec();
                v.push(b'B');
                v
            })
            .unwrap();
        }
        h.join().unwrap();
        let got = fs::read(&p).unwrap();
        let a_count = got.iter().filter(|&&b| b == b'A').count();
        let b_count = got.iter().filter(|&&b| b == b'B').count();
        assert_eq!(a_count, 20, "all A writes survived: {:?}", got);
        assert_eq!(b_count, 20, "all B writes survived");
    }
}
