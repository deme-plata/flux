// MIR-drift corpus — multi-field data-carrying enum (ladder rung 4+)
pub enum Multi {
    A(i64),
    B(i64, i64),
    C,
}

pub fn make_a(x: i64) -> Multi { Multi::A(x) }
pub fn make_b(x: i64, y: i64) -> Multi { Multi::B(x, y) }
pub fn make_c() -> Multi { Multi::C }

pub fn sum(o: Multi) -> i64 {
    match o {
        Multi::A(x) => x,
        Multi::B(x, y) => x + y,
        Multi::C => 0,
    }
}
