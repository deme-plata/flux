//! magic — the guarded build. One word that does the whole safe thing on a box that also runs
//! two blockchain nodes (Quillon Graph production + the SIGIL g3 producer).
//!
//! The rules are not taste; each is a measured incident on Epsilon (62 GB RAM):
//! * 2026-10-04 and 2026-10-17: a release build tipped the box into global OOM; Quillon's
//!   watchdog saw its heartbeat stall and RESTARTED THE PRODUCTION NODE. Rule: MemAvailable
//!   ≥ 12 GB before any build, ≥ 20 GB before a release build.
//! * 2026-10-22: sshd's children inherit `oom_score_adj=-1000`, so a cgroup-capped build could
//!   not be OOM-killed at its cap and livelocked the box. Fix: every cargo fluxc spawns writes
//!   1000 to its own oom_score_adj before exec (the `choom -n 1000` fix, in-process).
//! * 2026-10-18: a 1 TB read saturated the RAID-0; q-api-server hung 30 s and restarted.
//!   `/proc/pressure/io` full avg10 > 20 % = stop.
//! * 2026-10-22: a watchdog restart left systemd's NRestarts at 0 — a false all-clear. The
//!   receipt is MainPID + ActiveEnterTimestamp + InvocationID sampled BEFORE and AFTER.
//! * Root is a 40 GB partition that has hit 0 bytes free three times; `/tmp` lives on it.
//!
//! What the 2026-10-28 review added (points 4, 5, 7):
//! * TELEMETRY IS NOT HEALTH: a measurement that could not be taken refuses, it never reads
//!   "fine". The receipt distinguishes "unchanged" from "could not be collected" (UNVERIFIED).
//! * ADMISSION CONTROL: every guarded build registers its memory cap in
//!   `~/.flux/cache/guard/active/<pid>.json`; the guard subtracts the caps of other live
//!   guarded builds from MemAvailable before judging, so N builds cannot all pass the same floor.
//! * CONTAINMENT (opt-in by default here until it has been exercised on a disposable host):
//!   `fluxc magic` runs the combo inside a transient systemd scope with MemoryMax/MemoryHigh,
//!   `OOMPolicy=kill` (memory.oom.group — the WHOLE build dies, not one rustc) and an IO cap on
//!   the device under the target dir. Measured on Epsilon: the scope applies memory.max,
//!   memory.high, memory.oom.group=1 and io.max; the kill path itself was NOT provoked here.
//! * HEALTH PROBES: the two nodes' status URLs are fetched before and after; a probe that stopped
//!   answering is reported in the headline. Chain height progress is shown, not required (the
//!   production chain produces a block every ~7 s; a 10 s build may legitimately see none).
//! * More filesystems: free space is checked on the target dir, the flux cache and the temp dir
//!   as WARNINGS (a full target fs fails the build, not a node); root stays a REFUSAL.
//! * Overrides in use (FLUX_GUARD, FLUX_GUARD_MEM_GB, FLUX_GUARD_OOM, FLUX_GUARD_CGROUP) are
//!   recorded in the result, so a bypass is visible, never silent.
//!
//! `fluxc magic <crate> [--release] [--contain|--no-contain]` = guard → register → receipt →
//! probes → combo (contained or not) → probes → receipt → artifacts' stardust → `magic_complete`.
//! The same guard runs in front of every CLI `fluxc build/check/run/test` (exit 3) and returns a
//! structured REFUSED result from the combo engine used by the MCP tools.

use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_FLOOR_GB: f64 = 12.0;
pub const DEFAULT_RELEASE_FLOOR_GB: f64 = 20.0;
pub const ROOT_FLOOR_GB: f64 = 2.0;
pub const TARGET_FS_WARN_GB: f64 = 5.0;
pub const CACHE_FS_WARN_GB: f64 = 5.0;
pub const TMP_FS_WARN_GB: f64 = 1.0;
pub const IO_FULL_WARN_PCT: f64 = 20.0;
pub const SWAP_WARN_GB: f64 = 1.0;
pub const DEFAULT_UNITS: &[&str] = &["q-api-server", "sigil-node"];
/// `url#json.path` — the path names the height field exactly (measured 2026-10-28: the SIGIL node's
/// height is on /v1/integrity; Quillon's is nested at data.upgrades.current_height, while a
/// name-based guess found a per-peer `height: 0` inside an array first). Without `#path` the
/// heuristic `find_height` is used (objects only, never array elements).
pub const DEFAULT_PROBES: &str = "http://127.0.0.1:18181/v1/integrity#height,http://127.0.0.1:8080/api/v1/status#data.upgrades.current_height";
/// Containment: the build's memory cap is MemAvailable − floor, clamped to [MIN_CAP, BUILD_MAX].
pub const DEFAULT_BUILD_MAX_GB: f64 = 16.0;
pub const MIN_CAP_GB: f64 = 4.0;
/// IO cap on the target dir's device inside a contained build; FLUX_GUARD_IO_MBPS overrides, 0 = none.
pub const IO_CAP_MBPS: u64 = 150;
/// What an UNCONTAINED guarded build is assumed to reserve, for admission control.
pub const UNCONTAINED_RESERVE_GB: f64 = 8.0;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MemInfo {
    pub available_kb: u64,
    pub total_kb: u64,
    pub swap_free_kb: u64,
    pub swap_total_kb: u64,
}

