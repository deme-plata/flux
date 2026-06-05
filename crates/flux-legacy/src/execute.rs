// execute.rs — Flux-Legacy Prototype 2 EXECUTOR

use crate::RefactorTask;
use serde::{Deserialize, Serialize};

/// A concise, executable brief for the build pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefactorBrief {
    pub rank: usize,
    pub crate_name: String,
    pub kind: String,
    pub target: String,
    pub prompt: String,
    pub acceptance: String,
    pub est_minutes: u64,
    pub budget_usd: f64,
}

/// Converts a ranked `RefactorTask` into a `RefactorBrief` with a kind-specific
/// instruction prompt and acceptance criterion.
pub fn brief_for(rt: &RefactorTask) -> RefactorBrief {
    let budget_usd = match rt.effort.as_str() {
        "high" => 0.05,
        "medium" => 0.02,
        _ => 0.01,
    };

    // Compose prompt and acceptance based on kind
    let (prompt, acceptance) = match rt.kind.as_str() {
        "split-god-file" => (
            format!(
                "Split the god file `{}` in crate `{}`. \
                 The file contains multiple responsibilities. \
                 Extract the following concern into a separate module: {}. \
                 Keep existing public API unchanged. Write idiomatic Rust.",
                rt.target, rt.crate_name, rt.detail
            ),
            format!(
                "APPROVE only if the god file `{}` has been successfully split: \
                 the original file no longer contains the extracted logic, and \
                 all existing tests still pass after minimal import adjustments.",
                rt.target
            ),
        ),
        "add-tests" => (
            format!(
                "Add integration and unit tests for `{}` in crate `{}`. \
                 Focus on: {}. \
                 Use `#[cfg(test)]`, `cargo test`, and `assert!` / `assert_eq!`. \
                 Do not modify production code or dependencies.",
                rt.target, rt.crate_name, rt.detail
            ),
            format!(
                "APPROVE only if the new tests cover the scenario described in `{}` \
                 and `cargo test` passes.",
                rt.detail
            ),
        ),
        "decouple" => (
            format!(
                "Decouple the module `{}` in crate `{}` from its current dependencies. \
                 Refactor to use trait abstractions or dependency injection. \
                 Required change: {}. Retain all existing functionality.",
                rt.target, rt.crate_name, rt.detail
            ),
            format!(
                "APPROVE only if `{}` no longer directly depends on the private implementation \
                 it was coupled to, and the refactored code compiles with no warnings.",
                rt.target
            ),
        ),
        _ => (
            format!(
                "Perform a generic refactoring task on `{}` in crate `{}`: {}.",
                rt.target, rt.crate_name, rt.detail
            ),
            "APPROVE only if the refactoring completes without breaking existing functionality."
                .to_string(),
        ),
    };

    RefactorBrief {
        rank: rt.rank,
        crate_name: rt.crate_name.clone(),
        kind: rt.kind.clone(),
        target: rt.target.clone(),
        prompt,
        acceptance,
        est_minutes: rt.est_minutes,
        budget_usd,
    }
}

/// Produces briefs for the first `top_n` ranked tasks.
/// If there are fewer tasks, returns all of them.
pub fn plan_execution(tasks: &[RefactorTask], top_n: usize) -> Vec<RefactorBrief> {
    let n = std::cmp::min(top_n, tasks.len());
    tasks[..n].iter().map(brief_for).collect()
}

/// Summary of a list of briefs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecSummary {
    pub briefs: usize,
    pub est_minutes_total: u64,
    pub budget_usd_total: f64,
}

/// Aggregates statistics from a slice of `RefactorBrief`.
pub fn summarize(briefs: &[RefactorBrief]) -> ExecSummary {
    ExecSummary {
        briefs: briefs.len(),
        est_minutes_total: briefs.iter().map(|b| b.est_minutes).sum(),
        budget_usd_total: briefs.iter().map(|b| b.budget_usd).sum(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RefactorTask;

    fn task(kind: &str, effort: &str, detail: &str, minutes: u64) -> RefactorTask {
        RefactorTask {
            rank: 1,
            crate_name: "test_crate".to_string(),
            kind: kind.to_string(),
            target: "target_item".to_string(),
            detail: detail.to_string(),
            impact: 0.5,
            effort: effort.to_string(),
            est_minutes: minutes,
        }
    }

    #[test]
    fn prompt_split_god_file() {
        let rt = task("split-god-file", "low", "extract parsing module", 30);
        let brief = brief_for(&rt);
        assert!(brief.prompt.contains("Split the god file"));
        assert!(brief.prompt.contains("target_item"));
        assert!(brief.prompt.contains("test_crate"));
        assert!(brief.prompt.contains("extract parsing module"));
        assert!(brief.acceptance.contains("APPROVE only if the god file"));
    }

    #[test]
    fn prompt_add_tests() {
        let rt = task("add-tests", "medium", "edge case: empty input", 15);
        let brief = brief_for(&rt);
        assert!(brief.prompt.contains("Add integration and unit tests"));
        assert!(brief.prompt.contains("target_item"));
        assert!(brief.acceptance.contains("cargo test"));
    }

    #[test]
    fn prompt_decouple() {
        let rt = task("decouple", "high", "replace direct calls with trait", 60);
        let brief = brief_for(&rt);
        assert!(brief.prompt.contains("Decouple the module"));
        assert!(brief.prompt.contains("trait abstractions"));
        assert!(brief.acceptance.contains("no longer directly depends"));
    }

    #[test]
    fn budget_by_effort() {
        assert_eq!(brief_for(&task("split-god-file", "low", "", 10)).budget_usd, 0.01);
        assert_eq!(brief_for(&task("split-god-file", "medium", "", 10)).budget_usd, 0.02);
        assert_eq!(brief_for(&task("split-god-file", "high", "", 10)).budget_usd, 0.05);
        // unknown effort defaults to 0.01
        assert_eq!(brief_for(&task("split-god-file", "unknown", "", 10)).budget_usd, 0.01);
    }

    #[test]
    fn plan_execution_caps_at_top_n() {
        let tasks: Vec<RefactorTask> = (0..5)
            .map(|i| task("add-tests", "low", "test", 10 + i))
            .collect();
        assert_eq!(plan_execution(&tasks, 3).len(), 3);
        assert_eq!(plan_execution(&tasks, 10).len(), 5);
        assert_eq!(plan_execution(&tasks, 0).len(), 0);
    }

    #[test]
    fn summarize_sums_correctly() {
        let briefs = vec![
            RefactorBrief {
                rank: 1,
                crate_name: "a".into(),
                kind: "add-tests".into(),
                target: "x".into(),
                prompt: "".into(),
                acceptance: "".into(),
                est_minutes: 30,
                budget_usd: 0.02,
            },
            RefactorBrief {
                rank: 2,
                crate_name: "b".into(),
                kind: "decouple".into(),
                target: "y".into(),
                prompt: "".into(),
                acceptance: "".into(),
                est_minutes: 45,
                budget_usd: 0.05,
            },
        ];
        let summary = summarize(&briefs);
        assert_eq!(summary.briefs, 2);
        assert_eq!(summary.est_minutes_total, 75);
        assert!((summary.budget_usd_total - 0.07).abs() < 1e-10);
    }

    #[test]
    fn summarize_empty() {
        let summary = summarize(&[]);
        assert_eq!(summary.briefs, 0);
        assert_eq!(summary.est_minutes_total, 0);
        assert!((summary.budget_usd_total).abs() < 1e-10);
    }
}
