//! Live demo of flux-legacy P2 (god-file splitter) on a real file.
//!   flux-cargo-wrapper run -p flux-legacy --example split_demo -- <file.rs> [max_modules]
use std::env;

fn main() {
    let path = env::args().nth(1).expect("usage: split_demo <file.rs> [max_modules]");
    let max: usize = env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(6);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let patch = flux_legacy::split::plan_split(&path, &src, max);
    print!("{}", flux_legacy::split::render_patch(&patch));
}
