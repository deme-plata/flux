// fluxc pulse — compile-cost consequences, measured on every build.
//
// The problem this exists for (Viktor, 2026-10-03): "during development of
// sigilgraph i noticed compile time keeps rising … in the run up to that point
// more data is needed so you can improve and enhance on the run". Compile time
// rarely jumps; it CREEPS — a dependency here, a 2 KB generic there, a crate
// that quietly became everyone's dependency. By the time a human notices, the
// cause is twenty commits back. So every build now reports what it cost
// compared with what that same work usually costs, while the change that
// caused it is still the one on screen.
//
// Data flow:
//   1. wrapper (one short-lived process per rustc unit) measures the rustc
//      child — CPU time + peak RSS via getrusage(RUSAGE_CHILDREN), not wall
//      clock, because wall clock on a box at load 37 measures the queue — and
//      appends ONE JSON line. With FLUX_INVOCATION_ID set (run_cargo sets it)
//      the line goes to `inv/<id>.units.jsonl`, else straight to the day log.
//   2. the parent fluxc invocation, after cargo exits, reads its own inv file,
//      judges every compiled unit against that unit's rolling baseline, writes
//      the Pulse (`last.json`, `inv/<id>.json`), folds its samples into the
//      baselines, and moves the lines into the day log for drift analysis.
//   3. the Pulse goes out as the `compile_pulse` webhook and is rendered into
//      the combo dashboards, so an agent SEES the consequence of the change it
//      just made, in the same tool response.
//
// Contract: advisory, never load-bearing. Every failure is swallowed; a
// missing or corrupt pulse store never breaks or slows a build. FLUX_PULSE=0
// disables recording entirely. FLUX_PULSE_DIR overrides the store (tests).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Samples kept per unit key. Enough for a stable median, small enough that
/// the baseline follows the code instead of last month's code.
const BASELINE_CAP: usize = 24;
/// A unit needs this many prior samples before we judge it.
const MIN_SAMPLES: usize = 3;
/// Per-unit regression: ≥ this ratio AND ≥ REGRESS_MIN_MS absolute.
const REGRESS_RATIO: f64 = 1.25;
const IMPROVE_RATIO: f64 = 0.80;
const REGRESS_MIN_MS: u64 = 300;

// ── store layout ────────────────────────────────────────────────────────────

pub fn pulse_dir() -> PathBuf {
    std::env::var("FLUX_PULSE_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| flux_cache::cache_dir().join("pulse"))
}

pub fn enabled() -> bool {
    std::env::var("FLUX_PULSE").map(|v| v != "0").unwrap_or(true)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Civil date (UTC) for a unix timestamp — Howard Hinnant's days_to_civil.
pub fn civil_date(unix: u64) -> (i64, u32, u32) {
    let z = (unix / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

fn day_log_path(dir: &Path, unix: u64) -> PathBuf {
    let (y, m, d) = civil_date(unix);
    dir.join(format!("units-{:04}-{:02}-{:02}.jsonl", y, m, d))
}

fn append_lines(path: &Path, lines: &str) {
    if lines.is_empty() {
        return;
    }
    if let Some(p) = path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    // O_APPEND: each write lands at the current end even with many concurrent
    // wrapper processes; one line per write keeps records whole.
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(lines.as_bytes());
    }
}

// ── the per-unit sample (wrapper side) ─────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UnitSample {
    pub ts: u64,
    #[serde(default)]
    pub inv: String,
    pub pkg: String,
    pub crate_name: String,
    /// rustc --crate-type (lib, bin, proc-macro, …).
    #[serde(default)]
    pub kind: String,
    /// "check" (metadata only) or "build" (codegen). Costs differ ~3-10×.
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub is_test: bool,
    /// opt-level, plus "+lto" when LTO is on.
    #[serde(default)]
    pub opt: String,
    #[serde(default)]
    pub target: String,
    /// Short hash of the enabled `--cfg feature=…` set ("" = none). The same
    /// crate built twice in one graph (host vs target features) is two
    /// different pieces of work and must not share a baseline.
    #[serde(default)]
    pub feat: String,
    /// Workspace/path crate (true) vs registry/git dependency (false).
    #[serde(default)]
    pub local: bool,
    /// "compiled" | "restored" (flux cache hit) | "failed".
    pub outcome: String,
    pub wall_ms: u64,
    #[serde(default)]
    pub cpu_ms: u64,
    #[serde(default)]
    pub rss_kb: u64,
    /// Bytes of the unit's root source + its module tree.
    #[serde(default)]
    pub src_bytes: u64,
    #[serde(default)]
    pub load1: f64,
    /// Unix ms when rustc finished (start = end_ms - wall_ms). Lets the parent
    /// split its wall clock into before-first-rustc / compiling / after.
    #[serde(default)]
    pub end_ms: u64,
    /// 12-hex digest of the unit's dependency CONTENT (the cache key's
    /// substituted `--extern` hashes) and of its own source tree. Comparing
    /// them with the previous build of the same unit says WHY it rebuilt.
    #[serde(default)]
    pub dep_digest: String,
    #[serde(default)]
    pub src_digest: String,
}

impl UnitSample {
    /// Stable identity of "the same work" across content changes.
    pub fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|o{}|{}|{}",
            self.pkg,
            self.crate_name,
            self.kind,
            self.mode,
            if self.is_test { "test" } else { "-" },
            self.opt,
            self.target,
            self.feat
        )
    }
    pub fn label(&self) -> String {
        let mut s = if self.crate_name.replace('_', "-") == self.pkg || self.crate_name == self.pkg {
            self.pkg.clone()
        } else {
            format!("{}::{}", self.pkg, self.crate_name)
        };
        if self.is_test {
            s.push_str(" (test)");
        }
        if self.kind == "bin" && self.crate_name == "build_script_build" {
            s = format!("{} (build.rs)", self.pkg);
        }
        s
    }
}

/// Facts the wrapper can read off the rustc command line.
pub struct RustcShape {
    pub kind: String,
    pub mode: String,
    pub opt: String,
    pub target: String,
    pub feat: String,
}

pub fn shape_from_args(args: &[String]) -> RustcShape {
    let mut kind = String::new();
    let mut emit = String::new();
    let mut opt = "0".to_string();
    let mut lto = false;
    let mut target = "host".to_string();
    let mut features: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let next = args.get(i + 1).map(|s| s.as_str()).unwrap_or("");
        if a == "--crate-type" {
            if kind.is_empty() {
                kind = next.to_string();
            }
            i += 1;
        } else if let Some(v) = a.strip_prefix("--crate-type=") {
            if kind.is_empty() {
                kind = v.to_string();
            }
        } else if a == "--emit" {
            emit = next.to_string();
            i += 1;
        } else if let Some(v) = a.strip_prefix("--emit=") {
            emit = v.to_string();
        } else if a == "--cfg" {
            if let Some(f) = next.strip_prefix("feature=") {
                features.push(f);
            }
            i += 1;
        } else if a == "--target" {
            target = next.to_string();
            i += 1;
        } else if let Some(v) = a.strip_prefix("--target=") {
            target = v.to_string();
        } else if a == "-C" || a.starts_with("-C") {
            let c = if a == "-C" {
                i += 1;
                next
            } else {
                &a[2..]
            };
            if let Some(v) = c.strip_prefix("opt-level=") {
                opt = v.to_string();
            } else if c == "lto" || (c.starts_with("lto=") && c != "lto=off" && c != "lto=no" && c != "lto=false") {
                lto = true;
            }
        }
        i += 1;
    }
    if args.iter().any(|a| a == "--test") && kind.is_empty() {
        kind = "bin".into();
    }
    if kind.is_empty() {
        kind = "lib".into();
    }
    // cargo check emits `dep-info,metadata` only; a codegen build adds `link`.
    let mode = if emit.contains("link") || emit.is_empty() { "build" } else { "check" };
    if lto {
        opt.push_str("+lto");
    }
    features.sort_unstable();
    let feat = if features.is_empty() {
        String::new()
    } else {
        blake3::hash(features.join(",").as_bytes()).to_hex()[..6].to_string()
    };
    RustcShape { kind, mode: mode.into(), opt, target, feat }
}

/// Registry and git dependencies live under CARGO_HOME; anything else is a
/// path crate — the code being developed.
pub fn is_local_manifest(manifest_dir: &str) -> bool {
    !(manifest_dir.is_empty()
        || manifest_dir.contains("/.cargo/registry/")
        || manifest_dir.contains("/.cargo/git/")
        || manifest_dir.contains("/registry/src/")
        || manifest_dir.contains("/vendor/"))
}

pub fn load1() -> f64 {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse().ok()))
        .unwrap_or(0.0)
}

/// CPU (user+sys, ms) and peak RSS (KiB) of all reaped children so far.
#[cfg(unix)]
pub fn children_rusage() -> (u64, u64) {
    unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_CHILDREN, &mut ru) != 0 {
            return (0, 0);
        }
        let ms = |tv: libc::timeval| (tv.tv_sec as u64) * 1000 + (tv.tv_usec as u64) / 1000;
        (ms(ru.ru_utime) + ms(ru.ru_stime), ru.ru_maxrss as u64)
    }
}

#[cfg(not(unix))]
pub fn children_rusage() -> (u64, u64) {
    (0, 0)
}

/// Wrapper-side writer: one line, one write. Never fails the build.
pub fn record_unit(sample: &UnitSample) {
    if !enabled() {
        return;
    }
    let Ok(line) = serde_json::to_string(sample) else { return };
    let dir = pulse_dir();
    let path = if sample.inv.is_empty() || sample.inv.contains('/') {
        day_log_path(&dir, sample.ts)
    } else {
        dir.join("inv").join(format!("{}.units.jsonl", sample.inv))
    };
    append_lines(&path, &(line + "\n"));
}

