// flux-bench — P2P Benchmark Suite
//
// Measures real P2P network performance with adaptive profiling,
// real-time progress, and historical comparison.
//
// Modules:
//   engine    — Core benchmark engine (throughput, latency, jitter)
//   adaptive  — Auto-detect connection parameters
//   progress  — Real-time progress streaming
//   history   — SQLite historical tracking + comparison
//   chunks    — Chunk strategy optimizer
//   resilience — Kill/revive, mesh reformation testing

pub mod engine;
pub mod adaptive;
pub mod progress;
pub mod history;
pub mod chunks;
pub mod resilience;

use serde::{Deserialize, Serialize};

/// A benchmark run identifier — unique per test.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchId {
    pub run_id: String,      // UUID
    pub started_at_ms: u64,
    pub node_id: String,
    pub peer_count: u32,
}

/// Benchmark configuration for a P2P throughput/latency test.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchConfig {
    /// Total data to transfer (bytes). Range: 1MB → 1TB.
    pub total_bytes: u64,
    /// Chunk size for streaming (bytes). Range: 64KB → 16MB.
    pub chunk_bytes: u64,
    /// Number of parallel streams.
    pub parallel_streams: u32,
    /// Duration limit (seconds). 0 = no limit.
    pub duration_secs: u32,
    /// Verify every N chunks via BLAKE3 hash.
    pub verify_every_n: u32,
    /// Target peer: "all", a specific peer_id, or "self" for loopback.
    pub target_peer: String,
    /// Data pattern: "random", "zeroes", "structured", "compressed".
    pub data_pattern: DataPattern,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DataPattern {
    Random,
    Zeroes,
    Structured,   // JSON-like patterns
    Compressed,   // Pre-compressed data (tests dedup)
}

impl Default for BenchConfig {
    fn default() -> Self {
        BenchConfig {
            total_bytes: 100 * 1024 * 1024, // 100 MB
            chunk_bytes: 1024 * 1024,       // 1 MB
            parallel_streams: 8,
            duration_secs: 30,
            verify_every_n: 10,
            target_peer: "all".into(),
            data_pattern: DataPattern::Random,
        }
    }
}

/// Live progress during a running benchmark.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchProgress {
    pub run_id: String,
    pub bytes_sent: u64,
    pub bytes_verified: u64,
    pub total_bytes: u64,
    pub current_mbps: f64,
    pub peak_mbps: f64,
    pub avg_mbps: f64,
    pub chunks_completed: u64,
    pub chunks_total: u64,
    pub chunks_retried: u64,
    pub eta_secs: u64,
    pub elapsed_secs: f64,
    pub phase: BenchPhase,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BenchPhase {
    Connecting,
    Handshake,
    Transferring,
    Verifying,
    Complete,
    Failed(String),
}

/// Final benchmark result — written to history DB.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchResult {
    pub id: BenchId,
    pub config: BenchConfig,
    /// Throughput in Mbps.
    pub throughput_mbps: f64,
    /// Peak throughput (best 1-second window).
    pub peak_mbps: f64,
    /// p50/p95/p99 latency in microseconds.
    pub latency_p50_us: u64,
    pub latency_p95_us: u64,
    pub latency_p99_us: u64,
    /// Jitter (stddev of inter-packet arrival).
    pub jitter_us: f64,
    /// Total duration in seconds.
    pub duration_secs: f64,
    /// Chunk retry count.
    pub retries: u64,
    /// Chunk loss rate (0.0–1.0).
    pub loss_rate: f64,
    /// Verification: all chunks passed?
    pub verified: bool,
    /// Recommended chunk size for this connection.
    pub recommended_chunk_bytes: u64,
    /// Recommended parallel streams.
    pub recommended_streams: u32,
    /// Connection quality score (0–100).
    pub quality_score: u8,
    /// Timestamp.
    pub completed_at_ms: u64,
}

/// Summary for MCP / dashboard display.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchSummary {
    pub total_runs: u64,
    pub best_mbps: f64,
    pub avg_mbps: f64,
    pub best_latency_p50_us: u64,
    pub avg_latency_p50_us: u64,
    pub total_bytes_benchmarked: u64,
    pub recommended_chunk_bytes: u64,
    pub recommended_streams: u32,
    pub quality_trend: String,   // "improving", "stable", "degrading"
}
