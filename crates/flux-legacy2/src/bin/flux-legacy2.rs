//! flux-legacy2 — run the architecture/build lens against a legacy workspace.
//!
//!   flux-legacy2 [workspace_root]    # default: the Quillon Graph node
use std::path::PathBuf;

use flux_graph::resolve_workspace;
use flux_legacy2::{analyze, modernization_plan, render_text};

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/home/orobit/q-narwhalknight-src".into());
    let ws = match resolve_workspace(&PathBuf::from(&root)) {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("flux-legacy2: could not resolve workspace at {root}: {e}");
            std::process::exit(1);
        }
    };
    let report = analyze(&ws);
    print!("{}", render_text(&report));
    println!("\n  MODERNIZATION PLAN");
    for step in modernization_plan(&report) {
        println!("    {step}");
    }
}
