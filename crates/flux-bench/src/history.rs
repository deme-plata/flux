// Historical benchmark database — SQLite-backed
//
// Stores every benchmark run for trend analysis and comparison.
// Schema:
//   CREATE TABLE benchmarks (
//     run_id TEXT PRIMARY KEY,
//     node_id TEXT,
//     started_at_ms INTEGER,
//     throughput_mbps REAL,
//     peak_mbps REAL,
//     latency_p50_us INTEGER,
//     latency_p95_us INTEGER,
//     latency_p99_us INTEGER,
//     jitter_us REAL,
//     duration_secs REAL,
//     total_bytes INTEGER,
//     chunk_bytes INTEGER,
//     streams INTEGER,
//     retries INTEGER,
//     loss_rate REAL,
//     verified INTEGER,
//     quality_score INTEGER,
//     data_pattern TEXT,
//     completed_at_ms INTEGER
//   );

use crate::{BenchResult, BenchSummary};
use rusqlite::{Connection, params};
use std::path::Path;

/// Open (or create) the benchmark history database.
pub fn open_db(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path)
        .map_err(|e| format!("Failed to open bench DB: {}", e))?;

    conn.execute_batch("
        CREATE TABLE IF NOT EXISTS benchmarks (
            run_id TEXT PRIMARY KEY,
            node_id TEXT NOT NULL,
            started_at_ms INTEGER NOT NULL,
            throughput_mbps REAL NOT NULL,
            peak_mbps REAL NOT NULL,
            latency_p50_us INTEGER NOT NULL,
            latency_p95_us INTEGER NOT NULL,
            latency_p99_us INTEGER NOT NULL,
            jitter_us REAL NOT NULL,
            duration_secs REAL NOT NULL,
            total_bytes INTEGER NOT NULL,
            chunk_bytes INTEGER NOT NULL,
            streams INTEGER NOT NULL,
            retries INTEGER NOT NULL,
            loss_rate REAL NOT NULL,
            verified INTEGER NOT NULL,
            quality_score INTEGER NOT NULL,
            data_pattern TEXT NOT NULL,
            completed_at_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_bench_node ON benchmarks(node_id);
        CREATE INDEX IF NOT EXISTS idx_bench_time ON benchmarks(completed_at_ms);
    ").map_err(|e| format!("Failed to create tables: {}", e))?;

    Ok(conn)
}

/// Store a benchmark result in the database.
pub fn store_result(conn: &Connection, result: &BenchResult) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO benchmarks
         (run_id, node_id, started_at_ms, throughput_mbps, peak_mbps,
          latency_p50_us, latency_p95_us, latency_p99_us, jitter_us,
          duration_secs, total_bytes, chunk_bytes, streams, retries,
          loss_rate, verified, quality_score, data_pattern, completed_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
        params![
            result.id.run_id,
            result.id.node_id,
            result.id.started_at_ms,
            result.throughput_mbps,
            result.peak_mbps,
            result.latency_p50_us,
            result.latency_p95_us,
            result.latency_p99_us,
            result.jitter_us,
            result.duration_secs,
            result.config.total_bytes,
            result.config.chunk_bytes,
            result.config.parallel_streams,
            result.retries,
            result.loss_rate,
            result.verified as i32,
            result.quality_score,
            format!("{:?}", result.config.data_pattern),
            result.completed_at_ms,
        ],
    ).map_err(|e| format!("Failed to store result: {}", e))?;

    Ok(())
}

/// Retrieve the last N benchmark runs for a node.
pub fn recent_runs(conn: &Connection, node_id: &str, limit: u32) -> Result<Vec<BenchResult>, String> {
    let mut stmt = conn.prepare(
        "SELECT run_id, node_id, started_at_ms, throughput_mbps, peak_mbps,
                latency_p50_us, latency_p95_us, latency_p99_us, jitter_us,
                duration_secs, total_bytes, chunk_bytes, streams, retries,
                loss_rate, verified, quality_score, data_pattern, completed_at_ms
         FROM benchmarks WHERE node_id = ?1
         ORDER BY completed_at_ms DESC LIMIT ?2"
    ).map_err(|e| format!("Query failed: {}", e))?;

    let results = stmt.query_map(params![node_id, limit], |row| {
        let run_id: String = row.get(0)?;
        let node_id: String = row.get(1)?;
        let started_at_ms: u64 = row.get::<_, i64>(2)? as u64;
        let throughput_mbps: f64 = row.get(3)?;
        let peak_mbps: f64 = row.get(4)?;
        let latency_p50_us: u64 = row.get::<_, i64>(5)? as u64;
        let latency_p95_us: u64 = row.get::<_, i64>(6)? as u64;
        let latency_p99_us: u64 = row.get::<_, i64>(7)? as u64;
        let jitter_us: f64 = row.get(8)?;
        let duration_secs: f64 = row.get(9)?;
        let total_bytes: u64 = row.get::<_, i64>(10)? as u64;
        let chunk_bytes: u64 = row.get::<_, i64>(11)? as u64;
        let streams: u32 = row.get::<_, i64>(12)? as u32;
        let retries: u64 = row.get::<_, i64>(13)? as u64;
        let loss_rate: f64 = row.get(14)?;
        let verified: bool = row.get::<_, i32>(15)? != 0;
        let quality_score: u8 = row.get::<_, i64>(16)? as u8;
        let data_pattern_str: String = row.get(17)?;
        let completed_at_ms: u64 = row.get::<_, i64>(18)? as u64;

        let data_pattern = match data_pattern_str.as_str() {
            "Zeroes" => crate::DataPattern::Zeroes,
            "Structured" => crate::DataPattern::Structured,
            "Compressed" => crate::DataPattern::Compressed,
            _ => crate::DataPattern::Random,
        };

        Ok(BenchResult {
            id: crate::BenchId { run_id, node_id, started_at_ms, peer_count: 0 },
            config: crate::BenchConfig {
                total_bytes, chunk_bytes,
                parallel_streams: streams,
                data_pattern,
                ..Default::default()
            },
            throughput_mbps, peak_mbps,
            latency_p50_us, latency_p95_us, latency_p99_us,
            jitter_us, duration_secs, retries, loss_rate, verified,
            recommended_chunk_bytes: chunk_bytes,
            recommended_streams: streams,
            quality_score,
            completed_at_ms,
        })
    }).map_err(|e| format!("Result mapping failed: {}", e))?;

    let mut vec = Vec::new();
    for r in results {
        vec.push(r.map_err(|e| format!("Row error: {}", e))?);
    }
    Ok(vec)
}