/// Everything the judgement looks at, gathered separately so the judgement itself is pure.
#[derive(Debug, Clone, Default)]
pub struct GuardInputs {
    pub mem: Option<MemInfo>,
    pub root_free_gb: Option<f64>,
    pub target_free_gb: Option<f64>,
    pub cache_free_gb: Option<f64>,
    pub tmp_free_gb: Option<f64>,
    pub tmp_is_root: bool,
    pub io_full_avg10: Option<f64>,
    pub load1: Option<f64>,
    pub ncpu: usize,
    pub release: bool,
    pub mode: String,
    pub floor_gb: f64,
    pub reserved_by_others_gb: f64,
    pub other_builds: usize,
    pub overrides: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Guard {
    pub mem_available_gb: f64,
    pub mem_total_gb: f64,
    pub swap_free_gb: f64,
    pub swap_total_gb: f64,
    /// MemAvailable minus the caps other live guarded builds registered.
    pub effective_available_gb: f64,
    pub reserved_by_others_gb: f64,
    pub other_builds: usize,
    pub root_free_gb: Option<f64>,
    pub target_free_gb: Option<f64>,
    pub cache_free_gb: Option<f64>,
    pub tmp_free_gb: Option<f64>,
    pub io_full_avg10: Option<f64>,
    pub load1: Option<f64>,
    pub ncpu: usize,
    pub floor_gb: f64,
    pub release: bool,
    /// Every measurement needed for the refusal rules could be taken.
    pub telemetry_ok: bool,
    /// refuse | warn | off — from FLUX_GUARD.
    pub mode: String,
    /// ok | warn | refuse
    pub verdict: String,
    pub refuse_reasons: Vec<String>,
    pub warn_reasons: Vec<String>,
    /// Env overrides in effect (FLUX_GUARD=warn, FLUX_GUARD_MEM_GB=…, …) — visible, never silent.
    pub overrides: Vec<String>,
    pub oom_victim: bool,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct UnitSnap {
    pub id: String,
    pub active_state: String,
    pub sub_state: String,
    pub main_pid: u32,
    pub active_enter: String,
    /// systemd's per-run id — changes on every (re)start, even with a reused PID.
    pub invocation_id: String,
    pub exec_main_start_mono: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Receipt {
    pub at_unix: u64,
    /// The receipt could be collected (systemctl answered for every unit asked).
    pub ok: bool,
    pub error: Option<String>,
    pub units: Vec<UnitSnap>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Probe {
    pub url: String,
    pub http: u16,
    pub height: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ProbeDelta {
    pub url: String,
    pub http_before: u16,
    pub http_after: u16,
    pub height_before: Option<u64>,
    pub height_after: Option<u64>,
    pub progressed: Option<bool>,
    /// Answered 200 before, not after.
    pub lost: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Contain {
    pub max_gb: f64,
    pub high_gb: f64,
    pub io_dev: Option<String>,
    pub io_mbps: u64,
    pub launcher: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct MagicResult {
    pub package: String,
    pub release: bool,
    /// REFUSED | RESTARTED | UNVERIFIED | GREEN | RED | YELLOW
    pub verdict: String,
    pub headline: String,
    pub guard: Guard,
    pub contained: Option<Contain>,
    pub receipt_before: Receipt,
    pub receipt_after: Receipt,
    pub receipt_problems: Vec<String>,
    pub restarted: Vec<String>,
    pub probes_before: Vec<Probe>,
    pub probes_after: Vec<Probe>,
    pub probe_deltas: Vec<ProbeDelta>,
    pub combo: Option<Value>,
    /// stardust of the test executables cargo reported for this package (not a path guess).
    pub artifacts: Vec<Value>,
    pub total_ms: u64,
}

// ───────────────────────────── guard ─────────────────────────────

pub fn mode_from(v: Option<&str>) -> String {
    match v.unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "0" | "off" | "false" | "no" => "off".into(),
        "warn" | "warning" => "warn".into(),
        _ => "refuse".into(),
    }
}
pub fn mode() -> String { mode_from(std::env::var("FLUX_GUARD").ok().as_deref()) }

pub fn floor_from(v: Option<&str>, release: bool) -> f64 {
    if let Some(f) = v.and_then(|s| s.trim().parse::<f64>().ok()) { return f; }
    if release { DEFAULT_RELEASE_FLOOR_GB } else { DEFAULT_FLOOR_GB }
}
pub fn floor_gb(release: bool) -> f64 { floor_from(std::env::var("FLUX_GUARD_MEM_GB").ok().as_deref(), release) }

pub fn parse_meminfo(s: &str) -> MemInfo {
    let mut m = MemInfo::default();
    for line in s.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let kb: u64 = v.trim().trim_end_matches("kB").trim().parse().unwrap_or(0);
        match k.trim() {
            "MemAvailable" => m.available_kb = kb,
            "MemTotal" => m.total_kb = kb,
            "SwapFree" => m.swap_free_kb = kb,
            "SwapTotal" => m.swap_total_kb = kb,
            _ => {}
        }
    }
    m
}

/// `/proc/pressure/io` → the `full avg10=` percentage.
pub fn parse_pressure_full_avg10(s: &str) -> Option<f64> {
    s.lines().find(|l| l.starts_with("full "))
        .and_then(|l| l.split_whitespace().find_map(|t| t.strip_prefix("avg10=")))
        .and_then(|v| v.parse().ok())
}

pub fn parse_loadavg(s: &str) -> Option<f64> {
    s.split_whitespace().next().and_then(|v| v.parse().ok())
}

/// Free gigabytes on the filesystem holding `path` (statvfs; walks up to an existing ancestor).
pub fn free_gb(path: &Path) -> Option<f64> {
    let mut p = path.to_path_buf();
    while !p.exists() { p = p.parent()?.to_path_buf(); }
    let c = std::ffi::CString::new(p.to_string_lossy().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 { return None; }
    Some((st.f_bavail as f64) * (st.f_frsize as f64) / 1e9)
}

fn dev_of(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    let mut p = path.to_path_buf();
    while !p.exists() { p = p.parent()?.to_path_buf(); }
    std::fs::metadata(&p).ok().map(|m| m.dev())
}

pub fn root_free_gb() -> Option<f64> { free_gb(Path::new("/")) }

fn gb(kb: u64) -> f64 { kb as f64 / 1024.0 / 1024.0 }

pub fn target_dir() -> PathBuf {
    std::env::var("CARGO_TARGET_DIR").ok().map(PathBuf::from)
        .unwrap_or_else(|| fluxc_util::version::workspace_root().join("target"))
}

pub fn tmp_dir() -> PathBuf { std::env::var("TMPDIR").ok().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp")) }

/// The pure judgement. Nothing read from env or /proc here.
pub fn judge(inp: &GuardInputs) -> Guard {
    let mut g = Guard {
        root_free_gb: inp.root_free_gb,
        target_free_gb: inp.target_free_gb,
        cache_free_gb: inp.cache_free_gb,
        tmp_free_gb: inp.tmp_free_gb,
        io_full_avg10: inp.io_full_avg10,
        load1: inp.load1,
        ncpu: inp.ncpu,
        floor_gb: inp.floor_gb,
        release: inp.release,
        mode: inp.mode.clone(),
        reserved_by_others_gb: inp.reserved_by_others_gb,
        other_builds: inp.other_builds,
        overrides: inp.overrides.clone(),
        oom_victim: oom_victim_enabled(),
        telemetry_ok: inp.mem.map(|m| m.available_kb > 0).unwrap_or(false) && inp.root_free_gb.is_some(),
        ..Default::default()
    };
    if let Some(m) = inp.mem {
        g.mem_available_gb = gb(m.available_kb);
        g.mem_total_gb = gb(m.total_kb);
        g.swap_free_gb = gb(m.swap_free_kb);
        g.swap_total_gb = gb(m.swap_total_kb);
    }
    g.effective_available_gb = (g.mem_available_gb - inp.reserved_by_others_gb).max(0.0);
    if !g.telemetry_ok {
        g.refuse_reasons.push("telemetry unavailable (/proc/meminfo or statvfs(/) could not be read) — a failed measurement is not a healthy measurement".into());
    } else {
        if g.effective_available_gb < inp.floor_gb {
            let who = if inp.other_builds > 0 { format!(" after {} other guarded build(s) reserving {:.1} GB", inp.other_builds, inp.reserved_by_others_gb) } else { String::new() };
            g.refuse_reasons.push(format!("MemAvailable {:.1} GB{who} < {:.0} GB floor ({}) — a build here can OOM the box and restart a production node (10-04, 10-17)",
                g.effective_available_gb, inp.floor_gb, if inp.release { "release" } else { "build" }));
        }
        if let Some(r) = inp.root_free_gb { if r < ROOT_FLOOR_GB {
            g.refuse_reasons.push(format!("root disk {:.1} GB free < {:.0} GB — /tmp and /root are the 40 GB root partition that has hit 0 B three times", r, ROOT_FLOOR_GB));
        } }
    }
    if let Some(t) = inp.target_free_gb { if t < TARGET_FS_WARN_GB { g.warn_reasons.push(format!("target dir filesystem {t:.1} GB free < {TARGET_FS_WARN_GB:.0} GB — the build may fail for space")); } }
    if let Some(c) = inp.cache_free_gb { if c < CACHE_FS_WARN_GB { g.warn_reasons.push(format!("flux cache filesystem {c:.1} GB free < {CACHE_FS_WARN_GB:.0} GB")); } }
    if let Some(t) = inp.tmp_free_gb { if !inp.tmp_is_root && t < TMP_FS_WARN_GB { g.warn_reasons.push(format!("temp dir filesystem {t:.1} GB free < {TMP_FS_WARN_GB:.0} GB")); } }
    if let Some(io) = inp.io_full_avg10 { if io > IO_FULL_WARN_PCT {
        g.warn_reasons.push(format!("IO pressure full avg10 {io:.1} % > {IO_FULL_WARN_PCT:.0} % — the RAID-0 is saturated; q-api-server hung 30 s and restarted at 70 % (10-18)"));
    } }
    if g.swap_total_gb > 0.0 && g.swap_free_gb < SWAP_WARN_GB {
        g.warn_reasons.push(format!("swap nearly full ({:.1} of {:.1} GB free) — the next squeeze livelocks (10-22)", g.swap_free_gb, g.swap_total_gb));
    }
    if let Some(l) = inp.load1 { if inp.ncpu > 0 && l > 2.0 * inp.ncpu as f64 {
        g.warn_reasons.push(format!("load {l:.0} > 2× {} cpus — wall clock here measures the queue, not the code", inp.ncpu));
    } }
    g.verdict = if !g.refuse_reasons.is_empty() {
        match inp.mode.as_str() { "refuse" => "refuse", "warn" => "warn", _ => "ok" }
    } else if !g.warn_reasons.is_empty() { "warn" } else { "ok" }.to_string();
    g
}

/// Which overrides are in force, as `NAME=value` strings (empty = none).
pub fn overrides_in_effect(env: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    ["FLUX_GUARD", "FLUX_GUARD_MEM_GB", "FLUX_GUARD_OOM", "FLUX_GUARD_CGROUP", "FLUX_GUARD_UNITS", "FLUX_GUARD_PROBES", "FLUX_GUARD_BUILD_MAX_GB"]
        .iter().filter_map(|k| env(k).map(|v| format!("{k}={v}"))).collect()
}

/// Gather the live inputs.
pub fn inputs(release: bool) -> GuardInputs {
    let mem = std::fs::read_to_string("/proc/meminfo").ok().map(|s| parse_meminfo(&s));
    let tgt = target_dir();
    let tmp = tmp_dir();
    let (reserved, others) = reserved_by_others();
    GuardInputs {
        mem,
        root_free_gb: root_free_gb(),
        target_free_gb: free_gb(&tgt),
        cache_free_gb: free_gb(&crate::stardust::cache_dir()),
        tmp_free_gb: free_gb(&tmp),
        tmp_is_root: dev_of(&tmp) == dev_of(Path::new("/")),
        io_full_avg10: std::fs::read_to_string("/proc/pressure/io").ok().and_then(|s| parse_pressure_full_avg10(&s)),
        load1: std::fs::read_to_string("/proc/loadavg").ok().and_then(|s| parse_loadavg(&s)),
        ncpu: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
        release,
        mode: mode(),
        floor_gb: floor_gb(release),
        reserved_by_others_gb: reserved,
        other_builds: others,
        overrides: overrides_in_effect(&|k| std::env::var(k).ok()),
    }
}

/// The live guard.
pub fn guard(release: bool) -> Guard { judge(&inputs(release)) }

pub fn oom_victim_enabled() -> bool {
    !matches!(std::env::var("FLUX_GUARD_OOM").unwrap_or_default().trim(), "0" | "off" | "false")
}

/// Make the child (and everything it forks: rustc, linkers) the kernel's FIRST OOM victim.
/// Raising oom_score_adj never needs privileges; raw libc in the forked child (no allocation).
pub fn mark_oom_victim(cmd: &mut Command) {
    if !oom_victim_enabled() { return; }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                let path = b"/proc/self/oom_score_adj\0";
                let fd = libc::open(path.as_ptr() as *const libc::c_char, libc::O_WRONLY | libc::O_CLOEXEC);
                if fd >= 0 {
                    let v = b"1000";
                    let _ = libc::write(fd, v.as_ptr() as *const libc::c_void, v.len());
                    libc::close(fd);
                }
                Ok(())
            });
        }
    }
}

pub fn guard_line(g: &Guard) -> String {
    let mut parts = vec![format!("available {:.1} GB (floor {:.0})", g.mem_available_gb, g.floor_gb)];
    if g.other_builds > 0 { parts.push(format!("− {:.1} GB reserved by {} other build(s) = {:.1}", g.reserved_by_others_gb, g.other_builds, g.effective_available_gb)); }
    if let Some(r) = g.root_free_gb { parts.push(format!("root {r:.1} GB")); }
    if let Some(t) = g.target_free_gb { parts.push(format!("target fs {t:.0} GB")); }
    if let Some(io) = g.io_full_avg10 { parts.push(format!("io full {io:.0}%")); }
    if let Some(l) = g.load1 { parts.push(format!("load {l:.0}")); }
    if !g.telemetry_ok { parts.push("TELEMETRY MISSING".into()); }
    let v = match g.verdict.as_str() { "refuse" => "🛑", "warn" => "⚠", _ => "✓" };
    let mut s = format!("  🛡 guard {v} {}{}", parts.join(" · "), if g.oom_victim { " · build = first OOM victim" } else { "" });
    if !g.overrides.is_empty() { s.push_str(&format!(" · OVERRIDES {}", g.overrides.join(" "))); }
    s
}

pub fn render_guard(g: &Guard) -> String {
    let mut s = guard_line(g);
    for r in &g.refuse_reasons { s.push_str(&format!("\n    ✗ {r}")); }
    for r in &g.warn_reasons { s.push_str(&format!("\n    ⚠ {r}")); }
    if g.verdict == "refuse" {
        s.push_str("\n    → not building. FLUX_GUARD=warn to build anyway (say so to the operator), FLUX_GUARD_MEM_GB=<n> to move the floor.");
    }
    s
}

/// For the CLI build path: print the guard; return Some(guard) when the build must not run.
pub fn preflight_cli(release: bool) -> Option<Guard> {
    if mode() == "off" { return None; }
    let g = guard(release);
    if g.verdict == "refuse" {
        eprintln!("{}", render_guard(&g));
        return Some(g);
    }
    if g.verdict == "warn" && std::env::var("FLUX_PULSE_QUIET").is_err() {
        eprintln!("{}", render_guard(&g));
    }
    None
}

// ───────────────────────────── admission control ─────────────────────────────

pub fn registry_dir() -> PathBuf { crate::stardust::cache_dir().join("guard").join("active") }

/// Register this process as a guarded build reserving `cap_gb`. Returns the entry's path.
pub fn register_build(cap_gb: f64, package: &str) -> Option<PathBuf> {
    let dir = registry_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let p = dir.join(format!("{}.json", std::process::id()));
    let v = json!({"pid": std::process::id(), "cap_gb": cap_gb, "package": package, "started_unix": now_unix()});
    std::fs::write(&p, v.to_string()).ok()?;
    Some(p)
}

pub fn unregister_build(p: &Path) { let _ = std::fs::remove_file(p); }

/// Sum of caps registered by OTHER live guarded builds (dead pids are swept). Pure given entries.
pub fn reserved_from_entries(entries: &[(u32, f64)], me: u32, alive: &dyn Fn(u32) -> bool) -> (f64, usize) {
    let mut total = 0.0; let mut n = 0;
    for (pid, cap) in entries {
        if *pid == me || !alive(*pid) { continue; }
        total += cap; n += 1;
    }
    (total, n)
}

pub fn reserved_by_others() -> (f64, usize) {
    let dir = registry_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else { return (0.0, 0) };
    let mut entries = Vec::new();
    for e in rd.flatten() {
        let Ok(txt) = std::fs::read_to_string(e.path()) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
        let pid = v.get("pid").and_then(|p| p.as_u64()).unwrap_or(0) as u32;
        let cap = v.get("cap_gb").and_then(|c| c.as_f64()).unwrap_or(0.0);
        if pid != 0 && !Path::new(&format!("/proc/{pid}")).exists() { let _ = std::fs::remove_file(e.path()); continue; }
        entries.push((pid, cap));
    }
    reserved_from_entries(&entries, std::process::id(), &|pid| Path::new(&format!("/proc/{pid}")).exists())
}

// ───────────────────────────── receipt ─────────────────────────────

pub fn units_from(v: Option<&str>) -> Vec<String> {
    match v {
        Some(s) if !s.trim().is_empty() => s.split(',').map(|u| u.trim().to_string()).filter(|u| !u.is_empty()).collect(),
        _ => DEFAULT_UNITS.iter().map(|s| s.to_string()).collect(),
    }
}
pub fn units() -> Vec<String> { units_from(std::env::var("FLUX_GUARD_UNITS").ok().as_deref()) }

fn now_unix() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

pub const RECEIPT_PROPS: &str = "Id,ActiveState,SubState,MainPID,ActiveEnterTimestamp,InvocationID,ExecMainStartTimestampMonotonic";

/// Parse `systemctl show <units> -p RECEIPT_PROPS` (blank-line separated blocks).
pub fn parse_receipt(text: &str) -> Vec<UnitSnap> {
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        let mut u = UnitSnap::default();
        for line in block.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "Id" => u.id = v.to_string(),
                "ActiveState" => u.active_state = v.to_string(),
                "SubState" => u.sub_state = v.to_string(),
                "MainPID" => u.main_pid = v.parse().unwrap_or(0),
                "ActiveEnterTimestamp" => u.active_enter = v.to_string(),
                "InvocationID" => u.invocation_id = v.to_string(),
                "ExecMainStartTimestampMonotonic" => u.exec_main_start_mono = v.parse().unwrap_or(0),
                _ => {}
            }
        }
        if !u.id.is_empty() { out.push(u); }
    }
    out
}

