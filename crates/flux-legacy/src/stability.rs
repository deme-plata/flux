//! stability.rs — PROTOTYPE 10, tailored for the LIVE-NODE job (owner: rocky-vision).
//!
//! P1–P9 analyze/split/verify/land the *source*. P10 audits a *running* node: it reads the live
//! q-api-server's stability signals and classifies them against the runbook thresholds into a
//! fatal-vs-cosmetic verdict — the exact "is the node stable for now?" reasoning, automated.
//!
//! Two halves: a PURE [`assess`] over [`NodeSignals`] (fully unit-tested, the thresholds live here)
//! and a best-effort [`probe`] that gathers signals from the system (`df`/`pgrep`/`journalctl`/
//! `curl`) — impure, environment-specific, not unit-tested.

use serde::{Deserialize, Serialize};
use std::process::Command;

/// Per-signal health.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Health {
    Ok,
    Watch,
    Danger,
}

/// Overall node verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    /// up + serving + correct DB; any issues are non-fatal (e.g. lag under standalone)
    StableForNow,
    /// non-fatal degradation worth watching (disk margin, peers, syslog, stall)
    WatchClosely,
    /// a fatal condition — act now (down, wrong DB, root critically low, not serving)
    InterventionNeeded,
}

/// Observed runtime signals of a running node. `u64::MAX` / `f64::NAN`-free: unknowns are 0 + the
/// corresponding `*_known` flag false so `assess` doesn't punish a signal it couldn't read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSignals {
    pub process_alive: bool,
    pub root_free_gb: f64,
    pub home_free_gb: f64,
    pub ram_avail_gb: f64,
    pub network_gap: u64,
    pub stall_rising: bool,
    pub peers: u32,
    /// false = nothing answered with a peer count. A count we could not read is not "0 peers".
    pub peers_known: bool,
    /// HTTP status of the public endpoint (200 = serving). 0 = not probed.
    pub serving_http: u32,
    /// the authoritative home DB is the one open (CLAUDE.md: data-mainnet-genesis)
    pub authoritative_db: bool,
    /// false = we never found the node's own process, so we do not know which DB it has open.
    pub authoritative_db_known: bool,
    /// the path marker the DB check used, so the report names THIS node's database
    pub db_marker: String,
    /// standalone mode produces on own tip → a sync gap is NOT fatal
    pub authoritative_standalone: bool,
    pub journal_volatile: bool,
    pub syslog_mb: u64,
}

impl Default for NodeSignals {
    fn default() -> Self {
        NodeSignals {
            process_alive: true,
            root_free_gb: 99.0,
            home_free_gb: 99.0,
            ram_avail_gb: 99.0,
            network_gap: 0,
            stall_rising: false,
            peers: 8,
            peers_known: true,
            serving_http: 200,
            authoritative_db: true,
            authoritative_db_known: true,
            db_marker: "data-mainnet-genesis".into(),
            authoritative_standalone: true,
            journal_volatile: true,
            syslog_mb: 0,
        }
    }
}

/// One classified signal, optionally blaming a crate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub signal: String,
    pub health: Health,
    pub detail: String,
    pub implicated_crate: Option<String>,
}

/// The stability audit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StabilityReport {
    pub verdict: Verdict,
    pub fatal: bool,
    pub findings: Vec<Finding>,
}

impl StabilityReport {
    fn n(&self, h: Health) -> usize {
        self.findings.iter().filter(|f| f.health == h).count()
    }
}

// ── runbook thresholds (CLAUDE.md) ──
/// root partition this empty = block production death risk
pub const ROOT_DANGER_GB: f64 = 2.0;
pub const ROOT_WATCH_GB: f64 = 5.0;
/// available RAM this low = OOM risk
pub const RAM_DANGER_GB: f64 = 2.0;
/// syslog over the logrotate cap
pub const SYSLOG_WATCH_MB: u64 = 200;
/// a gap bigger than this is a real lag (cosmetic under standalone, fatal otherwise)
pub const GAP_WATCH: u64 = 1000;