/// Compute a summary across all benchmark runs for a node.
pub fn summary(conn: &Connection, node_id: &str) -> Result<BenchSummary, String> {
    let mut stmt = conn.prepare(
        "SELECT COUNT(*),
                MAX(throughput_mbps), AVG(throughput_mbps),
                MIN(latency_p50_us), AVG(latency_p50_us),
                SUM(total_bytes),
                AVG(chunk_bytes), AVG(streams),
                AVG(quality_score)
         FROM benchmarks WHERE node_id = ?1"
    ).map_err(|e| format!("Summary query failed: {}", e))?;

    let summary = stmt.query_row(params![node_id], |row| {
        let total_runs: u64 = row.get::<_, i64>(0)? as u64;
        let best_mbps: f64 = row.get(1)?;
        let avg_mbps: f64 = row.get(2)?;
        let best_lat: u64 = row.get::<_, f64>(3)? as u64;
        let avg_lat: u64 = row.get::<_, f64>(4)? as u64;
        let total_bytes: u64 = row.get::<_, i64>(5)? as u64;
        let rec_chunk: u64 = row.get::<_, f64>(6)? as u64;
        let rec_streams: u32 = row.get::<_, f64>(7)? as u32;
        let avg_quality: f64 = row.get(8)?;

        // Determine trend by comparing last 2 runs
        let trend = if total_runs < 2 {
            "stable".to_string()
        } else {
            // Get last 2 quality scores
            let mut stmt2 = conn.prepare(
                "SELECT quality_score FROM benchmarks WHERE node_id = ?1
                 ORDER BY completed_at_ms DESC LIMIT 2"
            ).unwrap();
            let scores: Vec<i64> = stmt2.query_map(params![node_id], |r| r.get(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect();
            if scores.len() == 2 {
                if scores[0] > scores[1] { "improving".into() }
                else if scores[0] < scores[1] { "degrading".into() }
                else { "stable".into() }
            } else {
                "stable".into()
            }
        };

        Ok(BenchSummary {
            total_runs,
            best_mbps,
            avg_mbps,
            best_latency_p50_us: best_lat,
            avg_latency_p50_us: avg_lat,
            total_bytes_benchmarked: total_bytes,
            recommended_chunk_bytes: rec_chunk.max(65536).min(16777216),
            recommended_streams: rec_streams.max(4).min(64),
            quality_trend: trend,
        })
    }).map_err(|e| format!("Summary row error: {}", e))?;

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BenchConfig, BenchResult, BenchId, DataPattern};

    fn temp_db() -> Connection {
        open_db(Path::new(":memory:")).unwrap()
    }

    fn sample_result(node: &str, mbps: f64, quality: u8) -> BenchResult {
        BenchResult {
            id: BenchId {
                run_id: format!("test-{}", rand::random::<u32>()),
                node_id: node.to_string(),
                started_at_ms: 1000,
                peer_count: 1,
            },
            config: BenchConfig::default(),
            throughput_mbps: mbps,
            peak_mbps: mbps * 1.1,
            latency_p50_us: 500,
            latency_p95_us: 1000,
            latency_p99_us: 2000,
            jitter_us: 100.0,
            duration_secs: 5.0,
            retries: 0,
            loss_rate: 0.0,
            verified: true,
            recommended_chunk_bytes: 1048576,
            recommended_streams: 16,
            quality_score: quality,
            completed_at_ms: 2000,
        }
    }

    #[test]
    fn test_store_and_retrieve() {
        let conn = temp_db();
        let result = sample_result("epsilon", 950.0, 85);
        store_result(&conn, &result).unwrap();

        let recent = recent_runs(&conn, "epsilon", 10).unwrap();
        assert_eq!(recent.len(), 1);
        assert!((recent[0].throughput_mbps - 950.0).abs() < 0.1);
    }

    #[test]
    fn test_summary() {
        let conn = temp_db();
        let mut r1 = sample_result("delta", 800.0, 75);
        r1.completed_at_ms = 1000; // older
        let mut r2 = sample_result("delta", 900.0, 80);
        r2.completed_at_ms = 2000; // newer
        store_result(&conn, &r1).unwrap();
        store_result(&conn, &r2).unwrap();

        let s = summary(&conn, "delta").unwrap();
        assert_eq!(s.total_runs, 2);
        assert!(s.best_mbps >= 900.0);
        assert_eq!(s.quality_trend, "improving");
    }
}