pub fn receipt() -> Receipt {
    let us = units();
    if us.is_empty() { return Receipt { at_unix: now_unix(), ok: true, error: None, units: vec![] }; }
    let mut c = Command::new("systemctl");
    c.arg("show");
    for u in &us { c.arg(u); }
    c.args(["-p", RECEIPT_PROPS]);
    match c.output() {
        Ok(o) if o.status.success() => {
            let units = parse_receipt(&String::from_utf8_lossy(&o.stdout));
            let missing: Vec<&String> = us.iter().filter(|u| !units.iter().any(|s| s.id.trim_end_matches(".service") == u.trim_end_matches(".service"))).collect();
            if missing.is_empty() {
                Receipt { at_unix: now_unix(), ok: true, error: None, units }
            } else {
                Receipt { at_unix: now_unix(), ok: false, error: Some(format!("systemctl answered without {:?}", missing)), units }
            }
        }
        Ok(o) => Receipt { at_unix: now_unix(), ok: false, error: Some(format!("systemctl show failed: {}", String::from_utf8_lossy(&o.stderr).trim())), units: vec![] },
        Err(e) => Receipt { at_unix: now_unix(), ok: false, error: Some(format!("systemctl not runnable: {e}")), units: vec![] },
    }
}

/// Units whose identity moved between two receipts (pid, start time, invocation id, state).
pub fn restarted(before: &Receipt, after: &Receipt) -> Vec<String> {
    let mut out = Vec::new();
    for b in &before.units {
        if let Some(a) = after.units.iter().find(|a| a.id == b.id) {
            if a.main_pid != b.main_pid || a.active_enter != b.active_enter || a.active_state != b.active_state
                || a.invocation_id != b.invocation_id || a.exec_main_start_mono != b.exec_main_start_mono {
                out.push(format!("{}: MainPID {} → {}, invocation {} → {}, ActiveEnter {} → {}, {}/{} → {}/{}",
                    b.id, b.main_pid, a.main_pid, short8(&b.invocation_id), short8(&a.invocation_id), b.active_enter, a.active_enter,
                    b.active_state, b.sub_state, a.active_state, a.sub_state));
            }
        }
    }
    out
}

