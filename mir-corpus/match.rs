// MIR-drift corpus (FIP-0001 keep-A-open #2) — n-way match.
// Forces rustc to emit a `switchInt(_d) -> [v: bbX, ..., otherwise: bbZ]` terminator, the exact
// form flux-frontend::mir::parse_mir lowers into a chained icmp+brif. If rustc's rendering of
// switchInt (operand spelling, target-list layout, `otherwise` keyword) drifts against the pinned
// RUSTC_VERSION, check.sh's diff fails — protecting the match codegen path.
pub fn classify(n: i64) -> i64 {
    match n {
        0 => 100,
        1 => 200,
        2 => 300,
        3 => 400,
        _ => 0,
    }
}
