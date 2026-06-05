//! flux-legacy PROTOTYPE 2 — the ARCHITECTURE / BUILD lens.
//!
//! Prototype 1 (`flux-legacy`) measures *files* (LOC, god-files, refactor tasks). This one reads
//! the whole-workspace **dependency graph** (via `flux-graph`) and reports what a modernizer
//! actually needs to sequence the work on a brownfield repo it didn't build — e.g. the 100-crate
//! Quillon Graph node:
//!   • build **parallelism** — critical-path depth (stages) + peak width,
//!   • blast-radius **keystones** (high fan-in → refactor LAST, with the most tests),
//!   • zero-dependent **leaves** (→ modernize FIRST, zero blast radius),
//!   • crypto-**agility** PQ migration order.
//!
//! Pure analysis over `flux_graph` types; no network, no build side effects.

use flux_graph::agility::{audit_agility, transitive_dependents};
use flux_graph::{CrateType, WorkspaceGraph};

/// The architecture report for a legacy workspace.
#[derive(Debug, Clone, Default)]
pub struct ArchReport {
    pub root: String,
    pub crate_count: usize,
    pub libs: usize,
    pub bins: usize,
    pub proc_macros: usize,
    /// Number of build STAGES (parallel batches) — the critical-path depth.
    pub build_stages: usize,
    /// Peak crates compilable in parallel (widest batch).
    pub peak_parallel: usize,
    /// Top crates by fan-in (how many crates transitively depend on them) — refactor LAST.
    pub keystones: Vec<(String, usize)>,
    /// Crates nothing depends on (fan-in 0) — safe to modernize FIRST.
    pub leaves: Vec<String>,
    pub agility_score: f64,
    pub pq_crates: usize,
    pub classical_crates: usize,
    /// (crate, recommended PQ replacement) for crates on classical crypto.
    pub migrations: Vec<(String, String)>,
}

/// Widest parallel batch — the peak compiler concurrency the build can use.
pub fn peak_width(batches: &[Vec<usize>]) -> usize {
    batches.iter().map(|b| b.len()).max().unwrap_or(0)
}

/// Analyze a resolved workspace into an [`ArchReport`] (pure).
pub fn analyze(ws: &WorkspaceGraph) -> ArchReport {
    let (mut libs, mut bins, mut proc_macros) = (0, 0, 0);
    for c in &ws.crates {
        match c.crate_type {
            CrateType::Lib => libs += 1,
            CrateType::Bin => bins += 1,
            CrateType::ProcMacro => proc_macros += 1,
        }
    }
    // Fan-in per crate = how many others transitively depend on it (blast radius).
    let mut fanin: Vec<(String, usize)> = ws
        .crates
        .iter()
        .map(|c| (c.name.clone(), transitive_dependents(ws, &c.name).len()))
        .collect();
    fanin.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let keystones: Vec<(String, usize)> = fanin.iter().filter(|(_, n)| *n > 0).take(8).cloned().collect();
    let leaves: Vec<String> = fanin.iter().filter(|(_, n)| *n == 0).map(|(n, _)| n.clone()).collect();

    let ag = audit_agility(ws);
    let migrations = ag
        .migration_needed
        .iter()
        .map(|m| (m.crate_name.clone(), m.recommended.clone()))
        .collect();

    ArchReport {
        root: ws.root.display().to_string(),
        crate_count: ws.crates.len(),
        libs,
        bins,
        proc_macros,
        build_stages: ws.batches.len(),
        peak_parallel: peak_width(&ws.batches),
        keystones,
        leaves,
        agility_score: ag.agility_score,
        pq_crates: ag.pq_crates,
        classical_crates: ag.classical_crates,
        migrations,
    }
}

