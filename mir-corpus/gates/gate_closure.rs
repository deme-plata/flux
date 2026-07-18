fn main() -> i64 {
    let k = 3;
    let add_k = |x: i64| x + k;
    add_k(4) + add_k(10)
}
