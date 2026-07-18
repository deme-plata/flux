fn main() -> i64 {
    let mut v: Vec<i64> = Vec::new();
    v.push(3);
    v.push(4);
    let k = 2;
    let total: i64 = v.into_iter().map(|x| x * k).sum();
    total
}
