fn main() -> i64 {
    let mut v = Vec::new();
    let mut i = 0;
    while i < 5 {
        v.push(i * 2);
        i = i + 1;
    }
    let mut sum = 0;
    let mut j = 0;
    while j < v.len() {
        sum = sum + v[j];
        j = j + 1;
    }
    sum + v.len() as i64
}
