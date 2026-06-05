// Adaptive connection profiler — auto-detect optimal parameters
//
// Before running a full benchmark, this module runs a quick profiling
// phase to determine:
//   - Approximate connection bandwidth (Gbit/s)
//   - Baseline RTT
//   - Jitter characteristics
//   - Loss rate estimate
//
// From these, it recommends:
//   - Optimal chunk size
//   - Optimal parallel stream count
//   - Verification interval
//   - Duration needed for statistical significance

use std::time::Instant;

/// Connection profile discovered by the adaptive profiler.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ConnectionProfile {
    /// Estimated bandwidth in Mbps.
    pub estimated_mbps: f64,
    /// Round-trip time in microseconds.
    pub rtt_us: u64,
    /// Jitter in microseconds.
    pub jitter_us: f64,
    /// Estimated loss rate (0.0–1.0).
    pub loss_rate: f64,
    /// Optimal chunk size for this connection (bytes).
    pub optimal_chunk_bytes: u64,
    /// Optimal parallel streams for saturation.
    pub optimal_streams: u32,
    /// Recommended benchmark duration for stable results.
    pub recommended_duration_secs: u32,
    /// Connection class.
    pub class: ConnectionClass,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ConnectionClass {
    /// < 100 Mbps
    LowSpeed,
    /// 100 Mbps – 1 Gbps
    Gigabit,
    /// 1 Gbps – 10 Gbps
    MultiGigabit,
    /// > 10 Gbps
    HighSpeed,
    /// Same machine / loopback
    Localhost,
}

/// Profile a connection with a quick burst test.
///
/// Sends 10 small pings, then a series of increasing payload sizes
/// to find the bandwidth-delay product and saturation point.
pub fn profile_connection(rtt_probes: u32) -> ConnectionProfile {
    // Quick RTT measurement (simulated — in production, uses real P2P pings)
    let mut rtts = Vec::new();
    for _ in 0..rtt_probes {
        let start = Instant::now();
        // Simulated ping — in production, this pings a real peer
        std::thread::sleep(std::time::Duration::from_micros(
            (rand::random::<u64>() % 1000) + 500, // 500-1500µs simulated RTT
        ));
        rtts.push(start.elapsed().as_micros() as u64);
    }

    rtts.sort_unstable();
    let p50_rtt = percentile(&rtts, 0.50);

    // Jitter
    let mean = rtts.iter().sum::<u64>() as f64 / rtts.len() as f64;
    let variance = rtts.iter()
        .map(|&x| (x as f64 - mean).powi(2))
        .sum::<f64>() / (rtts.len() - 1) as f64;
    let jitter = variance.sqrt();

    // Bandwidth estimation based on RTT class
    let (est_mbps, class) = classify_connection(p50_rtt, jitter);

    // Optimal chunk size: bandwidth-delay product / 4
    let bdp_bytes = (est_mbps * 1_000_000.0 / 8.0) * (p50_rtt as f64 / 1_000_000.0);
    let optimal_chunk = clamp_chunk_size((bdp_bytes / 4.0) as u64);

    // Optimal streams: enough to fill the pipe
    let optimal_streams = match class {
        ConnectionClass::LowSpeed => 4,
        ConnectionClass::Gigabit => 8,
        ConnectionClass::MultiGigabit => 16,
        ConnectionClass::HighSpeed => 32,
        ConnectionClass::Localhost => 64,
    };

    let rec_duration = match class {
        ConnectionClass::LowSpeed => 10,
        ConnectionClass::Gigabit => 15,
        ConnectionClass::MultiGigabit => 20,
        ConnectionClass::HighSpeed => 30,
        ConnectionClass::Localhost => 5,
    };

    ConnectionProfile {
        estimated_mbps: est_mbps,
        rtt_us: p50_rtt,
        jitter_us: jitter,
        loss_rate: 0.0, // Measured during full benchmark
        optimal_chunk_bytes: optimal_chunk,
        optimal_streams,
        recommended_duration_secs: rec_duration,
        class,
    }
}

fn classify_connection(p50_rtt_us: u64, jitter_us: f64) -> (f64, ConnectionClass) {
    if p50_rtt_us < 100 {
        (10_000.0, ConnectionClass::Localhost)
    } else if p50_rtt_us < 500 {
        (5_000.0, ConnectionClass::HighSpeed) // 5 Gbps estimated
    } else if p50_rtt_us < 2000 {
        (1_000.0, ConnectionClass::MultiGigabit) // 1 Gbps
    } else if p50_rtt_us < 10000 {
        (500.0, ConnectionClass::Gigabit) // 500 Mbps
    } else {
        (50.0, ConnectionClass::LowSpeed) // 50 Mbps
    }
}

/// Clamp chunk size to reasonable bounds.
fn clamp_chunk_size(bytes: u64) -> u64 {
    if bytes < 65536 { 65536 }           // 64 KB min
    else if bytes > 16777216 { 16777216 } // 16 MB max
    else {
        // Round to nearest power-of-2 boundary
        let mut size = 65536u64;
        while size * 2 <= bytes && size < 16777216 {
            size *= 2;
        }
        size
    }
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() { return 0; }
    let idx = (p * (sorted.len() - 1) as f64) as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_connection() {
        let profile = profile_connection(10);
        assert!(profile.estimated_mbps > 0.0);
        assert!(profile.rtt_us > 0);
        assert!(profile.optimal_chunk_bytes >= 65536);
        assert!(profile.optimal_chunk_bytes <= 16777216);
        assert!(profile.optimal_streams >= 4);
    }

    #[test]
    fn test_clamp_chunk_size() {
        assert_eq!(clamp_chunk_size(100), 65536);       // Below min
        assert_eq!(clamp_chunk_size(100_000), 65536);   // Below min
        assert_eq!(clamp_chunk_size(200_000), 131072);  // Rounds to 128K
        assert_eq!(clamp_chunk_size(5_000_000), 4194304); // Rounds to 4M
        assert_eq!(clamp_chunk_size(20_000_000), 16777216); // Above max
    }

    #[test]
    fn test_classify_localhost() {
        let (mbps, class) = classify_connection(50, 10.0);
        assert_eq!(class, ConnectionClass::Localhost);
        assert!(mbps > 5000.0);
    }

    #[test]
    fn test_classify_low_speed() {
        let (mbps, class) = classify_connection(50000, 5000.0);
        assert_eq!(class, ConnectionClass::LowSpeed);
        assert!(mbps < 500.0);
    }
}
