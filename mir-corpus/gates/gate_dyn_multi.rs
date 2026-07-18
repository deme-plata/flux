trait Area { fn area(&self) -> i64; }
struct Sq { s: i64 }
struct Rect { w: i64, h: i64 }
impl Area for Sq { fn area(&self) -> i64 { self.s * self.s } }
impl Area for Rect { fn area(&self) -> i64 { self.w * self.h } }
fn dyn_call(a: &dyn Area) -> i64 { a.area() }
fn main() -> i64 {
    let s = Sq { s: 4 };
    let r = Rect { w: 2, h: 3 };
    dyn_call(&s) + dyn_call(&r)
}
