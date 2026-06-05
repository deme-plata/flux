// Real-time progress reporting
//
// Provides a callback-based progress stream for long-running benchmarks.
// The progress reporter is cloneable and Send-safe, suitable for:
//   - Dashboard SSE streaming
//   - MCP tool progress updates
//   - Console progress bars

use crate::{BenchPhase, BenchProgress};
use std::sync::Arc;
use parking_lot::Mutex;
use std::time::Instant;

/// Progress reporter — thread-safe, cloneable.
///
/// The benchmark engine calls `update()` periodically.
/// Consumers call `snapshot()` to get the latest progress.
#[derive(Clone)]
pub struct ProgressReporter {
    inner: Arc<Mutex<ProgressInner>>,
}

struct ProgressInner {
    progress: BenchProgress,
    start: Instant,
    updates: u64,
}

impl ProgressReporter {
    /// Create a new progress reporter for a benchmark run.
    pub fn new(run_id: &str, total_bytes: u64, total_chunks: u64) -> Self {
        ProgressReporter {
            inner: Arc::new(Mutex::new(ProgressInner {
                progress: BenchProgress {
                    run_id: run_id.to_string(),
                    bytes_sent: 0,
                    bytes_verified: 0,
                    total_bytes,
                    current_mbps: 0.0,
                    peak_mbps: 0.0,
                    avg_mbps: 0.0,
                    chunks_completed: 0,
                    chunks_total: total_chunks,
                    chunks_retried: 0,
                    eta_secs: 0,
                    elapsed_secs: 0.0,
                    phase: BenchPhase::Connecting,
                },
                start: Instant::now(),
                updates: 0,
            })),
        }
    }

    /// Update progress with new data from the benchmark engine.
    pub fn update(&self,
        bytes_sent: u64,
        bytes_verified: u64,
        chunks_done: u64,
        chunks_retried: u64,
        phase: BenchPhase,
        current_mbps: f64,
    ) {
        let mut inner = self.inner.lock();
        let elapsed = inner.start.elapsed().as_secs_f64();
        inner.updates += 1;

        let avg_mbps = if elapsed > 0.0 {
            (bytes_sent as f64 * 8.0) / (elapsed * 1_000_000.0)
        } else {
            0.0
        };

        let peak = if current_mbps > inner.progress.peak_mbps {
            current_mbps
        } else {
            inner.progress.peak_mbps
        };

        let remaining = inner.progress.total_bytes.saturating_sub(bytes_sent);
        let eta = if current_mbps > 0.0 && remaining > 0 {
            ((remaining as f64 * 8.0) / (current_mbps * 1_000_000.0)) as u64
        } else {
            0
        };

        inner.progress.bytes_sent = bytes_sent;
        inner.progress.bytes_verified = bytes_verified;
        inner.progress.current_mbps = current_mbps;
        inner.progress.peak_mbps = peak;
        inner.progress.avg_mbps = avg_mbps;
        inner.progress.chunks_completed = chunks_done;
        inner.progress.chunks_retried = chunks_retried;
        inner.progress.eta_secs = eta;
        inner.progress.elapsed_secs = elapsed;
        inner.progress.phase = phase;
    }

    /// Get the current progress snapshot.
    pub fn snapshot(&self) -> BenchProgress {
        self.inner.lock().progress.clone()
    }

    /// Get the percentage complete (0.0–100.0).
    pub fn percent(&self) -> f64 {
        let inner = self.inner.lock();
        if inner.progress.total_bytes == 0 { return 100.0; }
        (inner.progress.bytes_sent as f64 / inner.progress.total_bytes as f64) * 100.0
    }

    /// Mark the benchmark as complete.
    pub fn complete(&self) {
        let mut inner = self.inner.lock();
        inner.progress.phase = BenchPhase::Complete;
        inner.progress.bytes_sent = inner.progress.total_bytes;
        inner.progress.bytes_verified = inner.progress.total_bytes;
    }

    /// Mark the benchmark as failed.
    pub fn fail(&self, reason: &str) {
        let mut inner = self.inner.lock();
        inner.progress.phase = BenchPhase::Failed(reason.to_string());
    }

    /// Number of progress updates so far.
    pub fn update_count(&self) -> u64 {
        self.inner.lock().updates
    }
}

/// Format progress as a human-readable status line.
pub fn format_progress(p: &BenchProgress) -> String {
    let pct = if p.total_bytes > 0 {
        (p.bytes_sent as f64 / p.total_bytes as f64) * 100.0
    } else {
        100.0
    };

    match &p.phase {
        BenchPhase::Connecting => format!("🔗 Connecting... ({:.1}s)", p.elapsed_secs),
        BenchPhase::Handshake => format!("🤝 Handshake... ({:.1}s)", p.elapsed_secs),
        BenchPhase::Transferring => format!(
            "📡 {:.1}% | {:.1} MB/s | {:.0}s ETA | {}/{} chunks",
            pct,
            p.current_mbps / 8.0,
            p.eta_secs,
            p.chunks_completed,
            p.chunks_total,
        ),
        BenchPhase::Verifying => format!(
            "🔍 Verifying... {:.1}% ({}/{} verified)",
            if p.bytes_sent > 0 { (p.bytes_verified as f64 / p.bytes_sent as f64) * 100.0 } else { 0.0 },
            p.bytes_verified,
            p.bytes_sent,
        ),
        BenchPhase::Complete => format!(
            "✅ Complete | {:.1} MB/s avg ({:.1} peak) | {} chunks | {:.1}s",
            p.avg_mbps / 8.0,
            p.peak_mbps / 8.0,
            p.chunks_total,
            p.elapsed_secs,
        ),
        BenchPhase::Failed(e) => format!("❌ Failed: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_basic() {
        let reporter = ProgressReporter::new("test-1", 100_000_000, 100);
        assert_eq!(reporter.snapshot().phase, BenchPhase::Connecting);

        reporter.update(50_000_000, 50_000_000, 50, 0, BenchPhase::Transferring, 500.0);
        let snap = reporter.snapshot();
        assert_eq!(snap.bytes_sent, 50_000_000);
        assert_eq!(snap.chunks_completed, 50);
        assert!((snap.current_mbps - 500.0).abs() < 0.1);

        reporter.complete();
        assert_eq!(reporter.snapshot().phase, BenchPhase::Complete);
        assert!(reporter.percent() > 99.0);
    }

    #[test]
    fn test_format_transferring() {
        let mut p = BenchProgress {
            run_id: "test".into(),
            bytes_sent: 50_000_000,
            bytes_verified: 50_000_000,
            total_bytes: 100_000_000,
            current_mbps: 800.0,
            peak_mbps: 950.0,
            avg_mbps: 750.0,
            chunks_completed: 50,
            chunks_total: 100,
            chunks_retried: 1,
            eta_secs: 60,
            elapsed_secs: 62.5,
            phase: BenchPhase::Transferring,
        };
        let formatted = format_progress(&p);
        assert!(formatted.contains("50.0%"));
        assert!(formatted.contains("100.0 MB/s")); // 800 Mbps / 8
        assert!(formatted.contains("60s ETA"));

        p.phase = BenchPhase::Complete;
        let done = format_progress(&p);
        assert!(done.starts_with("✅"));
    }
}
