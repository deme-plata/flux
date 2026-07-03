// FIP-0001 ladder rung 7 (traits) sample 3: DYN dispatch — the vtable case.
// The call renders as Call{ func: <indirect load through the vtable slot> }.
pub trait Area { fn area(&self) -> i64; }
pub struct Sq { pub s: i64 }
impl Area for Sq { fn area(&self) -> i64 { self.s * self.s } }
pub fn dyn_call(a: &dyn Area) -> i64 { a.area() }
