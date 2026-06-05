//! flux-legacy PROTOTYPE 3 — the BUILD STRATEGY. P1 says what the workspace *is*, P2 *refactors* a
//! god-file; P3 answers the other half of "build this for me": in what ORDER, and how PARALLEL, does
//! a 100-crate brownfield compile?
//!
//! [`plan_build`] does a Kahn topological sort over the intra-workspace dependency graph
//! ([`LegacyCrate::deps`]) and groups crates into **build layers**: every crate in a layer depends
//! only on earlier layers, so a layer builds fully in parallel. The number of layers is the build's
//! critical-path depth; the widest layer is the max useful parallelism. Crates that never resolve are
//! caught in a **dependency cycle** and reported as blockers.
//!
//! HONEST SCOPE: this orders from PARSED path-deps only. It does NOT compile, and it can't see deps
//! outside the workspace — e.g. the real q-ai-inference → `mistralrs` manifest failure that breaks
//! `cargo metadata` on the Quillon tree is an *external* edge, invisible to this intra-workspace graph.
//! Surfacing that needs a manifest-resolve layer (a later prototype). What P3 gives you is correct and
//! useful on its own: the parallel build schedule + any cycles that would stall it.

use crate::{LegacyReport, RefactorTask};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A crate that can't be placed in the build order (its intra-workspace deps never all resolve).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Blocker {
    pub crate_name: String,
    pub reason: String,
    /// the still-unmet member deps at the point the sort stalled
    pub unmet_deps: Vec<String>,
}

/// The parallel build schedule for a workspace.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildPlan {
    pub crate_count: usize,
    /// ordered build layers; every crate in `batches[i]` depends only on crates in `batches[<i]`,
    /// so a layer compiles fully in parallel.
    pub batches: Vec<Vec<String>>,
    /// number of layers = build critical-path depth
    pub layers: usize,
    /// widest layer = the most crates that can compile at once
    pub max_parallel: usize,
    /// crates stuck in a dependency cycle (unbuildable as-is)
    pub blocked: Vec<Blocker>,
    /// the highest-fan-in crates — the hubs every later layer waits on
    pub critical_hubs: Vec<(String, usize)>,
}

/// Kahn topological sort of the intra-workspace dep graph → parallel build layers + cycle blockers.
/// Pure: reads only the [`LegacyReport`].
pub fn plan_build(report: &LegacyReport) -> BuildPlan {
    let members: HashSet<&str> = report.crates.iter().map(|c| c.name.as_str()).collect();

    // in-degree = number of this crate's deps that are workspace members (external deps don't gate order)
    let mut indeg: HashMap<String, usize> = HashMap::new();
    // dependents[x] = member crates that depend on x (so when x is built, decrement them)
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
    for c in &report.crates {
        let member_deps: Vec<&String> = c.deps.iter().filter(|d| members.contains(d.as_str())).collect();
        indeg.insert(c.name.clone(), member_deps.len());
        for d in member_deps {
            dependents.entry(d.clone()).or_default().push(c.name.clone());
        }
    }

    let mut placed: HashSet<String> = HashSet::new();
    let mut batches: Vec<Vec<String>> = Vec::new();
    loop {
        // a layer = every not-yet-placed crate whose remaining member-deps are all satisfied
        let mut batch: Vec<String> = report
            .crates
            .iter()
            .map(|c| c.name.clone())
            .filter(|n| !placed.contains(n) && indeg.get(n).copied().unwrap_or(0) == 0)
            .collect();
        if batch.is_empty() {
            break;
        }
        batch.sort(); // deterministic
        for n in &batch {
            placed.insert(n.clone());
        }
        // relax dependents of everything we just placed
        for n in &batch {
            if let Some(deps_of) = dependents.get(n) {
                for dep in deps_of {
                    if let Some(x) = indeg.get_mut(dep) {
                        *x = x.saturating_sub(1);
                    }
                }
            }
        }
        batches.push(batch);
    }

    // anything unplaced is in (or behind) a cycle
    let blocked: Vec<Blocker> = report
        .crates
        .iter()
        .filter(|c| !placed.contains(&c.name))
        .map(|c| Blocker {
            crate_name: c.name.clone(),
            reason: "dependency cycle: intra-workspace deps never all resolve".into(),
            unmet_deps: c
                .deps
                .iter()
                .filter(|d| members.contains(d.as_str()) && !placed.contains(*d))
                .cloned()
                .collect(),
        })
        .collect();

    // critical hubs: most-depended-on members (these gate the widest swath of the build)
    let mut hubs: Vec<(String, usize)> = report
        .crates
        .iter()
        .map(|c| (c.name.clone(), c.dependents.len()))
        .filter(|(_, n)| *n > 0)
        .collect();
    hubs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    hubs.truncate(8);

    let max_parallel = batches.iter().map(|b| b.len()).max().unwrap_or(0);
    BuildPlan {
        crate_count: report.crates.len(),
        layers: batches.len(),
        max_parallel,
        batches,
        blocked,
        critical_hubs: hubs,
    }
}

