//! flux-rsync <src> <dst> [threads] — content-addressed, verified, parallel copy.
use std::path::Path;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (src, dst) = (Path::new(a.get(1).map(|s|s.as_str()).unwrap_or(".")), Path::new(a.get(2).map(|s|s.as_str()).unwrap_or("./dst")));
    let threads: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(8);
    match flux_rsync::sync(src, dst, threads) {
        Ok(r) => println!("flux-rsync: {} files · {} copied · {} skipped(dedup) · {:.2} GB written · {} verified · {:.0} MB/s · {}ms",
            r.files, r.copied, r.skipped, r.bytes_copied as f64/1e9, r.verify_pass, r.mbps, r.elapsed_ms),
        Err(e) => eprintln!("error: {e}"),
    }
}
