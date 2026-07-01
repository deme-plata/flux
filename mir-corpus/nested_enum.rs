// Nested aggregate: struct inside data-carrying enum
pub struct Point { pub x: i64, pub y: i64 }

pub enum Shape {
    Circle(i64),           // radius
    Rect(Point, Point),    // top-left, bottom-right
    None,
}

pub fn make_circle(r: i64) -> Shape { Shape::Circle(r) }
pub fn make_rect(x1: i64, y1: i64, x2: i64, y2: i64) -> Shape {
    Shape::Rect(Point { x: x1, y: y1 }, Point { x: x2, y: y2 })
}

pub fn area(s: Shape) -> i64 {
    match s {
        Shape::Circle(r) => r * r * 3,
        Shape::Rect(p1, p2) => (p2.x - p1.x) * (p2.y - p1.y),
        Shape::None => 0,
    }
}