/// Is any `<target>/<profile>/.cargo-lock` held right now? cargo takes an
/// exclusive flock there for the whole build, so a non-blocking SHARED probe
/// fails exactly when another build owns it. Released immediately.
pub fn cargo_lock_busy(target_dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(target_dir) else { return false };
    let mut locks: Vec<PathBuf> = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let l = p.join(".cargo-lock");
        if l.exists() {
            locks.push(l);
        }
        // cross targets nest one level deeper: <target>/<triple>/<profile>/.cargo-lock
        if let Ok(rd2) = std::fs::read_dir(&p) {
            for e2 in rd2.flatten() {
                let l2 = e2.path().join(".cargo-lock");
                if e2.path().is_dir() && l2.exists() {
                    locks.push(l2);
                }
            }
        }
    }
    #[cfg(unix)]
    for l in locks {
        use std::os::unix::io::AsRawFd;
        if let Ok(f) = std::fs::File::open(&l) {
            let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
            if r != 0 {
                return true;
            }
            unsafe {
                libc::flock(f.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
    false
}

/// Seconds cargo spent saying "Blocking waiting for file lock on …".
#[derive(Debug, Clone, Default)]
pub struct LockWaits {
    pub build_dir_ms: u64,
    pub package_cache_ms: u64,
    /// cargo printed "error: test failed" — the BUILD was fine, a test was not.
    pub tests_failed: bool,
}

/// Run `cmd` with its stderr relayed line-by-line to ours, timing every
/// stretch that begins with cargo's "Blocking waiting for file lock" notice
/// and ends at the next line cargo prints. Output is unchanged for a reader;
/// colour is preserved by forcing CARGO_TERM_COLOR when our stderr is a TTY.
pub fn status_with_lock_watch(cmd: &mut std::process::Command) -> std::io::Result<(std::process::ExitStatus, LockWaits)> {
    use std::io::Read;
    // On a terminal, keep what an interactive user sees: colour AND cargo's
    // progress bar, which cargo only draws on a TTY — so tell it explicitly,
    // with the real width. Relaying raw chunks (not lines) keeps the bar's
    // carriage-return redraws flowing.
    #[cfg(unix)]
    {
        if unsafe { libc::isatty(2) } == 1 {
            if std::env::var_os("CARGO_TERM_COLOR").is_none() {
                cmd.env("CARGO_TERM_COLOR", "always");
            }
            if std::env::var_os("CARGO_TERM_PROGRESS_WHEN").is_none() {
                let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
                let cols = if unsafe { libc::ioctl(2, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_col > 0 { ws.ws_col } else { 100 };
                cmd.env("CARGO_TERM_PROGRESS_WHEN", "always");
                cmd.env("CARGO_TERM_PROGRESS_WIDTH", cols.to_string());
            }
        }
    }
    cmd.stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn()?;
    let err = child.stderr.take();
    let watcher = std::thread::spawn(move || {
        let mut w = LockWaits::default();
        let Some(mut err) = err else { return w };
        let mut out = std::io::stderr();
        let mut buf = [0u8; 8192];
        let mut tail = String::new(); // last partial line, so a notice split across reads still matches
        let mut open: Option<(std::time::Instant, bool)> = None; // (since, is_pkg_cache)
        loop {
            let n = match err.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let _ = out.write_all(&buf[..n]);
            // Any output after a Blocking notice means the wait is over.
            if let Some((since, pkg)) = open.take() {
                let ms = since.elapsed().as_millis() as u64;
                if pkg { w.package_cache_ms += ms } else { w.build_dir_ms += ms }
            }
            tail.push_str(&String::from_utf8_lossy(&buf[..n]));
            if tail.contains("error: test failed") {
                w.tests_failed = true;
            }
            if let Some(pos) = tail.rfind("waiting for file lock") {
                let line_end = tail[pos..].find(['\n', '\r']).map(|k| pos + k);
                let rest_after = line_end.map(|k| tail[k..].trim().is_empty()).unwrap_or(true);
                if rest_after && tail[..pos].contains("Blocking") {
                    let is_pkg = tail[pos..].contains("package cache");
                    open = Some((std::time::Instant::now(), is_pkg));
                }
            }
            if let Some(k) = tail.rfind(['\n', '\r']) {
                tail.drain(..=k);
            }
            if tail.len() > 4096 {
                let cut = tail.len() - 1024;
                let cut = (cut..tail.len()).find(|&c| tail.is_char_boundary(c)).unwrap_or(tail.len());
                tail.drain(..cut);
            }
        }
        if let Some((since, pkg)) = open {
            let ms = since.elapsed().as_millis() as u64;
            if pkg { w.package_cache_ms += ms } else { w.build_dir_ms += ms }
        }
        w
    });
    let status = child.wait()?;
    let waits = watcher.join().unwrap_or_default();
    Ok((status, waits))
}

pub fn new_invocation_id() -> String {
    let seed = format!(
        "{}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
        std::thread::current().id()
    );
    blake3::hash(seed.as_bytes()).to_hex()[..16].to_string()
}

// ── baselines ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Sample {
    pub ts: u64,
    pub cpu_ms: u64,
    pub wall_ms: u64,
    pub src_bytes: u64,
    /// 1-min load when sampled (0 = unknown) — CPU time itself inflates when
    /// SMT siblings are busy, so baselines remember how contended they were.
    #[serde(default)]
    pub load1: f64,
    #[serde(default)]
    pub dep_digest: String,
    #[serde(default)]
    pub src_digest: String,
    /// Why this sample rebuilt vs the previous one of the same unit:
    /// "dep" | "self" | "same" | "first" | "" (legacy, unknown).
    #[serde(default)]
    pub trigger: String,
}

/// Why a unit rebuilt, relative to its previous build. Incremental rustc
/// makes these two different kinds of work: a dependency change can force a
/// near-full recompile (measured: fluxc-mcp ~100 s), an own-source edit is
/// often a few seconds (fluxc-mcp ~5 s). They must never share one median.
pub fn trigger_of(prev: Option<&Sample>, dep_digest: &str, src_digest: &str) -> &'static str {
    let Some(prev) = prev else { return "first" };
    if dep_digest.is_empty() || prev.dep_digest.is_empty() {
        return "";
    }
    if prev.dep_digest != dep_digest {
        "dep"
    } else if prev.src_digest != src_digest {
        "self"
    } else {
        "same"
    }
}

/// The comparison class a trigger belongs to.
fn trigger_class(t: &str) -> &'static str {
    match t {
        "dep" => "dep",
        "self" | "same" => "self",
        _ => "",
    }
}

/// Physical cores (unique core_id × package), cached. Falls back to half the
/// logical CPUs when sysfs topology is unavailable.
pub fn physical_cores() -> f64 {
    static PHYS: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *PHYS.get_or_init(|| {
        let mut set = BTreeSet::new();
        if let Ok(rd) = std::fs::read_dir("/sys/devices/system/cpu") {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if !n.starts_with("cpu") || !n[3..].chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                let t = e.path().join("topology");
                let core = std::fs::read_to_string(t.join("core_id")).unwrap_or_default();
                let pkg = std::fs::read_to_string(t.join("physical_package_id")).unwrap_or_default();
                if !core.is_empty() {
                    set.insert((pkg.trim().to_string(), core.trim().to_string()));
                }
            }
        }
        if set.is_empty() {
            (std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2) as f64 / 2.0).max(1.0)
        } else {
            set.len() as f64
        }
    })
}

/// How much per-instruction CPU time inflates at a given load: 1.0 up to the
/// physical core count, rising to 1.5 once every SMT sibling is busy.
pub fn contention(load: f64) -> f64 {
    let phys = physical_cores();
    1.0 + 0.5 * ((load - phys) / phys).clamp(0.0, 1.0)
}

/// Tolerance multiplier for comparing a sample taken at `load_now` against a
/// baseline taken at `load_base` (0 = unknown → assume a quiet baseline).
pub fn load_allowance(load_now: f64, load_base: f64) -> f64 {
    let base = if load_base > 0.0 { contention(load_base) } else { 1.0 };
    (contention(load_now) / base).clamp(1.0, 1.6)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Baselines {
    pub units: BTreeMap<String, Vec<Sample>>,
}

impl Baselines {
    pub fn median_cpu(&self, key: &str) -> Option<(u64, usize)> {
        let v = self.units.get(key)?;
        if v.is_empty() {
            return None;
        }
        let mut c: Vec<u64> = v.iter().map(|s| s.cpu_ms).collect();
        c.sort_unstable();
        Some((c[c.len() / 2], c.len()))
    }
    /// Median CPU over prior samples of the same trigger class ("dep"/"self").
    /// Legacy samples (no digests) never count — mixing classes is the bug.
    pub fn median_cpu_class(&self, key: &str, class: &str) -> Option<(u64, usize)> {
        let v = self.units.get(key)?;
        let mut c: Vec<u64> = v.iter().filter(|s| trigger_class(&s.trigger) == class && !class.is_empty()).map(|s| s.cpu_ms).collect();
        if c.is_empty() {
            return None;
        }
        c.sort_unstable();
        Some((c[c.len() / 2], c.len()))
    }

    /// Median load of the samples that recorded one (0 = none did).
    pub fn median_load(&self, key: &str) -> f64 {
        let Some(v) = self.units.get(key) else { return 0.0 };
        let mut l: Vec<f64> = v.iter().map(|s| s.load1).filter(|x| *x > 0.0).collect();
        if l.is_empty() {
            return 0.0;
        }
        l.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        l[l.len() / 2]
    }
    pub fn last_src(&self, key: &str) -> Option<u64> {
        self.units.get(key)?.last().map(|s| s.src_bytes).filter(|b| *b > 0)
    }
    pub fn fold(&mut self, s: &UnitSample) {
        if s.outcome != "compiled" || s.cpu_ms == 0 {
            return;
        }
        let v = self.units.entry(s.key()).or_default();
        let trigger = trigger_of(v.last(), &s.dep_digest, &s.src_digest).to_string();
        v.push(Sample {
            ts: s.ts,
            cpu_ms: s.cpu_ms,
            wall_ms: s.wall_ms,
            src_bytes: s.src_bytes,
            load1: s.load1,
            dep_digest: s.dep_digest.clone(),
            src_digest: s.src_digest.clone(),
            trigger,
        });
        if v.len() > BASELINE_CAP {
            let drop = v.len() - BASELINE_CAP;
            v.drain(..drop);
        }
    }
    /// Any key for this package+crate seen under ANY shape (opt/mode/target)?
    pub fn knows_crate(&self, pkg: &str, crate_name: &str) -> bool {
        let prefix = format!("{}|{}|", pkg, crate_name);
        self.units.range(prefix.clone()..).next().map(|(k, _)| k.starts_with(&prefix)).unwrap_or(false)
    }
}

fn baselines_path(dir: &Path) -> PathBuf {
    dir.join("baselines.json")
}

pub fn load_baselines(dir: &Path) -> Baselines {
    std::fs::read_to_string(baselines_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_baselines(dir: &Path, b: &Baselines) {
    let Ok(json) = serde_json::to_string(b) else { return };
    let tmp = dir.join(format!("baselines.json.tmp-{}", std::process::id()));
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, baselines_path(dir));
    }
}

/// Exclusive advisory lock over the read-modify-write of baselines.json, so
/// two agents' builds finishing together cannot lose each other's samples.
struct DirLock(Option<std::fs::File>);

impl DirLock {
    fn acquire(dir: &Path) -> DirLock {
        let _ = std::fs::create_dir_all(dir);
        let Ok(f) = std::fs::OpenOptions::new().create(true).write(true).open(dir.join(".lock")) else {
            return DirLock(None);
        };
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            unsafe {
                libc::flock(f.as_raw_fd(), libc::LOCK_EX);
            }
        }
        DirLock(Some(f))
    }
}

