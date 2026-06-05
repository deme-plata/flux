//! Live demo of flux-legacy P3 (parallel build schedule) on a real workspace.
//!   flux-cargo-wrapper run -p flux-legacy --example buildplan_demo -- <workspace_root>
use std::env;

fn main() {
    let root = env::args().nth(1).unwrap_or_else(|| ".".into());
    let report = flux_legacy::analyze::analyze_workspace_legacy(&root);
    let plan = flux_legacy::buildplan::plan_build(&report);
    print!("{}", flux_legacy::buildplan::render_build_plan(&plan));
}
