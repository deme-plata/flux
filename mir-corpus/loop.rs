// MIR-drift corpus (FIP-0001 keep-A-open #2) — while loop.
// Forces a `goto -> bbN` BACK-EDGE plus the loop-condition `switchInt`, the CFG shape that drives
// flux-backend's MIR-direct path (Cranelift Variables/SSA) rather than the Expr path. If rustc's
// rendering of goto / loop headers drifts against the pinned RUSTC_VERSION, check.sh's diff fails.
pub fn sum_to(n: i64) -> i64 {
    let mut acc = 0i64;
    let mut i = 0i64;
    while i < n {
        acc += i;
        i += 1;
    }
    acc
}
