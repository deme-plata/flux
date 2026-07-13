trait Area { fn area(&self) -> i64; }
struct Sq { s: i64 }
impl Area for Sq { fn area(&self) -> i64 { self.s * self.s } }
fn dyn_call(a: &dyn Area) -> i64 { a.area() }
fn main() -> i64 { let q = Sq { s: 4 }; dyn_call(&q) }
