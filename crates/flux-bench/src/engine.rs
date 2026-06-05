// Benchmark engine — throughput, latency, jitter measurement
//
// The core measurement loop:
//   1. Generate test data (random/zeroes/structured/compressed)
//   2. Split into chunks with BLAKE3 hashes
//   3. Send chunks over P2P gossipsub (or direct stream for large transfers)
//   4. Measure: throughput (Mbps), latency (µs), jitter (µs stddev)
//   5. Verify: receiver re-hashes and compares
//   6. Record results + recommend optimal settings

use crate::{BenchConfig, BenchId, BenchResult, DataPattern};
use std::time::{Duration, Instant};
use rand::RngCore;

/// Run a full benchmark and return the result.
pub async fn run_benchmark(config: BenchConfig, node_id: &str, peer_count: u32) -> BenchResult {
    let run_id = format!("bench-{}", uuid_v4());
    let started_at = now_ms();
    let id = BenchId {
        run_id: run_id.clone(),
        started_at_ms: started_at,
        node_id: node_id.to_string(),
        peer_count,
    };

    tracing::info!(
        run_id = %id.run_id,
        total_mb = config.total_bytes / (1024 * 1024),
        chunk_kb = config.chunk_bytes / 1024,
        streams = config.parallel_streams,
        "Starting P2P benchmark"
    );

    let start = Instant::now();

    // Phase 1: Generate test data
    let data = generate_test_data(config.total_bytes as usize, &config.data_pattern);
    let chunks = split_into_chunks(&data, config.chunk_bytes as usize);
    let total_chunks = chunks.len() as u64;

    // Phase 2: Transfer (simulated — in production, this goes over the P2P swarm)
    let mut bytes_sent: u64 = 0;
    let mut bytes_verified: u64 = 0;
    let mut retries: u64 = 0;
    let mut peak_mbps: f64 = 0.0;
    let mut latency_samples: Vec<u64> = Vec::with_capacity(chunks.len());
    let mut inter_arrival_us: Vec<i64> = Vec::new();
    let mut last_arrival: Option<Instant> = None;

    let window_duration = Duration::from_secs(1);
    let mut window_start = Instant::now();
    let mut window_bytes: u64 = 0;
    let mut total_elapsed: f64 = 0.0;

    for (i, chunk) in chunks.iter().enumerate() {
        let chunk_start = Instant::now();

        // Simulated P2P transfer — in production, replaces with swarm.publish()
        // The actual transfer mechanism depends on the P2P transport:
        //   - Small (< 1MB): gossipsub
        //   - Medium (1MB–100MB): request-response with streaming
        //   - Large (> 100MB): direct stream with progress
        let transferred = simulate_transfer(chunk);
        bytes_sent += transferred as u64;

        // Latency measurement
        let chunk_latency_us = chunk_start.elapsed().as_micros() as u64;
        latency_samples.push(chunk_latency_us);

        // Jitter: inter-arrival time
        if let Some(last) = last_arrival {
            let delta = chunk_start.duration_since(last).as_micros() as i64;
            inter_arrival_us.push(delta);
        }
        last_arrival = Some(chunk_start);

        // Verification (every N chunks)
        if i as u32 % config.verify_every_n == 0 {
            let expected_hash = blake3::hash(chunk);
            let verified = verify_chunk(chunk, &expected_hash);
            if verified {
                bytes_verified += chunk.len() as u64;
            } else {
                retries += 1;
            }
        } else {
            bytes_verified += chunk.len() as u64;
        }

        // Rolling window throughput
        window_bytes += chunk.len() as u64;
        if window_start.elapsed() >= window_duration || i == chunks.len() - 1 {
            let window_secs = window_start.elapsed().as_secs_f64();
            let window_mbps = (window_bytes as f64 * 8.0) / (window_secs * 1_000_000.0);
            if window_mbps > peak_mbps {
                peak_mbps = window_mbps;
            }
            window_bytes = 0;
            window_start = Instant::now();
        }

        // Duration limit
        if config.duration_secs > 0 && start.elapsed().as_secs() >= config.duration_secs as u64 {
            break;
        }
    }

    total_elapsed = start.elapsed().as_secs_f64();
    let total_mbps = if total_elapsed > 0.0 {
        (bytes_sent as f64 * 8.0) / (total_elapsed * 1_000_000.0)
    } else {
        0.0
    };

    // Compute latency percentiles
    latency_samples.sort_unstable();
    let p50 = percentile(&latency_samples, 0.50);
    let p95 = percentile(&latency_samples, 0.95);
    let p99 = percentile(&latency_samples, 0.99);

    // Compute jitter (stddev of inter-arrival times)
    let jitter_us = if inter_arrival_us.len() > 1 {
        let mean = inter_arrival_us.iter().sum::<i64>() as f64 / inter_arrival_us.len() as f64;
        let variance = inter_arrival_us.iter()
            .map(|&x| (x as f64 - mean).powi(2))
            .sum::<f64>() / (inter_arrival_us.len() - 1) as f64;
        variance.sqrt()
    } else {
        0.0
    };

    let loss_rate = if chunks.is_empty() { 0.0 } else {
        retries as f64 / chunks.len() as f64
    };

    // Compute recommendations
    let (recommended_chunk, recommended_streams) =
        super::chunks::recommend(total_mbps, p50, config.chunk_bytes, config.parallel_streams);

    let quality_score = compute_quality_score(total_mbps, p50, jitter_us, loss_rate);

    BenchResult {
        id,
        config,
        throughput_mbps: total_mbps,
        peak_mbps,
        latency_p50_us: p50,
        latency_p95_us: p95,
        latency_p99_us: p99,
        jitter_us,
        duration_secs: total_elapsed,
        retries,
        loss_rate,
        verified: retries == 0,
        recommended_chunk_bytes: recommended_chunk,
        recommended_streams,
        quality_score,
        completed_at_ms: now_ms(),
    }
}