impl Drop for DirLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(f) = &self.0 {
            use std::os::unix::io::AsRawFd;
            unsafe {
                libc::flock(f.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

// ── the Pulse: one invocation, judged ──────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UnitCost {
    pub unit: String,
    pub local: bool,
    pub cpu_ms: u64,
    pub wall_ms: u64,
    #[serde(default)]
    pub rss_kb: u64,
    /// Median of prior samples (0 = no baseline).
    #[serde(default)]
    pub typical_ms: u64,
    #[serde(default)]
    pub samples: usize,
    /// Source growth vs the last baseline sample, bytes (signed).
    #[serde(default)]
    pub src_delta: i64,
    #[serde(default)]
    pub src_bytes: u64,
    /// Why it rebuilt: "dep" (a dependency's content changed), "self" (own
    /// source changed), "same" (neither — fingerprint churn), "first", "".
    #[serde(default)]
    pub cause: String,
    /// Typical cost of the OTHER class, when known — e.g. what an own-source
    /// edit costs for a unit that just rebuilt because a dependency changed.
    #[serde(default)]
    pub other_class_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Pulse {
    pub inv: String,
    pub ts: u64,
    pub cmd: String,
    pub package: String,
    pub release: bool,
    pub green: bool,
    pub wall_ms: u64,
    pub load1: f64,
    pub units_compiled: usize,
    pub units_restored: usize,
    pub units_failed: usize,
    /// rustc runs that failed inside a green build: build-script probes.
    #[serde(default)]
    pub probe_failures: usize,
    /// Units recompiled with byte-identical dependencies AND source — cargo
    /// thought them dirty for no content reason (env/wrapper/fingerprint churn).
    #[serde(default)]
    pub churn_units: usize,
    #[serde(default)]
    pub churn_cpu_ms: u64,
    /// Local units rebuilt only because a dependency changed, and their cost.
    #[serde(default)]
    pub upstream_rebuilds: Vec<UnitCost>,
    pub cpu_ms_total: u64,
    pub cpu_ms_local: u64,
    /// CPU the judged (baselined) units USUALLY take, and what they took now.
    pub expected_cpu_ms: u64,
    pub judged_cpu_ms: u64,
    /// Fraction of compiled units that had a baseline to judge against.
    pub coverage: f64,
    pub cache_rate: f64,
    pub peak_rss_kb: u64,
    pub peak_rss_unit: String,
    pub local_rebuilt: Vec<UnitCost>,
    pub top: Vec<UnitCost>,
    pub regressions: Vec<UnitCost>,
    pub improvements: Vec<UnitCost>,
    /// Units never compiled here before under any shape — new dependencies.
    pub new_crates: Vec<UnitCost>,
    /// Known crate, first time under this shape (new opt-level/target/mode).
    pub new_shapes: Vec<UnitCost>,
    /// Largest load tolerance applied to any judged unit (1.0 = none): CPU
    /// time measured on a box busier than its baseline gets more slack.
    #[serde(default)]
    pub load_allowance: f64,
    /// Wall-clock phases of the invocation: cargo's own time before the first
    /// rustc started (resolution, fingerprinting, build-dir LOCK WAIT), the
    /// span in which rustc units ran, and the tail after the last one ended
    /// (link of the final unit is inside the span; test execution, build-
    /// script runs and cargo bookkeeping are after).
    #[serde(default)]
    pub before_ms: u64,
    #[serde(default)]
    pub compile_span_ms: u64,
    #[serde(default)]
    pub after_ms: u64,
    /// Unit shapes in the baseline store when judged — below MATURE_KEYS a
    /// first-seen crate is "not measured yet", not "a new dependency".
    #[serde(default)]
    pub store_keys: usize,
    /// Another cargo held a `target/*/.cargo-lock` when this invocation began
    /// — its `before_ms` is (at least partly) queueing behind that build.
    #[serde(default)]
    pub lock_busy_at_start: bool,
    /// Time cargo itself reported as "Blocking waiting for file lock on …",
    /// measured line-to-line on its stderr. build dir = another build on the
    /// same target/; package cache = ANY cargo on the box holding ~/.cargo.
    #[serde(default)]
    pub lock_wait_build_dir_ms: u64,
    #[serde(default)]
    pub lock_wait_pkg_cache_ms: u64,
    /// steady | heavier | lighter | learning | idle
    pub verdict: String,
    pub headline: String,
    /// What to do about it — each lever is tied to a measured condition in
    /// this pulse, never a generic tip.
    #[serde(default)]
    pub levers: Vec<String>,
}

/// Baseline-store size at which a never-seen external crate is credible
/// evidence of a NEW dependency rather than of a young store.
const MATURE_KEYS: usize = 150;

fn pct(a: u64, b: u64) -> i64 {
    if b == 0 {
        return 0;
    }
    ((a as f64 / b as f64 - 1.0) * 100.0).round() as i64
}

pub fn fmt_ms(ms: u64) -> String {
    if ms >= 60_000 {
        format!("{}m{:02}s", ms / 60_000, (ms % 60_000) / 1000)
    } else if ms >= 10_000 {
        format!("{:.0}s", ms as f64 / 1000.0)
    } else if ms >= 1000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{}ms", ms)
    }
}

fn fmt_signed_ms(d: i64) -> String {
    let s = fmt_ms(d.unsigned_abs());
    if d >= 0 { format!("+{}", s) } else { format!("-{}", s) }
}

fn fmt_bytes_signed(d: i64) -> String {
    let a = d.unsigned_abs();
    let s = if a >= 1024 * 1024 {
        format!("{:.1} MB", a as f64 / 1048576.0)
    } else if a >= 1024 {
        format!("{:.1} KB", a as f64 / 1024.0)
    } else {
        format!("{} B", a)
    };
    if d >= 0 { format!("+{}", s) } else { format!("-{}", s) }
}

/// Judge one invocation's units against the baselines. Pure — the I/O lives
/// in `finish_invocation`, so this is unit-testable with synthetic samples.
pub fn judge(
    units: &[UnitSample],
    base: &Baselines,
    meta: (&str, &str, &str, bool, bool, u64),
) -> Pulse {
    judge_at(units, base, meta, 0)
}

/// `judge` with the invocation's start time (unix ms), enabling the
/// before/compiling/after wall split. `start_ms == 0` skips the split.
pub fn judge_at(
    units: &[UnitSample],
    base: &Baselines,
    meta: (&str, &str, &str, bool, bool, u64),
    start_ms: u64,
) -> Pulse {
    judge_full(units, base, meta, start_ms, false)
}

/// What the parent observed about its cargo run, beyond the units.
#[derive(Debug, Clone, Default)]
pub struct InvocationFacts {
    pub start_ms: u64,
    pub lock_busy_at_start: bool,
    pub lock_wait_build_dir_ms: u64,
    pub lock_wait_pkg_cache_ms: u64,
    /// Package names in the working Cargo.lock but not in `HEAD:Cargo.lock`.
    /// `Some` = git could answer: a first-seen crate is a NEW dependency only
    /// if it is in here. `None` = unknown → fall back to the store heuristic.
    pub lock_added: Option<BTreeSet<String>>,
    /// Red invocation because a TEST failed, not the build.
    pub tests_failed: bool,
}

pub fn judge_full(
    units: &[UnitSample],
    base: &Baselines,
    meta: (&str, &str, &str, bool, bool, u64),
    start_ms: u64,
    lock_busy_at_start: bool,
) -> Pulse {
    judge_facts(units, base, meta, &InvocationFacts { start_ms, lock_busy_at_start, ..Default::default() })
}

pub fn judge_facts(
    units: &[UnitSample],
    base: &Baselines,
    meta: (&str, &str, &str, bool, bool, u64),
    facts: &InvocationFacts,
) -> Pulse {
    let start_ms = facts.start_ms;
    let (inv, cmd, package, release, green, wall_ms) = meta;
    // A failing test makes the invocation red but says nothing about the build.
    let green = green || facts.tests_failed;
    let mut p = Pulse {
        lock_busy_at_start: facts.lock_busy_at_start,
        lock_wait_build_dir_ms: facts.lock_wait_build_dir_ms,
        lock_wait_pkg_cache_ms: facts.lock_wait_pkg_cache_ms,
        inv: inv.into(),
        ts: now_unix(),
        cmd: cmd.into(),
        package: package.into(),
        release,
        green,
        wall_ms,
        load1: load1(),
        ..Default::default()
    };
    let mut judged = 0usize;
    let mut all: Vec<UnitCost> = Vec::new();
    for u in units {
        match u.outcome.as_str() {
            "restored" => {
                p.units_restored += 1;
                continue;
            }
            // In a GREEN build a failing rustc is a build script's feature probe
            // (compiling a test program that is expected to fail) — not a failure.
            "failed" if green => {
                p.probe_failures += 1;
                continue;
            }
            "failed" => p.units_failed += 1,
            _ => p.units_compiled += 1,
        }
        p.cpu_ms_total += u.cpu_ms;
        if u.local {
            p.cpu_ms_local += u.cpu_ms;
        }
        if u.rss_kb > p.peak_rss_kb {
            p.peak_rss_kb = u.rss_kb;
            p.peak_rss_unit = u.label();
        }
        let key = u.key();
        let cause = trigger_of(base.units.get(&key).and_then(|v| v.last()), &u.dep_digest, &u.src_digest);
        let class = trigger_class(cause);
        let (typical, n) = if class.is_empty() {
            // no digests on one side (samples from an older wrapper): the old
            // all-samples median is the only comparison available
            base.median_cpu(&key).unwrap_or((0, 0))
        } else {
            base.median_cpu_class(&key, class).unwrap_or((0, 0))
        };
        let other_class_ms = match class {
            "dep" => base.median_cpu_class(&key, "self").map(|x| x.0).unwrap_or(0),
            "self" => base.median_cpu_class(&key, "dep").map(|x| x.0).unwrap_or(0),
            _ => 0,
        };
        if cause == "same" && u.outcome == "compiled" {
            p.churn_units += 1;
            p.churn_cpu_ms += u.cpu_ms;
        }
        let src_delta = match base.last_src(&key) {
            Some(prev) if u.src_bytes > 0 => u.src_bytes as i64 - prev as i64,
            _ => 0,
        };
        let c = UnitCost {
            unit: u.label(),
            local: u.local,
            cpu_ms: u.cpu_ms,
            wall_ms: u.wall_ms,
            rss_kb: u.rss_kb,
            typical_ms: typical,
            samples: n,
            src_delta,
            src_bytes: u.src_bytes,
            cause: cause.to_string(),
            other_class_ms,
        };
        if cause == "dep" && u.local && u.outcome == "compiled" {
            p.upstream_rebuilds.push(c.clone());
        }
        if u.outcome == "compiled" {
            if n == 0 {
                // A never-seen EXTERNAL crate in a mature store = a new dependency.
                // A never-seen local crate, or anything in a young store, is just
                // a first measurement under this shape.
                let added_here = match &facts.lock_added {
                    Some(set) => set.contains(&u.pkg),
                    None => base.units.len() >= MATURE_KEYS,
                };
                if !u.local && added_here && !base.knows_crate(&u.pkg, &u.crate_name) {
                    p.new_crates.push(c.clone());
                } else {
                    p.new_shapes.push(c.clone());
                }
            } else if n >= MIN_SAMPLES {
                judged += 1;
                p.expected_cpu_ms += typical;
                p.judged_cpu_ms += u.cpu_ms;
                let allow = load_allowance(u.load1, base.median_load(&key));
                if allow > p.load_allowance {
                    p.load_allowance = allow;
                }
                let up = u.cpu_ms as f64 >= typical as f64 * REGRESS_RATIO * allow
                    && u.cpu_ms.saturating_sub(typical) >= REGRESS_MIN_MS;
                let down = (u.cpu_ms as f64) <= typical as f64 * IMPROVE_RATIO
                    && typical.saturating_sub(u.cpu_ms) >= REGRESS_MIN_MS;
                if up {
                    p.regressions.push(c.clone());
                } else if down {
                    p.improvements.push(c.clone());
                }
            }
        }
        if u.local {
            p.local_rebuilt.push(c.clone());
        }
        all.push(c);
    }
    p.store_keys = base.units.len();
    // Wall split. Only units that actually ran rustc carry a meaningful span.
    let ran: Vec<&UnitSample> = units.iter().filter(|u| u.end_ms > 0 && u.outcome != "restored").collect();
    if start_ms > 0 && !ran.is_empty() {
        let first_start = ran.iter().map(|u| u.end_ms.saturating_sub(u.wall_ms)).min().unwrap_or(start_ms);
        let last_end = ran.iter().map(|u| u.end_ms).max().unwrap_or(start_ms);
        let end_ms = start_ms + wall_ms;
        p.before_ms = first_start.saturating_sub(start_ms);
        p.compile_span_ms = last_end.saturating_sub(first_start.max(start_ms));
        p.after_ms = end_ms.saturating_sub(last_end);
    }
    let denom = (p.units_restored + p.units_compiled) as f64;
    p.cache_rate = if denom > 0.0 { p.units_restored as f64 / denom } else { 0.0 };
    p.coverage = if p.units_compiled > 0 { judged as f64 / p.units_compiled as f64 } else { 1.0 };
    let by_cost = |v: &mut Vec<UnitCost>| v.sort_by(|a, b| b.cpu_ms.cmp(&a.cpu_ms));
    all.sort_by(|a, b| b.cpu_ms.cmp(&a.cpu_ms));
    p.top = all.into_iter().take(5).collect();
    p.regressions.sort_by(|a, b| (b.cpu_ms.saturating_sub(b.typical_ms)).cmp(&a.cpu_ms.saturating_sub(a.typical_ms)));
    p.improvements.sort_by(|a, b| (b.typical_ms.saturating_sub(b.cpu_ms)).cmp(&a.typical_ms.saturating_sub(a.cpu_ms)));
    by_cost(&mut p.new_crates);
    by_cost(&mut p.new_shapes);
    by_cost(&mut p.local_rebuilt);
    by_cost(&mut p.upstream_rebuilds);

    if p.load_allowance < 1.0 {
        p.load_allowance = 1.0;
    }
    let new_cost: u64 = p.new_crates.iter().map(|c| c.cpu_ms).sum();
    let delta = p.judged_cpu_ms as i64 - p.expected_cpu_ms as i64;
    let heavy_ratio = 1.15 * p.load_allowance;
    p.verdict = if p.units_failed > 0 || (!green && p.units_compiled == 0) {
        // a red invocation where no unit compiled = cargo itself failed
        // (bad args, manifest, resolution) — never a "slow no-op"
        "failed".into()
    } else if p.units_compiled == 0 && p.units_failed == 0 {
        "idle".into()
    } else if p.coverage < 0.3 && p.new_crates.is_empty() && p.regressions.is_empty() {
        "learning".into()
    } else if (p.expected_cpu_ms > 0
        && p.judged_cpu_ms as f64 >= p.expected_cpu_ms as f64 * heavy_ratio
        && delta >= 2000)
        || new_cost >= 5000
        || !p.regressions.is_empty() && p.regressions.iter().map(|r| r.cpu_ms - r.typical_ms).sum::<u64>() >= 3000
    {
        "heavier".into()
    } else if p.expected_cpu_ms > 0
        && (p.judged_cpu_ms as f64) <= p.expected_cpu_ms as f64 * 0.87
        && -delta >= 2000
    {
        "lighter".into()
    } else {
        "steady".into()
    };
    p.headline = headline(&p);
    if facts.tests_failed {
        p.headline.push_str(" · ✗ tests FAILED (the build itself was fine)");
    }
    p.levers = levers(&p);
    p
}

/// Map measured conditions to the lever that addresses them.
pub fn levers(p: &Pulse) -> Vec<String> {
    let mut v = Vec::new();
    let share = |x: u64| if p.wall_ms > 0 { x as f64 / p.wall_ms as f64 } else { 0.0 };
    if share(p.lock_wait_build_dir_ms) >= 0.2 || (p.lock_busy_at_start && share(p.before_ms) >= 0.4) {
        v.push("another build owns this target dir — give this lane its own CARGO_TARGET_DIR under /home/storage (cold once, then warm, never queued)".into());
    }
    if share(p.lock_wait_pkg_cache_ms) >= 0.2 {
        v.push("~/.cargo package-cache lock: cargo resolves the whole graph under ONE box-wide exclusive lock on every invocation (measured 5–25 s each at load 40–80, offline too) — batch crates into one call (`-p a -p b`) instead of many short ones; resolution is paid per invocation, not per crate".into());
    }
    if !p.new_crates.is_empty() {
        let cost: u64 = p.new_crates.iter().map(|c| c.cpu_ms).sum();
        let names: Vec<&str> = p.new_crates.iter().take(3).map(|c| c.unit.as_str()).collect();
        v.push(format!(
            "new dependencies cost {} per cold build ({}) — make them optional behind a feature, or check `fluxc pulse deps` for what pulled them in",
            fmt_ms(cost),
            names.join(", ")
        ));
    }
    for r in p.regressions.iter().take(2) {
        if r.src_delta >= 8 * 1024 && r.local {
            v.push(format!(
                "{} grew {} and got {:+}% slower — the edit loop pays this on every change: split the module into its own crate, or move heavy generics behind dyn/non-generic wrappers",
                r.unit,
                fmt_bytes_signed(r.src_delta),
                pct(r.cpu_ms, r.typical_ms)
            ));
        } else if r.local && (r.cause == "self" || r.cause.is_empty()) {
            v.push(format!(
                "{} {:+}% slower after an edit to its own code with ~no source growth — likely a new generic instantiation or proc-macro expansion; `cargo +nightly rustc -- -Zself-profile` on it to see which",
                r.unit,
                pct(r.cpu_ms, r.typical_ms)
            ));
        } else if r.local && r.cause == "dep" {
            v.push(format!(
                "{} {:+}% slower than its usual dependency-triggered rebuild — an upstream crate's interface grew (more generics/inline items crossing the crate boundary)",
                r.unit,
                pct(r.cpu_ms, r.typical_ms)
            ));
        }
    }
    if let Some(u) = p.upstream_rebuilds.first() {
        if u.cpu_ms >= 30_000 {
            let own = if u.other_class_ms > 0 { format!(" (an edit inside it costs ~{})", fmt_ms(u.other_class_ms)) } else { String::new() };
            v.push(format!(
                "{} spent {} rebuilding only because a dependency changed{} — edits to the crates it depends on are the expensive kind; `fluxc pulse blast <dep>` shows which",
                u.unit,
                fmt_ms(u.cpu_ms),
                own
            ));
        }
    }
    if p.churn_units >= 3 {
        v.push(format!(
            "{} unit(s) ({} CPU) recompiled with byte-identical dependencies and source — cargo fingerprint churn: raw cargo, a different RUSTC_WRAPPER/env, or features flipping in this target dir",
            p.churn_units,
            fmt_ms(p.churn_cpu_ms)
        ));
    }
    if p.verdict == "idle" && p.wall_ms >= 5000 && !p.lock_busy_at_start && p.lock_wait_build_dir_ms + p.lock_wait_pkg_cache_ms == 0 {
        v.push("a no-op build should take ~1 s — slow no-ops mean fingerprint churn (env/wrapper/features flipping between invocations) or a huge target dir; compare `fluxc pulse` across two identical runs".into());
    }
    if let Some(t) = p.local_rebuilt.first() {
        if p.local_rebuilt.len() >= 8 && p.units_compiled > 0 {
            v.push(format!(
                "{} local crates rebuilt for one change — run `fluxc pulse blast <crate>` on what you edited; a crate many others depend on is the expensive place to change (heaviest here: {})",
                p.local_rebuilt.len(),
                t.unit
            ));
        }
    }
    v
}

fn headline(p: &Pulse) -> String {
    let mut h = match p.verdict.as_str() {
        "failed" if p.units_failed == 0 => format!(
            "✗ cargo failed before any rustc ran ({} wall) — arguments, manifest or resolution, not code",
            fmt_ms(p.wall_ms)
        ),
        "failed" => format!(
            "✗ failed — {} unit(s) red after {} CPU ({} compiled fine)",
            p.units_failed,
            fmt_ms(p.cpu_ms_total),
            p.units_compiled
        ),
        "idle" if p.wall_ms >= 5000 => format!(
            "◌ no-op took {} — nothing compiled ({} restored), so all of it was cargo: {}",
            fmt_ms(p.wall_ms),
            p.units_restored,
            if p.lock_busy_at_start { "queued behind another build's lock" } else { "fingerprint scan / resolution (lock was free)" }
        ),
        "idle" => format!("◌ idle — {} unit(s) restored from cache, nothing compiled", p.units_restored),
        "learning" => format!(
            "◔ learning — {} unit(s), {} CPU; {:.0}% had a baseline (needs {} prior builds per unit)",
            p.units_compiled,
            fmt_ms(p.cpu_ms_total),
            p.coverage * 100.0,
            MIN_SAMPLES
        ),
        v => {
            let delta = p.judged_cpu_ms as i64 - p.expected_cpu_ms as i64;
            let mark = match v {
                "heavier" => "▲ heavier",
                "lighter" => "▼ lighter",
                _ => "● steady",
            };
            if p.expected_cpu_ms == 0 {
                format!("{} — {} CPU over {} unit(s); none had a baseline yet", mark, fmt_ms(p.cpu_ms_total), p.units_compiled)
            } else {
                format!(
                    "{} — {} CPU over {} unit(s); judged work {} vs typical ({:+}%)",
                    mark,
                    fmt_ms(p.cpu_ms_total),
                    p.units_compiled,
                    fmt_signed_ms(delta),
                    pct(p.judged_cpu_ms, p.expected_cpu_ms)
                )
            }
        }
    };
    if let Some(r) = p.regressions.first() {
        let mut s = format!(
            "; {} {} (typ {}, {:+}%)",
            r.unit,
            fmt_ms(r.cpu_ms),
            fmt_ms(r.typical_ms),
            pct(r.cpu_ms, r.typical_ms)
        );
        if r.src_delta != 0 {
            s.push_str(&format!(" src {}", fmt_bytes_signed(r.src_delta)));
        }
        h.push_str(&s);
    }
    if !p.new_crates.is_empty() {
        let names: Vec<String> = p.new_crates.iter().take(3).map(|c| c.unit.clone()).collect();
        let cost: u64 = p.new_crates.iter().map(|c| c.cpu_ms).sum();
        h.push_str(&format!(
            "; {} new dependency crate(s) +{} [{}]",
            p.new_crates.len(),
            fmt_ms(cost),
            names.join(", ")
        ));
    }
    if let Some(w) = wall_note(p) {
        h.push_str("; ");
        h.push_str(&w);
    }
    if p.load_allowance > 1.05 {
        h.push_str(&format!(
            " [load {:.0} on {} cores: CPU-time tolerance ×{:.2}]",
            p.load1,
            physical_cores() as u64,
            p.load_allowance
        ));
    }
    h
}

/// When most of the wall clock was NOT rustc, say where it went — a slow
/// build that isn't compiling is a lock, a fingerprint storm or a test run,
/// and no amount of code trimming will fix it.
pub fn wall_note(p: &Pulse) -> Option<String> {
    if p.wall_ms < 5000 || p.compile_span_ms + p.before_ms + p.after_ms == 0 {
        return None;
    }
    let share = |x: u64| (x as f64 / p.wall_ms as f64 * 100.0).round() as u64;
    let waited = p.lock_wait_build_dir_ms + p.lock_wait_pkg_cache_ms;
    if share(waited) >= 20 {
        let mut parts = Vec::new();
        if p.lock_wait_pkg_cache_ms > 0 {
            parts.push(format!("{} on the box-wide package cache (~/.cargo — any agent's cargo)", fmt_ms(p.lock_wait_pkg_cache_ms)));
        }
        if p.lock_wait_build_dir_ms > 0 {
            parts.push(format!("{} on this target dir (another build here)", fmt_ms(p.lock_wait_build_dir_ms)));
        }
        return Some(format!(
            "wall {}: cargo reported {} ({}%) blocked on file locks — {}",
            fmt_ms(p.wall_ms),
            fmt_ms(waited),
            share(waited),
            parts.join(", ")
        ));
    }
    if share(p.before_ms) >= 40 {
        let why = if p.lock_busy_at_start {
            "another build held the cargo lock — queueing, not compiling"
        } else {
            "lock was free: cargo resolution / fingerprint scan / build scripts"
        };
        Some(format!(
            "wall {}: {} ({}%) passed before the first rustc — {}",
            fmt_ms(p.wall_ms),
            fmt_ms(p.before_ms),
            share(p.before_ms),
            why
        ))
    } else if share(p.after_ms) >= 50 && p.cmd == "test" {
        Some(format!("wall {}: {} ({}%) after compiling — running the tests", fmt_ms(p.wall_ms), fmt_ms(p.after_ms), share(p.after_ms)))
    } else if share(p.after_ms) >= 50 {
        Some(format!("wall {}: {} ({}%) after the last rustc — build scripts / cargo bookkeeping", fmt_ms(p.wall_ms), fmt_ms(p.after_ms), share(p.after_ms)))
    } else {
        None
    }
}

/// Compact dashboard rows (≤ 50 visible chars each) for combo panels / CLI.
pub fn render_rows(p: &Pulse) -> Vec<String> {
    let mut rows = Vec::new();
    let mark = match p.verdict.as_str() {
        "heavier" => "▲ HEAVIER",
        "lighter" => "▼ LIGHTER",
        "steady" => "● STEADY",
        "learning" => "◔ LEARNING",
        "failed" => "✗ FAILED",
        _ => "◌ IDLE",
    };
    rows.push(format!(
        " PULSE     {}  {} CPU · {} units",
        mark,
        fmt_ms(p.cpu_ms_total),
        p.units_compiled
    ));
    if p.expected_cpu_ms > 0 {
        let delta = p.judged_cpu_ms as i64 - p.expected_cpu_ms as i64;
        rows.push(format!(
            "           vs typical {} ({:+}%) · cover {:.0}%",
            fmt_signed_ms(delta),
            pct(p.judged_cpu_ms, p.expected_cpu_ms),
            p.coverage * 100.0
        ));
    }
    rows.push(format!(
        "           flux-cache {:.0}% ({} restored) · load {:.0}",
        p.cache_rate * 100.0,
        p.units_restored,
        p.load1
    ));
    if p.lock_wait_build_dir_ms + p.lock_wait_pkg_cache_ms > 0 {
        rows.push(format!(
            " LOCKS     {} pkg-cache · {} target-dir (cargo said Blocking)",
            fmt_ms(p.lock_wait_pkg_cache_ms),
            fmt_ms(p.lock_wait_build_dir_ms)
        ));
    }
    if p.compile_span_ms + p.before_ms + p.after_ms > 0 {
        rows.push(format!(
            " WALL      {} = {} wait · {} rustc · {} after",
            fmt_ms(p.wall_ms),
            fmt_ms(p.before_ms),
            fmt_ms(p.compile_span_ms),
            fmt_ms(p.after_ms)
        ));
    }
    if !p.local_rebuilt.is_empty() {
        let names: Vec<String> = p.local_rebuilt.iter().take(3).map(|c| short(&c.unit, 14)).collect();
        rows.push(format!(
            " BLAST     {} local crate(s): {}{}",
            p.local_rebuilt.len(),
            names.join(", "),
            if p.local_rebuilt.len() > 3 { " …" } else { "" }
        ));
    }
    if let Some(u) = p.upstream_rebuilds.first() {
        rows.push(format!(
            " UPSTREAM  {} {} (dep changed{})",
            short(&u.unit, 16),
            fmt_ms(u.cpu_ms),
            if u.other_class_ms > 0 { format!("; own edit ~{}", fmt_ms(u.other_class_ms)) } else { String::new() }
        ));
    }
    if p.churn_units > 0 {
        rows.push(format!(" CHURN     {} unit(s) {} recompiled w/ identical inputs", p.churn_units, fmt_ms(p.churn_cpu_ms)));
    }
    for r in p.regressions.iter().take(2) {
        rows.push(format!(
            " ▲ SLOWER  {} {}→{}{}",
            short(&r.unit, 18),
            fmt_ms(r.typical_ms),
            fmt_ms(r.cpu_ms),
            if r.src_delta != 0 { format!(" src{}", fmt_bytes_signed(r.src_delta)) } else { String::new() }
        ));
    }
    for r in p.improvements.iter().take(1) {
        rows.push(format!(" ▼ FASTER  {} {}→{}", short(&r.unit, 18), fmt_ms(r.typical_ms), fmt_ms(r.cpu_ms)));
    }
    if !p.new_crates.is_empty() {
        let cost: u64 = p.new_crates.iter().map(|c| c.cpu_ms).sum();
        let names: Vec<String> = p.new_crates.iter().take(2).map(|c| short(&c.unit, 14)).collect();
        rows.push(format!(" ✚ NEW DEP {} crate(s) +{}: {}", p.new_crates.len(), fmt_ms(cost), names.join(", ")));
    }
    if let Some(t) = p.top.first() {
        rows.push(format!(" HEAVIEST  {} {}", short(&t.unit, 22), fmt_ms(t.cpu_ms)));
    }
    if p.peak_rss_kb > 0 {
        rows.push(format!(
            " PEAK RSS  {:.1} GB  {}",
            p.peak_rss_kb as f64 / 1048576.0,
            short(&p.peak_rss_unit, 22)
        ));
    }
    for l in p.levers.iter().take(2) {
        rows.push(format!(" LEVER     {}", short(l, 72)));
    }
    rows
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let t: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{}…", t)
    }
}

fn read_units(path: &Path) -> Vec<UnitSample> {
    let Ok(txt) = std::fs::read_to_string(path) else { return Vec::new() };
    txt.lines().filter_map(|l| serde_json::from_str::<UnitSample>(l).ok()).collect()
}

/// Inv files left by a parent that died before finishing (killed build):
/// move them into the day log so their samples still count for drift.
fn sweep_orphans(dir: &Path) {
    let inv = dir.join("inv");
    let Ok(rd) = std::fs::read_dir(&inv) else { return };
    let now = std::time::SystemTime::now();
    for e in rd.flatten() {
        let p = e.path();
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .map(|d| d.as_secs() > 6 * 3600)
            .unwrap_or(false);
        if !old {
            continue;
        }
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name.ends_with(".units.jsonl") {
            let units = read_units(&p);
            let mut by_day: BTreeMap<PathBuf, String> = BTreeMap::new();
            for u in units {
                if let Ok(l) = serde_json::to_string(&u) {
                    by_day.entry(day_log_path(dir, u.ts)).or_default().push_str(&(l + "\n"));
                }
            }
            for (path, lines) in by_day {
                append_lines(&path, &lines);
            }
            let _ = std::fs::remove_file(&p);
        } else if name.ends_with(".json") {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// Parent side: judge this invocation, persist, fold into baselines.
/// Returns None when nothing was recorded (pulse disabled, no rustc ran).
pub fn finish_invocation(inv: &str, cmd: &str, package: Option<&str>, release: bool, green: bool, wall_ms: u64) -> Option<Pulse> {
    let start_ms = now_ms().saturating_sub(wall_ms);
    finish_invocation_at(inv, cmd, package, release, green, wall_ms, start_ms)
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn finish_invocation_at(inv: &str, cmd: &str, package: Option<&str>, release: bool, green: bool, wall_ms: u64, start_ms: u64) -> Option<Pulse> {
    finish_invocation_full(inv, cmd, package, release, green, wall_ms, start_ms, false)
}

#[allow(clippy::too_many_arguments)]
pub fn finish_invocation_full(inv: &str, cmd: &str, package: Option<&str>, release: bool, green: bool, wall_ms: u64, start_ms: u64, lock_busy: bool) -> Option<Pulse> {
    finish_invocation_facts(inv, cmd, package, release, green, wall_ms,
        &InvocationFacts { start_ms, lock_busy_at_start: lock_busy, ..Default::default() })
}

pub fn finish_invocation_facts(inv: &str, cmd: &str, package: Option<&str>, release: bool, green: bool, wall_ms: u64, facts: &InvocationFacts) -> Option<Pulse> {
    if !enabled() || inv.is_empty() {
        return None;
    }
    let dir = pulse_dir();
    let inv_units = dir.join("inv").join(format!("{}.units.jsonl", inv));
    let units = read_units(&inv_units);
    let pulse = {
        // A no-op build has nothing to judge or fold: skip the lock and the
        // baselines read entirely (keeps the no-op overhead near zero).
        let _lock = if units.is_empty() { DirLock(None) } else { DirLock::acquire(&dir) };
        let mut base = if units.is_empty() { Baselines::default() } else { load_baselines(&dir) };
        // Only pay for a lockfile diff when an unknown external crate showed up.
        let mut facts = facts.clone();
        if facts.lock_added.is_none()
            && units.iter().any(|u| !u.local && u.outcome == "compiled" && !base.knows_crate(&u.pkg, &u.crate_name))
        {
            facts.lock_added = lock_added_vs_head(&std::env::current_dir().unwrap_or_default());
        }
        let pulse = judge_facts(&units, &base, (inv, cmd, package.unwrap_or("<workspace>"), release, green, wall_ms), &facts);
        for u in &units {
            base.fold(u);
        }
        if !units.is_empty() {
            // Shapes nobody has built for 45 days (removed crates, old opt-levels)
            // only slow every build's read-modify-write of this file.
            let cutoff = now_unix().saturating_sub(45 * 86_400);
            base.units.retain(|_, v| v.iter().any(|s| s.ts >= cutoff));
            save_baselines(&dir, &base);
        }
        pulse
    };
    // Move the unit lines into the day log (drift history), then drop the inv file.
    if !units.is_empty() {
        let mut lines = String::new();
        for u in &units {
            if let Ok(l) = serde_json::to_string(u) {
                lines.push_str(&l);
                lines.push('\n');
            }
        }
        append_lines(&day_log_path(&dir, now_unix()), &lines);
    }
    let _ = std::fs::remove_file(&inv_units);
    if let Ok(j) = serde_json::to_string_pretty(&pulse) {
        let _ = std::fs::write(dir.join("inv").join(format!("{}.json", inv)), &j);
        if pulse.units_compiled + pulse.units_failed > 0 || pulse.wall_ms >= 5000 {
            let _ = std::fs::write(dir.join("last.json"), &j);
        }
    }
    sweep_orphans(&dir);
    Some(pulse)
}

/// The pulse a specific invocation produced (written by `finish_invocation`).
pub fn read_invocation(inv: &str) -> Option<Pulse> {
    let p = pulse_dir().join("inv").join(format!("{}.json", inv));
    let txt = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p); // consumed: one reader per invocation
    serde_json::from_str(&txt).ok()
}

pub fn read_last() -> Option<Pulse> {
    let txt = std::fs::read_to_string(pulse_dir().join("last.json")).ok()?;
    serde_json::from_str(&txt).ok()
}

// ── webhook ─────────────────────────────────────────────────────────────────

/// Fire `compile_pulse`. A CLI build exits the moment cargo is done, which
/// would kill the fire-and-forget sink's threads before curl even starts, so
/// prefer the blocking sink — bounded to 3 s so a dead endpoint can never
/// hold a build hostage. The MCP server (long-lived) is fine either way.
pub fn dispatch(p: &Pulse) {
    let Ok(v) = serde_json::to_value(p) else { return };
    if fluxc_util::hooks::has_blocking_webhook_sink() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            fluxc_util::hooks::dispatch_webhook_blocking("compile_pulse", v);
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(std::time::Duration::from_secs(3));
    } else {
        fluxc_util::hooks::dispatch_webhook("compile_pulse", v);
    }
}

// ── drift: the slow creep across days ──────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Drift {
    pub unit: String,
    pub key: String,
    pub local: bool,
    pub prior_ms: u64,
    pub recent_ms: u64,
    pub prior_n: usize,
    pub recent_n: usize,
    pub prior_src: u64,
    pub recent_src: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DriftReport {
    pub days: u64,
    pub recent_days: u64,
    pub samples: usize,
    pub keys: usize,
    /// Σ median CPU over units seen in BOTH windows — the same work, then vs now.
    pub prior_total_ms: u64,
    pub recent_total_ms: u64,
    pub growing: Vec<Drift>,
    pub shrinking: Vec<Drift>,
}

fn median(v: &mut [u64]) -> u64 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[v.len() / 2]
}

pub fn drift(days: u64, recent_days: u64, top: usize) -> DriftReport {
    let dir = pulse_dir();
    let now = now_unix();
    let cutoff_recent = now.saturating_sub(recent_days * 86_400);
    let mut rep = DriftReport { days, recent_days, ..Default::default() };
    // key → (label, local, prior cpu, recent cpu, prior src, recent src)
    struct Acc {
        label: String,
        local: bool,
        prior: Vec<u64>,
        recent: Vec<u64>,
        prior_src: Vec<u64>,
        recent_src: Vec<u64>,
    }
    let mut acc: BTreeMap<String, Acc> = BTreeMap::new();
    for d in 0..=days {
        let path = day_log_path(&dir, now.saturating_sub(d * 86_400));
        for u in read_units(&path) {
            if u.outcome != "compiled" || u.cpu_ms == 0 {
                continue;
            }
            rep.samples += 1;
            let a = acc.entry(u.key()).or_insert_with(|| Acc {
                label: u.label(),
                local: u.local,
                prior: vec![],
                recent: vec![],
                prior_src: vec![],
                recent_src: vec![],
            });
            if u.ts >= cutoff_recent {
                a.recent.push(u.cpu_ms);
                if u.src_bytes > 0 {
                    a.recent_src.push(u.src_bytes);
                }
            } else {
                a.prior.push(u.cpu_ms);
                if u.src_bytes > 0 {
                    a.prior_src.push(u.src_bytes);
                }
            }
        }
    }
    rep.keys = acc.len();
    let mut all: Vec<Drift> = Vec::new();
    for (k, mut a) in acc {
        if a.prior.len() < 2 || a.recent.len() < 2 {
            continue;
        }
        let d = Drift {
            unit: a.label.clone(),
            key: k,
            local: a.local,
            prior_ms: median(&mut a.prior),
            recent_ms: median(&mut a.recent),
            prior_n: a.prior.len(),
            recent_n: a.recent.len(),
            prior_src: median(&mut a.prior_src),
            recent_src: median(&mut a.recent_src),
        };
        rep.prior_total_ms += d.prior_ms;
        rep.recent_total_ms += d.recent_ms;
        all.push(d);
    }
    let delta = |d: &Drift| d.recent_ms as i64 - d.prior_ms as i64;
    let mut growing: Vec<Drift> = all.iter().filter(|d| delta(d) >= 200).cloned().collect();
    growing.sort_by_key(|d| std::cmp::Reverse(delta(d)));
    let mut shrinking: Vec<Drift> = all.iter().filter(|d| delta(d) <= -200).cloned().collect();
    shrinking.sort_by_key(|d| delta(d));
    rep.growing = growing.into_iter().take(top).collect();
    rep.shrinking = shrinking.into_iter().take(top).collect();
    rep
}

pub fn render_drift(r: &DriftReport) -> String {
    let mut out = vec![format!(
        "⏱ compile drift — last {} day(s) vs the {} before · {} samples · {} unit shapes",
        r.recent_days,
        r.days.saturating_sub(r.recent_days),
        r.samples,
        r.keys
    )];
    if r.prior_total_ms > 0 {
        out.push(format!(
            "  same work, then → now: {} → {} CPU ({:+}%)",
            fmt_ms(r.prior_total_ms),
            fmt_ms(r.recent_total_ms),
            pct(r.recent_total_ms, r.prior_total_ms)
        ));
    } else {
        out.push("  (not enough history in both windows yet — drift needs ≥2 samples per unit on each side)".into());
    }
    let row = |d: &Drift| {
        let src = if d.prior_src > 0 && d.recent_src > 0 && d.recent_src != d.prior_src {
            format!("  src {}", fmt_bytes_signed(d.recent_src as i64 - d.prior_src as i64))
        } else {
            String::new()
        };
        format!(
            "    {}{:<34} {:>7} → {:>7} ({:+}%){}",
            if d.local { "◆ " } else { "  " },
            short(&d.unit, 34),
            fmt_ms(d.prior_ms),
            fmt_ms(d.recent_ms),
            pct(d.recent_ms, d.prior_ms),
            src
        )
    };
    if !r.growing.is_empty() {
        out.push("  ▲ growing (◆ = your code):".into());
        out.extend(r.growing.iter().map(row));
    }
    if !r.shrinking.is_empty() {
        out.push("  ▼ shrinking:".into());
        out.extend(r.shrinking.iter().map(row));
    }
    out.join("\n")
}

// ── rebuild tax: where development CPU actually goes ────────────────────────

/// One unit's share of the compile CPU spent over a window. A crate that is
/// cheap but rebuilt 200 times a day can cost more than a heavy one built
/// twice — and only measured frequency tells the two apart.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaxRow {
    pub unit: String,
    pub pkg: String,
    pub local: bool,
    pub builds: usize,
    pub cpu_total_ms: u64,
    pub median_ms: u64,
    pub src_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaxReport {
    pub days: u64,
    pub units_compiled: usize,
    pub cpu_total_ms: u64,
    pub cpu_local_ms: u64,
    pub rows: Vec<TaxRow>,
}

pub fn rebuild_tax(days: u64, top: usize, local_only: bool) -> TaxReport {
    let dir = pulse_dir();
    let now = now_unix();
    let cutoff = now.saturating_sub(days * 86_400);
    let mut rep = TaxReport { days, ..Default::default() };
    let mut acc: BTreeMap<String, (TaxRow, Vec<u64>)> = BTreeMap::new();
    for d in 0..=days {
        for u in read_units(&day_log_path(&dir, now.saturating_sub(d * 86_400))) {
            if u.outcome != "compiled" || u.ts < cutoff {
                continue;
            }
            rep.units_compiled += 1;
            rep.cpu_total_ms += u.cpu_ms;
            if u.local {
                rep.cpu_local_ms += u.cpu_ms;
            }
            if local_only && !u.local {
                continue;
            }
            let e = acc.entry(u.key()).or_insert_with(|| {
                (TaxRow { unit: u.label(), pkg: u.pkg.clone(), local: u.local, ..Default::default() }, vec![])
            });
            e.0.builds += 1;
            e.0.cpu_total_ms += u.cpu_ms;
            if u.src_bytes > 0 {
                e.0.src_bytes = u.src_bytes;
            }
            e.1.push(u.cpu_ms);
        }
    }
    let mut rows: Vec<TaxRow> = acc
        .into_values()
        .map(|(mut r, mut v)| {
            r.median_ms = median(&mut v);
            r
        })
        .collect();
    rows.sort_by(|a, b| b.cpu_total_ms.cmp(&a.cpu_total_ms));
    rep.rows = rows.into_iter().take(top).collect();
    rep
}

pub fn render_tax(r: &TaxReport) -> String {
    let mut out = vec![format!(
        "💸 rebuild tax — last {} day(s): {} units compiled, {} CPU ({} of it your code)",
        r.days,
        r.units_compiled,
        fmt_ms(r.cpu_total_ms),
        fmt_ms(r.cpu_local_ms)
    )];
    out.push(format!("    {:<36} {:>6} {:>8} {:>8} {:>9}", "unit", "builds", "median", "total", "src"));
    for t in &r.rows {
        out.push(format!(
            "    {}{:<34} {:>6} {:>8} {:>8} {:>9}",
            if t.local { "◆ " } else { "  " },
            short(&t.unit, 34),
            t.builds,
            fmt_ms(t.median_ms),
            fmt_ms(t.cpu_total_ms),
            if t.src_bytes > 0 { format!("{:.0} KB", t.src_bytes as f64 / 1024.0) } else { "-".into() }
        ));
    }
    if r.rows.is_empty() {
        out.push("    (no compiled units recorded in this window yet)".into());
    }
    out.join("\n")
}

// ── ambient: one line for a status bar, a block for session start ──────────

/// ≤ ~60 chars: the newest verdict, for a status line or a prompt.
pub fn status_line() -> String {
    let Some(p) = read_last() else { return "⏱ pulse: no builds yet".into() };
    let age = now_unix().saturating_sub(p.ts);
    let when = if age < 120 { format!("{}s", age) } else if age < 7200 { format!("{}m", age / 60) } else { format!("{}h", age / 3600) };
    let mark = match p.verdict.as_str() {
        "heavier" => "▲",
        "lighter" => "▼",
        "steady" => "●",
        "learning" => "◔",
        "failed" => "✗",
        _ => "◌",
    };
    let what = if let Some(r) = p.regressions.first() {
        format!("{} {:+}%", short(&r.unit, 18), pct(r.cpu_ms, r.typical_ms))
    } else if p.expected_cpu_ms > 0 {
        format!("{:+}% vs typ", pct(p.judged_cpu_ms, p.expected_cpu_ms))
    } else {
        format!("{} units", p.units_compiled)
    };
    format!("⏱ {} {} {} · {} CPU · {} ago", mark, short(&p.package, 14), what, fmt_ms(p.cpu_ms_total), when)
}

/// A few lines for session start: what's been expensive lately and what's
/// creeping, so planning begins from cost, not from a guess.
pub fn weather() -> String {
    let tax = rebuild_tax(1, 3, true);
    let dr = drift(10, 3, 3); // bounded: quickstart must stay fast even after a heavy month
    let mut out = vec!["## Compile weather (fluxc pulse — measured)".to_string()];
    out.push(format!(
        "  last 24h: {} units compiled, {} CPU ({} your code)",
        tax.units_compiled,
        fmt_ms(tax.cpu_total_ms),
        fmt_ms(tax.cpu_local_ms)
    ));
    if !tax.rows.is_empty() {
        let hot: Vec<String> = tax.rows.iter().map(|t| format!("{} {}×{}", t.unit, t.builds, fmt_ms(t.median_ms))).collect();
        out.push(format!("  hottest: {}", hot.join(" · ")));
    }
    if let Some(g) = dr.growing.first() {
        out.push(format!(
            "  creeping: {} {} → {} ({:+}%)",
            g.unit,
            fmt_ms(g.prior_ms),
            fmt_ms(g.recent_ms),
            pct(g.recent_ms, g.prior_ms)
        ));
    }
    out.push(format!("  last: {}", status_line()));
    out.push("  before editing a crate: flux_compile_pulse view=blast crate=<it>".into());
    out.join("\n")
}

// ── blast radius: what an edit WILL cost, before you make it ───────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Blast {
    pub workspace: String,
    pub target: String,
    pub crates: Vec<(String, u64)>,
    pub known: usize,
    pub cpu_ms: u64,
    pub critical_path_ms: u64,
    pub critical_path: Vec<String>,
    pub mode: String,
}

/// Typical CPU for rebuilding one package's units under `mode` ("build" or
/// "check"): the lib/bin units (not tests), summed over their newest shape.
fn package_cost(base: &Baselines, pkg: &str, mode: &str, opt: Option<&str>) -> Option<u64> {
    let prefix = format!("{}|", pkg);
    // crate_name|kind → (newest ts, median)
    let mut best: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for (k, v) in base.units.range(prefix.clone()..) {
        if !k.starts_with(&prefix) {
            break;
        }
        let parts: Vec<&str> = k.split('|').collect();
        if parts.len() < 7 || parts[3] != mode || parts[4] == "test" {
            continue;
        }
        if let Some(o) = opt {
            if parts[5] != format!("o{}", o) {
                continue;
            }
        }
        let newest = v.iter().map(|s| s.ts).max().unwrap_or(0);
        let mut c: Vec<u64> = v.iter().map(|s| s.cpu_ms).collect();
        let m = median(&mut c);
        let slot = format!("{}|{}", parts[1], parts[2]);
        let e = best.entry(slot).or_insert((0, 0));
        if newest >= e.0 {
            *e = (newest, m);
        }
    }
    if best.is_empty() {
        None
    } else {
        Some(best.values().map(|(_, m)| *m).sum())
    }
}

pub fn blast_radius(workspace: &Path, target: &str, mode: &str, opt: Option<&str>) -> Result<Blast, String> {
    let ws = flux_graph::resolve_workspace(&workspace.to_path_buf())?;
    let Some(t_idx) = ws.crates.iter().position(|c| c.name == target) else {
        return Err(format!("crate '{}' is not a member of {}", target, workspace.display()));
    };
    let dag = flux_graph::graph::build_dag(&ws.crates)?;
    let mut affected: BTreeSet<usize> = flux_graph::agility::transitive_dependents(&ws, target).into_iter().collect();
    affected.insert(t_idx);
    let base = load_baselines(&pulse_dir());
    let mut b = Blast {
        workspace: workspace.display().to_string(),
        target: target.into(),
        mode: mode.into(),
        ..Default::default()
    };
    let mut cost: BTreeMap<usize, u64> = BTreeMap::new();
    for &i in &affected {
        let name = &ws.crates[i].name;
        let c = package_cost(&base, name, mode, opt);
        if c.is_some() {
            b.known += 1;
        }
        let c = c.unwrap_or(0);
        cost.insert(i, c);
        b.cpu_ms += c;
        b.crates.push((name.clone(), c));
    }
    b.crates.sort_by(|a, b| b.1.cmp(&a.1));
    // Critical path through the affected sub-DAG: the wall-clock floor even
    // with infinite cores, since a crate can't start before its deps finish.
    let order: Vec<usize> = ws.batches.iter().flatten().copied().filter(|i| affected.contains(i)).collect();
    let mut finish: BTreeMap<usize, (u64, Option<usize>)> = BTreeMap::new();
    for &i in &order {
        let (start, via) = dag.depends_on[i]
            .iter()
            .filter_map(|d| finish.get(d).map(|(f, _)| (*f, *d)))
            .max_by_key(|(f, _)| *f)
            .map(|(f, d)| (f, Some(d)))
            .unwrap_or((0, None));
        finish.insert(i, (start + cost.get(&i).copied().unwrap_or(0), via));
    }
    if let Some((&end, &(ms, _))) = finish.iter().max_by_key(|(_, (f, _))| *f) {
        b.critical_path_ms = ms;
        let mut cur = Some(end);
        while let Some(c) = cur {
            b.critical_path.push(ws.crates[c].name.clone());
            cur = finish.get(&c).and_then(|(_, v)| *v);
        }
        b.critical_path.reverse();
    }
    Ok(b)
}

pub fn render_blast(b: &Blast) -> String {
    let mut out = vec![format!(
        "💥 blast radius — editing `{}` rebuilds {} crate(s) [{} mode] in {}",
        b.target,
        b.crates.len(),
        b.mode,
        b.workspace
    )];
    out.push(format!(
        "  ≈ {} CPU total · critical path ≈ {} wall (infinite cores) · {}/{} crates have a baseline",
        fmt_ms(b.cpu_ms),
        fmt_ms(b.critical_path_ms),
        b.known,
        b.crates.len()
    ));
    if !b.critical_path.is_empty() {
        out.push(format!("  critical path: {}", b.critical_path.join(" → ")));
    }
    out.push("  heaviest rebuilds:".into());
    for (n, c) in b.crates.iter().take(8) {
        out.push(format!("    {:<32} {}", n, if *c > 0 { fmt_ms(*c) } else { "?".into() }));
    }
    if b.known < b.crates.len() {
        out.push("  (? = never built through fluxc since pulse began — cost unknown, not zero)".into());
    }
    out.join("\n")
}

// ── dependency diff: what a Cargo.lock change brings in ────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DepDiff {
    pub added: Vec<(String, String, u64)>,
    pub removed: Vec<(String, String)>,
    pub known_cost_ms: u64,
    pub unknown: usize,
}

