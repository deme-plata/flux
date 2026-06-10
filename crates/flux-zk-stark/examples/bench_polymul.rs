//! Real microbenchmark for Polynomial::multiply (the O(n²) STARK hot loop).
//! Deterministic input, release-mode, checksum printed to defeat DCE.
//! Run: fluxc build --release --package flux-zk-stark --example bench_polymul
//!      then execute the binary; compare ns/op before vs after an optimization.

use flux_zk_stark::polynomials::Polynomial;
use std::time::Instant;

// 2^61 - 1 (Mersenne). < 2^63 so the ORIGINAL u64 `(a+b) % p` field_add cannot
// overflow — both original and optimized must agree here (correctness proof).
const P: u64 = 2305843009213693951;

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state % P
}

fn main() {
    const DEG: usize = 1500; // ~1500×1500 = 2.25M field ops per multiply
    const ITERS: usize = 50;

    let mut s = 0x1234_5678_9abc_def0u64;
    let a_coeffs: Vec<u64> = (0..=DEG).map(|_| lcg(&mut s)).collect();
    let b_coeffs: Vec<u64> = (0..=DEG).map(|_| lcg(&mut s)).collect();
    let a = Polynomial::new(a_coeffs, P);
    let b = Polynomial::new(b_coeffs, P);

    // warmup
    let mut checksum: u64 = 0;
    let warm = a.multiply(&b);
    checksum ^= warm.coefficients.iter().fold(0u64, |x, &c| x.wrapping_add(c));

    let t0 = Instant::now();
    for _ in 0..ITERS {
        let r = a.multiply(&b);
        // fold the result into a checksum so the optimizer can't elide the work
        checksum ^= r.coefficients.iter().fold(0u64, |x, &c| x.wrapping_add(c));
    }
    let elapsed = t0.elapsed();

    let per_op = elapsed.as_nanos() as f64 / ITERS as f64;
    println!(
        "polymul deg={DEG} iters={ITERS}  total={:.3}ms  per_op={:.3}ms ({:.0} ns)  checksum={checksum:#018x}",
        elapsed.as_secs_f64() * 1e3,
        per_op / 1e6,
        per_op
    );
}
