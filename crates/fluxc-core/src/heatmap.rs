// flux_heatmap — Stability & OOM Diagnostics Engine
//
// Measures runtime health across 5 dimensions and produces a composite
// stability score that feeds into the X-Algo prediction algorithm.
//
// Dimensions:
//   1. Memory Pressure    — RSS/virtual memory ratio, OOM proximity
//   2. CPU Saturation     — load average vs core count
//   3. FD Pressure        — open file descriptors vs limit
//   4. Process Longevity  — uptime stability, crash frequency
//   5. I/O Stall          — disk wait time, write amplification
//
// The stability_score (0.0-1.0) becomes the 6th X-Algo dimension,
// penalizing predictions when the system is under memory/CPU pressure.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

// ── Heatmap Snapshot ──

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HeatmapSnapshot {
    /// Unix timestamp
    pub timestamp_secs: u64,
    /// Memory pressure score (0.0 = critical, 1.0 = healthy)
    pub memory_pressure: f64,
    /// RSS in bytes
    pub memory_rss_bytes: u64,
    /// Virtual memory in bytes
    pub memory_vms_bytes: u64,
    /// CPU saturation score (0.0 = overloaded, 1.0 = idle)
    pub cpu_saturation: f64,
    /// Load average (1-min)
    pub load_avg_1m: f64,
    /// FD pressure score (0.0 = exhausted, 1.0 = plenty)
    pub fd_pressure: f64,
    /// Open file descriptors
    pub fd_open: u64,
    /// FD limit
    pub fd_limit: u64,
    /// Process uptime in seconds
    pub uptime_secs: u64,
    /// I/O stall score (0.0 = stalled, 1.0 = clean)
    pub io_stall: f64,
    /// Composite stability score (0.0 = dying, 1.0 = rock-solid)
    pub stability_score: f64,
    /// Is the system under OOM threat? (memory_pressure < 0.3)
    pub oom_risk: bool,
    /// Human-readable status
    pub status: String,
}

impl Default for HeatmapSnapshot {
    fn default() -> Self {
        HeatmapSnapshot {
            timestamp_secs: 0,
            memory_pressure: 1.0,
            memory_rss_bytes: 0,
            memory_vms_bytes: 0,
            cpu_saturation: 1.0,
            load_avg_1m: 0.0,
            fd_pressure: 1.0,
            fd_open: 0,
            fd_limit: 65536,
            uptime_secs: 0,
            io_stall: 1.0,
            stability_score: 1.0,
            oom_risk: false,
            status: "unknown".into(),
        }
    }
}

// ── Heatmap History ──

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct HeatmapHistory {
    pub snapshots: Vec<HeatmapSnapshot>,
    /// Count of OOM events detected
    pub oom_events: u64,
    /// Longest stable period (seconds without OOM risk)
    pub longest_stable_period_secs: u64,
    /// Average stability score
    pub avg_stability: f64,
}

fn history_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".flux").join("heatmap.json")
}

pub fn load_history() -> HeatmapHistory {
    let path = history_path();
    if path.exists() {
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        HeatmapHistory::default()
    }
}

fn save_history(history: &HeatmapHistory) {
    let path = history_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(history) {
        let _ = fs::write(&path, json);
    }
}

// ── Snapshot Capture ──