/// Visual build schedule (Viktor=visual): layers with widths, the parallelism ceiling, critical hubs,
/// and any cycle blockers.
pub fn render_build_plan(p: &BuildPlan) -> String {
    let mut o = String::new();
    o.push_str(&format!(
        "🏗  BUILD PLAN · {} crates · {} layers (critical-path depth) · up to {}× parallel\n",
        p.crate_count, p.layers, p.max_parallel
    ));
    for (i, b) in p.batches.iter().enumerate() {
        let sample: Vec<String> = b.iter().take(8).cloned().collect();
        let mut s = sample.join(", ");
        if b.len() > 8 {
            s.push_str(&format!(", +{}", b.len() - 8));
        }
        o.push_str(&format!("  L{:<2} [{:>3} ‖] {}\n", i, b.len(), s));
    }
    if !p.critical_hubs.is_empty() {
        o.push_str("  critical hubs (most-depended-on — everything waits on these):\n");
        for (name, fanin) in &p.critical_hubs {
            o.push_str(&format!("    fan-in {:>3}  {}\n", fanin, name));
        }
    }
    if p.blocked.is_empty() {
        o.push_str("  ✓ no dependency cycles — workspace is build-orderable\n");
    } else {
        o.push_str(&format!("  ✗ {} crate(s) in dependency cycles:\n", p.blocked.len()));
        for b in p.blocked.iter().take(20) {
            o.push_str(&format!("    {} ⇠ unmet: {}\n", b.crate_name, b.unmet_deps.join(", ")));
        }
    }
    o
}

/// Bridge to the plan lane: a dependency cycle is a refactor target too (break the cycle). Emits a
/// `RefactorTask` per blocked crate so cycles show up in the same prioritized list as god-files.
pub fn cycle_tasks(plan: &BuildPlan, start_rank: usize) -> Vec<RefactorTask> {
    plan.blocked
        .iter()
        .enumerate()
        .map(|(i, b)| RefactorTask {
            rank: start_rank + i,
            crate_name: b.crate_name.clone(),
            kind: "break-cycle".into(),
            target: b.unmet_deps.join(", "),
            detail: format!("dependency cycle with: {}", b.unmet_deps.join(", ")),
            impact: 0.9, // cycles block parallel build → high impact
            effort: "high".into(),
            est_minutes: 240,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LegacyCrate;

    fn crat(name: &str, deps: &[&str], dependents: &[&str]) -> LegacyCrate {
        LegacyCrate {
            name: name.into(),
            deps: deps.iter().map(|s| s.to_string()).collect(),
            dependents: dependents.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn report(crates: Vec<LegacyCrate>) -> LegacyReport {
        LegacyReport { crate_count: crates.len(), crates, ..Default::default() }
    }

    #[test]
    fn layers_a_simple_dag_in_dependency_order() {
        // q-types(leaf) → q-storage → q-api-server ; q-network also depends on q-types
        let r = report(vec![
            crat("q-types", &[], &["q-storage", "q-network", "q-api-server"]),
            crat("q-storage", &["q-types"], &["q-api-server"]),
            crat("q-network", &["q-types"], &[]),
            crat("q-api-server", &["q-types", "q-storage"], &[]),
        ]);
        let p = plan_build(&r);
        assert!(p.blocked.is_empty(), "acyclic → no blockers");
        // L0 = the leaf q-types alone
        assert_eq!(p.batches[0], vec!["q-types".to_string()]);
        // q-storage + q-network build together once q-types is done (parallel)
        assert!(p.batches[1].contains(&"q-storage".to_string()));
        assert!(p.batches[1].contains(&"q-network".to_string()));
        // q-api-server is last (needs q-storage)
        assert_eq!(p.batches.last().unwrap(), &vec!["q-api-server".to_string()]);
        assert_eq!(p.layers, 3);
        assert_eq!(p.max_parallel, 2);
        // q-types is the top hub
        assert_eq!(p.critical_hubs[0].0, "q-types");
    }

    #[test]
    fn detects_a_dependency_cycle() {
        // x ↔ y cycle, plus a clean leaf z
        let r = report(vec![
            crat("x", &["y"], &["y"]),
            crat("y", &["x"], &["x"]),
            crat("z", &[], &[]),
        ]);
        let p = plan_build(&r);
        // z builds; x and y are stuck in the cycle
        assert_eq!(p.batches, vec![vec!["z".to_string()]]);
        let blocked: HashSet<&str> = p.blocked.iter().map(|b| b.crate_name.as_str()).collect();
        assert!(blocked.contains("x") && blocked.contains("y"), "x,y in cycle; got {blocked:?}");
        assert!(p.blocked.iter().all(|b| !b.unmet_deps.is_empty()));
    }

    #[test]
    fn external_deps_dont_gate_order() {
        // a depends on a NON-member crate "serde" → treated as already available (in-degree 0)
        let r = report(vec![crat("a", &["serde", "tokio"], &[])]);
        let p = plan_build(&r);
        assert!(p.blocked.is_empty());
        assert_eq!(p.batches, vec![vec!["a".to_string()]]);
    }

    #[test]
    fn render_and_cycle_tasks() {
        let r = report(vec![crat("x", &["y"], &["y"]), crat("y", &["x"], &["x"])]);
        let p = plan_build(&r);
        let txt = render_build_plan(&p);
        assert!(txt.contains("BUILD PLAN"));
        assert!(txt.contains("dependency cycle"));
        let tasks = cycle_tasks(&p, 1);
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].kind, "break-cycle");
    }
}
