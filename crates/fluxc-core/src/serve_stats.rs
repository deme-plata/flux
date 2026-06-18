// serve_stats — Live stats and related for serve
// Split from serve.rs as part of fluxc-core refactor (legacy_plan god-file split).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot;

use crate::version;

pub struct LiveStats {
    pub builds_completed: AtomicU64,
    pub cache_hits: AtomicU64,
    pub cache_misses: AtomicU64,
    pub total_build_time_ms: AtomicU64,
    pub tests_passed: AtomicU64,
    pub tests_failed: AtomicU64,
    pub p2p_peers: AtomicU64,
    pub dagknight_round: AtomicU64,
    pub mempool_txs: AtomicU64,
    pub last_event: parking_lot::Mutex<String>,
    /// In-process event queue: (event_type, data_json). Drained by SSE loop for sub-ms delivery.
    pub event_queue: parking_lot::Mutex<Vec<(String, String)>>,
    /// AI feed events: (agent, message, tool, timestamp_ms). Drained by SSE loop.
    pub feed_events: parking_lot::Mutex<Vec<(String, String, String, u64)>>,
    pub start_time_ms: u64,
}

impl LiveStats {
    pub fn new() -> Arc<Self> {
        Arc::new(LiveStats {
            builds_completed: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            total_build_time_ms: AtomicU64::new(0),
            tests_passed: AtomicU64::new(0),
            tests_failed: AtomicU64::new(0),
            p2p_peers: AtomicU64::new(0),
            dagknight_round: AtomicU64::new(0),
            mempool_txs: AtomicU64::new(0),
            last_event: parking_lot::Mutex::new(String::new()),
            event_queue: parking_lot::Mutex::new(Vec::new()),
            feed_events: parking_lot::Mutex::new(Vec::new()),
            start_time_ms: now_ms(),
        })
    }

    pub fn to_json(&self) -> String {
        let builds = self.builds_completed.load(Ordering::Relaxed);
        let hits = self.cache_hits.load(Ordering::Relaxed);
        let misses = self.cache_misses.load(Ordering::Relaxed);
        let total = hits + misses;
        let rate = if total > 0 { (hits as f64 / total as f64) * 100.0 } else { 0.0 };
        let build_time = self.total_build_time_ms.load(Ordering::Relaxed);
        let avg_time = if builds > 0 { build_time / builds } else { 0 };
        let uptime = (now_ms() - self.start_time_ms) / 1000;

        static SERVE_VERSION: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
            crate::version::VersionInfo::load(&crate::version::workspace_root())
                .map(|v| v.display())
                .unwrap_or_else(|_: String| "unknown".into())
        });

        let sap_json = r#"{"contrib":0.87,"latency":0.91,"stake":0.72,"accuracy":0.98,"uptime":0.94,"total":0.884,"peers":8}"#;
        format!(r#"{{"builds":{},"cache_hits":{},"cache_misses":{},"cache_rate":{:.1},"avg_build_ms":{},"total_build_ms":{},"tests_passed":{},"tests_failed":{},"p2p_peers":{},"dagknight_round":{},"mempool_txs":{},"uptime_secs":{},"timestamp":{},"mcp_tools":44,"version":"{}","sap":{},"xalgo":{{"temporal_trust":0.87,"consensus_align":0.94,"tx_quality":0.91,"topology_rank":0.78,"econ_efficiency":0.83,"total":0.866,"peers_scored":12}}}}"#,
            builds, hits, misses, rate, avg_time, build_time,
            self.tests_passed.load(Ordering::Relaxed),
            self.tests_failed.load(Ordering::Relaxed),
            self.p2p_peers.load(Ordering::Relaxed),
            self.dagknight_round.load(Ordering::Relaxed),
            self.mempool_txs.load(Ordering::Relaxed),
            uptime, now_ms(), &**SERVE_VERSION, sap_json
        )
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn init_live_stats() -> Arc<LiveStats> {
    LiveStats::new()
}
