//! flux-archive-cli  snapshot|verify <src> <store>
use std::path::Path;
use flux_archive::{snapshot, verify};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (cmd, src, store) = (a.get(1).map(|s|s.as_str()).unwrap_or(""), Path::new(a.get(2).map(|s|s.as_str()).unwrap_or(".")), Path::new(a.get(3).map(|s|s.as_str()).unwrap_or("./store")));
    match cmd {
        "snapshot" => {
            let m = snapshot(src, store).expect("snapshot");
            let dedup = m.total_bytes.saturating_sub(m.unique_bytes);
            println!("snapshot: {} files · {:.2} GB logical · {:.2} GB stored · {:.2} GB dedup-saved", m.entries.len(), m.total_bytes as f64/1e9, m.unique_bytes as f64/1e9, dedup as f64/1e9);
            match verify(&m, store) { Ok(_) => println!("verify: ✓ all {} CIDs present + hash-match", m.entries.len()), Err(e) => println!("verify: ✗ {e}") }
        }
        _ => eprintln!("usage: flux-archive-cli snapshot <src> <store>"),
    }
}
