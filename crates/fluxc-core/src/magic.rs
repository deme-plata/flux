//! magic — the guarded build. One word that does the whole safe thing on a box that also
//! runs two blockchains (Quillon Graph production + the SIGIL g3 producer).
//!
//! The rules here are not taste; each one is a measured incident on Epsilon (62 GB RAM):
//! * 2026-10-04 and 2026-10-17: a release build tipped the box into global OOM. Quillon's
//!   watchdog saw its heartbeat stall and RESTARTED THE PRODUCTION NODE. Rule (CLAUDE.md
//!   firewall): MemAvailable ≥ 12 GB before any build, ≥ 20 GB before a release build.
//! * 2026-10-22: sshd's children inherit `oom_score_adj=-1000`, so a cgroup-capped build
//!   could not be OOM-killed at its cap and livelocked the box ("no killable processes",
//!   swap full, load ~100). Fix = `choom -n 1000`: here every cargo fluxc spawns writes
//!   1000 to its own `/proc/self/oom_score_adj` before exec, so the BUILD is the kernel's
//!   first victim, never a node.
//! * 2026-10-18: a 1 TB read saturated the RAID-0; q-api-server's producer tasks hung
//!   30 s and it restarted. `/proc/pressure/io` full avg10 > 20 % = stop.
//! * 2026-10-22: a watchdog restart left systemd's NRestarts at 0 — a false all-clear.
//!   The receipt that proves "production did not restart during my build" is MainPID +
//!   ActiveEnterTimestamp sampled BEFORE and AFTER. If either moved, the FIRST line says so.
//! * Root is a 40 GB partition that has hit 0 bytes free three times; `/tmp` lives on it.
//!
//! `fluxc magic <crate> [--release]` = guard → receipt → combo (the one wrapper-cached
//! `cargo test -p` + predictor) → receipt → stardust on the built binary → `magic_complete`.
//! `FLUX_GUARD=warn` turns a refusal into a warning, `FLUX_GUARD=0` turns the guard off,
//! `FLUX_GUARD_MEM_GB` moves the floor, `FLUX_GUARD_UNITS` names the units to receipt,
//! `FLUX_GUARD_OOM=0` stops the victim marking.

