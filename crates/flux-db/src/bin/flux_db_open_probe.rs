//! flux-db-open-probe — time a cold Database::open (WAL replay included).
//!
//! The 0.36.x release gate is "cold-open from a >=1.5 GB WAL <= 2s"; this is the
//! measuring tool. Also used to size per-consumer max_wal_bytes decisions (the
//! chronos-v035 matrix showed WAL=1GiB wins +22% write throughput and ~20x read
//! p99 on the storage curve — the replay cost measured here is the other side
//! of that trade).
//!
//!   flux-db-open-probe <db-dir>
//!
//! Prints: wal_bytes, open_ms, and a get-probe latency to prove the DB is live.

use std::time::Instant;

fn main() {
    let dir = std::env::args().nth(1).expect("usage: flux-db-open-probe <db-dir>");
    let wal = std::fs::metadata(format!("{}/flux.wal", dir)).map(|m| m.len()).unwrap_or(0);
    let t = Instant::now();
    let db = flux_db::Database::open(&dir).expect("open");
    let open_ms = t.elapsed().as_millis();
    let t2 = Instant::now();
    let _ = db.get(b"blk/probe");
    let get_us = t2.elapsed().as_micros();
    println!("wal_bytes={} open_ms={} first_get_us={}", wal, open_ms, get_us);
}
