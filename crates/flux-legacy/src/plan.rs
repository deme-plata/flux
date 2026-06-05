//! plan.rs (LEGACY-2) — turn a measured [`LegacyReport`] into a PRIORITIZED refactor plan.
//!
//! Enhances rocky-vision's baseline. Ranks actions by **impact × (1/effort)** so the biggest
//! wins per hour float up:
//!   1. **split god-files** — a single .rs over [`GOD_FILE_LOC`]; impact now scales with size
//!      AND **fan-in** (a 2400-LOC file that 6 crates depend on is far worse than an isolated one
//!      — the baseline ignored fan-in here);
//!   2. **add tests** — big crates with NO test module (risk grows with size when nothing covers it);
//!   3. **decouple** — high fan-in hubs everything pulls the full implementation of.
//!
//! PURE: derives everything from the report, no I/O. Fully unit-tested. Verify with `flux_combo`.
//! For MCP-handler-style god-files, `flux_refactor::handler_extract::suggest_modules(tool_names)`
//! deepens the split once the per-file tool list is known (LEGACY-4/5 supply it); the report itself
//! carries no tool names, so the generic LOC-based module estimate is what's honest at this layer.

use crate::{LegacyReport, RefactorTask, GOD_FILE_LOC};

/// Effort bucket + a rough minute estimate from a line count.
fn effort_for_loc(loc: usize) -> (&'static str, u64) {
    if loc > 2000 {
        ("high", 180)
    } else if loc > 1000 {
        ("medium", 90)
    } else {
        ("low", 40)
    }
}

/// The ranking score. Impact IS the rank: the biggest architectural problems lead, and effort is
/// a *tiebreak* (quick wins first among equally-impactful tasks), never a multiplier — multiplying
/// by 1/effort floated tiny low-effort files above the 27k-LOC main.rs. Exposed so [`render`] can
/// show "why this rank" without recomputing.
pub fn task_score(t: &RefactorTask) -> f64 {
    t.impact
}

/// Build a prioritized refactor plan from a measured workspace report. Pure.
pub fn refactor_plan(report: &LegacyReport) -> Vec<RefactorTask> {
    let fan_in = |name: &str| {
        report
            .crates
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.dependents.len())
            .unwrap_or(0)
    };
    let mut tasks: Vec<RefactorTask> = Vec::new();

    // Self-calibrate to THIS repo: normalize size + fan-in against the observed maxima so impact is
    // a discriminating [0,1] score, not a saturated 1.0. (Hardcoded divisors saturated every
    // high-fan-in crate — a 5066-LOC file that 63 crates depend on and an 850-LOC file both pinned
    // to 1.0.) Floored at 1 to avoid div-by-zero on an empty/degenerate report.
    let max_god_loc = report.god_files.iter().map(|g| g.loc).max().unwrap_or(1).max(1) as f64;
    let max_crate_loc = report.crates.iter().map(|c| c.loc).max().unwrap_or(1).max(1) as f64;
    let max_fanin = report
        .crates
        .iter()
        .map(|c| c.dependents.len())
        .max()
        .unwrap_or(1)
        .max(1) as f64;

    // 1) GOD-FILES — split the worst single files first. impact = 0.70·size + 0.30·fan-in, both
    //    normalized to the repo max. The biggest file (main.rs) gets size≈1.0 → leads; a hub file
    //    (q-types/lib.rs, fan-in 63) is lifted by the fan-in term even at moderate size. No
    //    multiplication, so nothing saturates and the bars actually discriminate.
    for g in &report.god_files {
        let fi = fan_in(&g.crate_name);
        let size = (g.loc as f64 / max_god_loc).min(1.0);
        let fanin = (fi as f64 / max_fanin).min(1.0);
        let impact = 0.70 * size + 0.30 * fanin;
        let (effort, est) = effort_for_loc(g.loc);
        let mods = (g.loc / 250).max(2); // ~250 LOC per focused module
        tasks.push(RefactorTask {
            rank: 0,
            crate_name: g.crate_name.clone(),
            kind: "split-god-file".into(),
            target: g.file.clone(),
            detail: format!(
                "{} LOC in one file (>{GOD_FILE_LOC}); {fi} crate(s) depend on this crate. Split into ~{mods} focused modules (handler_extract::suggest_modules deepens it once tool names are known).",
                g.loc
            ),
            impact,
            effort: effort.into(),
            est_minutes: est,
        });
    }

    // 2) UNTESTED high-LOC crates — add a test module (a safety net before any refactor).
    for c in &report.crates {
        if !c.has_tests && c.loc >= 300 {
            // a safety net, weighted below the big structural splits: 0.45·size at most.
            let impact = 0.45 * (c.loc as f64 / max_crate_loc).min(1.0);
            let (effort, est) = effort_for_loc(c.loc);
            tasks.push(RefactorTask {
                rank: 0,
                crate_name: c.name.clone(),
                kind: "add-tests".into(),
                target: c.name.clone(),
                detail: format!(
                    "{} LOC, NO tests. Add a unit-test module covering its {} pub fn / {} pub types.",
                    c.loc, c.pub_fns, c.pub_types
                ),
                impact,
                effort: effort.into(),
                est_minutes: est,
            });
        }
    }

    // 3) HIGH FAN-IN hubs — decouple behind a thin interface so changes don't ripple everywhere.
    for c in &report.crates {
        if c.dependents.len() >= 5 && c.loc >= 500 {
            // fan-in hub risk: 0.10 floor + up to 0.55·(relative fan-in). The worst hub
            // (q-types, 63 dependents) lands just under a giant god-file split — decouple it early.
            let impact = 0.10 + 0.55 * (c.dependents.len() as f64 / max_fanin).min(1.0);
            tasks.push(RefactorTask {
                rank: 0,
                crate_name: c.name.clone(),
                kind: "decouple".into(),
                target: c.name.clone(),
                detail: format!(
                    "{} dependents (fan-in hub), {} LOC. Extract a thin trait/types crate so dependents don't pull the whole implementation.",
                    c.dependents.len(),
                    c.loc
                ),
                impact,
                effort: "high".into(),
                est_minutes: 240,
            });
        }
    }

    // RANK by impact (biggest problems first); among equal impact prefer the quicker win
    // (fewer est_minutes), then crate name for a stable, deterministic order.
    tasks.sort_by(|a, b| {
        task_score(b)
            .partial_cmp(&task_score(a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.est_minutes.cmp(&b.est_minutes))
            .then(a.crate_name.cmp(&b.crate_name))
    });
    for (i, t) in tasks.iter_mut().enumerate() {
        t.rank = i + 1;
    }
    tasks
}