use serde::Serialize;
use serde_json::{json, Value};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_FLOOR_GB: f64 = 12.0;
pub const DEFAULT_RELEASE_FLOOR_GB: f64 = 20.0;
pub const ROOT_FLOOR_GB: f64 = 2.0;
pub const IO_FULL_WARN_PCT: f64 = 20.0;
pub const SWAP_WARN_GB: f64 = 1.0;
pub const DEFAULT_UNITS: &[&str] = &["q-api-server", "sigil-node"];

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MemInfo {
    pub available_kb: u64,
    pub total_kb: u64,
    pub swap_free_kb: u64,
    pub swap_total_kb: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Guard {
    pub mem_available_gb: f64,
    pub mem_total_gb: f64,
    pub swap_free_gb: f64,
    pub swap_total_gb: f64,
    pub root_free_gb: Option<f64>,
    pub io_full_avg10: Option<f64>,
    pub load1: Option<f64>,
    pub ncpu: usize,
    pub floor_gb: f64,
    pub release: bool,
    /// refuse | warn | off — from FLUX_GUARD.
    pub mode: String,
    /// ok | warn | refuse
    pub verdict: String,
    pub refuse_reasons: Vec<String>,
    pub warn_reasons: Vec<String>,
    pub oom_victim: bool,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct UnitSnap {
    pub id: String,
    pub active_state: String,
    pub main_pid: u32,
    pub active_enter: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Receipt {
    pub at_unix: u64,
    pub units: Vec<UnitSnap>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct MagicResult {
    pub package: String,
    pub release: bool,
    /// REFUSED | RESTARTED | GREEN | RED | YELLOW
    pub verdict: String,
    pub headline: String,
    pub guard: Guard,
    pub receipt_before: Receipt,
    pub receipt_after: Receipt,
    pub restarted: Vec<String>,
    pub combo: Option<Value>,
    pub dust: Option<Value>,
    pub total_ms: u64,
}

// ───────────────────────────── guard ─────────────────────────────

/// FLUX_GUARD → refuse (default) | warn | off. Pure form for tests.
pub fn mode_from(v: Option<&str>) -> String {
    match v.unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "0" | "off" | "false" | "no" => "off".into(),
        "warn" | "warning" => "warn".into(),
        _ => "refuse".into(),
    }
}

pub fn mode() -> String { mode_from(std::env::var("FLUX_GUARD").ok().as_deref()) }

/// FLUX_GUARD_MEM_GB overrides; else 12 GB (20 GB for release). Pure form for tests.
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

pub fn root_free_gb() -> Option<f64> {
    let path = std::ffi::CString::new("/").ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(path.as_ptr(), &mut st) };
    if rc != 0 { return None; }
    Some((st.f_bavail as f64) * (st.f_frsize as f64) / 1e9)
}

fn gb(kb: u64) -> f64 { kb as f64 / 1024.0 / 1024.0 }

/// Judge the box for a build of this kind with the live env (mode + floor). See `judge_with`.
pub fn judge(mem: MemInfo, root_free: Option<f64>, io_full: Option<f64>, load1: Option<f64>, ncpu: usize, release: bool) -> Guard {
    judge_with(mem, root_free, io_full, load1, ncpu, release, &mode(), floor_gb(release))
}

/// The pure judgement: every input explicit, nothing read from env (tests run in parallel
/// threads — env mutation in tests races; this is why the first green run was luck).
pub fn judge_with(mem: MemInfo, root_free: Option<f64>, io_full: Option<f64>, load1: Option<f64>, ncpu: usize, release: bool, mode: &str, floor: f64) -> Guard {
    let mode = mode.to_string();
    let mut g = Guard {
        mem_available_gb: gb(mem.available_kb),
        mem_total_gb: gb(mem.total_kb),
        swap_free_gb: gb(mem.swap_free_kb),
        swap_total_gb: gb(mem.swap_total_kb),
        root_free_gb: root_free,
        io_full_avg10: io_full,
        load1,
        ncpu,
        floor_gb: floor,
        release,
        mode: mode.clone(),
        oom_victim: oom_victim_enabled(),
        ..Default::default()
    };
    if g.mem_available_gb < floor {
        g.refuse_reasons.push(format!("MemAvailable {:.1} GB < {:.0} GB floor ({}) — a build here can OOM the box and restart a production node (10-04, 10-17)",
            g.mem_available_gb, floor, if release { "release" } else { "build" }));
    }
    if let Some(r) = root_free { if r < ROOT_FLOOR_GB {
        g.refuse_reasons.push(format!("root disk {:.1} GB free < {:.0} GB — /tmp and /root are the 40 GB root partition that has hit 0 B three times", r, ROOT_FLOOR_GB));
    } }
    if let Some(io) = io_full { if io > IO_FULL_WARN_PCT {
        g.warn_reasons.push(format!("IO pressure full avg10 {io:.1} % > {IO_FULL_WARN_PCT:.0} % — the RAID-0 is saturated; q-api-server hung 30 s and restarted at 70 % (10-18)"));
    } }
    if g.swap_total_gb > 0.0 && g.swap_free_gb < SWAP_WARN_GB {
        g.warn_reasons.push(format!("swap nearly full ({:.1} of {:.1} GB free) — the next squeeze livelocks (10-22)", g.swap_free_gb, g.swap_total_gb));
    }
    if let Some(l) = load1 { if ncpu > 0 && l > 2.0 * ncpu as f64 {
        g.warn_reasons.push(format!("load {l:.0} > 2× {ncpu} cpus — wall clock here measures the queue, not the code"));
    } }
    g.verdict = if !g.refuse_reasons.is_empty() {
        match mode.as_str() { "refuse" => "refuse", "warn" => "warn", _ => "ok" }
    } else if !g.warn_reasons.is_empty() { "warn" } else { "ok" }.to_string();
    g
}

/// The live guard: reads /proc and judges.
pub fn guard(release: bool) -> Guard {
    let mem = parse_meminfo(&std::fs::read_to_string("/proc/meminfo").unwrap_or_default());
    let io = std::fs::read_to_string("/proc/pressure/io").ok().and_then(|s| parse_pressure_full_avg10(&s));
    let load = std::fs::read_to_string("/proc/loadavg").ok().and_then(|s| parse_loadavg(&s));
    let ncpu = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0);
    judge(mem, root_free_gb(), io, load, ncpu, release)
}

pub fn oom_victim_enabled() -> bool {
    !matches!(std::env::var("FLUX_GUARD_OOM").unwrap_or_default().trim(), "0" | "off" | "false")
}

/// Make the child (and everything it forks: rustc, linkers) the kernel's FIRST OOM
/// victim. Raising oom_score_adj never needs privileges; the write is done with raw libc in
/// the forked child (no allocation after fork). Inherited across fork+exec.
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

/// One stderr line about the guard, for the universal spawn hook. Never blocks.
pub fn guard_line(g: &Guard) -> String {
    let mut parts = vec![format!("available {:.1} GB (floor {:.0})", g.mem_available_gb, g.floor_gb)];
    if let Some(r) = g.root_free_gb { parts.push(format!("root {r:.1} GB")); }
    if let Some(io) = g.io_full_avg10 { parts.push(format!("io full {io:.0}%")); }
    if let Some(l) = g.load1 { parts.push(format!("load {l:.0}")); }
    let v = match g.verdict.as_str() { "refuse" => "🛑", "warn" => "⚠", _ => "✓" };
    format!("  🛡 guard {v} {}{}", parts.join(" · "), if g.oom_victim { " · build = first OOM victim" } else { "" })
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

// ───────────────────────────── receipt ─────────────────────────────

pub fn units_from(v: Option<&str>) -> Vec<String> {
    match v {
        Some(s) if !s.trim().is_empty() => s.split(',').map(|u| u.trim().to_string()).filter(|u| !u.is_empty()).collect(),
        _ => DEFAULT_UNITS.iter().map(|s| s.to_string()).collect(),
    }
}

pub fn units() -> Vec<String> { units_from(std::env::var("FLUX_GUARD_UNITS").ok().as_deref()) }

fn now_unix() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

/// Parse `systemctl show -p Id,ActiveState,MainPID,ActiveEnterTimestamp` for N units (blank-line separated).
pub fn parse_receipt(text: &str) -> Vec<UnitSnap> {
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        let mut u = UnitSnap::default();
        for line in block.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            match k.trim() {
                "Id" => u.id = v.trim().to_string(),
                "ActiveState" => u.active_state = v.trim().to_string(),
                "MainPID" => u.main_pid = v.trim().parse().unwrap_or(0),
                "ActiveEnterTimestamp" => u.active_enter = v.trim().to_string(),
                _ => {}
            }
        }
        if !u.id.is_empty() { out.push(u); }
    }
    out
}

