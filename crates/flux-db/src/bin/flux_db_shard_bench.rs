//! Sharded-writer A/B bench with a MANDATORY presence audit.
//!
//! The flux-db law (learned at terabyte cost): never report a speed number
//! without an outcome check on the same artifact — a store can be "fast" while
//! silently dropping data. Every leg here writes N GiB of 8 KiB entries in
//! put_many batches, fsync-barriers, then audits SAMPLES random keys for
//! presence AND value correctness before printing its MB/s.
//!
//!   flux_db_shard_bench <dir> <gib> <shards> [batch=256] [samples=20000]
//!
//! shards=1 exercises ShardedDb's single-shard path (== the coalesced single
//! writer plus facade overhead); shards=N is the parallel pipeline.

use flux_db::shard::ShardedDb;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: flux_db_shard_bench <dir> <gib> <shards> [batch] [samples]");
        std::process::exit(2);
    }
    let dir = &args[1];
    let gib: u64 = args[2].parse().expect("gib");
    let shards: usize = args[3].parse().expect("shards");
    let batch: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(256);
    let samples: u64 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(20_000);

    const VAL: usize = 8192;
    let total_entries: u64 = gib * 1024 * 1024 * 1024 / VAL as u64;
    let db = ShardedDb::open(dir, shards).expect("open");
    db.set_defer_compaction(true);

    // Deterministic value derived from the key index — auditable content.
    let mkval = |i: u64| -> Vec<u8> {
        let mut v = vec![0u8; VAL];
        v[..8].copy_from_slice(&i.to_le_bytes());
        v[VAL - 8..].copy_from_slice(&i.wrapping_mul(0x9E3779B97F4A7C15).to_le_bytes());
        v
    };

    let t0 = std::time::Instant::now();
    let mut buf: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(batch);
    for i in 0..total_entries {
        buf.push((format!("k{:012}", i).into_bytes(), mkval(i)));
        if buf.len() == batch {
            db.put_many(&buf).expect("put_many");
            buf.clear();
        }
        if i % 2_000_000 == 0 && i > 0 {
            let mbs = (i * VAL as u64) as f64 / 1e6 / t0.elapsed().as_secs_f64();
            eprintln!("  … {} entries, {:.0} MB/s", i, mbs);
        }
    }
    if !buf.is_empty() {
        db.put_many(&buf).expect("put_many tail");
    }
    db.sync_wal().expect("sync");
    let write_s = t0.elapsed().as_secs_f64();
    let mbs = (total_entries * VAL as u64) as f64 / 1e6 / write_s;

    // AUDIT: presence + content, deterministic LCG sampling.
    let mut ok: u64 = 0;
    let mut bad: u64 = 0;
    let mut x: u64 = 0x243F6A8885A308D3;
    for _ in 0..samples {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let i = x % total_entries;
        match db.get(format!("k{:012}", i).as_bytes()) {
            Ok(Some(v)) if v == mkval(i) => ok += 1,
            _ => bad += 1,
        }
    }
    println!(
        "shards={} batch={} wrote {} GiB in {:.1}s = {:.0} MB/s · audit {}/{} present+correct ({:.2}%)",
        shards, batch, gib, write_s, mbs, ok, ok + bad,
        100.0 * ok as f64 / (ok + bad) as f64
    );
    if bad > 0 {
        eprintln!("AUDIT FAILURE — speed number is VOID");
        std::process::exit(1);
    }
}