/// Capture a heatmap snapshot of current system health.
pub fn capture_heatmap() -> HeatmapSnapshot {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // ── Memory Pressure ──
    let (memory_pressure, rss_bytes, vms_bytes) = measure_memory();

    // ── CPU Saturation ──
    let (cpu_saturation, load_avg_1m) = measure_cpu();

    // ── FD Pressure ──
    let (fd_pressure, fd_open, fd_limit) = measure_fds();

    // ── I/O Stall ──
    let io_stall = measure_io_stall();

    // ── Uptime ──
    let uptime_secs = measure_uptime();

    // ── Composite stability score ──
    // Weighted: memory 35%, CPU 25%, FD 20%, I/O 10%, uptime 10%
    let stability_score = memory_pressure * 0.35
        + cpu_saturation * 0.25
        + fd_pressure * 0.20
        + io_stall * 0.10
        + uptime_factor(uptime_secs) * 0.10;

    let oom_risk = memory_pressure < 0.3;
    let status = if oom_risk {
        "OOM_RISK".into()
    } else if stability_score < 0.5 {
        "UNSTABLE".into()
    } else if stability_score < 0.75 {
        "DEGRADED".into()
    } else if stability_score < 0.9 {
        "STABLE".into()
    } else {
        "HEALTHY".into()
    };

    let snapshot = HeatmapSnapshot {
        timestamp_secs: now,
        memory_pressure,
        memory_rss_bytes: rss_bytes,
        memory_vms_bytes: vms_bytes,
        cpu_saturation,
        load_avg_1m,
        fd_pressure,
        fd_open,
        fd_limit,
        uptime_secs,
        io_stall,
        stability_score: stability_score.clamp(0.0, 1.0),
        oom_risk,
        status,
    };

    // Persist to history
    let mut history = load_history();
    history.snapshots.push(snapshot.clone());
    if history.snapshots.len() > 500 {
        history.snapshots = history.snapshots.split_off(100);
    }
    if oom_risk {
        history.oom_events += 1;
    }

    // Compute rolling average
    let total_stability: f64 = history.snapshots.iter().map(|s| s.stability_score).sum();
    history.avg_stability = total_stability / history.snapshots.len() as f64;

    // Longest stable period
    let mut current_stable = 0u64;
    let longest = history.longest_stable_period_secs;
    for s in history.snapshots.iter().rev() {
        if !s.oom_risk {
            current_stable += 1;
        } else {
            break;
        }
    }
    // Each snapshot represents ~1 second (typical poll interval)
    if current_stable > longest {
        history.longest_stable_period_secs = current_stable;
    }

    save_history(&history);

    snapshot
}

// ── Measurement Functions ──

fn measure_memory() -> (f64, u64, u64) {
    let pid = std::process::id();
    let status_path = format!("/proc/{}/status", pid);

    let mut rss_bytes = 0u64;
    let mut vms_bytes = 0u64;

    if let Ok(content) = fs::read_to_string(&status_path) {
        for line in content.lines() {
            if line.starts_with("VmRSS:") {
                rss_bytes = parse_kb_field(line);
            }
            if line.starts_with("VmSize:") {
                vms_bytes = parse_kb_field(line) * 1024; // kB → bytes
            }
        }
    }

    // Memory pressure: ratio of RSS to typical limit.
    // For fluxc, assume 256MB is nominal, 512MB is danger.
    let nominal_bytes = 256 * 1024 * 1024; // 256MB
    let danger_bytes = 512 * 1024 * 1024;  // 512MB
    let pressure = if rss_bytes > danger_bytes {
        0.1 // critical
    } else if rss_bytes > nominal_bytes {
        1.0 - (rss_bytes - nominal_bytes) as f64 / (danger_bytes - nominal_bytes) as f64
    } else {
        1.0 - rss_bytes as f64 / nominal_bytes as f64 * 0.3 // slight penalty for any usage
    };

    (pressure.max(0.0).min(1.0), rss_bytes, vms_bytes)
}

fn parse_kb_field(line: &str) -> u64 {
    line.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0)
}

fn measure_cpu() -> (f64, f64) {
    let load_avg_1m = if let Ok(content) = fs::read_to_string("/proc/loadavg") {
        content.split_whitespace()
            .next()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
    } else {
        0.0
    };

    // Count CPU cores
    let cpu_count = if let Ok(content) = fs::read_to_string("/proc/cpuinfo") {
        content.lines().filter(|l| l.starts_with("processor")).count() as f64
    } else {
        4.0 // assume 4 cores
    };

    let cpu_count = cpu_count.max(1.0);

    // Saturation: load / cores
    let saturation = load_avg_1m / cpu_count;
    let score = if saturation > 2.0 {
        0.1 // severely overloaded
    } else if saturation > 1.0 {
        1.0 - (saturation - 1.0) * 0.5
    } else {
        1.0 - saturation * 0.3
    };

    (score.max(0.0).min(1.0), load_avg_1m)
}

