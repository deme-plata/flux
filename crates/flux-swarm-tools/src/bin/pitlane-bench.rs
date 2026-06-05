//! pitlane-bench — measure the F1 pit-stop speed of the swarm-combo primitives.
//!
//! Every `flux_swarm_*` op (register / claim / message / complete / box-register)
//! is ONE atomic locked read-modify-write ([`with_locked`]) over a small JSON file.
//! This times that primitive on a scratch file so we know the per-op latency and
//! the full-pit-stop latency — "is the pitlane fast?" measured, not asserted.

use std::time::Instant;

use flux_swarm_tools::atomic_lock::with_locked;

fn main() {
    let n: u128 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(2000);
    let d = std::env::temp_dir();
    let lock = d.join("pitlane.lock");
    let path = d.join("pitlane.json");
    let _ = std::fs::remove_file(&path);

    // warm up (create the file once).
    with_locked(&lock, &path, |_| b"{}".to_vec()).unwrap();

    // the core swarm op: one atomic locked R-M-W (what register/claim/message/complete do).
    let t0 = Instant::now();
    for i in 0..n {
        with_locked(&lock, &path, |_cur| format!("{{\"n\":{i}}}").into_bytes()).unwrap();
    }
    let op_us = t0.elapsed().as_micros() as f64 / n as f64;

    // a full F1 PIT STOP ≈ the combo a swarm task runs end-to-end:
    // register + claim(lane-lock) + ship-message + complete(settle) + may_destroy guard ≈ 5 ops.
    let pit_us = op_us * 5.0;

    println!("🏁 FLUX PITLANE BENCH (n={n}, scratch file, real with_locked primitive)");
    println!("  atomic swarm op (flock + read + write tmp + atomic rename) : {op_us:.1} µs/op");
    println!("  ── full pit stop (register+claim+ship+settle+guard ≈ 5 ops) ──");
    println!("  PIT STOP TIME : {pit_us:.0} µs  (~{:.3} ms)", pit_us / 1000.0);
    println!("  PIT STOPS/SEC : {:.0}", 1_000_000.0 / pit_us);
    let _ = std::fs::remove_file(&path);
}
