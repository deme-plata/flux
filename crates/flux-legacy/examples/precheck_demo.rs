//! Live demo of flux-legacy P5 (split pre-check) on a real file.
//!   flux-cargo-wrapper run -p flux-legacy --example precheck_demo -- <file.rs> [max_modules]
use std::env;

fn main() {
    let path = env::args().nth(1).expect("usage: precheck_demo <file.rs> [max_modules]");
    let max: usize = env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(8);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let report = flux_legacy::precheck::precheck_file(&path, &src, max);
    print!("{}", flux_legacy::precheck::render_precheck(&report));
}
