trait Area { fn area(&self) -> i64; }
struct Sq { s: i64 }
impl Area for Sq { fn area(&self) -> i64 { self.s * self.s } }
fn main() -> i64 { let q = Sq { s: 6 }; q.area() }