fn measure_fds() -> (f64, u64, u64) {
    let pid = std::process::id();
    let fd_dir = format!("/proc/{}/fd", pid);

    let fd_open = if let Ok(entries) = fs::read_dir(&fd_dir) {
        entries.count() as u64
    } else {
        0
    };

    // FD limit from /proc/<pid>/limits
    let fd_limit = if let Ok(content) = fs::read_to_string(format!("/proc/{}/limits", pid)) {
        content.lines()
            .find(|l| l.contains("open files"))
            .and_then(|l| l.split_whitespace().nth(3))
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(65536)
    } else {
        65536
    };

    let ratio = fd_open as f64 / fd_limit as f64;
    let pressure = if ratio > 0.9 {
        0.1 // critical
    } else if ratio > 0.5 {
        1.0 - (ratio - 0.5) * 2.0
    } else {
        1.0 - ratio * 0.5
    };

    (pressure.max(0.0).min(1.0), fd_open, fd_limit)
}

fn measure_io_stall() -> f64 {
    // Check /proc/diskstats for I/O wait
    // Simplified: check if any disk has high avg queue length
    if let Ok(content) = fs::read_to_string("/proc/diskstats") {
        let mut max_io_time = 0u64;
        for line in content.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 14 {
                // Field 13 (0-indexed: 12) is time spent doing I/O (ms)
                if let Ok(io_ms) = fields[12].parse::<u64>() {
                    max_io_time = max_io_time.max(io_ms);
                }
            }
        }
        // If any disk has > 10000ms I/O time, it's under heavy load
        let score = if max_io_time > 10000 {
            0.3
        } else if max_io_time > 5000 {
            0.6
        } else {
            1.0 - (max_io_time as f64 / 5000.0) * 0.4
        };
        return score.max(0.1).min(1.0);
    }
    0.8 // can't measure, assume okay
}

fn measure_uptime() -> u64 {
    if let Ok(content) = fs::read_to_string("/proc/uptime") {
        content.split_whitespace()
            .next()
            .and_then(|s| s.parse::<f64>().ok())
            .map(|u| u as u64)
            .unwrap_or(0)
    } else {
        0
    }
}

fn uptime_factor(uptime_secs: u64) -> f64 {
    // Longer uptime = more stable (up to 24 hours = 1.0)
    if uptime_secs > 86400 {
        1.0
    } else {
        uptime_secs as f64 / 86400.0
    }
}

// ── Heatmap Generators ──

/// Generate an ASCII heatmap of the last N stability snapshots.
/// Each character represents a stability tier:
///   █ = healthy (>0.9)   ▓ = stable (>0.75)   ▒ = degraded (>0.5)   ░ = unstable (>0.3)   · = critical
pub fn generate_ascii_heatmap(history: &HeatmapHistory, width: usize) -> String {
    if history.snapshots.is_empty() {
        return "No heatmap data. Run flux_heatmap first.".into();
    }

    let recent: Vec<&HeatmapSnapshot> = history.snapshots.iter().rev().take(width).collect();
    let chars: String = recent.iter().map(|s| {
        if s.stability_score > 0.9 { '█' }
        else if s.stability_score > 0.75 { '▓' }
        else if s.stability_score > 0.5 { '▒' }
        else if s.stability_score > 0.3 { '░' }
        else { '·' }
    }).collect();

    let oom_count = history.snapshots.iter().filter(|s| s.oom_risk).count();
    let avg = history.avg_stability;
    let max_stable = history.longest_stable_period_secs;

    format!(
        "🔥 Stability Heatmap (last {} snapshots)\n\
         {}\n\
         \n  Average stability: {:.1}%\n\
         OOM events: {}\n\
         Longest stable run: {}s\n\
         Status: {}",
        recent.len(),
        chars,
        avg * 100.0,
        oom_count,
        max_stable,
        if avg > 0.9 { "HEALTHY ✓" }
        else if avg > 0.75 { "STABLE" }
        else if avg > 0.5 { "DEGRADED ⚠" }
        else if avg > 0.3 { "UNSTABLE ✗" }
        else { "CRITICAL 🚨" }
    )
}

