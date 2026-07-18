fn main() -> i64 {
    let mut v = Vec::new();
    v.push(3);
    v.push(4);
    let mut sum = 0;
    for x in v {
        sum = sum + x;
    }
    sum
}