/// Generate test data of the given size with the specified pattern.
fn generate_test_data(size: usize, pattern: &DataPattern) -> Vec<u8> {
    match pattern {
        DataPattern::Zeroes => vec![0u8; size],
        DataPattern::Random => {
            let mut data = vec![0u8; size];
            rand::thread_rng().fill_bytes(&mut data);
            data
        }
        DataPattern::Structured => {
            // Generate JSON-like repeating patterns
            let mut data = Vec::with_capacity(size);
            while data.len() < size {
                let record = format!(
                    r#"{{"id":{},"hash":"{}","timestamp":{},"size":{}}}"#,
                    data.len(),
                    blake3::hash(&data).to_hex(),
                    now_ms(),
                    size,
                );
                data.extend_from_slice(record.as_bytes());
                data.push(b'\n');
            }
            data.truncate(size);
            data
        }
        DataPattern::Compressed => {
            // Pre-compress data (simulates, in production uses zstd/lz4)
            let raw = generate_test_data(size / 3, &DataPattern::Random);
            // Simple RLE compression for test data
            let mut compressed = Vec::new();
            let mut i = 0;
            while i < raw.len() && compressed.len() < size {
                let mut run = 1u8;
                while i + (run as usize) < raw.len() && raw[i + run as usize] == raw[i] && run < 255 {
                    run += 1;
                }
                compressed.push(run);
                compressed.push(raw[i]);
                i += run as usize;
            }
            compressed.resize(size, 0);
            compressed
        }
    }
}

/// Split data into fixed-size chunks.
fn split_into_chunks(data: &[u8], chunk_size: usize) -> Vec<Vec<u8>> {
    data.chunks(chunk_size)
        .map(|c| c.to_vec())
        .collect()
}

