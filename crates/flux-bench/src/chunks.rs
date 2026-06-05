// Chunk strategy optimizer
//
// Given benchmark results, recommends optimal chunk size and
// parallel stream count to maximize throughput on a given link.

/// Recommend chunk size and stream count based on benchmark metrics.
///
/// Rules of thumb:
///   - For 1 Gbit: 256KB–1MB chunks, 8–16 streams
///   - For 10 Gbit: 1MB–4MB chunks, 16–64 streams
///   - Chunk too small → too much framing overhead
///   - Chunk too large → head-of-line blocking, poor parallelism
///   - BDP / 4 is the sweet spot for TCP-friendly streaming
pub fn recommend(
    throughput_mbps: f64,
    latency_p50_us: u64,
    current_chunk_bytes: u64,
    current_streams: u32,
) -> (u64, u32) {
    // Bandwidth-delay product (bytes)
    let bdp_bytes = (throughput_mbps * 1_000_000.0 / 8.0) * (latency_p50_us as f64 / 1_000_000.0);

    // Optimal chunk = BDP / 4 (TCP sawtooth sweet spot)
    let optimal_chunk = clamp_chunk((bdp_bytes / 4.0) as u64);

    // Optimal streams: saturate the pipe
    // Rule: each stream gets BDP/4 bytes in-flight
    let optimal_streams = match throughput_mbps as u64 {
        m if m < 100  => 4,
        m if m < 1000 => 8,
        m if m < 5000 => 16,
        _             => 32,
    };

    // Don't reduce below current if current is already good
    let chunk = if (current_chunk_bytes as i64 - optimal_chunk as i64).abs() < (optimal_chunk as i64 / 4) {
        current_chunk_bytes // Within 25% of optimal — keep current
    } else {
        optimal_chunk
    };

    let streams = if current_streams >= optimal_streams {
        current_streams // Already high enough
    } else {
        optimal_streams
    };

    (chunk, streams)
}

fn clamp_chunk(bytes: u64) -> u64 {
    if bytes < 65536 { 65536 }
    else if bytes > 16777216 { 16777216 }
    else {
        // Round to nearest power-of-2 boundary
        let mut size = 65536u64;
        while size * 2 <= bytes && size < 16777216 {
            size *= 2;
        }
        size
    }
}

/// Given a target file size and connection class, estimate transfer time.
pub fn estimate_transfer_time(
    total_bytes: u64,
    throughput_mbps: f64,
    chunk_bytes: u64,
    streams: u32,
    loss_rate: f64,
) -> u64 {
    let raw_secs = (total_bytes as f64 * 8.0) / (throughput_mbps * 1_000_000.0);
    // Add overhead: retransmission due to loss, framing, verification
    let loss_overhead = 1.0 + loss_rate;
    let framing_overhead = 1.0 + (64.0 / chunk_bytes as f64); // 64B framing per chunk
    let stream_efficiency = streams as f64 / (streams as f64 + 1.0); // diminishing returns
    let adjusted = raw_secs * loss_overhead * framing_overhead / stream_efficiency;
    adjusted as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recommend_gigabit() {
        let (chunk, streams) = recommend(1000.0, 500, 262144, 8);
        assert!(chunk >= 65536);
        assert!(streams >= 8);
    }

    #[test]
    fn test_recommend_keeps_good_current() {
        // If current params are close to optimal, keep them
        let bdp = (1000.0 * 1_000_000.0 / 8.0) * (500.0 / 1_000_000.0);
        let optimal = clamp_chunk((bdp / 4.0) as u64);
        // optimal is min(65536) for this low BDP
        let (chunk, streams) = recommend(1000.0, 500, optimal, 16);
        assert_eq!(chunk, optimal, "Should keep current when within 25%");
        assert_eq!(streams, 16, "Already above optimal");
    }

    #[test]
    fn test_estimate_transfer() {
        let eta = estimate_transfer_time(
            1_000_000_000, // 1 GB
            1000.0,        // 1 Gbps
            1_048_576,     // 1 MB chunks
            16,            // 16 streams
            0.001,         // 0.1% loss
        );
        // ~8 seconds raw + overhead ~ 9-10 seconds
        assert!(eta > 7 && eta < 15, "ETA should be ~10s, got {}s", eta);
    }
}