pub fn receipt() -> Receipt {
    let us = units();
    if us.is_empty() { return Receipt { at_unix: now_unix(), units: vec![] }; }
    let mut c = Command::new("systemctl");
    c.arg("show");
    for u in &us { c.arg(u); }
    c.args(["-p", "Id,ActiveState,MainPID,ActiveEnterTimestamp"]);
    let units = match c.output() {
        Ok(o) if o.status.success() => parse_receipt(&String::from_utf8_lossy(&o.stdout)),
        _ => vec![],
    };
    Receipt { at_unix: now_unix(), units }
}

/// Units whose MainPID or ActiveEnterTimestamp moved between two receipts.
pub fn restarted(before: &Receipt, after: &Receipt) -> Vec<String> {
    let mut out = Vec::new();
    for b in &before.units {
        if let Some(a) = after.units.iter().find(|a| a.id == b.id) {
            if a.main_pid != b.main_pid || a.active_enter != b.active_enter || a.active_state != b.active_state {
                out.push(format!("{}: MainPID {} → {}, ActiveEnter {} → {}, {} → {}",
                    b.id, b.main_pid, a.main_pid, b.active_enter, a.active_enter, b.active_state, a.active_state));
            }
        }
    }
    out
}

// ───────────────────────────── the spell ─────────────────────────────