fn short8(s: &str) -> &str { if s.len() > 8 { &s[..8] } else { s } }

/// What a receipt cannot vouch for: units that were not healthy to begin with.
pub fn receipt_problems(r: &Receipt) -> Vec<String> {
    let mut v = Vec::new();
    if !r.ok { v.push(format!("receipt not collected: {}", r.error.clone().unwrap_or_default())); }
    for u in &r.units {
        if u.active_state != "active" || u.main_pid == 0 {
            v.push(format!("{} was {}/{} (MainPID {}) before the build — an unchanged receipt proves nothing about it", u.id, u.active_state, u.sub_state, u.main_pid));
        }
    }
    v
}

// ───────────────────────────── health probes ─────────────────────────────

pub fn probe_urls() -> Vec<String> {
    let s = std::env::var("FLUX_GUARD_PROBES").unwrap_or_else(|_| DEFAULT_PROBES.to_string());
    s.split(',').map(str::trim).filter(|u| !u.is_empty()).map(str::to_string).collect()
}

/// Heuristic height: breadth-first over OBJECTS only (array elements such as peer lists are
/// skipped — a peer's `height: 0` is not the chain's), preferring well-known key names, then any
/// key containing "height" with a positive value.
pub fn find_height(v: &Value) -> Option<u64> {
    const PREFERRED: &[&str] = &["height", "current_height", "applied_height", "block_height", "best_height", "tip_height", "chain_height"];
    let mut level: Vec<&Value> = vec![v];
    let mut any_positive: Option<u64> = None;
    let mut depth = 0;
    while !level.is_empty() && depth < 8 {
        let mut next = Vec::new();
        for pref in PREFERRED {
            for obj in &level {
                if let Value::Object(m) = obj {
                    if let Some(n) = m.get(*pref).and_then(|x| x.as_u64()) { return Some(n); }
                }
            }
        }
        for obj in &level {
            if let Value::Object(m) = obj {
                for (k, x) in m {
                    if k.to_ascii_lowercase().contains("height") { if let Some(n) = x.as_u64() { if n > 0 && any_positive.is_none() { any_positive = Some(n); } } }
                    if x.is_object() { next.push(x); }
                }
            }
        }
        level = next; depth += 1;
    }
    any_positive
}

