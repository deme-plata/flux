fn apply<F: Fn(i64) -> i64>(f: F, x: i64) -> i64 { f(x) }
fn main() -> i64 {
    let k = 3;
    apply(|x| x + k, 4) + apply(|x| x * k, 5)
}