fn lock_packages(txt: &str) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    let Ok(v) = txt.parse::<toml::Value>() else { return out };
    if let Some(arr) = v.get("package").and_then(|p| p.as_array()) {
        for p in arr {
            let n = p.get("name").and_then(|x| x.as_str()).unwrap_or("");
            let ver = p.get("version").and_then(|x| x.as_str()).unwrap_or("");
            if !n.is_empty() {
                out.insert((n.to_string(), ver.to_string()));
            }
        }
    }
    out
}

/// Names of packages present in `<ws>/Cargo.lock` but absent from
/// `HEAD:Cargo.lock` (walks up from `start` to the lockfile). None when there
/// is no lockfile or git cannot answer — callers must then not guess.
pub fn lock_added_vs_head(start: &Path) -> Option<BTreeSet<String>> {
    let mut dir = start.to_path_buf();
    let ws = loop {
        if dir.join("Cargo.lock").exists() {
            break dir;
        }
        if !dir.pop() {
            return None;
        }
    };
    let now = std::fs::read_to_string(ws.join("Cargo.lock")).ok()?;
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&ws)
        .args(["show", "HEAD:Cargo.lock"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let then: BTreeSet<String> = lock_packages(&String::from_utf8_lossy(&out.stdout)).into_iter().map(|(n, _)| n).collect();
    Some(lock_packages(&now).into_iter().map(|(n, _)| n).filter(|n| !then.contains(n)).collect())
}

/// Cargo.lock in the working tree vs `git show <rev>:Cargo.lock`.
pub fn dep_diff(workspace: &Path, rev: &str) -> Result<DepDiff, String> {
    let now = std::fs::read_to_string(workspace.join("Cargo.lock")).map_err(|e| format!("Cargo.lock: {}", e))?;
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["show", &format!("{}:Cargo.lock", rev)])
        .output()
        .map_err(|e| format!("git: {}", e))?;
    if !out.status.success() {
        return Err(format!("git show {}:Cargo.lock failed", rev));
    }
    let then = String::from_utf8_lossy(&out.stdout).to_string();
    let (a, b) = (lock_packages(&then), lock_packages(&now));
    let names_then: BTreeSet<&String> = a.iter().map(|(n, _)| n).collect();
    let base = load_baselines(&pulse_dir());
    let mut d = DepDiff::default();
    for (n, v) in b.difference(&a) {
        // a version bump of a known name is still a recompile, but it's not a NEW crate
        let c = package_cost(&base, n, "build", None).or_else(|| package_cost(&base, n, "check", None)).unwrap_or(0);
        if c > 0 {
            d.known_cost_ms += c;
        } else {
            d.unknown += 1;
        }
        let tag = if names_then.contains(n) { format!("{} (bump)", v) } else { v.clone() };
        d.added.push((n.clone(), tag, c));
    }
    for (n, v) in a.difference(&b) {
        d.removed.push((n.clone(), v.clone()));
    }
    d.added.sort_by(|x, y| y.2.cmp(&x.2));
    Ok(d)
}