/// Classify the signals against the runbook thresholds. PURE.
pub fn assess(s: &NodeSignals) -> StabilityReport {
    let mut f = Vec::new();
    let mut push = |signal: &str, health: Health, detail: String, krate: Option<&str>| {
        f.push(Finding { signal: signal.into(), health, detail, implicated_crate: krate.map(String::from) });
    };

    // process — the most fatal
    if s.process_alive {
        push("process", Health::Ok, "q-api-server is running".into(), None);
    } else {
        push("process", Health::Danger, "q-api-server process is GONE".into(), Some("q-api-server"));
    }

    // authoritative DB — wrong DB corrupts emission/balances (CLAUDE.md)
    if !s.authoritative_db_known {
        push("db", Health::Watch, "could not find the node's process — which DB is open is UNKNOWN".into(), None);
    } else if s.authoritative_db {
        push("db", Health::Ok, format!("expected DB open ({})", s.db_marker), None);
    } else {
        push("db", Health::Danger, "NOT the authoritative DB — emission/balance corruption risk".into(), Some("q-storage"));
    }

    // serving the public endpoint
    match s.serving_http {
        200 => push("serving", Health::Ok, "endpoint returns 200".into(), None),
        0 => push("serving", Health::Watch, "endpoint not probed".into(), None),
        code => push("serving", Health::Danger, format!("endpoint returns {code}"), Some("q-api-server")),
    }

    // root disk — the silent killer
    if s.root_free_gb < ROOT_DANGER_GB {
        push("root-disk", Health::Danger, format!("{:.1}G free — block production dies when root fills", s.root_free_gb), None);
    } else if s.root_free_gb < ROOT_WATCH_GB {
        push("root-disk", Health::Watch, format!("{:.1}G free — tight, watch syslog/journal", s.root_free_gb), None);
    } else {
        push("root-disk", Health::Ok, format!("{:.1}G free", s.root_free_gb), None);
    }

    // RAM (available, not RSS — RSS alone is a false alarm)
    if s.ram_avail_gb < RAM_DANGER_GB {
        push("ram", Health::Danger, format!("{:.1}G available — OOM risk", s.ram_avail_gb), None);
    } else {
        push("ram", Health::Ok, format!("{:.1}G available", s.ram_avail_gb), None);
    }

    // journal must be volatile or it fills root at INFO/DEBUG
    if !s.journal_volatile {
        push("journal", Health::Watch, "journal NOT volatile — fills root at INFO/DEBUG".into(), None);
    }
    if s.syslog_mb > SYSLOG_WATCH_MB {
        push("syslog", Health::Watch, format!("{}MB — over the {}MB logrotate cap", s.syslog_mb, SYSLOG_WATCH_MB), None);
    }

    // sync gap — cosmetic under standalone, otherwise a real divergence
    if s.network_gap > GAP_WATCH {
        let (h, why) = if s.authoritative_standalone {
            (Health::Watch, "lagging but standalone → produces on own tip, NOT fatal")
        } else {
            (Health::Danger, "lagging and NOT standalone → divergence risk")
        };
        let detail = format!("{} blocks behind{}{}", s.network_gap, if s.stall_rising { " · stall rising" } else { "" }, format!(" — {why}"));
        push("sync-gap", h, detail, Some("q-storage"));
    }

    // peers
    if !s.peers_known {
        push("peers", Health::Watch, "peer count not readable (metrics, status and journal all silent)".into(), None);
    } else if s.peers < 2 {
        push("peers", Health::Danger, format!("{} peers — isolated", s.peers), Some("q-network"));
    } else if s.peers < 4 {
        push("peers", Health::Watch, format!("{} peers — thin", s.peers), Some("q-network"));
    } else {
        push("peers", Health::Ok, format!("{} peers", s.peers), None);
    }

    // verdict
    let report_dangers = f.iter().filter(|x| x.health == Health::Danger).count();
    let fatal = report_dangers > 0;
    // StableForNow = pristine (no danger, no watch); WatchClosely = up but non-fatal degradation;
    // InterventionNeeded = any fatal danger. The operator-meaningful split is `fatal`.
    let verdict = if fatal {
        Verdict::InterventionNeeded
    } else if f.iter().any(|x| x.health == Health::Watch) {
        Verdict::WatchClosely
    } else {
        Verdict::StableForNow
    };

    StabilityReport { verdict, fatal, findings: f }
}