/// A data-driven modernization plan: ordered, concrete steps for sequencing the work.
pub fn modernization_plan(r: &ArchReport) -> Vec<String> {
    let mut plan = Vec::new();
    if !r.leaves.is_empty() {
        let sample: Vec<&str> = r.leaves.iter().take(5).map(|s| s.as_str()).collect();
        plan.push(format!(
            "1. MODERNIZE LEAVES FIRST — {} crate(s) with zero dependents (no blast radius). Start: {}",
            r.leaves.len(),
            sample.join(", ")
        ));
    }
    plan.push(format!(
        "2. BUILD IN PARALLEL — {} stages on the critical path, up to {} crates compiled at once. \
         Flux schedules these {} batches, not {} sequential builds.",
        r.build_stages, r.peak_parallel, r.build_stages, r.crate_count
    ));
    if !r.migrations.is_empty() {
        let first = &r.migrations[0];
        plan.push(format!(
            "3. CRYPTO-AGILITY — {} crate(s) on classical crypto need a PQ path (agility {:.0}%). \
             Migrate leaves→keystones; start with {} → {}.",
            r.migrations.len(),
            r.agility_score * 100.0,
            first.0,
            first.1
        ));
    }
    if let Some((name, fanin)) = r.keystones.first() {
        plan.push(format!(
            "4. KEYSTONES LAST — refactor the high-fan-in crates with the MOST test coverage; \
             top is `{name}` ({fanin} dependents). Changing it ripples furthest.",
        ));
    }
    plan
}

/// Human-readable report.
pub fn render_text(r: &ArchReport) -> String {
    let mut s = String::new();
    s.push_str(&format!("🏛  flux-legacy2 — architecture lens\n   {}\n\n", r.root));
    s.push_str(&format!(
        "  crates {} ({} lib · {} bin · {} proc-macro)\n  build  {} stages · peak {} parallel\n  agility {:.0}%  ({} PQ · {} classical · {} to migrate)\n",
        r.crate_count, r.libs, r.bins, r.proc_macros,
        r.build_stages, r.peak_parallel,
        r.agility_score * 100.0, r.pq_crates, r.classical_crates, r.migrations.len()
    ));
    s.push_str("\n  KEYSTONES (refactor last — highest blast radius):\n");
    for (name, n) in &r.keystones {
        s.push_str(&format!("    {n:>4} dependents  {name}\n"));
    }
    s.push_str(&format!("\n  LEAVES (modernize first — 0 dependents): {}\n", r.leaves.len()));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_width_is_widest_batch() {
        assert_eq!(peak_width(&[vec![0, 1, 2], vec![3], vec![4, 5]]), 3);
        assert_eq!(peak_width(&[]), 0);
    }

    fn sample_report() -> ArchReport {
        ArchReport {
            root: "/ws".into(),
            crate_count: 100,
            libs: 90,
            bins: 8,
            proc_macros: 2,
            build_stages: 7,
            peak_parallel: 22,
            keystones: vec![("q-types".into(), 64), ("q-storage".into(), 31)],
            leaves: vec!["q-api-server".into(), "q-miner".into()],
            agility_score: 0.4,
            pq_crates: 6,
            classical_crates: 9,
            migrations: vec![("q-network".into(), "dilithium5".into())],
        }
    }

    #[test]
    fn plan_orders_leaves_first_keystones_last() {
        let p = modernization_plan(&sample_report());
        assert!(p[0].contains("LEAVES FIRST"), "first step modernizes leaves: {}", p[0]);
        assert!(p.iter().any(|s| s.contains("BUILD IN PARALLEL") && s.contains("7 stages")));
        assert!(p.iter().any(|s| s.contains("CRYPTO-AGILITY") && s.contains("q-network")));
        assert!(p.last().unwrap().contains("KEYSTONES LAST") && p.last().unwrap().contains("q-types"));
    }

    #[test]
    fn plan_skips_empty_sections() {
        // no leaves, no migrations → only the build-parallelism step (always present).
        let mut r = sample_report();
        r.leaves.clear();
        r.migrations.clear();
        r.keystones.clear();
        let p = modernization_plan(&r);
        assert_eq!(p.len(), 1);
        assert!(p[0].contains("BUILD IN PARALLEL"));
    }

    #[test]
    fn render_text_has_the_headline_numbers() {
        let t = render_text(&sample_report());
        assert!(t.contains("crates 100"));
        assert!(t.contains("7 stages"));
        assert!(t.contains("q-types"));
    }
}
