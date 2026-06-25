// MIR-drift corpus (FIP-0001 keep-A-open #2).
// Exercises the rustc --emit=mir forms that flux-frontend::mir::parse_mir consumes:
// scalar arithmetic, tuples, flat structs, field access, and calls. If rustc's MIR
// textual dialect drifts against the pinned RUSTC_VERSION, check.sh's diff fails CI,
// so the "contracted frontend" is enforced operationally rather than assumed.
pub struct P { pub x: i64, pub y: i64 }

pub fn mk() -> P { P { x: 6, y: 7 } }
pub fn tup() -> (i64, i64) { (3, 4) }
pub fn add(a: i64, b: i64) -> i64 { a + b }

pub fn calc() -> i64 {
    let p = mk();
    let t = tup();
    add(p.x * p.y, t.0 + t.1)
}