/// Render the audit (operator-facing, the "is it stable?" panel).
pub fn render(r: &StabilityReport) -> String {
    let mut s = String::from("🩺 NODE STABILITY\n");
    for fnd in &r.findings {
        let icon = match fnd.health { Health::Ok => "✅", Health::Watch => "🟡", Health::Danger => "🔴" };
        let blame = fnd.implicated_crate.as_deref().map(|c| format!("  [{c}]")).unwrap_or_default();
        s.push_str(&format!("  {icon} {:<11} {}{}\n", fnd.signal, fnd.detail, blame));
    }
    let (icon, word) = match r.verdict {
        Verdict::StableForNow => ("✅", "STABLE FOR NOW"),
        Verdict::WatchClosely => ("🟡", "WATCH CLOSELY"),
        Verdict::InterventionNeeded => ("🔴", "INTERVENTION NEEDED"),
    };
    s.push_str(&format!(
        "  ── {icon} {word} — {} ok · {} watch · {} danger\n",
        r.n(Health::Ok), r.n(Health::Watch), r.n(Health::Danger),
    ));
    s
}

// ───────────────────────── best-effort probe (impure) ─────────────────────────

fn sh(cmd: &str) -> String {
    Command::new("sh").arg("-c").arg(cmd).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Which process IS the node? The unit's MainPID when systemd knows it. Otherwise the first candidate
/// whose EXECUTABLE matches: `pgrep -f … | head -1` also matches the `sh -c` running pgrep and any
/// watchdog script naming the node, and read that process's (empty) fd table as "wrong database".
pub fn pick_node_pid(main_pid: &str, candidates: &[(u32, String)], process_match: &str) -> Option<u32> {
    if let Ok(pid) = main_pid.trim().parse::<u32>() {
        if pid != 0 {
            return Some(pid);
        }
    }
    candidates.iter().find(|(_, exe)| {
        exe.rsplit('/').next().map(|name| name.contains(process_match)).unwrap_or(false)
    }).map(|(pid, _)| *pid)
}

/// One sample from Prometheus text, by exact metric name (`qnk_gap` must not match `qnk_gap_to_tip`).
pub fn metric_value(text: &str, name: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        (parts.next()? == name).then(|| parts.next()?.parse::<f64>().ok().map(|v| v as u64)).flatten()
    })
}

/// The node's `/metrics` URL, from any endpoint on the same host.
pub fn metrics_url(endpoint: &str) -> Option<String> {
    let (scheme, rest) = endpoint.split_once("://")?;
    let host = rest.split('/').next().filter(|h| !h.is_empty())?;
    Some(format!("{scheme}://{host}/metrics"))
}

/// Peer count from the `/api/v1/status` JSON (`data.peers`, or the same field at the top level).
pub fn status_peers(json: &str) -> Option<u32> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let d = v.get("data").unwrap_or(&v);
    d.get("peers").or_else(|| d.get("connected_peers"))?.as_u64().map(|n| n as u32)
}