/// Follow a dotted JSON path (`data.upgrades.current_height`) to a u64.
pub fn height_at_path(v: &Value, path: &str) -> Option<u64> {
    let mut cur = v;
    for seg in path.split('.').filter(|s| !s.is_empty()) { cur = cur.get(seg)?; }
    cur.as_u64()
}

/// Split a probe spec `url#json.path` into (url, Some(path)).
pub fn split_probe_spec(spec: &str) -> (String, Option<String>) {
    match spec.split_once('#') { Some((u, p)) if !p.is_empty() => (u.to_string(), Some(p.to_string())), _ => (spec.to_string(), None) }
}

pub fn probe(spec: &str) -> Probe {
    let (url, path) = split_probe_spec(spec);
    let out = Command::new("curl").args(["-s", "--max-time", "5", "-o", "-", "-w", "\n%{http_code}", &url]).output();
    match out {
        Ok(o) => {
            let txt = String::from_utf8_lossy(&o.stdout).into_owned();
            let (body, code) = txt.rsplit_once('\n').unwrap_or(("", "0"));
            let http: u16 = code.trim().parse().unwrap_or(0);
            let json = serde_json::from_str::<Value>(body).ok();
            let height = json.as_ref().and_then(|v| match &path { Some(p) => height_at_path(v, p), None => find_height(v) });
            Probe { url: spec.into(), http, height, error: if http == 0 { Some("no HTTP answer".into()) } else { None } }
        }
        Err(e) => Probe { url: spec.into(), http: 0, height: None, error: Some(format!("curl: {e}")) },
    }
}

pub fn probe_all() -> Vec<Probe> { probe_urls().iter().map(|u| probe(u)).collect() }

pub fn probe_deltas(before: &[Probe], after: &[Probe]) -> Vec<ProbeDelta> {
    before.iter().filter_map(|b| {
        let a = after.iter().find(|a| a.url == b.url)?;
        Some(ProbeDelta {
            url: b.url.clone(), http_before: b.http, http_after: a.http,
            height_before: b.height, height_after: a.height,
            progressed: match (b.height, a.height) { (Some(x), Some(y)) => Some(y > x), _ => None },
            lost: b.http == 200 && a.http != 200,
        })
    }).collect()
}

// ───────────────────────────── containment ─────────────────────────────

/// The memory cap for a contained build: what is available above the floor, clamped.
pub fn cap_for(available_gb: f64, floor_gb: f64, build_max_gb: f64) -> f64 {
    (available_gb - floor_gb).min(build_max_gb).max(MIN_CAP_GB)
}

/// The block device under `path` from /proc/mounts (longest mount-point prefix), `/dev/…` only.
pub fn block_device_for(path: &Path, mounts: &str) -> Option<String> {
    let p = path.to_string_lossy();
    let mut best: Option<(usize, String)> = None;
    for line in mounts.lines() {
        let mut it = line.split_whitespace();
        let (Some(dev), Some(mp)) = (it.next(), it.next()) else { continue };
        if !dev.starts_with("/dev/") { continue; }
        let mp_ok = mp == "/" || p == mp || p.starts_with(&format!("{mp}/"));
        if mp_ok && best.as_ref().map(|(l, _)| mp.len() > *l).unwrap_or(true) { best = Some((mp.len(), dev.to_string())); }
    }
    best.map(|(_, d)| d)
}

/// Whether containment is wanted: explicit arg, else FLUX_GUARD_CGROUP (default OFF until the
/// kill path has been exercised on a disposable host), and only when systemd-run exists and we
/// are root (a user scope needs --user, not wired).
pub fn contain_wanted(explicit: Option<bool>) -> bool {
    let wanted = match explicit {
        Some(b) => b,
        None => matches!(std::env::var("FLUX_GUARD_CGROUP").ok().as_deref().map(str::trim), Some("1") | Some("on") | Some("true")),
    };
    if !wanted { return false; }
    let have = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join("systemd-run").exists())).unwrap_or(false);
    have && unsafe { libc::geteuid() } == 0
}

pub fn contain_plan(g: &Guard) -> Contain {
    let build_max = std::env::var("FLUX_GUARD_BUILD_MAX_GB").ok().and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_BUILD_MAX_GB);
    let max_gb = cap_for(g.effective_available_gb, g.floor_gb, build_max);
    let high_gb = (max_gb * 0.8).max(MIN_CAP_GB - 1.0);
    let io_mbps: u64 = std::env::var("FLUX_GUARD_IO_MBPS").ok().and_then(|s| s.trim().parse().ok()).unwrap_or(IO_CAP_MBPS);
    let io_dev = if io_mbps == 0 { None } else { std::fs::read_to_string("/proc/mounts").ok().and_then(|m| block_device_for(&target_dir(), &m)) };
    let mut launcher = vec!["systemd-run".to_string(), "--scope".into(), "--quiet".into(),
        "-p".into(), format!("MemoryMax={}M", (max_gb * 1024.0) as u64),
        "-p".into(), format!("MemoryHigh={}M", (high_gb * 1024.0) as u64),
        "-p".into(), "OOMPolicy=kill".into()];
    if let Some(dev) = &io_dev {
        launcher.push("-p".into()); launcher.push(format!("IOReadBandwidthMax={dev} {io_mbps}M"));
        launcher.push("-p".into()); launcher.push(format!("IOWriteBandwidthMax={dev} {io_mbps}M"));
    }
    launcher.push("--".into());
    Contain { max_gb, high_gb, io_dev, io_mbps, launcher }
}

