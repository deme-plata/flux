//! flux-legacy3 — converged risk backlog for a legacy workspace (P1 ∧ P2).
//!
//!   flux-legacy3 [workspace_root] [top_n]   # default: Quillon Graph node, top 20
use std::path::PathBuf;

use flux_legacy::analyze_workspace_legacy;
use flux_legacy3::{converge, render_backlog};

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/home/orobit/q-narwhalknight-src".into());
    let top_n: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(20);

    // P1 — file lens (walks <root>/crates/*).
    let p1 = analyze_workspace_legacy(&root);
    // P2 — architecture lens (dependency graph via flux-graph).
    let ws = match flux_graph::resolve_workspace(&PathBuf::from(&root)) {
        Ok(ws) => ws,
        Err(e) => { eprintln!("flux-legacy3: resolve_workspace({root}) failed: {e}"); std::process::exit(1); }
    };
    let p2 = flux_legacy2::analyze(&ws);

    let converged = converge(&p1, &p2);
    print!("{}", render_backlog(&converged, top_n));
}