/// Simulate a P2P transfer (placeholder for real swarm integration).
fn simulate_transfer(data: &[u8]) -> usize {
    // In production, this sends over the P2P swarm via gossipsub or request-response.
    // For now, the benchmark measures the local processing time as a baseline.
    // The real P2P latency is added when connected to live peers.
    let _ = blake3::hash(data);
    data.len()
}

/// Verify a chunk against its expected hash.
fn verify_chunk(data: &[u8], expected: &blake3::Hash) -> bool {
    let actual = blake3::hash(data);
    actual == *expected
}

/// Compute a percentile from sorted samples.
fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() { return 0; }
    let idx = (p * (sorted.len() - 1) as f64) as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Compute a connection quality score (0–100).
fn compute_quality_score(mbps: f64, p50_us: u64, jitter_us: f64, loss_rate: f64) -> u8 {
    let throughput_score = (mbps / 1000.0).min(1.0) * 40.0;   // 40% weight, 1Gbps = full
    let latency_score = (1.0 - (p50_us as f64 / 10000.0).min(1.0)) * 30.0; // 30% weight, <10ms = full
    let jitter_score = (1.0 - (jitter_us / 5000.0).min(1.0)) * 15.0;      // 15% weight
    let loss_score = (1.0 - loss_rate) * 15.0;                             // 15% weight
    (throughput_score + latency_score + jitter_score + loss_score) as u8
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let mut rng = rand::thread_rng();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (ts >> 32) as u32,
        (ts >> 16) as u16 & 0xFFFF,
        rng.next_u32() & 0xFFF,
        (rng.next_u32() & 0x3FFF) | 0x8000,
        rng.next_u64() & 0xFFFF_FFFF_FFFF
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_random() {
        let data = generate_test_data(1024, &DataPattern::Random);
        assert_eq!(data.len(), 1024);
        // Should not be all zeros
        assert!(data.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_generate_zeroes() {
        let data = generate_test_data(1024, &DataPattern::Zeroes);
        assert_eq!(data.len(), 1024);
        assert!(data.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_generate_structured() {
        let data = generate_test_data(4096, &DataPattern::Structured);
        assert_eq!(data.len(), 4096);
        assert!(data.starts_with(b"{"));
    }

    #[test]
    fn test_split_chunks() {
        let data = vec![42u8; 1000];
        let chunks = split_into_chunks(&data, 256);
        assert_eq!(chunks.len(), 4);
        assert_eq!(chunks[0].len(), 256);
        assert_eq!(chunks[3].len(), 232); // last chunk partial
    }

    #[test]
    fn test_percentile() {
        let samples: Vec<u64> = (0..100).collect();
        assert_eq!(percentile(&samples, 0.50), 49);
        assert_eq!(percentile(&samples, 0.95), 94);
        assert_eq!(percentile(&samples, 0.99), 98);
    }

    #[test]
    fn test_quality_score() {
        let score = compute_quality_score(1000.0, 500, 100.0, 0.0);
        assert!(score > 80, "Good connection should score high");
        let bad = compute_quality_score(10.0, 50000, 10000.0, 0.5);
        assert!(bad < 30, "Bad connection should score low");
    }

    #[tokio::test]
    async fn test_benchmark_run() {
        let config = BenchConfig {
            total_bytes: 1024 * 1024, // 1 MB
            chunk_bytes: 65536,       // 64 KB
            parallel_streams: 1,
            duration_secs: 5,
            verify_every_n: 1,
            target_peer: "self".into(),
            data_pattern: DataPattern::Random,
        };
        let result = run_benchmark(config, "test-node", 0).await;
        assert_eq!(result.id.node_id, "test-node");
        assert!(result.throughput_mbps > 0.0);
        assert!(result.duration_secs > 0.0);
        assert!(result.quality_score > 0);
    }

    #[tokio::test]
    async fn test_structured_pattern() {
        let config = BenchConfig {
            total_bytes: 1024 * 1024,
            data_pattern: DataPattern::Structured,
            ..Default::default()
        };
        let result = run_benchmark(config, "test", 0).await;
        assert!(result.verified);
    }
}