// ───────────────────────────── the spell ─────────────────────────────

/// The verdict precedence, pure: REFUSED > RESTARTED > UNVERIFIED > the combo's own verdict.
pub fn verdict_for(combo_verdict: &str, restarted: usize, receipts_ok: bool, probes_lost: usize) -> (String, String) {
    if restarted > 0 { return ("RESTARTED".into(), "🛑 a production unit RESTARTED during this build — say this first".into()); }
    if !receipts_ok { return ("UNVERIFIED".into(), "⚠ the receipt could not be collected — production was NOT verified unchanged".into()); }
    if probes_lost > 0 { return (combo_verdict.to_string(), format!("⚠ {probes_lost} health probe(s) answered before the build and not after")); }
    (combo_verdict.to_string(), String::new())
}

/// guard → register → receipt → probes → combo → probes → receipt → artifacts → webhook.
pub fn cast(package: &str, release: bool, with_dust: bool, contain: Option<bool>) -> MagicResult {
    let t0 = Instant::now();
    let mut r = MagicResult { package: package.to_string(), release, ..Default::default() };
    r.guard = guard(release);
    if r.guard.verdict == "refuse" {
        r.verdict = "REFUSED".into();
        r.headline = format!("🛡 magic refused {package}: {}", r.guard.refuse_reasons.join("; "));
        r.total_ms = t0.elapsed().as_millis() as u64;
        fluxc_util::hooks::dispatch_webhook_blocking("magic_complete", to_json(&r));
        return r;
    }
    let plan = if contain_wanted(contain) { Some(contain_plan(&r.guard)) } else { None };
    let reg = register_build(plan.as_ref().map(|c| c.max_gb).unwrap_or(UNCONTAINED_RESERVE_GB), package);
    r.contained = plan.clone();
    r.receipt_before = receipt();
    r.receipt_problems = receipt_problems(&r.receipt_before);
    r.probes_before = probe_all();
    let launcher: Vec<String> = plan.as_ref().map(|c| c.launcher.clone()).unwrap_or_default();
    let combo = crate::combo_v2::run_combo_with(package, release, &launcher);
    r.probes_after = probe_all();
    r.receipt_after = receipt();
    if let Some(p) = &reg { unregister_build(p); }
    r.restarted = restarted(&r.receipt_before, &r.receipt_after);
    r.probe_deltas = probe_deltas(&r.probes_before, &r.probes_after);
    r.combo = Some(combo.to_json());
    if with_dust && combo.verdict != "REFUSED" {
        for (_, exe) in test_executables(package, release).into_iter().take(3) {
            r.artifacts.push(crate::stardust::to_json(&crate::stardust::read_path(Path::new(&exe), &crate::stardust::Opts::default())));
        }
    }
    let lost = r.probe_deltas.iter().filter(|d| d.lost).count();
    let receipts_ok = r.receipt_before.ok && r.receipt_after.ok;
    let (verdict, note) = verdict_for(&combo.verdict, r.restarted.len(), receipts_ok, lost);
    r.verdict = verdict;
    r.headline = if !r.restarted.is_empty() {
        format!("{} — {} · combo {} {}✓/{}✗", note, r.restarted.join(" | "), combo.verdict, combo.tests_passed, combo.tests_failed)
    } else if !receipts_ok {
        format!("{} · combo {} {} {}✓/{}✗", note, combo.verdict, package, combo.tests_passed, combo.tests_failed)
    } else {
        format!("✨ magic {} {}  tests {}✓/{}✗  {} ms  next={}  guard {}  receipt {} unit(s) unchanged{}{}",
            combo.verdict, package, combo.tests_passed, combo.tests_failed, combo.total_ms, combo.next_action,
            r.guard.verdict, r.receipt_after.units.len(),
            if r.contained.is_some() { "  contained" } else { "" },
            if note.is_empty() { String::new() } else { format!("  {note}") })
    };
    r.total_ms = t0.elapsed().as_millis() as u64;
    fluxc_util::hooks::dispatch_webhook_blocking("magic_complete", to_json(&r));
    r
}

