// FIP-0001 ladder rung 7 (traits) sample 2: GENERIC (monomorphizable) dispatch.
// rustc emits polymorphic MIR for area_of<T>; the instantiating caller pins T=Sq.
pub trait Area { fn area(&self) -> i64; }
pub struct Sq { pub s: i64 }
impl Area for Sq { fn area(&self) -> i64 { self.s * self.s } }
pub fn area_of<T: Area>(t: &T) -> i64 { t.area() }
pub fn call_generic(x: Sq) -> i64 { area_of(&x) }