/// Format a single snapshot as text.
pub fn format_snapshot(s: &HeatmapSnapshot) -> String {
    format!(
        "🔥 Flux Stability Snapshot\n\
         \n  Memory:   {:.1}% pressure · RSS {:.1}MB / VMS {:.1}MB\n\
         CPU:      {:.1}% idle · load {:.2} · {:.0}% saturation\n\
         FDs:      {} / {} ({:.1}%)\n\
         I/O:      {:.1}% stall\n\
         Uptime:   {}s\n\
         \n  Stability: {:.1}% — {}{}",
        (1.0 - s.memory_pressure) * 100.0,
        s.memory_rss_bytes as f64 / 1_048_576.0,
        s.memory_vms_bytes as f64 / 1_048_576.0,
        s.cpu_saturation * 100.0,
        s.load_avg_1m,
        (1.0 - s.cpu_saturation) * 100.0,
        s.fd_open, s.fd_limit,
        (s.fd_open as f64 / s.fd_limit as f64) * 100.0,
        (1.0 - s.io_stall) * 100.0,
        s.uptime_secs,
        s.stability_score * 100.0,
        s.status,
        if s.oom_risk { " ⚠ OOM RISK" } else { "" }
    )
}

/// JSON snapshot for webhooks/API.
pub fn snapshot_json(s: &HeatmapSnapshot) -> serde_json::Value {
    serde_json::json!({
        "timestamp_secs": s.timestamp_secs,
        "stability_score": s.stability_score,
        "status": s.status,
        "oom_risk": s.oom_risk,
        "memory": {
            "pressure_pct": (1.0 - s.memory_pressure) * 100.0,
            "rss_mb": s.memory_rss_bytes as f64 / 1_048_576.0,
            "vms_mb": s.memory_vms_bytes as f64 / 1_048_576.0,
        },
        "cpu": {
            "idle_pct": s.cpu_saturation * 100.0,
            "load_1m": s.load_avg_1m,
        },
        "fd": {
            "open": s.fd_open,
            "limit": s.fd_limit,
            "usage_pct": (s.fd_open as f64 / s.fd_limit as f64) * 100.0,
        },
        "io": {
            "stall_pct": (1.0 - s.io_stall) * 100.0,
        },
        "uptime_secs": s.uptime_secs,
    })
}

/// Get the current stability score for prediction integration.
/// Returns 0.0-1.0 where lower = more unstable.
pub fn current_stability_score() -> f64 {
    let history = load_history();
    if history.snapshots.is_empty() {
        return 0.8; // assume stable if no data
    }
    // Weighted: 70% last snapshot, 30% average
    let last = history.snapshots.last().map(|s| s.stability_score).unwrap_or(0.8);
    let avg = history.avg_stability;
    last * 0.7 + avg * 0.3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capture_heatmap() {
        let snapshot = capture_heatmap();
        println!("Heatmap: stability={:.2}, status={}, oom={}", 
            snapshot.stability_score, snapshot.status, snapshot.oom_risk);
        assert!(snapshot.stability_score >= 0.0 && snapshot.stability_score <= 1.0);
        assert!(!snapshot.status.is_empty());
    }

    #[test]
    fn test_ascii_heatmap() {
        let mut history = HeatmapHistory::default();
        for i in 0..50 {
            history.snapshots.push(HeatmapSnapshot {
                timestamp_secs: i,
                stability_score: 0.5 + (i as f64 * 0.01),
                status: if i < 40 { "STABLE".into() } else { "HEALTHY".into() },
                oom_risk: false,
                ..Default::default()
            });
        }
        let ascii = generate_ascii_heatmap(&history, 30);
        println!("{}", ascii);
        assert!(ascii.contains('█') || ascii.contains('▓'));
    }

    #[test]
    fn test_stability_score() {
        let score = current_stability_score();
        println!("Current stability: {:.2}", score);
        assert!(score >= 0.0 && score <= 1.0);
    }

    #[test]
    fn test_oom_detection() {
        let mut snap = HeatmapSnapshot::default();
        snap.memory_pressure = 0.2;
        snap.stability_score = 0.2;
        snap.oom_risk = true;
        snap.status = "OOM_RISK".into();
        let json = snapshot_json(&snap);
        assert_eq!(json["oom_risk"], true);
        assert_eq!(json["status"], "OOM_RISK");
    }
}