/// Gather live signals from the system. `service` = unit name (for journalctl), `db_marker` = a path
/// substring proving the authoritative DB, `endpoint` = the public URL to curl. Best-effort: a field
/// that can't be read falls back to a benign default.
pub fn probe(service: &str, process_match: &str, db_marker: &str, endpoint: &str) -> NodeSignals {
    let candidates: Vec<(u32, String)> = sh(&format!("pgrep -f '{process_match}'"))
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .map(|p| (p, sh(&format!("readlink -f /proc/{p}/exe 2>/dev/null"))))
        .collect();
    let main_pid = sh(&format!("systemctl show {service} -p MainPID --value 2>/dev/null"));
    let pid = pick_node_pid(&main_pid, &candidates, process_match).map(|p| p.to_string()).unwrap_or_default();
    let process_alive = !pid.is_empty();

    let avail_kb = |mount: &str| -> f64 {
        sh(&format!("df --output=avail {mount} 2>/dev/null | tail -1 | tr -d ' '"))
            .parse::<f64>().unwrap_or(0.0) / 1_048_576.0
    };
    let ram_avail_gb = sh("free -m | awk '/Mem:/{print $7}'").parse::<f64>().unwrap_or(0.0) / 1024.0;

    let journal = sh(&format!("journalctl -u {service} --since '5 minutes ago' --no-pager 2>/dev/null"));
    let last_num = |re: &str| -> u64 {
        journal.lines().rev().filter_map(|l| {
            l.find(re).and_then(|i| l[i + re.len()..].split(|c: char| !c.is_ascii_digit()).next())
                .and_then(|d| d.parse::<u64>().ok())
        }).next().unwrap_or(0)
    };
    // The node's own counters first: production logs q_network at WARN, so the old `gap=`/`peers=` journal
    // lines are absent there and read as "0 peers" while 8 are connected, and "no gap" on a stuck follower.
    let metrics = metrics_url(endpoint)
        .map(|u| sh(&format!("curl -s --max-time 8 {u} 2>/dev/null")))
        .unwrap_or_default();
    let status = sh(&format!("curl -s --max-time 8 {endpoint} 2>/dev/null"));
    let network_gap = metric_value(&metrics, "qnk_gap_to_tip").unwrap_or_else(|| last_num("gap="));
    let journal_peers = last_num("peers=") as u32;
    let read_peers = metric_value(&metrics, "qnk_peers_connected").map(|n| n as u32)
        .or_else(|| status_peers(&status))
        .or((journal_peers > 0).then_some(journal_peers));
    let peers_known = read_peers.is_some();
    let peers = read_peers.unwrap_or(0);
    let stalls: Vec<u64> = journal.lines().filter_map(|l| {
        l.find("stall=").and_then(|i| l[i + 6..].split(|c: char| !c.is_ascii_digit()).next()).and_then(|d| d.parse().ok())
    }).collect();
    let stall_rising = stalls.len() >= 2 && stalls[stalls.len() - 1] > stalls[0];

    let serving_http = sh(&format!("curl -s -o /dev/null -w '%{{http_code}}' --max-time 8 {endpoint} 2>/dev/null"))
        .parse().unwrap_or(0);

    let authoritative_db = !pid.is_empty()
        && sh(&format!("ls -l /proc/{pid}/fd 2>/dev/null | grep -c '{db_marker}'")).parse::<u64>().unwrap_or(0) > 100;
    let authoritative_standalone = !pid.is_empty()
        && sh(&format!("tr '\\0' '\\n' < /proc/{pid}/environ 2>/dev/null | grep -c 'Q_AUTHORITATIVE_STANDALONE=true'")).parse::<u64>().unwrap_or(0) > 0;
    let journal_volatile = sh("du -s /var/log/journal 2>/dev/null | awk '{print $1}'").parse::<u64>().unwrap_or(0) < 1024;
    let syslog_mb = sh("du -m /var/log/syslog 2>/dev/null | awk '{print $1}'").parse().unwrap_or(0);

    NodeSignals {
        process_alive,
        root_free_gb: avail_kb("/"),
        home_free_gb: avail_kb("/home"),
        ram_avail_gb,
        network_gap,
        stall_rising,
        peers,
        peers_known,
        serving_http,
        authoritative_db,
        authoritative_db_known: process_alive,
        db_marker: db_marker.to_string(),
        authoritative_standalone,
        journal_volatile,
        syslog_mb,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epsilon_node_we_audited_reads_as_watch_not_fatal() {
        // exactly the state we found by hand: stalled 21K but standalone, disk 3.2G, 2 peers, serving,
        // right DB. NON-FATAL ("stable for now") but with watch items → WatchClosely, not pristine.
        let s = NodeSignals {
            root_free_gb: 3.2,
            ram_avail_gb: 48.0,
            network_gap: 21474,
            stall_rising: true,
            peers: 2,
            serving_http: 200,
            authoritative_db: true,
            authoritative_standalone: true,
            syslog_mb: 225,
            ..Default::default()
        };
        let r = assess(&s);
        assert!(!r.fatal, "the node is NOT dying — no intervention");
        assert_eq!(r.verdict, Verdict::WatchClosely, "{:?}", r.findings);
        // the gap is blamed on q-storage, non-fatal under standalone
        assert!(r.findings.iter().any(|f| f.signal == "sync-gap" && f.implicated_crate.as_deref() == Some("q-storage") && f.health == Health::Watch));
        // peers thin → q-network watch
        assert!(r.findings.iter().any(|f| f.signal == "peers" && f.implicated_crate.as_deref() == Some("q-network")));
    }

    #[test]
    fn root_disk_critical_is_fatal_intervention() {
        let s = NodeSignals { root_free_gb: 1.2, ..Default::default() };
        let r = assess(&s);
        assert!(r.fatal);
        assert_eq!(r.verdict, Verdict::InterventionNeeded);
    }

    #[test]
    fn wrong_db_or_dead_process_or_not_serving_is_fatal() {
        for s in [
            NodeSignals { authoritative_db: false, ..Default::default() },
            NodeSignals { process_alive: false, ..Default::default() },
            NodeSignals { serving_http: 502, ..Default::default() },
        ] {
            let r = assess(&s);
            assert_eq!(r.verdict, Verdict::InterventionNeeded, "{:?}", r.findings);
        }
    }

    #[test]
    fn gap_without_standalone_is_a_danger() {
        let s = NodeSignals { network_gap: 21474, authoritative_standalone: false, ..Default::default() };
        let r = assess(&s);
        assert_eq!(r.verdict, Verdict::InterventionNeeded);
        assert!(r.findings.iter().any(|f| f.signal == "sync-gap" && f.health == Health::Danger));
    }

    // Measured on Epsilon 2026-11-04: MainPID 768045 holds 47,931 fds on the right DB and 8 peers, yet the
    // old probe printed "NOT the authoritative DB" and "0 peers" — two false REDs on a healthy node.
    #[test]
    fn the_node_is_its_unit_mainpid_not_the_first_pgrep_hit() {
        let hits = vec![(1200, "/usr/bin/dash".to_string()), (768045, "/opt/q-api-server-v10.11.114k".to_string())];
        assert_eq!(pick_node_pid("768045\n", &hits, "q-api-server"), Some(768045));
    }

    #[test]
    fn without_a_unit_the_executable_decides_not_the_command_line() {
        let hits = vec![(1200, "/usr/bin/dash".to_string()), (1300, "/usr/bin/bash".to_string()), (9, "/opt/q-api-server-v10.11.114k".to_string())];
        assert_eq!(pick_node_pid("0", &hits, "q-api-server"), Some(9));
        assert_eq!(pick_node_pid("", &hits[..2], "q-api-server"), None, "a watchdog script is not the node");
    }

    #[test]
    fn counters_come_from_the_nodes_own_metrics_by_exact_name() {
        let text = "# HELP qnk_gap_to_tip x\nqnk_gap_to_tip 115912\nqnk_peers_connected 2\nqnk_gap_to_tip_extra 7\n";
        assert_eq!(metric_value(text, "qnk_gap_to_tip"), Some(115912));
        assert_eq!(metric_value(text, "qnk_peers_connected"), Some(2));
        assert_eq!(metric_value(text, "qnk_gap"), None);
        assert_eq!(metric_value("qnk_block_pack_response_bytes_sum 121774557.0", "qnk_block_pack_response_bytes_sum"), Some(121774557));
        assert_eq!(metrics_url("http://127.0.0.1:8080/api/v1/status").as_deref(), Some("http://127.0.0.1:8080/metrics"));
        assert_eq!(metrics_url("not a url"), None);
    }

    #[test]
    fn peer_count_reads_the_status_shape_the_node_really_sends() {
        assert_eq!(status_peers(r#"{"success":true,"data":{"peers":8,"connected_peers":8}}"#), Some(8));
        assert_eq!(status_peers(r#"{"connected_peers":3}"#), Some(3));
        assert_eq!(status_peers("<html>"), None);
    }

    #[test]
    fn an_unreadable_signal_is_a_watch_never_a_fatal() {
        let r = assess(&NodeSignals { peers: 0, peers_known: false, authoritative_db: false, authoritative_db_known: false, ..Default::default() });
        assert!(!r.fatal, "{:?}", r.findings);
        assert_eq!(r.verdict, Verdict::WatchClosely);
    }

    #[test]
    fn the_db_line_names_the_database_this_node_was_checked_for() {
        let r = assess(&NodeSignals { db_marker: "/var/lib/quillon/db".into(), ..Default::default() });
        let db = r.findings.iter().find(|f| f.signal == "db").unwrap();
        assert!(db.detail.contains("/var/lib/quillon/db"), "{}", db.detail);
        assert!(!db.detail.contains("data-mainnet-genesis"), "{}", db.detail);
    }

    #[test]
    fn a_real_zero_and_a_real_wrong_db_still_alarm() {
        assert!(assess(&NodeSignals { peers: 0, ..Default::default() }).fatal, "connected to nobody, measured");
        assert!(assess(&NodeSignals { authoritative_db: false, ..Default::default() }).fatal, "process found, DB not open");
    }

    #[test]
    fn a_truly_healthy_node_is_stable() {
        let r = assess(&NodeSignals::default());
        assert_eq!(r.verdict, Verdict::StableForNow);
        assert!(!r.fatal);
    }
}
