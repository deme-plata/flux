//! pulse.rs — flux-legacy **Pulse**: RUNTIME EVIDENCE drives the refactor priority.
//!
//! Static LOC says `q-api-server/main.rs` (27,054 LOC) is the biggest file. The LIVE Epsilon node
//! log says its VDF/shard path emits ~302k WARNs/hour ("Dropping submissions: VDF verification
//! already in flight"). Pulse mines the node's journald stream, maps each log line's module target
//! (`q_api_server::streaming` → crate `q-api-server`) to a **per-crate runtime-pain** score, and
//! fuses it into the [`crate::RefactorTask`] ranking — so a god-file that is ALSO on fire jumps the
//! queue. Pure over an injected log blob (testable); the live reader shells `journalctl`.

use crate::RefactorTask;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Log severity, parsed from the journald line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level { Error, Warn, Info, Other }

/// What kind of pain a message represents (keyword-classified).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Category { Panic, Freeze, Timeout, Rejection, VdfContention, Other }

/// Map a Rust log target ("q_api_server::high_performance_server") to a crate name ("q-api-server").
pub fn module_to_crate(module: &str) -> String {
    module.split("::").next().unwrap_or(module).trim().replace('_', "-")
}

/// Classify a log message into a [`Category`] by keyword (cheap, order matters: most-specific first).
pub fn classify(msg: &str) -> Category {
    let m = msg.to_lowercase();
    if m.contains("panic") { Category::Panic }
    else if m.contains("freeze") || m.contains("stall") || m.contains("stuck") || m.contains("hung") || m.contains("deadlock") { Category::Freeze }
    else if m.contains("vdf") && (m.contains("in flight") || m.contains("stale") || m.contains("contention")) { Category::VdfContention }
    else if m.contains("timeout") || m.contains("timed out") { Category::Timeout }
    else if m.contains("reject") || m.contains("dropping") || m.contains("invalid") || m.contains("unauthorized") { Category::Rejection }
    else { Category::Other }
}

/// Parse one journald line → `(Level, crate_name, Category)`. Returns `None` for non-log lines.
/// Shape: `… <LEVEL> <module>: <message>` (e.g. `…Z  WARN q_api_server: 🛡️ [Shard] Dropping…`).
pub fn parse_line(line: &str) -> Option<(Level, String, Category)> {
    let (level, after) = [(" ERROR ", Level::Error), (" WARN ", Level::Warn), (" INFO ", Level::Info), (" DEBUG ", Level::Other), (" TRACE ", Level::Other)]
        .iter()
        .find_map(|(kw, lvl)| line.find(kw).map(|i| (*lvl, &line[i + kw.len()..])))?;
    let colon = after.find(": ")?;
    let module = after[..colon].trim();
    // module must look like a log target (no spaces) — guards against matching prose containing ": "
    if module.is_empty() || module.contains(' ') { return None; }
    let msg = &after[colon + 2..];
    Some((level, module_to_crate(module), classify(msg)))
}

/// Per-crate runtime-pain rollup.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CratePulse {
    pub crate_name: String,
    pub errors: u64,
    pub warns: u64,
    pub panics: u64,
    pub timeouts: u64,
    pub rejections: u64,
    pub vdf_contention: u64,
    /// weighted pain (panics dominate; volume of warns still counts).
    pub pain: f64,
}

/// Whole-node runtime snapshot, crates sorted by pain (worst-first).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PulseReport {
    pub window: String,
    pub total_lines: u64,
    pub parsed: u64,
    pub crates: Vec<CratePulse>,
}

/// Pain weights — a panic is catastrophic; a flood of warns (302k VDF-contention/hr) is real pain too.
fn pain_of(c: &CratePulse) -> f64 {
    (c.panics as f64) * 1000.0
        + (c.errors as f64) * 5.0
        + (c.timeouts as f64) * 3.0
        + (c.rejections as f64) * 1.0
        + (c.vdf_contention as f64) * 0.5
        + (c.warns as f64) * 0.1
}

/// Mine a journald log blob → [`PulseReport`] (aggregate by crate). `window` is a human label
/// ("last 30 min") for the report.
pub fn mine(log: &str, window: &str) -> PulseReport {
    let mut by_crate: BTreeMap<String, CratePulse> = BTreeMap::new();
    let mut total = 0u64;
    let mut parsed = 0u64;
    for line in log.lines() {
        total += 1;
        let Some((level, krate, cat)) = parse_line(line) else { continue };
        parsed += 1;
        let e = by_crate.entry(krate.clone()).or_insert_with(|| CratePulse { crate_name: krate, ..Default::default() });
        match level { Level::Error => e.errors += 1, Level::Warn => e.warns += 1, _ => {} }
        match cat {
            Category::Panic => e.panics += 1,
            Category::Timeout => e.timeouts += 1,
            Category::Rejection => e.rejections += 1,
            Category::VdfContention => e.vdf_contention += 1,
            _ => {}
        }
    }
    let mut crates: Vec<CratePulse> = by_crate.into_values().map(|mut c| { c.pain = pain_of(&c); c }).collect();
    crates.sort_by(|a, b| b.pain.partial_cmp(&a.pain).unwrap_or(std::cmp::Ordering::Equal));
    PulseReport { window: window.to_string(), total_lines: total, parsed, crates }
}

/// crate → pain, for fusing into the static plan.
pub fn pain_index(report: &PulseReport) -> BTreeMap<String, f64> {
    report.crates.iter().map(|c| (c.crate_name.clone(), c.pain)).collect()
}

