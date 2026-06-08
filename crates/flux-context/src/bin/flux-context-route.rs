//! flux-context-route — route a task to a model tier + a ripple-prioritized chunk
//! slice within that tier's token budget.
//!
//! Usage: `flux-context-route [root] --task read|edit|refactor|review|audit|build|swarm`

use flux_context::chunk::compute_manifest;
use flux_context::router::{ContextRouter, TaskKind};
use std::path::PathBuf;

fn main() {
    let mut root = std::env::current_dir().expect("cwd");
    let mut kind = TaskKind::ReadExplain;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--task" | "-t" => {
                if let Some(v) = it.next() {
                    match TaskKind::parse(&v) {
                        Some(k) => kind = k,
                        None => {
                            eprintln!("unknown task '{v}' (read|edit|refactor|review|audit|build|swarm)");
                            std::process::exit(2);
                        }
                    }
                }
            }
            other => root = PathBuf::from(other),
        }
    }

    let manifest = match compute_manifest(&root) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("flux-context-route: {e}");
            std::process::exit(1);
        }
    };
    let plan = ContextRouter::new(&manifest).route(kind, &[]);

    eprintln!(
        "task {:?} → tier {:?} ({}) · budget {} tok · used {} ({:.0}% fill) · {} chunks",
        plan.kind,
        plan.tier,
        plan.model,
        plan.budget_tokens,
        plan.used_tokens,
        plan.fill_ratio * 100.0,
        plan.selected.len()
    );
    for (i, name) in plan.selected.iter().take(20).enumerate() {
        eprintln!("  {:>2}. {name}", i + 1);
    }
    if plan.selected.len() > 20 {
        eprintln!("  … +{} more", plan.selected.len() - 20);
    }
}