/// What adding one dependency would bring into a workspace — BEFORE adding it.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DepCost {
    pub spec: String,
    pub workspace: String,
    /// Packages the candidate pulls in that the workspace does not have yet.
    pub new_packages: Vec<(String, u64, u64)>, // (name, check_ms, build_ms) — 0 = never measured
    pub already_present: usize,
    pub check_ms: u64,
    pub build_ms: u64,
    pub unknown: usize,
}

/// Resolve `spec` ("name" or "name@ver") in a scratch manifest (host platform
/// only), subtract the workspace's Cargo.lock, price the rest from baselines.
pub fn dep_cost(workspace: &Path, spec: &str, features: &[String]) -> Result<DepCost, String> {
    let (name, ver) = match spec.split_once('@') {
        Some((n, v)) => (n.trim().to_string(), v.trim().to_string()),
        None => (spec.trim().to_string(), "*".to_string()),
    };
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("bad crate name '{}'", name));
    }
    let probe = pulse_dir().join("probe").join(blake3::hash(format!("{}|{}|{:?}", name, ver, features).as_bytes()).to_hex()[..12].to_string());
    std::fs::create_dir_all(probe.join("src")).map_err(|e| format!("probe dir: {}", e))?;
    let feats = if features.is_empty() {
        String::new()
    } else {
        format!(", features = [{}]", features.iter().map(|f| format!("{:?}", f)).collect::<Vec<_>>().join(", "))
    };
    let manifest = format!(
        "[package]\nname = \"flux-pulse-probe\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[workspace]\n\n[dependencies]\n{} = {{ version = {:?}{} }}\n",
        name, ver, feats
    );
    std::fs::write(probe.join("Cargo.toml"), manifest).map_err(|e| format!("probe manifest: {}", e))?;
    let _ = std::fs::write(probe.join("src/lib.rs"), "");
    let host = std::process::Command::new("rustc").arg("-vV").output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("host: ").map(String::from)))
        .unwrap_or_else(|| "x86_64-unknown-linux-gnu".into());
    let run = |offline: bool| {
        let mut c = std::process::Command::new("cargo");
        c.args(["metadata", "--format-version", "1", "--filter-platform", &host, "--manifest-path"])
            .arg(probe.join("Cargo.toml"));
        if offline {
            c.arg("--offline");
        }
        c.stderr(std::process::Stdio::null()).output()
    };
    let out = match run(true) {
        Ok(o) if o.status.success() => o,
        _ => run(false).map_err(|e| format!("cargo metadata: {}", e))?,
    };
    if !out.status.success() {
        return Err(format!("could not resolve '{}' (does that crate/version exist?)", spec));
    }
    let j: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("metadata json: {}", e))?;
    // Resolved (platform-filtered) node ids → package names.
    let mut id_to_name: BTreeMap<String, String> = BTreeMap::new();
    if let Some(pk) = j["packages"].as_array() {
        for p in pk {
            if let (Some(id), Some(n)) = (p["id"].as_str(), p["name"].as_str()) {
                id_to_name.insert(id.to_string(), n.to_string());
            }
        }
    }
    let mut pulled: BTreeSet<String> = BTreeSet::new();
    if let Some(nodes) = j["resolve"]["nodes"].as_array() {
        for nd in nodes {
            if let Some(n) = nd["id"].as_str().and_then(|id| id_to_name.get(id)) {
                if n != "flux-pulse-probe" {
                    pulled.insert(n.clone());
                }
            }
        }
    }
    let have: BTreeSet<String> = std::fs::read_to_string(workspace.join("Cargo.lock"))
        .map(|t| lock_packages(&t).into_iter().map(|(n, _)| n).collect())
        .unwrap_or_default();
    let base = load_baselines(&pulse_dir());
    let mut c = DepCost { spec: spec.into(), workspace: workspace.display().to_string(), ..Default::default() };
    for n in &pulled {
        if have.contains(n) {
            c.already_present += 1;
            continue;
        }
        let ck = package_cost(&base, n, "check", None).unwrap_or(0);
        let bd = package_cost(&base, n, "build", None).unwrap_or(0);
        if ck == 0 && bd == 0 {
            c.unknown += 1;
        }
        c.check_ms += ck;
        c.build_ms += bd;
        c.new_packages.push((n.clone(), ck, bd));
    }
    c.new_packages.sort_by(|a, b| (b.1 + b.2).cmp(&(a.1 + a.2)));
    Ok(c)
}