/// guard → receipt → combo → receipt → dust → webhook.
pub fn cast(package: &str, release: bool, with_dust: bool) -> MagicResult {
    let t0 = Instant::now();
    let start_unix = now_unix();
    let mut r = MagicResult { package: package.to_string(), release, ..Default::default() };
    r.guard = guard(release);
    if r.guard.verdict == "refuse" {
        r.verdict = "REFUSED".into();
        r.headline = format!("🛡 magic refused {package}: {}", r.guard.refuse_reasons.join("; "));
        r.total_ms = t0.elapsed().as_millis() as u64;
        fluxc_util::hooks::dispatch_webhook_blocking("magic_complete", to_json(&r));
        return r;
    }
    r.receipt_before = receipt();
    let combo = crate::combo_v2::run_combo(package, release);
    r.receipt_after = receipt();
    r.restarted = restarted(&r.receipt_before, &r.receipt_after);
    r.combo = Some(combo.to_json());
    if with_dust {
        r.dust = built_binary(package, release, start_unix).map(|p| {
            crate::stardust::to_json(&crate::stardust::read_path(&p, &crate::stardust::Opts::default()))
        });
    }
    if !r.restarted.is_empty() {
        r.verdict = "RESTARTED".into();
        r.headline = format!("🛑 {} RESTARTED during this build — say this first. combo {} {}✓/{}✗",
            r.restarted.join(" | "), combo.verdict, combo.tests_passed, combo.tests_failed);
    } else {
        r.verdict = combo.verdict.clone();
        r.headline = format!("✨ magic {} {}  tests {}✓/{}✗  {} ms  next={}  guard {}  receipt {} unit(s) unchanged",
            combo.verdict, package, combo.tests_passed, combo.tests_failed, combo.total_ms, combo.next_action,
            r.guard.verdict, r.receipt_after.units.len());
    }
    r.total_ms = t0.elapsed().as_millis() as u64;
    fluxc_util::hooks::dispatch_webhook_blocking("magic_complete", to_json(&r));
    r
}

/// `<target>/{release|debug}/<package>` if it exists and was written by this run.
fn built_binary(package: &str, release: bool, start_unix: u64) -> Option<std::path::PathBuf> {
    let target = std::env::var("CARGO_TARGET_DIR").ok().map(std::path::PathBuf::from)
        .unwrap_or_else(|| fluxc_util::version::workspace_root().join("target"));
    let p = target.join(if release { "release" } else { "debug" }).join(package);
    let md = std::fs::metadata(&p).ok()?;
    if !md.is_file() { return None; }
    let mt = md.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    if mt + 1 < start_unix { return None; }
    Some(p)
}

pub fn to_json(r: &MagicResult) -> Value {
    serde_json::to_value(r).unwrap_or_else(|_| json!({"error": "serialize"}))
}

