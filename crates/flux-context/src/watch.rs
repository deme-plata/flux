//! watch — Task 3 of `docs/1M_CONTEXT_WINDOW_PLAN.md`: a context-watch daemon.
//!
//! Keeps the chunk manifest + diff hot so the 1M context is ready in <500ms. This
//! is a **poll-based** watcher (std-only, portable — no inotify dependency): each
//! tick computes a cheap **mtime signature** (stat only, no read/hash) and does the
//! expensive recompute+diff ONLY when something actually changed. On change it
//! re-runs the Task-2 diff, saves a new snapshot (skipping touch-only no-ops), and
//! refreshes the L1 hot cache in **`/dev/shm`** (true ramdisk — NOT `/tmp`, which on
//! Epsilon is a tiny 40 G root partition).
//!
//! inotify/kqueue is a later optimization behind the same `tick()` API.

use crate::chunk::{compute_manifest, ChunkManifest};
use crate::diff::{self, ContextSnapshot};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct WatchConfig {
    pub root: PathBuf,
    /// How often to check the mtime signature.
    pub poll: Duration,
    /// Settle window after a detected change before the follow-up tick.
    pub debounce: Duration,
    /// Stop after this many ticks (None = run forever). Used by tests/bounded runs.
    pub max_ticks: Option<u64>,
}

impl WatchConfig {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            poll: Duration::from_secs(2),
            debounce: Duration::from_millis(500),
            max_ticks: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WatchStatus {
    pub running: bool,
    pub workspace: String,
    pub version: u64,
    pub crate_count: usize,
    pub total_tokens: u64,
    pub ticks: u64,
    pub changes_detected: u64,
    pub last_scan_ns: u64,
    pub last_change_ns: u64,
    pub l1_path: String,
    pub last_diff_summary: String,
}

fn now_ns() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0)
}

/// L1 hot-cache dir: `/dev/shm/flux-context-hot` (ramdisk) if writable, else `ctx_dir/cache`.
pub fn l1_dir(ctx_dir: &Path) -> PathBuf {
    let shm = PathBuf::from("/dev/shm/flux-context-hot");
    if std::fs::create_dir_all(&shm).is_ok() {
        return shm;
    }
    ctx_dir.join("cache")
}

/// Cheap change signature: max mtime (ns) over all `.rs`/`.toml` files under `root`,
/// skipping target/.git/node_modules/.whale/.flux-rev. Stat-only — no reads or hashes.
fn workspace_mtime_sig(root: &Path) -> u64 {
    let mut max = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                let skip = p
                    .file_name()
                    .map(|n| {
                        n == "target" || n == ".git" || n == "node_modules" || n == ".whale" || n == ".flux-rev"
                    })
                    .unwrap_or(false);
                if !skip {
                    stack.push(p);
                }
            } else if p.extension().map(|x| x == "rs" || x == "toml").unwrap_or(false) {
                if let Ok(m) = e.metadata().and_then(|md| md.modified()) {
                    if let Ok(dur) = m.duration_since(UNIX_EPOCH) {
                        max = max.max(dur.as_nanos() as u64);
                    }
                }
            }
        }
    }
    max
}

pub fn status_path(ctx_dir: &Path) -> PathBuf {
    ctx_dir.join("watch-status.json")
}

pub fn write_status(ctx_dir: &Path, s: &WatchStatus) {
    let _ = std::fs::create_dir_all(ctx_dir);
    if let Ok(b) = serde_json::to_vec_pretty(s) {
        let _ = std::fs::write(status_path(ctx_dir), b);
    }
}