pub fn render_dep_cost(c: &DepCost) -> String {
    let mut out = vec![format!(
        "🧾 adding `{}` to {} → {} new package(s) ({} already there)",
        c.spec,
        c.workspace,
        c.new_packages.len(),
        c.already_present
    )];
    out.push(format!(
        "  measured cost per cold build: check ≈ {} · build ≈ {}{}",
        fmt_ms(c.check_ms),
        fmt_ms(c.build_ms),
        if c.unknown > 0 { format!("  (+{} never measured here — cost unknown, not zero)", c.unknown) } else { String::new() }
    ));
    for (n, ck, bd) in c.new_packages.iter().take(12) {
        let f = |x: &u64| if *x > 0 { fmt_ms(*x) } else { "?".into() };
        out.push(format!("    + {:<28} check {:>7}  build {:>7}", n, f(ck), f(bd)));
    }
    if c.new_packages.len() > 12 {
        out.push(format!("    … {} more", c.new_packages.len() - 12));
    }
    out.join("\n")
}

pub fn render_dep_diff(d: &DepDiff, rev: &str) -> String {
    if d.added.is_empty() && d.removed.is_empty() {
        return format!("📦 Cargo.lock unchanged vs {} — no dependency consequences", rev);
    }
    let mut out = vec![format!(
        "📦 dependency diff vs {} — +{} / -{} package versions · known extra CPU per cold build ≈ {}{}",
        rev,
        d.added.len(),
        d.removed.len(),
        fmt_ms(d.known_cost_ms),
        if d.unknown > 0 { format!(" + {} never-built (unknown)", d.unknown) } else { String::new() }
    )];
    for (n, v, c) in d.added.iter().take(15) {
        out.push(format!("    + {:<28} {:<16} {}", n, v, if *c > 0 { fmt_ms(*c) } else { "?".into() }));
    }
    for (n, v) in d.removed.iter().take(10) {
        out.push(format!("    - {:<28} {}", n, v));
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(pkg: &str, cpu: u64, outcome: &str, local: bool) -> UnitSample {
        UnitSample {
            ts: now_unix(),
            inv: "t".into(),
            pkg: pkg.into(),
            crate_name: pkg.replace('-', "_"),
            kind: "lib".into(),
            mode: "build".into(),
            opt: "0".into(),
            target: "host".into(),
            local,
            outcome: outcome.into(),
            wall_ms: cpu,
            cpu_ms: cpu,
            src_bytes: 1000,
            ..Default::default()
        }
    }

    fn base_with(pkg: &str, cpus: &[u64]) -> Baselines {
        let mut b = Baselines::default();
        for &c in cpus {
            b.fold(&s(pkg, c, "compiled", true));
        }
        b
    }

    #[test]
    fn civil_date_matches_known_days() {
        assert_eq!(civil_date(0), (1970, 1, 1));
        assert_eq!(civil_date(951_782_400), (2000, 2, 29)); // leap day
        assert_eq!(civil_date(1_790_985_600), (2026, 10, 3));
    }

    #[test]
    fn shape_reads_check_vs_build_opt_and_lto() {
        let a: Vec<String> = ["--crate-name", "x", "--edition=2021", "src/lib.rs", "--crate-type", "lib",
            "--emit=dep-info,metadata", "-C", "opt-level=3", "-C", "lto=thin"]
            .iter().map(|s| s.to_string()).collect();
        let sh = shape_from_args(&a);
        assert_eq!((sh.kind.as_str(), sh.mode.as_str(), sh.opt.as_str(), sh.target.as_str()),
            ("lib", "check", "3+lto", "host"));
        let b: Vec<String> = ["src/main.rs", "--crate-type", "bin", "--emit=dep-info,link",
            "-Copt-level=1", "--target", "x86_64-pc-windows-gnu"].iter().map(|s| s.to_string()).collect();
        let sh = shape_from_args(&b);
        assert_eq!((sh.kind.as_str(), sh.mode.as_str(), sh.opt.as_str(), sh.target.as_str()),
            ("bin", "build", "1", "x86_64-pc-windows-gnu"));
        assert_eq!(sh.feat, "");
        let f = |v: &[&str]| shape_from_args(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>()).feat;
        let ab = f(&["x.rs", "--cfg", "feature=\"a\"", "--cfg", "feature=\"b\""]);
        assert_eq!(ab, f(&["x.rs", "--cfg", "feature=\"b\"", "--cfg", "feature=\"a\""]), "order-independent");
        assert_ne!(ab, f(&["x.rs", "--cfg", "feature=\"a\""]));
    }

    #[test]
    fn local_vs_registry_manifest() {
        assert!(is_local_manifest("/home/storage/deepseek-codewhale/sigil/crates/sigil-node"));
        assert!(!is_local_manifest("/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/serde-1.0.200"));
        assert!(!is_local_manifest("/root/.cargo/git/checkouts/foo-abc/123"));
    }

    #[test]
    fn steady_build_is_steady() {
        let base = base_with("sigil-node", &[30_000, 31_000, 29_500, 30_500]);
        let p = judge(&[s("sigil-node", 30_800, "compiled", true)], &base, ("i", "build", "sigil-node", false, true, 31_000));
        assert_eq!(p.verdict, "steady", "{}", p.headline);
        assert!(p.regressions.is_empty());
        assert_eq!(p.local_rebuilt.len(), 1);
    }

    #[test]
    fn regression_is_named_with_its_size() {
        let base = base_with("sigil-node", &[30_000, 31_000, 29_500, 30_500]);
        let mut u = s("sigil-node", 42_000, "compiled", true);
        u.src_bytes = 1000 + 6 * 1024;
        let p = judge(&[u], &base, ("i", "build", "sigil-node", false, true, 42_000));
        assert_eq!(p.verdict, "heavier", "{}", p.headline);
        assert_eq!(p.regressions.len(), 1);
        assert_eq!(p.regressions[0].src_delta, 6 * 1024);
        assert!(p.headline.contains("sigil-node"), "{}", p.headline);
        assert!(p.headline.contains("+6.0 KB"), "{}", p.headline);
        assert!(p.levers.iter().any(|l| l.contains("sigil-node")), "regression must yield a lever: {:?}", p.levers);
    }

    #[test]
    fn saturated_box_widens_tolerance_but_never_narrows_it() {
        let phys = physical_cores();
        assert!((load_allowance(phys * 0.5, phys * 0.5) - 1.0).abs() < 1e-9);
        assert!((load_allowance(phys * 3.0, 0.0) - 1.5).abs() < 1e-9, "quiet-unknown baseline vs saturated now");
        assert!((load_allowance(phys * 3.0, phys * 3.0) - 1.0).abs() < 1e-9, "same contention both sides cancels");
        assert!((load_allowance(phys * 0.1, phys * 3.0) - 1.0).abs() < 1e-9, "a quieter box never tightens");
        // +52% at load 2.5×phys vs a quiet baseline is within tolerance — the fluxc false alarm
        let base = base_with("fluxc", &[6000, 6100, 6200]);
        let mut u = s("fluxc", 9_200, "compiled", true);
        u.load1 = phys * 2.5;
        let p = judge(&[u], &base, ("i", "build", "fluxc", false, true, 9_200));
        assert!(p.regressions.is_empty(), "{}", p.headline);
        // the same +52% on a quiet box IS a regression
        let mut q = s("fluxc", 9_200, "compiled", true);
        q.load1 = phys * 0.3;
        let p = judge(&[q], &base, ("i", "build", "fluxc", false, true, 9_200));
        assert_eq!(p.regressions.len(), 1, "{}", p.headline);
    }

    #[test]
    fn noise_below_absolute_floor_is_not_a_regression() {
        // 2× slower but only +150 ms — not worth anyone's attention.
        let base = base_with("tiny", &[150, 160, 140]);
        let p = judge(&[s("tiny", 300, "compiled", false)], &base, ("i", "build", "tiny", false, true, 300));
        assert!(p.regressions.is_empty());
    }

    #[test]
    fn young_store_never_cries_new_dependency() {
        let base = base_with("serde", &[4000, 4100, 3900]);
        let p = judge(&[s("brand-new-dep", 6000, "compiled", false)], &base, ("i", "build", "x", true, true, 6000));
        assert!(p.new_crates.is_empty(), "3-key store is too young to call anything a new dependency");
        assert_eq!(p.new_shapes.len(), 1);
        assert_eq!(p.verdict, "learning", "{}", p.headline);
    }

    #[test]
    fn wall_split_names_lock_wait() {
        let base = Baselines::default();
        let start = 1_000_000u64;
        let mut u = s("a", 2000, "compiled", true);
        u.wall_ms = 2400;
        u.end_ms = start + 38_000 + 2400; // first rustc began 38 s in
        let p = judge_at(&[u], &base, ("i", "check", "a", false, true, 41_000), start);
        assert_eq!((p.before_ms, p.compile_span_ms, p.after_ms), (38_000, 2400, 600));
        assert!(p.headline.contains("before the first rustc"), "{}", p.headline);
    }

    #[test]
    fn measured_lock_wait_beats_guessing() {
        let start = 1_000_000u64;
        let mut u = s("a", 2000, "compiled", true);
        u.wall_ms = 2000;
        u.end_ms = start + 40_000;
        let facts = InvocationFacts { start_ms: start, lock_wait_pkg_cache_ms: 36_000, ..Default::default() };
        let p = judge_facts(&[u], &Baselines::default(), ("i", "check", "a", false, true, 41_000), &facts);
        assert!(p.headline.contains("package cache"), "{}", p.headline);
        assert!(render_rows(&p).iter().any(|r| r.contains("LOCKS")));
        assert!(p.levers.iter().any(|l| l.contains("package-cache")), "{:?}", p.levers);
    }

    #[test]
    fn lock_watch_times_blocking_lines() {
        // a fake "cargo": prints the Blocking notice, sleeps, prints the next line
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "echo '    Blocking waiting for file lock on package cache' >&2; sleep 0.3; echo '   Compiling x' >&2"]);
        let (st, w) = status_with_lock_watch(&mut cmd).unwrap();
        assert!(st.success());
        assert!(w.package_cache_ms >= 250, "{:?}", w);
        assert_eq!(w.build_dir_ms, 0);
    }

    #[test]
    fn tax_ranks_by_total_not_by_single_build() {
        let _g = crate::tdg::TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!("flux-pulse-tax-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("FLUX_PULSE_DIR", &dir);
        // heavy: 1 × 20 s ; cheap-but-hot: 30 × 1 s
        let mut heavy = s("heavy", 20_000, "compiled", true);
        heavy.inv = String::new();
        record_unit(&heavy);
        for _ in 0..30 {
            let mut hot = s("hot", 1_000, "compiled", true);
            hot.inv = String::new();
            record_unit(&hot);
        }
        let r = rebuild_tax(1, 5, true);
        assert_eq!(r.rows[0].pkg, "hot", "{}", render_tax(&r));
        assert_eq!((r.rows[0].builds, r.rows[0].cpu_total_ms), (30, 30_000));
        std::env::remove_var("FLUX_PULSE_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lock_probe_sees_a_held_cargo_lock() {
        let dir = std::env::temp_dir().join(format!("flux-pulse-lock-{}", std::process::id()));
        let prof = dir.join("debug");
        std::fs::create_dir_all(&prof).unwrap();
        let lock = prof.join(".cargo-lock");
        std::fs::write(&lock, b"").unwrap();
        assert!(!cargo_lock_busy(&dir));
        let f = std::fs::File::open(&lock).unwrap();
        use std::os::unix::io::AsRawFd;
        unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) };
        // flock is per open-file-description: a probe through a NEW open conflicts
        assert!(cargo_lock_busy(&dir));
        unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
        assert!(!cargo_lock_busy(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_seen_but_already_in_head_lock_is_not_new() {
        let base = base_with("serde", &[4000, 4100, 3900]);
        let mut facts = InvocationFacts::default();
        facts.lock_added = Some(["really-added".to_string()].into_iter().collect());
        let p = judge_facts(
            &[s("tokio-test", 1700, "compiled", false), s("really-added", 6000, "compiled", false)],
            &base, ("i", "test", "x", false, true, 8000), &facts);
        assert_eq!(p.new_crates.iter().map(|c| c.unit.as_str()).collect::<Vec<_>>(), vec!["really-added"]);
        assert_eq!(p.new_shapes.len(), 1, "tokio-test is just a first measurement");
    }

    #[test]
    fn new_dependency_vs_new_shape() {
        let mut base = base_with("serde", &[4000, 4100, 3900]);
        for i in 0..MATURE_KEYS {
            base.fold(&s(&format!("filler-{}", i), 1000, "compiled", false));
        }
        let mut reshaped = s("serde", 9000, "compiled", false);
        reshaped.opt = "3".into();
        let fresh = s("brand-new-dep", 6000, "compiled", false);
        let p = judge(&[reshaped, fresh], &base, ("i", "build", "x", true, true, 9000));
        assert_eq!(p.new_shapes.len(), 1);
        assert_eq!(p.new_crates.len(), 1);
        assert_eq!(p.new_crates[0].unit, "brand-new-dep");
        assert_eq!(p.verdict, "heavier", "a 6 s new dependency must register: {}", p.headline);
    }

    fn sd(pkg: &str, cpu: u64, dep: &str, src: &str) -> UnitSample {
        let mut u = s(pkg, cpu, "compiled", true);
        u.dep_digest = dep.into();
        u.src_digest = src.into();
        u
    }

    #[test]
    fn dependency_triggered_rebuild_is_judged_against_its_own_kind() {
        // fluxc-mcp: own edits ~5 s, dependency-triggered ~100 s (measured 10-03)
        let mut b = Baselines::default();
        let seq = [(5_000, "d1", "s1"), (100_000, "d2", "s1"), (4_800, "d2", "s2"), (98_000, "d3", "s2"),
                   (5_200, "d3", "s3"), (102_000, "d4", "s3"), (5_100, "d4", "s4")];
        for (c, d, sr) in seq { b.fold(&sd("fluxc-mcp", c, d, sr)); }
        // a new dependency change: 121 s is +21% vs the dep median (100 s) — NOT +2374% vs 4.9 s
        let p = judge(&[sd("fluxc-mcp", 121_000, "d5", "s4")], &b, ("i", "build", "fluxc", false, true, 121_000));
        assert!(p.regressions.is_empty(), "{}", p.headline);
        assert_eq!(p.upstream_rebuilds.len(), 1);
        assert_eq!(p.upstream_rebuilds[0].other_class_ms, 5_100);
        assert!(p.levers.iter().any(|l| l.contains("only because a dependency changed")), "{:?}", p.levers);
        // an own-source edit costing 20 s IS a regression vs ~5 s
        let p = judge(&[sd("fluxc-mcp", 20_000, "d4", "s5")], &b, ("i", "build", "fluxc", false, true, 20_000));
        assert_eq!(p.regressions.len(), 1, "{}", p.headline);
    }

    #[test]
    fn identical_inputs_recompiled_is_churn() {
        let mut b = Baselines::default();
        for name in ["a", "b", "c"] { b.fold(&sd(name, 1000, "dX", "sX")); }
        let units: Vec<UnitSample> = ["a", "b", "c"].iter().map(|n| sd(n, 1000, "dX", "sX")).collect();
        let p = judge(&units, &b, ("i", "build", "x", false, true, 3000));
        assert_eq!(p.churn_units, 3);
        assert!(p.levers.iter().any(|l| l.contains("fingerprint churn")), "{:?}", p.levers);
    }

    #[test]
    fn probe_failures_in_a_green_build_are_not_failures() {
        let p = judge(&[s("a", 2000, "compiled", true), s("probe", 50, "failed", false)],
            &Baselines::default(), ("i", "build", "a", false, true, 3000));
        assert_ne!(p.verdict, "failed", "{}", p.headline);
        assert_eq!((p.units_failed, p.probe_failures), (0, 1));
    }

    #[test]
    fn cargo_error_before_any_rustc_is_failed_not_a_slow_noop() {
        // measured 10-03: `test -p sigil-top --lib` on a bin crate → "no library targets"
        let p = judge(&[], &Baselines::default(), ("i", "test", "sigil-top", false, false, 29_000));
        assert_eq!(p.verdict, "failed");
        assert!(p.headline.contains("before any rustc"), "{}", p.headline);
        assert!(!p.levers.iter().any(|l| l.contains("no-op")), "{:?}", p.levers);
    }

    #[test]
    fn failing_tests_are_not_a_cargo_error() {
        let facts = InvocationFacts { tests_failed: true, ..Default::default() };
        let p = judge_facts(&[], &Baselines::default(), ("i", "test", "sigil-top", false, false, 73_000), &facts);
        assert_ne!(p.verdict, "failed", "{}", p.headline);
        assert!(p.headline.contains("tests FAILED"), "{}", p.headline);
    }

    #[test]
    fn a_red_unit_is_a_failed_verdict_not_steady() {
        let p = judge(&[s("a", 2700, "failed", true)], &Baselines::default(), ("i", "test", "a", false, false, 49_000));
        assert_eq!(p.verdict, "failed");
        assert!(p.headline.starts_with("✗ failed"), "{}", p.headline);
    }

    #[test]
    fn restored_units_count_as_cache_not_cost() {
        let base = base_with("a", &[1000, 1000, 1000]);
        let p = judge(&[s("a", 0, "restored", true), s("b", 0, "restored", false)], &base, ("i", "build", "a", false, true, 50));
        assert_eq!(p.verdict, "idle");
        assert_eq!(p.units_restored, 2);
        assert!((p.cache_rate - 1.0).abs() < 1e-9);
    }

    #[test]
    fn baseline_ring_is_capped_and_ignores_restores() {
        let mut b = Baselines::default();
        for i in 0..40 {
            b.fold(&s("a", 1000 + i, "compiled", true));
        }
        b.fold(&s("a", 0, "restored", true));
        let k = s("a", 0, "compiled", true).key();
        assert_eq!(b.units[&k].len(), BASELINE_CAP);
        assert_eq!(b.units[&k].last().unwrap().cpu_ms, 1039);
    }

    #[test]
    fn finish_invocation_roundtrip_and_drift() {
        let _g = crate::tdg::TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!("flux-pulse-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("FLUX_PULSE_DIR", &dir);
        let now = now_unix();
        // three earlier invocations establish the baseline, two of them 10 days ago
        for (i, (cpu, age)) in [(10_000u64, 10u64), (10_200, 10), (10_100, 10)].iter().enumerate() {
            let inv = format!("old{}", i);
            let mut u = s("sigil-node", *cpu, "compiled", true);
            u.inv = inv.clone();
            u.ts = now - age * 86_400;
            record_unit(&u);
            finish_invocation(&inv, "build", Some("sigil-node"), false, true, *cpu).unwrap();
        }
        // invocation under test: 50% slower
        let mut u = s("sigil-node", 15_500, "compiled", true);
        u.inv = "cur".into();
        record_unit(&u);
        let p = finish_invocation("cur", "build", Some("sigil-node"), false, true, 15_500).unwrap();
        assert_eq!(p.verdict, "heavier", "{}", p.headline);
        assert_eq!(read_invocation("cur").map(|x| x.verdict), Some("heavier".into()));
        assert!(read_last().is_some());
        assert!(!dir.join("inv").join("cur.units.jsonl").exists(), "inv file must be folded away");
        // a second slow build today, so the recent window has 2 samples
        let mut u = s("sigil-node", 15_000, "compiled", true);
        u.inv = "cur2".into();
        record_unit(&u);
        finish_invocation("cur2", "build", Some("sigil-node"), false, true, 15_000).unwrap();
        let d = drift(14, 7, 5);
        assert_eq!(d.growing.len(), 1, "{}", render_drift(&d));
        assert_eq!((d.growing[0].prior_ms, d.growing[0].recent_ms), (10_100, 15_500));
        std::env::remove_var("FLUX_PULSE_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