pub fn render(r: &MagicResult) -> String {
    let mut s = String::new();
    s.push_str(&r.headline); s.push('\n');
    s.push_str(&render_guard(&r.guard)); s.push('\n');
    for u in &r.receipt_after.units {
        let b = r.receipt_before.units.iter().find(|b| b.id == u.id);
        let same = b.map(|b| b == u).unwrap_or(false);
        s.push_str(&format!("  receipt  {}  {}  MainPID {}  since {}  {}\n", u.id, u.active_state, u.main_pid, u.active_enter,
            if same { "unchanged ✓" } else { "MOVED ✗" }));
    }
    if r.receipt_after.units.is_empty() && r.verdict != "REFUSED" {
        s.push_str("  receipt  (no systemd units read — FLUX_GUARD_UNITS or systemctl unavailable)\n");
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
    if let Some(d) = &r.dust {
        s.push_str(&format!("  dust     {}  network {}  flux-rev {}\n",
            d.get("file").and_then(|v| v.as_str()).unwrap_or("?"),
            d.get("network").and_then(|v| v.as_str()).unwrap_or("-"),
            d.get("flux_rev").and_then(|v| v.as_str()).unwrap_or("-")));
    }
    s.push_str(&format!("  {} ms total · magic_complete webhook fired\n", r.total_ms));
    s
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const MEMINFO: &str = "MemTotal:       65000000 kB\nMemFree:         1000000 kB\nMemAvailable:   22020096 kB\nBuffers:          100 kB\nSwapTotal:       7340032 kB\nSwapFree:         524288 kB\n";

    #[test]
    fn meminfo_parses_the_four_fields() {
        let m = parse_meminfo(MEMINFO);
        assert_eq!(m.available_kb, 22020096);
        assert_eq!(m.total_kb, 65000000);
        assert_eq!(m.swap_total_kb, 7340032);
        assert_eq!(m.swap_free_kb, 524288);
        assert_eq!(parse_meminfo(""), MemInfo::default());
    }

    #[test]
    fn pressure_and_loadavg_parse() {
        let p = "some avg10=11.54 avg60=7.54 avg300=5.27 total=141065561782\nfull avg10=9.24 avg60=6.35 avg300=4.58 total=80218408680\n";
        assert_eq!(parse_pressure_full_avg10(p), Some(9.24));
        assert_eq!(parse_pressure_full_avg10("garbage"), None);
        assert_eq!(parse_loadavg("21.01 19.36 19.42 15/2736 1063711\n"), Some(21.01));
    }

    #[test]
    fn judge_refuses_below_floor_and_warns_on_pressure() {
        // 21 GB available: ok for a build (floor 12), refused for a release (floor 20) only if below.
        let m = parse_meminfo(MEMINFO); // 21.0 GB
        let g = judge_with(m, Some(3.8), Some(9.2), Some(21.0), 24, false, "refuse", DEFAULT_FLOOR_GB);
        assert_eq!(g.verdict, "warn", "{g:?}"); // swap 0.5 GB free → warn, nothing refused
        assert!(g.refuse_reasons.is_empty());
        let low = MemInfo { available_kb: 9 * 1024 * 1024, ..m };
        let g2 = judge_with(low, Some(3.8), Some(30.0), Some(60.0), 24, false, "refuse", DEFAULT_FLOOR_GB);
        assert_eq!(g2.verdict, "refuse");
        assert_eq!(g2.refuse_reasons.len(), 1);
        assert!(g2.warn_reasons.iter().any(|w| w.contains("IO pressure")));
        assert!(g2.warn_reasons.iter().any(|w| w.contains("load")));
        // Release floor is 20 GB: 21 passes, 19 does not.
        let rel_ok = judge_with(m, Some(10.0), None, None, 24, true, "refuse", DEFAULT_RELEASE_FLOOR_GB);
        assert!(rel_ok.refuse_reasons.is_empty());
        let rel_low = judge_with(MemInfo { available_kb: 19 * 1024 * 1024, ..m }, Some(10.0), None, None, 24, true, "refuse", DEFAULT_RELEASE_FLOOR_GB);
        assert_eq!(rel_low.verdict, "refuse");
        // Root disk below 2 GB refuses on its own.
        let root = judge_with(m, Some(1.5), None, None, 24, false, "refuse", DEFAULT_FLOOR_GB);
        assert!(root.refuse_reasons.iter().any(|r| r.contains("root disk")));
    }

    #[test]
    fn guard_mode_and_floor_parse_without_env() {
        assert_eq!(mode_from(Some("warn")), "warn");
        assert_eq!(mode_from(Some(" WARNING ")), "warn");
        assert_eq!(mode_from(Some("0")), "off");
        assert_eq!(mode_from(Some("off")), "off");
        assert_eq!(mode_from(None), "refuse");
        assert_eq!(mode_from(Some("strict")), "refuse");
        assert_eq!(floor_from(Some("0.5"), true), 0.5);
        assert_eq!(floor_from(Some("garbage"), false), DEFAULT_FLOOR_GB);
        assert_eq!(floor_from(None, false), DEFAULT_FLOOR_GB);
        assert_eq!(floor_from(None, true), DEFAULT_RELEASE_FLOOR_GB);
        // A refusal becomes a warning in warn mode and disappears in off mode — reasons stay.
        let low = MemInfo { available_kb: 1024 * 1024, total_kb: 2, swap_free_kb: 0, swap_total_kb: 0 };
        let w = judge_with(low, None, None, None, 1, false, "warn", DEFAULT_FLOOR_GB);
        assert_eq!(w.verdict, "warn");
        assert_eq!(w.refuse_reasons.len(), 1);
        assert_eq!(judge_with(low, None, None, None, 1, false, "off", DEFAULT_FLOOR_GB).verdict, "ok");
        assert_eq!(judge_with(low, None, None, None, 1, true, "refuse", 0.5).verdict, "ok");
        // The live wrappers read env but default correctly when it is unset here.
        if std::env::var("FLUX_GUARD").is_err() { assert_eq!(mode(), "refuse"); }
    }

    #[test]
    fn receipt_parses_blocks_and_detects_a_restart() {
        let text = "MainPID=911356\nId=q-api-server.service\nActiveState=active\nActiveEnterTimestamp=Thu 2026-10-22 19:53:53 CEST\n\nMainPID=3601513\nId=sigil-node.service\nActiveState=active\nActiveEnterTimestamp=Fri 2026-10-23 12:38:40 CEST\n";
        let before = Receipt { at_unix: 1, units: parse_receipt(text) };
        assert_eq!(before.units.len(), 2);
        assert_eq!(before.units[0].main_pid, 911356);
        let mut after = before.clone();
        assert!(restarted(&before, &after).is_empty());
        after.units[0].main_pid = 912000;
        after.units[0].active_enter = "Thu 2026-10-28 11:00:00 CET".into();
        let r = restarted(&before, &after);
        assert_eq!(r.len(), 1);
        assert!(r[0].starts_with("q-api-server.service: MainPID 911356 → 912000"));
    }

    #[test]
    fn units_default_to_the_two_production_nodes() {
        assert_eq!(units_from(None), vec!["q-api-server".to_string(), "sigil-node".to_string()]);
        assert_eq!(units_from(Some("  ")), vec!["q-api-server".to_string(), "sigil-node".to_string()]);
        assert_eq!(units_from(Some("a, b ,,c")), vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[test]
    fn oom_victim_marking_applies_to_a_child() {
        if !oom_victim_enabled() { return; } // honour an operator opt-out in the test env, no mutation
        // Control: an unmarked child inherits whatever we have (under `fluxc test` that is
        // already 1000 — the test binary is itself a cargo child of a marking fluxc).
        let own = std::fs::read_to_string("/proc/self/oom_score_adj").unwrap_or_default().trim().to_string();
        let ctrl = Command::new("cat").arg("/proc/self/oom_score_adj").output().expect("cat");
        assert_eq!(String::from_utf8_lossy(&ctrl.stdout).trim(), own, "an unmarked child inherits the parent's value");
        // Marked: the child is the first OOM victim regardless of what we inherited.
        let mut c = Command::new("cat");
        c.arg("/proc/self/oom_score_adj");
        mark_oom_victim(&mut c);
        let out = c.output().expect("cat");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "1000", "the marked child must be the first OOM victim");
        // And marking the child never changes the parent.
        let own_after = std::fs::read_to_string("/proc/self/oom_score_adj").unwrap_or_default().trim().to_string();
        assert_eq!(own_after, own);
    }
}