pub fn read_status(ctx_dir: &Path) -> Option<WatchStatus> {
    std::fs::read(status_path(ctx_dir)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

fn refresh_l1(ctx_dir: &Path, manifest: &ChunkManifest) {
    let dir = l1_dir(ctx_dir);
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(json) = serde_json::to_vec(manifest) {
        let _ = std::fs::write(dir.join("chunks.json"), json);
    }
}

/// One scan. Recomputes + diffs + snapshots + refreshes L1 only when the mtime
/// signature moved (or `*last_sig == 0` to force). Returns `true` if a real
/// content change produced a new snapshot version.
pub fn tick(cfg: &WatchConfig, last_sig: &mut u64, status: &mut WatchStatus) -> std::io::Result<bool> {
    let ctx_dir = cfg.root.join(".whale/context");
    let sig = workspace_mtime_sig(&cfg.root);
    status.ticks += 1;
    status.last_scan_ns = now_ns();

    // Idle fast-path: signature unchanged and we already have a baseline.
    if sig == *last_sig && status.version != 0 {
        write_status(&ctx_dir, status);
        return Ok(false);
    }
    *last_sig = sig;

    let manifest = compute_manifest(&cfg.root)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let version = diff::next_version(&ctx_dir);
    let snap = ContextSnapshot::from_manifest(&manifest, version, now_ns());

    let summary = match diff::load_latest(&ctx_dir) {
        Some(old) => {
            let d = diff::diff_snapshots(&old, &snap);
            if d.is_empty() {
                // mtime moved but content identical (e.g. `touch`) — refresh hot
                // cache, but don't churn a new snapshot version.
                status.last_diff_summary = "touch-only (no content change)".into();
                refresh_l1(&ctx_dir, &manifest);
                write_status(&ctx_dir, status);
                return Ok(false);
            }
            format!(
                "+{} ~{} -{} stale{} Δ{}tok",
                d.added.len(), d.modified.len(), d.deleted.len(), d.stale_ripple.len(), d.delta_tokens
            )
        }
        None => format!("baseline {} chunks ~{}tok", manifest.crate_count, manifest.total_tokens_estimated),
    };

    diff::save_snapshot(&ctx_dir, &snap)?;
    refresh_l1(&ctx_dir, &manifest);
    status.changes_detected += 1;
    status.last_change_ns = now_ns();
    status.version = version;
    status.crate_count = manifest.crate_count;
    status.total_tokens = manifest.total_tokens_estimated;
    status.workspace = manifest.workspace.clone();
    status.last_diff_summary = summary;
    status.l1_path = l1_dir(&ctx_dir).to_string_lossy().to_string();
    write_status(&ctx_dir, status);
    Ok(true)
}

/// Run the watch loop until `max_ticks` (if set) or forever. Debounces: after a
/// detected change, wait the settle window then run one more tick to catch the tail.
pub fn run(cfg: &WatchConfig) -> std::io::Result<()> {
    let ctx_dir = cfg.root.join(".whale/context");
    let mut status = read_status(&ctx_dir).unwrap_or_default();
    status.running = true;
    let mut last_sig = 0u64;
    loop {
        let changed = tick(cfg, &mut last_sig, &mut status).unwrap_or(false);
        if changed {
            std::thread::sleep(cfg.debounce);
            let _ = tick(cfg, &mut last_sig, &mut status);
        }
        if let Some(max) = cfg.max_ticks {
            if status.ticks >= max {
                break;
            }
        }
        std::thread::sleep(cfg.poll);
    }
    status.running = false;
    write_status(&ctx_dir, &status);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_workspace(dir: &Path) {
        let w = |rel: &str, body: &str| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        w("Cargo.toml", "[workspace]\nmembers = [\"a\", \"b\"]\nresolver = \"2\"\n");
        w("a/Cargo.toml", "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
        w("a/src/lib.rs", "pub fn a() -> u32 { 1 }\n");
        w("b/Cargo.toml", "[package]\nname = \"b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\na = { path = \"../a\" }\n");
        w("b/src/lib.rs", "pub fn b() -> u32 { a::a() + 1 }\n");
    }

    #[test]
    fn watch_tick_lifecycle() {
        let dir = std::env::temp_dir().join(format!("flux-ctx-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        mk_workspace(&dir);

        let cfg = WatchConfig::new(dir.clone());
        let mut sig = 0u64;
        let mut st = WatchStatus::default();

        // 1) baseline = a change
        assert!(tick(&cfg, &mut sig, &mut st).unwrap(), "baseline should register a change");
        assert_eq!(st.version, 1);
        assert_eq!(st.crate_count, 2);

        // 2) no change → idle fast-path
        assert!(!tick(&cfg, &mut sig, &mut st).unwrap(), "second tick: no change");
        assert_eq!(st.version, 1);

        // 3) real content change (force recompute via sig=0 to avoid mtime-granularity flakiness)
        std::fs::write(dir.join("a/src/lib.rs"), "pub fn a() -> u32 { 1 }\npub fn a2() -> u32 { 2 }\n").unwrap();
        sig = 0;
        assert!(tick(&cfg, &mut sig, &mut st).unwrap(), "content change should bump a snapshot");
        assert_eq!(st.version, 2, "snapshot version should advance");
        assert!(st.last_diff_summary.contains('~'), "diff summary should show a modified chunk: {}", st.last_diff_summary);

        // L1 hot cache written
        let l1 = l1_dir(&dir.join(".whale/context"));
        assert!(l1.join("chunks.json").exists(), "L1 chunks.json should exist at {}", l1.display());

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(PathBuf::from("/dev/shm/flux-context-hot"));
    }
}