// ───────────────────────── PROTOTYPE-2: actionable god-file SPLIT PREVIEW ─────────────────────────
//
// Prototype-1 STOPS at "split this god-file". Prototype-2 turns that into a concrete plan: read the
// file, find its top-level items, and group them into ~250-LOC modules so a human (or an apply step)
// has an actual breakdown to execute. Propose-only — never writes.

/// One proposed module of a god-file split.
#[derive(Debug, Clone, PartialEq)]
pub struct ModulePlan {
    /// suggested module name (snake_case), derived from the items it holds
    pub name: String,
    /// the top-level item names that move into this module
    pub items: Vec<String>,
    /// approximate LOC this module would carry
    pub est_loc: usize,
}

/// Extract a top-level Rust item name from a (non-indented) source line.
fn item_name(line: &str) -> Option<String> {
    if line.starts_with(char::is_whitespace) {
        return None; // top-level (column 0) declarations only
    }
    for kw in [
        "pub async fn ", "async fn ", "pub fn ", "fn ",
        "pub struct ", "struct ", "pub enum ", "enum ",
        "pub trait ", "trait ", "pub type ", "type ",
    ] {
        if let Some(rest) = line.strip_prefix(kw) {
            let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

/// Name a module after the longest shared `_`-delimited prefix of its items (e.g. `render_text` +
/// `render_report` → `render`), falling back to the first item.
fn module_name_for(items: &[String]) -> String {
    let first = items.first().cloned().unwrap_or_else(|| "part".into());
    let prefix = first.split('_').next().unwrap_or(&first);
    if items.iter().all(|i| i.starts_with(prefix)) && prefix.len() >= 2 {
        prefix.to_string()
    } else {
        first
    }
}

/// PURE: given a god-file's source, propose a module split (~250 LOC per module). Empty when
/// there's nothing worth splitting (<2 top-level items). Testable without touching the filesystem.
pub fn split_modules_from_source(src: &str) -> Vec<ModulePlan> {
    let lines: Vec<&str> = src.lines().collect();
    let total = lines.len();
    let items: Vec<(usize, String)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| item_name(l).map(|n| (i, n)))
        .collect();
    if items.len() < 2 {
        return Vec::new();
    }
    const TARGET_LOC: usize = 250;
    let mut mods: Vec<ModulePlan> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut start = items[0].0;
    for k in 0..items.len() {
        let next = if k + 1 < items.len() { items[k + 1].0 } else { total };
        cur.push(items[k].1.clone());
        let loc = next - start;
        if loc >= TARGET_LOC || k + 1 == items.len() {
            mods.push(ModulePlan { name: module_name_for(&cur), items: std::mem::take(&mut cur), est_loc: loc });
            start = next;
        }
    }
    // de-dup module names (render, render → render, render_2)
    let mut seen = std::collections::HashMap::<String, usize>::new();
    for m in &mut mods {
        let n = seen.entry(m.name.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            m.name = format!("{}_{}", m.name, *n);
        }
    }
    mods
}

/// Read a god-file and propose its split (see [`split_modules_from_source`]). Propose-only.
pub fn preview_split(path: &str) -> std::io::Result<Vec<ModulePlan>> {
    let src = std::fs::read_to_string(path)?;
    Ok(split_modules_from_source(&src))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GodFile, LegacyCrate};

    fn report() -> LegacyReport {
        let crates = vec![
            // a huge hub god-file crate — high LOC, 6 dependents
            LegacyCrate { name: "core".into(), path: "c/core".into(), loc: 3200, file_count: 4,
                biggest_file: "c/core/src/lib.rs".into(), biggest_file_loc: 2400, pub_fns: 40, pub_types: 12,
                has_tests: true, deps: vec![],
                dependents: vec!["a".into(),"b".into(),"d".into(),"e".into(),"f".into(),"g".into()] },
            // a big UNTESTED crate
            LegacyCrate { name: "util".into(), path: "c/util".into(), loc: 1500, file_count: 3,
                biggest_file: "c/util/src/lib.rs".into(), biggest_file_loc: 700, pub_fns: 20, pub_types: 5,
                has_tests: false, deps: vec!["core".into()], dependents: vec![] },
            // a small tested leaf — no task
            LegacyCrate { name: "leaf".into(), path: "c/leaf".into(), loc: 120, file_count: 1,
                biggest_file: "c/leaf/src/lib.rs".into(), biggest_file_loc: 120, pub_fns: 3, pub_types: 1,
                has_tests: true, deps: vec!["util".into()], dependents: vec![] },
        ];
        LegacyReport {
            root: "/repo".into(), workspace_name: "demo".into(), crate_count: 3, total_loc: 4820,
            crates,
            god_files: vec![GodFile { crate_name: "core".into(), file: "c/core/src/lib.rs".into(), loc: 2400 }],
            analyze_ms: 5,
        }
    }

    #[test]
    fn god_file_with_high_fan_in_ranks_first() {
        let plan = refactor_plan(&report());
        assert!(!plan.is_empty());
        assert_eq!(plan[0].rank, 1);
        assert_eq!(plan[0].kind, "split-god-file");
        assert_eq!(plan[0].crate_name, "core");
        assert!(plan.iter().all(|t| (0.0..=1.0).contains(&t.impact)), "impact stays in [0,1]");
    }

    #[test]
    fn fan_in_lifts_god_file_impact() {
        // same file, more dependents → strictly higher impact (the enhancement over the baseline)
        let mut lo = report();
        lo.crates[0].dependents = vec!["a".into()];
        let hi = report(); // 6 dependents
        let i_lo = refactor_plan(&lo).into_iter().find(|t| t.kind == "split-god-file").unwrap().impact;
        let i_hi = refactor_plan(&hi).into_iter().find(|t| t.kind == "split-god-file").unwrap().impact;
        assert!(i_hi >= i_lo, "more fan-in must not lower a god-file's impact ({i_hi} vs {i_lo})");
    }

    #[test]
    fn untested_big_crate_gets_add_tests_and_leaf_gets_nothing() {
        let plan = refactor_plan(&report());
        assert!(plan.iter().any(|t| t.kind == "add-tests" && t.crate_name == "util"));
        assert!(!plan.iter().any(|t| t.crate_name == "leaf"), "small tested leaf needs no work");
    }

    #[test]
    fn high_fan_in_hub_gets_a_decouple_task() {
        let plan = refactor_plan(&report());
        assert!(plan.iter().any(|t| t.kind == "decouple" && t.crate_name == "core"));
    }

    #[test]
    fn ranks_dense_and_score_descending() {
        let plan = refactor_plan(&report());
        for (i, t) in plan.iter().enumerate() {
            assert_eq!(t.rank, i + 1, "ranks are 1..N, dense");
        }
        for w in plan.windows(2) {
            assert!(task_score(&w[0]) >= task_score(&w[1]) - 1e-9, "sorted by impact×(1/effort) desc");
        }
    }

    #[test]
    fn empty_report_empty_plan() {
        assert!(refactor_plan(&LegacyReport::default()).is_empty());
    }
}
