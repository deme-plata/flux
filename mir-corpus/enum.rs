// MIR-drift corpus (FIP-0001 keep-A-open #2) — C-like enum (newest ladder rung, commit 1626fadc).
// Forces `_d = discriminant(_1)` + an exhaustive-match `switchInt` whose `otherwise` arm is
// `unreachable;` (lowered by flux-backend to a Cranelift trap), and an enum-construction const.
// These are the forms parse_mir learned most recently, so they are the most likely to drift
// silently; snapshotting them here turns that risk into a CI failure against the pinned rustc.
pub enum Color {
    Red,
    Green,
    Blue,
}

pub fn code(c: Color) -> i64 {
    match c {
        Color::Red => 1,
        Color::Green => 2,
        Color::Blue => 3,
    }
}

pub fn make() -> Color {
    Color::Green
}