/// FUSE runtime evidence into the static plan: a task whose crate is on fire gets its `impact`
/// boosted (log-normalized so 302k warns lifts but doesn't infinitely dominate), then re-rank.
/// This is "use the live logs in the analysis" — the queue follows real pain, not just LOC.
pub fn apply_pulse(tasks: &mut [RefactorTask], pain: &BTreeMap<String, f64>) {
    for t in tasks.iter_mut() {
        if let Some(&p) = pain.get(&t.crate_name) {
            if p > 0.0 {
                let boost = (1.0 + p).ln() / 10.0; // 302k pain → ~+1.26; modest, bounded
                t.impact = (t.impact + boost).min(2.0);
            }
        }
    }
    tasks.sort_by(|a, b| b.impact.partial_cmp(&a.impact).unwrap_or(std::cmp::Ordering::Equal));
    for (i, t) in tasks.iter_mut().enumerate() { t.rank = i + 1; }
}

/// Live reader (Epsilon): read the running node's journald window. `since` e.g. "30 min ago".
pub fn read_journal(unit: &str, since: &str) -> Result<String, String> {
    let out = std::process::Command::new("journalctl")
        .args(["-u", unit, "--since", since, "--no-pager"])
        .output()
        .map_err(|e| format!("journalctl: {e}"))?;
    if !out.status.success() {
        return Err(format!("journalctl failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    // sanitized real lines from the live Epsilon q-api-server log (2026-06-03).
    const LOG: &str = "\
Jun 03 host q-api-server-v10-standalone[9]: 2026-06-03T00:00:00.0Z  WARN q_api_server: 🛡️ [Shard 0] Dropping 3 submissions: VDF verification already in flight (stale_for=12ms)
Jun 03 host q-api-server-v10-standalone[9]: 2026-06-03T00:00:00.1Z  WARN q_api_server: 🛡️ [Shard 1] Dropping 2 submissions: VDF verification already in flight (stale_for=9ms)
Jun 03 host q-api-server-v10-standalone[9]: 2026-06-03T00:00:00.2Z  WARN q_storage::kalman_predictor: 🔭 [KALMAN] Rejecting invalid bandwidth: -1
Jun 03 host q-api-server-v10-standalone[9]: 2026-06-03T00:00:00.3Z  WARN q_api_server::high_performance_server: Connection error from 1.2.3.4: read header from client timeout
Jun 03 host q-api-server-v10-standalone[9]: 2026-06-03T00:00:00.4Z  ERROR q_storage::turbo_sync: chunk apply failed
Jun 03 host systemd[1]: some unrelated line without a level
";

    #[test]
    fn module_maps_to_crate() {
        assert_eq!(module_to_crate("q_api_server::streaming"), "q-api-server");
        assert_eq!(module_to_crate("q_storage::kalman_predictor"), "q-storage");
        assert_eq!(module_to_crate("q_api_server"), "q-api-server");
    }

    #[test]
    fn classify_real_signals() {
        assert_eq!(classify("Dropping submissions: VDF verification already in flight (stale_for=12ms)"), Category::VdfContention);
        assert_eq!(classify("read header from client timeout"), Category::Timeout);
        assert_eq!(classify("Rejecting invalid bandwidth"), Category::Rejection);
        assert_eq!(classify("thread panicked at ..."), Category::Panic);
    }

    #[test]
    fn parse_line_extracts_level_crate_category() {
        let (lvl, krate, cat) = parse_line(LOG.lines().next().unwrap()).unwrap();
        assert_eq!(lvl, Level::Warn);
        assert_eq!(krate, "q-api-server");
        assert_eq!(cat, Category::VdfContention);
        assert!(parse_line("some unrelated line without a level").is_none());
    }

    #[test]
    fn mine_parses_mixed_log() {
        let r = mine(LOG, "test");
        assert_eq!(r.total_lines, 6);
        assert_eq!(r.parsed, 5); // the systemd line (no level) is skipped
        let api = r.crates.iter().find(|c| c.crate_name == "q-api-server").unwrap();
        assert_eq!(api.vdf_contention, 2);
        assert_eq!(api.timeouts, 1);
    }

    #[test]
    fn mine_ranks_flooding_crate_worst() {
        // realistic proportions: q-api-server FLOODS with VDF-contention warns (~302k/hr in prod),
        // which must out-rank a crate with a single louder ERROR — volume of pain is real pain.
        let mut log = String::new();
        for _ in 0..50 {
            log.push_str("host x[9]: T  WARN q_api_server: Dropping submissions: VDF verification already in flight (stale_for=12ms)\n");
        }
        log.push_str("host x[9]: T  ERROR q_storage::turbo_sync: chunk apply failed\n");
        let r = mine(&log, "test");
        assert_eq!(r.crates[0].crate_name, "q-api-server", "the flooding crate ranks first by pain");
        assert_eq!(r.crates[0].vdf_contention, 50);
        assert!(r.crates[0].pain > r.crates[1].pain);
    }

    #[test]
    fn apply_pulse_boosts_on_fire_crate() {
        let mut tasks = vec![
            RefactorTask { rank: 1, crate_name: "q-quiet".into(), kind: "add-tests".into(), target: "q-quiet".into(), detail: String::new(), impact: 0.9, effort: "low".into(), est_minutes: 30 },
            RefactorTask { rank: 2, crate_name: "q-api-server".into(), kind: "split-god-file".into(), target: "main.rs".into(), detail: String::new(), impact: 0.5, effort: "high".into(), est_minutes: 240 },
        ];
        let mut pain = BTreeMap::new();
        pain.insert("q-api-server".to_string(), 300_000.0); // on fire
        apply_pulse(&mut tasks, &pain);
        // q-api-server started lower-impact but its runtime pain lifts it to rank 1
        assert_eq!(tasks[0].crate_name, "q-api-server");
        assert_eq!(tasks[0].rank, 1);
    }
}
