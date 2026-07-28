//! Measured capture telemetry, and its honest mapping into SAP.
//!
//! Every number here is **observed**, never asserted: latencies come from a
//! monotonic clock around the actual capture call, and `integrity_failures`
//! counts frames whose BLAKE3 did not re-derive. Nothing is estimated, so the
//! SAP score this produces is a real measurement of the capture path rather
//! than a decorative figure.

use flux_p2p::sap::{PeerId, SAPScore, ScoreTable};
use flux_sap_feed::BenchResult;
use serde::{Deserialize, Serialize};

/// Rolling counters for one capture engine.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CaptureStats {
    pub attempts: u64,
    pub successes: u64,
    pub failures: u64,
    /// Frames that arrived but failed content-hash verification.
    pub integrity_failures: u32,
    pub bytes_captured: u64,
    /// Per-capture wall-clock latencies, milliseconds.
    pub latencies_ms: Vec<f64>,
}

impl CaptureStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_success(&mut self, latency_ms: f64, bytes: usize) {
        self.attempts += 1;
        self.successes += 1;
        self.bytes_captured += bytes as u64;
        self.latencies_ms.push(latency_ms);
    }

    pub fn record_failure(&mut self) {
        self.attempts += 1;
        self.failures += 1;
    }

    pub fn record_integrity_failure(&mut self) {
        self.integrity_failures += 1;
    }

    /// Fraction of attempts that produced a frame (0.0 when nothing tried).
    pub fn success_rate(&self) -> f64 {
        if self.attempts == 0 {
            0.0
        } else {
            self.successes as f64 / self.attempts as f64
        }
    }

    /// Median capture latency. Returns 0.0 with no samples — callers should
    /// treat "no samples" as unknown, not as instantaneous.
    pub fn p50_ms(&self) -> f64 {
        if self.latencies_ms.is_empty() {
            return 0.0;
        }
        let mut v = self.latencies_ms.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = v.len() / 2;
        if v.len() % 2 == 0 {
            (v[mid - 1] + v[mid]) / 2.0
        } else {
            v[mid]
        }
    }

    pub fn p95_ms(&self) -> f64 {
        if self.latencies_ms.is_empty() {
            return 0.0;
        }
        let mut v = self.latencies_ms.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = (((v.len() as f64) * 0.95).ceil() as usize).saturating_sub(1);
        v[idx.min(v.len() - 1)]
    }

    /// Project these counters onto the benchmark shape `flux-sap-feed` consumes.
    ///
    /// The mapping, and why each one is defensible:
    /// - `dev_score` ← success rate ×100 → **contribution** (did it deliver?)
    /// - `build_p50_ms` ← measured median capture latency → **latency**
    /// - `fabrications` ← integrity failures → **accuracy** (a frame that did
    ///   not hash to its own claim is exactly the "claimed a number that didn't
    ///   match the file" violation the penalty was written for)
    /// - `passed`/`rounds` ← successes/attempts → **uptime**
    /// Latency stand-in for a source that has never successfully delivered.
    ///
    /// This exists because of a real bug caught by this crate's own test suite:
    /// SAP maps latency as `exp(-p50/100)`, and [`CaptureStats::p50_ms`] returns
    /// `0.0` when there are no samples — so a source that failed *every* attempt
    /// scored a **perfect 1.0 latency**, as if it had answered instantly. "No
    /// data" must never read as "infinitely fast". A large sentinel drives
    /// `exp(-p50/100)` to 0, which is the honest reading.
    pub const UNRESPONSIVE_P50_MS: f64 = 1.0e6;

    pub fn to_bench_result(&self, agent: impl Into<String>, stake_qug: u64) -> BenchResult {
        let p50 = if self.successes == 0 { Self::UNRESPONSIVE_P50_MS } else { self.p50_ms() };
        BenchResult {
            agent: agent.into(),
            dev_score: (self.success_rate() * 100.0).round().clamp(0.0, 100.0) as u8,
            build_p50_ms: p50,
            stake_qug,
            fabrications: self.integrity_failures,
            rounds: self.attempts,
            passed: self.successes,
        }
    }

    /// Feed these stats into a SAP [`ScoreTable`] and return the resulting total (0–1).
    pub fn feed_sap(&self, table: &mut ScoreTable, agent: &str, stake_qug: u64) -> f64 {
        flux_sap_feed::feed(table, &self.to_bench_result(agent, stake_qug))
    }

    /// Same, but hand back the full component breakdown for dashboards.
    pub fn sap_score<'a>(
        &self,
        table: &'a mut ScoreTable,
        agent: &str,
        stake_qug: u64,
    ) -> Option<&'a SAPScore> {
        let bench = self.to_bench_result(agent, stake_qug);
        flux_sap_feed::feed_full(table, &bench)
    }

    /// Convenience for reporting: the peer key this agent scores under.
    pub fn peer_id(agent: &str) -> PeerId {
        PeerId(agent.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stats_are_zero_not_perfect() {
        let s = CaptureStats::new();
        assert_eq!(s.success_rate(), 0.0, "no attempts must not read as 100% success");
        assert_eq!(s.p50_ms(), 0.0);
    }

    #[test]
    fn p50_is_the_median() {
        let mut s = CaptureStats::new();
        for ms in [10.0, 30.0, 20.0] {
            s.record_success(ms, 100);
        }
        assert_eq!(s.p50_ms(), 20.0);

        let mut even = CaptureStats::new();
        for ms in [10.0, 20.0, 30.0, 40.0] {
            even.record_success(ms, 1);
        }
        assert_eq!(even.p50_ms(), 25.0, "even counts average the middle pair");
    }

    #[test]
    fn failures_drag_the_success_rate() {
        let mut s = CaptureStats::new();
        s.record_success(5.0, 10);
        s.record_success(5.0, 10);
        s.record_failure();
        s.record_failure();
        assert_eq!(s.success_rate(), 0.5);
        let b = s.to_bench_result("grogu", 0);
        assert_eq!(b.dev_score, 50);
        assert_eq!(b.rounds, 4);
        assert_eq!(b.passed, 2);
    }

    #[test]
    fn sap_total_is_in_range_and_responds_to_quality() {
        let mut table = ScoreTable::new();

        let mut good = CaptureStats::new();
        for _ in 0..10 {
            good.record_success(3.0, 1000);
        }
        let good_total = good.feed_sap(&mut table, "good-agent", 100);

        let mut bad = CaptureStats::new();
        for _ in 0..2 {
            bad.record_success(900.0, 1000);
        }
        for _ in 0..8 {
            bad.record_failure();
        }
        bad.record_integrity_failure();
        let bad_total = bad.feed_sap(&mut table, "bad-agent", 100);

        assert!((0.0..=1.0).contains(&good_total), "SAP total must be normalised");
        assert!((0.0..=1.0).contains(&bad_total));
        assert!(
            good_total > bad_total,
            "a fast, reliable, hash-clean capturer must outscore a slow, failing one ({good_total} vs {bad_total})"
        );
    }

    #[test]
    fn a_source_that_never_delivered_does_not_score_as_instant() {
        // Regression guard for the "no samples == 0 ms == perfect latency" flaw.
        let mut dead = CaptureStats::new();
        for _ in 0..5 {
            dead.record_failure();
        }
        let bench = dead.to_bench_result("dead", 0);
        assert_eq!(bench.build_p50_ms, CaptureStats::UNRESPONSIVE_P50_MS);

        let mut table = ScoreTable::new();
        let comps = dead.sap_score(&mut table, "dead", 0).map(|s| s.components.latency);
        assert_eq!(comps, Some(0.0), "an unresponsive source must score 0 latency, not 1.0");
    }

    #[test]
    fn integrity_failures_cost_accuracy() {
        let mut table = ScoreTable::new();
        let mut clean = CaptureStats::new();
        let mut dirty = CaptureStats::new();
        for _ in 0..5 {
            clean.record_success(10.0, 500);
            dirty.record_success(10.0, 500);
        }
        dirty.record_integrity_failure();
        dirty.record_integrity_failure();

        let clean_score = clean.sap_score(&mut table, "clean", 10).map(|s| s.components.accuracy);
        let dirty_score = dirty.sap_score(&mut table, "dirty", 10).map(|s| s.components.accuracy);
        assert!(
            clean_score > dirty_score,
            "unverifiable frames must reduce the accuracy component"
        );
    }
}
