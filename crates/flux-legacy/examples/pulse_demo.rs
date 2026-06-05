//! Mine a captured journald window → per-crate runtime pain (flux-legacy Pulse).
//!   journalctl -u q-api-server --since "5 min ago" > /tmp/jl.txt
//!   flux-cargo-wrapper run -p flux-legacy --example pulse_demo -- /tmp/jl.txt "last 5 min"
use flux_legacy::pulse::mine;

fn main() {
    let path = std::env::args().nth(1).expect("usage: pulse_demo <journald-file> [window-label]");
    let window = std::env::args().nth(2).unwrap_or_else(|| "live".into());
    let log = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let r = mine(&log, &window);
    println!("🫀 PULSE [{}] — {} lines, {} parsed as log", r.window, r.total_lines, r.parsed);
    println!(
        "  {:<22} {:>8} {:>6} {:>7} {:>5} {:>7} {:>8}",
        "crate", "pain", "errors", "warns", "panic", "timeout", "vdf-cont"
    );
    for c in r.crates.iter().take(12) {
        println!(
            "  {:<22} {:>8.0} {:>6} {:>7} {:>5} {:>7} {:>8}",
            c.crate_name, c.pain, c.errors, c.warns, c.panics, c.timeouts, c.vdf_contention
        );
    }
}