/// The test executables cargo reports for this package (`cargo test --no-run
/// --message-format=json`, fast when fresh) — the authoritative "which binary did we just make",
/// instead of guessing `target/<profile>/<package>` (which `cargo test` does not reliably produce).
pub fn test_executables(package: &str, release: bool) -> Vec<(String, String)> {
    let mut cmd = Command::new("cargo");
    cmd.args(["test", "-p", package, "--no-run", "--message-format=json"]);
    if release { cmd.arg("--release"); }
    cmd.current_dir(fluxc_util::version::workspace_root());
    crate::apply_wrapper_env(&mut cmd);
    cmd.stdin(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    mark_oom_victim(&mut cmd);
    match cmd.output() {
        Ok(o) if o.status.success() => crate::parse_test_executables(&String::from_utf8_lossy(&o.stdout)),
        _ => vec![],
    }
}

pub fn to_json(r: &MagicResult) -> Value {
    serde_json::to_value(r).unwrap_or_else(|_| json!({"error": "serialize"}))
}

pub fn render(r: &MagicResult) -> String {
    let mut s = String::new();
    s.push_str(&r.headline); s.push('\n');
    s.push_str(&render_guard(&r.guard)); s.push('\n');
    if let Some(c) = &r.contained {
        s.push_str(&format!("  contained  systemd scope · MemoryMax {:.1} GB · MemoryHigh {:.1} GB · OOMPolicy=kill (whole build) · IO {} MB/s on {}\n",
            c.max_gb, c.high_gb, c.io_mbps, c.io_dev.clone().unwrap_or_else(|| "(no device cap)".into())));
    }
    for p in &r.receipt_problems { s.push_str(&format!("  ⚠ {p}\n")); }
    for u in &r.receipt_after.units {
        let b = r.receipt_before.units.iter().find(|b| b.id == u.id);
        let same = b.map(|b| b == u).unwrap_or(false);
        s.push_str(&format!("  receipt  {}  {}/{}  MainPID {}  inv {}  since {}  {}\n", u.id, u.active_state, u.sub_state, u.main_pid, short8(&u.invocation_id), u.active_enter,
            if same { "unchanged ✓" } else { "MOVED ✗" }));
    }
    if !r.receipt_after.ok || !r.receipt_before.ok {
        s.push_str(&format!("  receipt  NOT COLLECTED: {}\n", r.receipt_after.error.clone().or(r.receipt_before.error.clone()).unwrap_or_default()));
    }
    for d in &r.probe_deltas {
        s.push_str(&format!("  probe    {}  HTTP {} → {}  height {} → {}  {}\n", d.url, d.http_before, d.http_after,
            d.height_before.map(|h| h.to_string()).unwrap_or_else(|| "-".into()), d.height_after.map(|h| h.to_string()).unwrap_or_else(|| "-".into()),
            if d.lost { "LOST ✗" } else { match d.progressed { Some(true) => "advanced ✓", Some(false) => "no new block in the window", None => "" } }));
    }
    if let Some(c) = &r.combo {
        if let Some(fe) = c.get("first_error").filter(|v| !v.is_null()) {
            s.push_str(&format!("  first_error  {}:{} {} {}\n",
                fe.get("file").and_then(|v| v.as_str()).unwrap_or(""),
                fe.get("line").and_then(|v| v.as_u64()).unwrap_or(0),
                fe.get("code").and_then(|v| v.as_str()).unwrap_or(""),
                fe.get("message").and_then(|v| v.as_str()).unwrap_or("")));
        }
    }
    for a in &r.artifacts {
        s.push_str(&format!("  artifact  {}  →  {} ({})\n",
            a.get("file").and_then(|v| v.as_str()).unwrap_or("?"),
            a.get("network").and_then(|v| v.as_str()).unwrap_or("no network"),
            a.get("network_basis").and_then(|v| v.as_str()).unwrap_or("-")));
    }
    s.push_str(&format!("  {} ms total · magic_complete webhook fired\n", r.total_ms));
    s
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const MEMINFO: &str = "MemTotal:       65000000 kB\nMemFree:         1000000 kB\nMemAvailable:   22020096 kB\nBuffers:          100 kB\nSwapTotal:       7340032 kB\nSwapFree:         524288 kB\n";

    fn base(release: bool) -> GuardInputs {
        GuardInputs {
            mem: Some(parse_meminfo(MEMINFO)), root_free_gb: Some(3.8), target_free_gb: Some(50.0), cache_free_gb: Some(50.0), tmp_free_gb: Some(3.8), tmp_is_root: true,
            io_full_avg10: Some(9.2), load1: Some(21.0), ncpu: 24, release, mode: "refuse".into(), floor_gb: if release { DEFAULT_RELEASE_FLOOR_GB } else { DEFAULT_FLOOR_GB },
            reserved_by_others_gb: 0.0, other_builds: 0, overrides: vec![],
        }
    }

    #[test]
    fn meminfo_pressure_loadavg_parse() {
        let m = parse_meminfo(MEMINFO);
        assert_eq!((m.available_kb, m.total_kb, m.swap_total_kb, m.swap_free_kb), (22020096, 65000000, 7340032, 524288));
        assert_eq!(parse_meminfo(""), MemInfo::default());
        assert_eq!(parse_pressure_full_avg10("some avg10=11.54 avg60=7.54\nfull avg10=9.24 avg60=6.35\n"), Some(9.24));
        assert_eq!(parse_pressure_full_avg10("garbage"), None);
        assert_eq!(parse_loadavg("21.01 19.36 19.42 15/2736 1063711\n"), Some(21.01));
    }

    #[test]
    fn judge_refuses_below_floor_and_warns_on_pressure() {
        let g = judge(&base(false));
        assert_eq!(g.verdict, "warn", "{g:?}"); // swap 0.5 GB free → warn only
        assert!(g.refuse_reasons.is_empty()); assert!(g.telemetry_ok);
        let mut low = base(false); low.mem = Some(MemInfo { available_kb: 9 * 1024 * 1024, ..parse_meminfo(MEMINFO) }); low.io_full_avg10 = Some(30.0); low.load1 = Some(60.0);
        let g2 = judge(&low);
        assert_eq!(g2.verdict, "refuse"); assert_eq!(g2.refuse_reasons.len(), 1);
        assert!(g2.warn_reasons.iter().any(|w| w.contains("IO pressure")));
        assert!(g2.warn_reasons.iter().any(|w| w.contains("load")));
        assert!(judge(&base(true)).refuse_reasons.is_empty());                        // 21 GB passes the 20 GB release floor
        let mut rl = base(true); rl.mem = Some(MemInfo { available_kb: 19 * 1024 * 1024, ..parse_meminfo(MEMINFO) });
        assert_eq!(judge(&rl).verdict, "refuse");
        let mut root = base(false); root.root_free_gb = Some(1.5);
        assert!(judge(&root).refuse_reasons.iter().any(|r| r.contains("root disk")));
        let mut tgt = base(false); tgt.target_free_gb = Some(2.0); tgt.cache_free_gb = Some(1.0); tgt.tmp_is_root = false; tgt.tmp_free_gb = Some(0.2);
        let gt = judge(&tgt);
        assert_eq!(gt.verdict, "warn"); assert!(gt.refuse_reasons.is_empty());
        assert_eq!(gt.warn_reasons.iter().filter(|w| w.contains("filesystem")).count(), 3);
    }

    #[test]
    fn missing_telemetry_refuses_instead_of_reading_healthy() {
        let mut inp = base(false); inp.mem = None;
        let g = judge(&inp);
        assert!(!g.telemetry_ok); assert_eq!(g.verdict, "refuse");
        assert!(g.refuse_reasons[0].contains("telemetry"));
        let mut inp2 = base(false); inp2.root_free_gb = None;
        assert!(!judge(&inp2).telemetry_ok);
        let mut inp3 = base(false); inp3.mem = Some(MemInfo::default()); // MemAvailable 0 = unreadable, not "full"
        assert_eq!(judge(&inp3).verdict, "refuse");
    }

    #[test]
    fn admission_control_subtracts_other_builds() {
        let mut inp = base(false); inp.reserved_by_others_gb = 12.0; inp.other_builds = 2; // 21 − 12 = 9 < 12
        let g = judge(&inp);
        assert_eq!(g.verdict, "refuse");
        assert!((g.effective_available_gb - 9.0).abs() < 0.2, "{}", g.effective_available_gb);
        assert!(g.refuse_reasons[0].contains("2 other guarded build(s)"));
        let (gb, n) = reserved_from_entries(&[(1, 8.0), (2, 4.0), (3, 16.0), (42, 5.0)], 42, &|pid| pid != 3);
        assert_eq!((gb, n), (12.0, 2)); // self and the dead pid excluded
    }

    #[test]
    fn guard_mode_floor_and_overrides_parse_without_env() {
        assert_eq!(mode_from(Some("warn")), "warn"); assert_eq!(mode_from(Some("0")), "off"); assert_eq!(mode_from(None), "refuse");
        assert_eq!(floor_from(Some("0.5"), true), 0.5); assert_eq!(floor_from(None, true), DEFAULT_RELEASE_FLOOR_GB);
        let mut inp = base(false); inp.mem = Some(MemInfo { available_kb: 1024 * 1024, total_kb: 2, swap_free_kb: 0, swap_total_kb: 0 }); inp.mode = "warn".into();
        let w = judge(&inp); assert_eq!(w.verdict, "warn"); assert_eq!(w.refuse_reasons.len(), 1);
        inp.mode = "off".into(); assert_eq!(judge(&inp).verdict, "ok");
        let ov = overrides_in_effect(&|k| if k == "FLUX_GUARD" { Some("warn".into()) } else { None });
        assert_eq!(ov, vec!["FLUX_GUARD=warn".to_string()]);
        let mut o = base(false); o.overrides = ov;
        assert!(guard_line(&judge(&o)).contains("OVERRIDES FLUX_GUARD=warn"));
    }

    #[test]
    fn receipt_parses_invocation_ids_and_detects_restarts_and_failures() {
        let text = "MainPID=911356\nId=q-api-server.service\nActiveState=active\nSubState=running\nActiveEnterTimestamp=Thu 2026-10-22 19:53:53 CEST\nInvocationID=49f422bcd7d84cc88cda8b9752fe1b0b\nExecMainStartTimestampMonotonic=4587224065133\n\nMainPID=3601513\nId=sigil-node.service\nActiveState=active\nSubState=running\nActiveEnterTimestamp=Fri 2026-10-23 12:38:40 CEST\nInvocationID=ffd5d580d9fc475ab40104588d9dc9fb\nExecMainStartTimestampMonotonic=4647510429202\n";
        let before = Receipt { at_unix: 1, ok: true, error: None, units: parse_receipt(text) };
        assert_eq!(before.units.len(), 2);
        assert_eq!(before.units[0].invocation_id, "49f422bcd7d84cc88cda8b9752fe1b0b");
        assert_eq!(before.units[1].exec_main_start_mono, 4647510429202);
        let mut after = before.clone();
        assert!(restarted(&before, &after).is_empty());
        after.units[1].invocation_id = "0000000000000000000000000000beef".into();   // same PID reused, new run
        let r = restarted(&before, &after);
        assert_eq!(r.len(), 1); assert!(r[0].starts_with("sigil-node.service: MainPID 3601513 → 3601513, invocation ffd5d580 → 00000000"));
        assert!(receipt_problems(&before).is_empty());
        let mut sick = before.clone(); sick.units[0].active_state = "inactive".into(); sick.units[0].main_pid = 0;
        assert_eq!(receipt_problems(&sick).len(), 1);
        let failed = Receipt { at_unix: 1, ok: false, error: Some("systemctl not runnable".into()), units: vec![] };
        assert!(receipt_problems(&failed)[0].contains("not collected"));
        // Verdict precedence: a missing receipt is never GREEN.
        assert_eq!(verdict_for("GREEN", 0, false, 0).0, "UNVERIFIED");
        assert_eq!(verdict_for("GREEN", 1, true, 0).0, "RESTARTED");
        assert_eq!(verdict_for("GREEN", 0, true, 1).0, "GREEN");
        assert!(verdict_for("GREEN", 0, true, 1).1.contains("probe"));
        assert_eq!(verdict_for("RED", 0, true, 0), ("RED".into(), String::new()));
    }

    #[test]
    fn probes_find_heights_and_losses() {
        assert_eq!(find_height(&json!({"ok":true,"data":{"height":42,"peers":[{"height":0}]}})), Some(42));
        assert_eq!(find_height(&json!({"current_height":7,"x":{"height":9}})), Some(7), "shallower preferred key wins");
        assert_eq!(find_height(&json!({"block_height":"7"})), None);
        assert_eq!(find_height(&json!({"peers":[{"height":5}]})), None, "array elements are never the chain height");
        assert_eq!(find_height(&json!({"data":{"upgrades":{"current_height":24329083},"peers":[{"height":0}]}})), Some(24329083));
        assert_eq!(height_at_path(&json!({"data":{"upgrades":{"current_height":24329083}}}), "data.upgrades.current_height"), Some(24329083));
        assert_eq!(height_at_path(&json!({"height":1}), "nope"), None);
        assert_eq!(split_probe_spec("http://x/v1/integrity#height"), ("http://x/v1/integrity".into(), Some("height".into())));
        assert_eq!(split_probe_spec("http://x/v1/status"), ("http://x/v1/status".into(), None));
        let b = vec![Probe { url: "u".into(), http: 200, height: Some(10), error: None }, Probe { url: "v".into(), http: 200, height: None, error: None }];
        let a = vec![Probe { url: "u".into(), http: 200, height: Some(11), error: None }, Probe { url: "v".into(), http: 0, height: None, error: Some("x".into()) }];
        let d = probe_deltas(&b, &a);
        assert_eq!(d[0].progressed, Some(true)); assert!(!d[0].lost);
        assert!(d[1].lost); assert_eq!(d[1].progressed, None);
    }

    #[test]
    fn containment_arithmetic_and_device_lookup() {
        assert_eq!(cap_for(21.0, 12.0, 16.0), 9.0);
        assert_eq!(cap_for(40.0, 12.0, 16.0), 16.0);
        assert_eq!(cap_for(13.0, 12.0, 16.0), MIN_CAP_GB);
        let mounts = "/dev/nvme0n1p3 / ext4 rw 0 0\n/dev/md0 /home/storage xfs rw 0 0\ntmpfs /tmp tmpfs rw 0 0\n/dev/sda1 /home ext4 rw 0 0\n";
        assert_eq!(block_device_for(Path::new("/home/storage/flux/target"), mounts).as_deref(), Some("/dev/md0"));
        assert_eq!(block_device_for(Path::new("/home/orobit/x"), mounts).as_deref(), Some("/dev/sda1"));
        assert_eq!(block_device_for(Path::new("/var/lib"), mounts).as_deref(), Some("/dev/nvme0n1p3"));
        assert_eq!(block_device_for(Path::new("/tmp/x"), mounts).as_deref(), Some("/dev/nvme0n1p3"), "tmpfs is skipped, root wins");
        let g = Guard { effective_available_gb: 21.0, floor_gb: 12.0, ..Default::default() };
        let c = contain_plan(&g);
        assert_eq!(c.max_gb, 9.0);
        assert_eq!(c.launcher[0], "systemd-run"); assert!(c.launcher.contains(&"OOMPolicy=kill".to_string()));
        assert_eq!(c.launcher.last().map(String::as_str), Some("--"));
        assert!(c.launcher.iter().any(|a| a == "MemoryMax=9216M"));
    }

    #[test]
    fn units_default_to_the_two_production_nodes() {
        assert_eq!(units_from(None), vec!["q-api-server".to_string(), "sigil-node".to_string()]);
        assert_eq!(units_from(Some("a, b ,,c")), vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[test]
    fn oom_victim_marking_applies_to_a_child() {
        if !oom_victim_enabled() { return; }
        let own = std::fs::read_to_string("/proc/self/oom_score_adj").unwrap_or_default().trim().to_string();
        let ctrl = Command::new("cat").arg("/proc/self/oom_score_adj").output().expect("cat");
        assert_eq!(String::from_utf8_lossy(&ctrl.stdout).trim(), own);
        let mut c = Command::new("cat"); c.arg("/proc/self/oom_score_adj"); mark_oom_victim(&mut c);
        let out = c.output().expect("cat");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "1000");
        assert_eq!(std::fs::read_to_string("/proc/self/oom_score_adj").unwrap_or_default().trim(), own);
    }
}
