//! flux_db_shard_migrate — offline single-store → sharded-root rewrite,
//! with a MANDATORY audit before the old dir may be retired.
//!
//! SHARDED-BLOCKSTORE-PLAN: "a single→sharded migration is a rewrite
//! (iter() → put_many into the new root), offline, with a presence audit
//! before the old dir is retired. No in-place conversion." This is that tool.
//!
//!   flux_db_shard_migrate <src-single-db> <dst-root> <shards> [batch=4096] [samples=20000]
//!
//! Refuses loudly when: src is itself sharded (has a SHARDS marker), or dst
//! already exists non-empty (never merge into a half-written root). After the
//! copy + all-shard fsync barrier it audits: (1) full recount via
//! iter_unordered == source live count, (2) `samples` evenly-spaced keys
//! value-compared byte-for-byte through the routed get(). Exit 0 ONLY when
//! both pass — the flux-db law: no migration is "done" on speed or vibes,
//! only on an outcome check against the same artifact.

use std::path::Path;

fn main() {
    let mut args = std::env::args().skip(1);
    let usage = "usage: flux_db_shard_migrate <src-single-db> <dst-root> <shards> [batch=4096] [samples=20000]";
    let src = args.next().expect(usage);
    let dst = args.next().expect(usage);
    let shards: usize = args.next().expect(usage).parse().expect("shards usize");
    let batch: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(4096).max(1);
    let samples: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(20_000).max(1);

    if flux_db::shard::exists(Path::new(&src)) {
        eprintln!("REFUSED: source {} is already a sharded root (SHARDS marker present)", src);
        std::process::exit(2);
    }
    if Path::new(&dst).exists()
        && std::fs::read_dir(&dst).map(|mut d| d.next().is_some()).unwrap_or(false)
    {
        eprintln!("REFUSED: destination {} exists and is non-empty — never merge into a half-written root", dst);
        std::process::exit(2);
    }

    let sdb = flux_db::Database::open(&src).expect("open source");
    let ddb = flux_db::shard::ShardedDb::open(&dst, shards).expect("open destination sharded root");
    ddb.set_defer_compaction(true);

    let t0 = std::time::Instant::now();
    let mut moved: u64 = 0;
    let mut bytes: u64 = 0;
    let mut buf: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(batch);
    for (k, v) in sdb.iter() {
        bytes += (k.len() + v.len()) as u64;
        buf.push((k, v));
        if buf.len() == batch {
            ddb.put_many(&buf).expect("put_many");
            moved += buf.len() as u64;
            buf.clear();
            if moved % 1_000_000 == 0 {
                eprintln!("  … {} entries, {:.0} MB/s", moved,
                    bytes as f64 / 1e6 / t0.elapsed().as_secs_f64());
            }
        }
    }
    if !buf.is_empty() {
        ddb.put_many(&buf).expect("put_many tail");
        moved += buf.len() as u64;
    }
    ddb.sync_wal().expect("all-shard sync_wal barrier");
    let write_s = t0.elapsed().as_secs_f64();
    eprintln!("copied {} entries ({:.1} MB) into {} shards in {:.1}s ({:.0} MB/s)",
        moved, bytes as f64 / 1e6, shards, write_s, bytes as f64 / 1e6 / write_s);

    // ── AUDIT 1: full recount across every shard ──
    let dst_count = ddb.iter_unordered().count() as u64;
    let recount_ok = dst_count == moved;
    println!("audit recount: dst={} src={} — {}", dst_count, moved,
        if recount_ok { "OK" } else { "MISMATCH" });

    // ── AUDIT 2: evenly-spaced value comparison through routed get() ──
    let step = ((moved as usize) / samples).max(1);
    let (mut checked, mut bad) = (0u64, 0u64);
    for (i, (k, v)) in sdb.iter().enumerate() {
        if i % step != 0 { continue; }
        checked += 1;
        match ddb.get(&k) {
            Ok(Some(dv)) if dv == v => {}
            other => {
                bad += 1;
                if bad <= 5 {
                    eprintln!("  MISMATCH key={:?}… got={:?}",
                        String::from_utf8_lossy(&k[..k.len().min(24)]),
                        other.map(|o| o.map(|dv| dv.len())));
                }
            }
        }
    }
    let sample_ok = bad == 0;
    println!("audit values: {}/{} sampled OK — {}", checked - bad, checked,
        if sample_ok { "OK" } else { "MISMATCH" });

    if recount_ok && sample_ok {
        println!("MIGRATION VERIFIED — safe to retire {}", src);
    } else {
        println!("MIGRATION FAILED AUDIT — do NOT retire the source");
        std::process::exit(1);
    }
}
