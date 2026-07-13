trait Area { fn area(&self) -> i64; }
struct Sq { s: i64 }
impl Area for Sq { fn area(&self) -> i64 { self.s * self.s } }
fn area_of<T: Area>(t: &T) -> i64 { t.area() }
fn main() -> i64 { let q = Sq { s: 5 }; area_of(&q) }
