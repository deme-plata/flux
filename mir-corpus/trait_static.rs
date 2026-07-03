// FIP-0001 ladder rung 7 (traits) sample 1: STATIC dispatch through a concrete impl.
// The call renders as a plain MIR Call to the mangled impl fn — no vtable involved.
pub trait Area { fn area(&self) -> i64; }
pub struct Sq { pub s: i64 }
impl Area for Sq { fn area(&self) -> i64 { self.s * self.s } }
pub fn static_call(x: Sq) -> i64 { x.area() }
