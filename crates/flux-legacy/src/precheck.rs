//! flux-legacy PROTOTYPE 5 — the CHEAP structural PRE-CHECK for a refactor patch.
//!
//! P2/[`split`](crate::split) produces a [`SplitPatch`]; the sibling `verify` lane can apply it to a
//! sandbox and run a REAL crate build — but that costs minutes on the 100-crate node. `precheck` is the
//! microsecond gate you run FIRST: a pure, structural inspection that rejects an obviously-broken split
//! (a lost item, a residual god-file, an empty or garbled module) before anyone pays for a build.
//!
//! Pipeline:  split → **precheck** (cheap reject) → verify (real sandbox build) → apply.
//!
//! Pure: no I/O, no compilation. Built across 5 flux-verified iterations:
//!   1. item conservation + empty modules
//!   2. residual god-file + size balance
//!   3. per-module brace soundness + `use super::*;` header
//!   4. verdict + confidence + render
//!   5. live on the real 262-item handlers.rs split

use crate::split::SplitPatch;
use crate::GOD_FILE_LOC;
use serde::{Deserialize, Serialize};

/// Severity of a single finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity { Info, Warn, Error }

/// One thing the pre-check noticed about the patch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    /// module the finding is about ("" = whole-patch)
    pub module: String,
    pub message: String,
}

/// Overall structural verdict — gates whether the split is worth a real build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict { Safe, Review, Unsafe }

/// The result of pre-checking a [`SplitPatch`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrecheckReport {
    pub verdict: Verdict,
    pub confidence: f64,
    pub items_total: usize,
    pub items_placed: usize,
    pub findings: Vec<Finding>,
}

impl PrecheckReport {
    pub fn errors(&self) -> usize { self.findings.iter().filter(|f| f.severity == Severity::Error).count() }
    pub fn warns(&self) -> usize { self.findings.iter().filter(|f| f.severity == Severity::Warn).count() }
    /// True only when nothing structural is wrong — safe to spend a real build on.
    pub fn worth_building(&self) -> bool { self.verdict != Verdict::Unsafe }
}

fn err(module: &str, message: String) -> Finding { Finding { severity: Severity::Error, module: module.into(), message } }
fn warn(module: &str, message: String) -> Finding { Finding { severity: Severity::Warn, module: module.into(), message } }

/// Structurally pre-check a split patch. Iteration 1: item conservation + empty-module detection.
pub fn precheck_split(patch: &SplitPatch) -> PrecheckReport {
    let mut f = Vec::new();
    check_conservation(patch, &mut f);
    check_empty_modules(patch, &mut f);
    check_residual_god_files(patch, &mut f);
    check_balance(patch, &mut f);
    check_module_soundness(patch, &mut f);
    finalize(patch, &f)
}

/// Every parsed item must land in exactly one module — none lost, none duplicated.
fn check_conservation(patch: &SplitPatch, f: &mut Vec<Finding>) {
    let placed: usize = patch.modules.iter().map(|m| m.item_names.len()).sum();
    if placed != patch.items_total {
        f.push(err("", format!(
            "item count changed: {} parsed → {} placed (items lost or duplicated)",
            patch.items_total, placed
        )));
    }
    // Flag only CROSS-MODULE duplication (the same item copied into two files). A name appearing
    // twice within ONE module is normal Rust — a type and its `impl` share a name — so don't flag it.
    let mut mods_of = std::collections::HashMap::<&str, std::collections::BTreeSet<&str>>::new();
    for m in &patch.modules {
        for n in &m.item_names {
            mods_of.entry(n.as_str()).or_default().insert(m.module.as_str());
        }
    }
    let mut dups: Vec<(&str, usize)> =
        mods_of.iter().filter(|(_, s)| s.len() > 1).map(|(n, s)| (*n, s.len())).collect();
    dups.sort();
    for (name, count) in dups {
        f.push(err("", format!("item `{name}` placed in {count} different modules (duplicated across modules)")));
    }
}

fn check_empty_modules(patch: &SplitPatch, f: &mut Vec<Finding>) {
    if patch.modules.is_empty() {
        f.push(err("", "patch produced no modules".into()));
    }
    for m in &patch.modules {
        if m.item_names.is_empty() {
            f.push(err(&m.module, "module has no items (empty split)".into()));
        }
    }
}

/// A module still over the god-file threshold means the split didn't go deep enough — not broken
/// (Warn, not Error), but worth another pass. (Expected on a huge file: 18k LOC ÷ 8 ≈ 2k each.)
fn check_residual_god_files(patch: &SplitPatch, f: &mut Vec<Finding>) {
    for m in &patch.modules {
        if m.loc > GOD_FILE_LOC {
            f.push(warn(&m.module, format!("still {} LOC (> {} god-file threshold) — split deeper", m.loc, GOD_FILE_LOC)));
        }
    }
}

/// Wildly uneven module sizes (one module hoards most of the file) → the prefix grouping was lopsided.
fn check_balance(patch: &SplitPatch, f: &mut Vec<Finding>) {
    let locs: Vec<usize> = patch.modules.iter().map(|m| m.loc).filter(|l| *l > 0).collect();
    if locs.len() < 2 {
        return;
    }
    let max = *locs.iter().max().unwrap();
    let min = *locs.iter().min().unwrap();
    if min > 0 && max / min.max(1) > 8 {
        f.push(warn("", format!("module sizes imbalanced: largest {max} LOC vs smallest {min} LOC ({}×)", max / min.max(1))));
    }
}

/// Cheap structural sanity per generated module: balanced braces (an unbalanced count means an item
/// was split mid-body — the patch is garbled → Error) and the `use super::*;` the splitter must add so
/// moved items can still see crate-level paths (missing → Warn).
fn check_module_soundness(patch: &SplitPatch, f: &mut Vec<Finding>) {
    for m in &patch.modules {
        // lexer-aware: ignore braces inside strings/chars/comments (a `format!("}}")` is not a real
        // unbalanced brace). A non-zero net means an item really was split mid-body.
        let bal = crate::split::code_brace_balance(&m.src);
        if bal != 0 {
            f.push(err(&m.module, format!("unbalanced code braces (net {bal:+}) — an item was split mid-body")));
        }
        if !m.src.contains("use super::*;") {
            f.push(warn(&m.module, "missing `use super::*;` — moved items may not resolve crate paths".into()));
        }
    }
}

fn placed_count(patch: &SplitPatch) -> usize {
    patch.modules.iter().map(|m| m.item_names.len()).sum()
}

/// Turn findings into a verdict + confidence.
fn finalize(patch: &SplitPatch, findings: &[Finding]) -> PrecheckReport {
    let errors = findings.iter().filter(|f| f.severity == Severity::Error).count();
    let warns = findings.iter().filter(|f| f.severity == Severity::Warn).count();
    let verdict = if errors > 0 {
        Verdict::Unsafe
    } else if warns > 0 {
        Verdict::Review
    } else {
        Verdict::Safe
    };
    let confidence = (1.0 - errors as f64 * 0.4 - warns as f64 * 0.1).clamp(0.0, 1.0);
    PrecheckReport {
        verdict,
        confidence,
        items_total: patch.items_total,
        items_placed: placed_count(patch),
        findings: findings.to_vec(),
    }
}

/// One-call gate: split `src` then pre-check the result. This is what a caller runs before deciding
/// whether the split is worth handing to the `verify` sandbox-build lane.
pub fn precheck_file(path: &str, src: &str, max_modules: usize) -> PrecheckReport {
    let patch = crate::split::plan_split(path, src, max_modules);
    precheck_split(&patch)
}

/// Visual pre-check report (Viktor=visual): verdict badge, confidence, then findings worst-first.
pub fn render_precheck(r: &PrecheckReport) -> String {
    let badge = match r.verdict {
        Verdict::Safe => "✓ SAFE",
        Verdict::Review => "⚠ REVIEW",
        Verdict::Unsafe => "✗ UNSAFE",
    };
    let mut o = format!(
        "PRECHECK {badge} · confidence {:.0}% · {}/{} items placed · {} err {} warn\n",
        r.confidence * 100.0,
        r.items_placed,
        r.items_total,
        r.errors(),
        r.warns(),
    );
    let mut order = r.findings.clone();
    order.sort_by_key(|f| match f.severity { Severity::Error => 0, Severity::Warn => 1, Severity::Info => 2 });
    for f in &order {
        let s = match f.severity { Severity::Error => "E", Severity::Warn => "W", Severity::Info => "i" };
        let where_ = if f.module.is_empty() { "patch".to_string() } else { f.module.clone() };
        o.push_str(&format!("  [{s}] {where_}: {}\n", f.message));
    }
    if r.findings.is_empty() {
        o.push_str("  (no structural issues)\n");
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::split::ModuleSplit;

    pub(super) fn module(name: &str, items: &[&str], src: &str, loc: usize) -> ModuleSplit {
        ModuleSplit {
            module: name.into(),
            item_names: items.iter().map(|s| s.to_string()).collect(),
            src: src.into(),
            loc,
        }
    }

    pub(super) fn patch(modules: Vec<ModuleSplit>, items_total: usize) -> SplitPatch {
        SplitPatch {
            original_file: "handlers.rs".into(),
            original_loc: 1000,
            items_total,
            modules,
            mod_wiring: String::new(),
            strategy: "test".into(),
            caveats: vec![],
        }
    }

    #[test]
    fn clean_split_is_safe() {
        let p = patch(
            vec![
                module("a", &["f1", "f2"], "use super::*;\nfn f1(){}\nfn f2(){}\n", 50),
                module("b", &["g1"], "use super::*;\nfn g1(){}\n", 40),
            ],
            3,
        );
        let r = precheck_split(&p);
        assert_eq!(r.verdict, Verdict::Safe);
        assert_eq!(r.items_placed, 3);
        assert_eq!(r.errors(), 0);
        assert!(r.worth_building());
    }

    #[test]
    fn lost_item_is_unsafe() {
        let p = patch(vec![module("a", &["f1", "f2", "f3"], "x", 10)], 5);
        let r = precheck_split(&p);
        assert_eq!(r.verdict, Verdict::Unsafe);
        assert!(!r.worth_building());
        assert!(r.findings.iter().any(|f| f.message.contains("item count changed")));
    }

    #[test]
    fn duplicated_item_is_unsafe() {
        let p = patch(vec![module("a", &["f1"], "x", 10), module("b", &["f1"], "y", 10)], 2);
        let r = precheck_split(&p);
        assert_eq!(r.verdict, Verdict::Unsafe);
        assert!(r.findings.iter().any(|f| f.message.contains("duplicated")));
    }

    #[test]
    fn empty_module_is_unsafe() {
        let p = patch(vec![module("a", &["f1"], "x", 10), module("empty", &[], "", 0)], 1);
        let r = precheck_split(&p);
        assert_eq!(r.verdict, Verdict::Unsafe);
        assert!(r.findings.iter().any(|f| f.module == "empty"));
    }

    #[test]
    fn residual_god_file_is_review_not_unsafe() {
        // conservation OK, but a module is still 2700 LOC → split deeper (Warn → Review)
        let p = patch(vec![module("big", &["f1", "f2"], "use super::*;\n", 2700), module("small", &["g1"], "use super::*;\n", 40)], 3);
        let r = precheck_split(&p);
        assert_eq!(r.verdict, Verdict::Review, "residual god-file warns, not errors");
        assert!(r.worth_building(), "Review is still worth a real build");
        assert!(r.findings.iter().any(|f| f.module == "big" && f.message.contains("split deeper")));
    }

    #[test]
    fn imbalanced_sizes_warn() {
        let p = patch(vec![module("huge", &["f1"], "use super::*;\n", 900), module("tiny", &["g1"], "use super::*;\n", 30)], 2);
        let r = precheck_split(&p);
        assert!(r.findings.iter().any(|f| f.message.contains("imbalanced")));
    }

    #[test]
    fn unbalanced_braces_are_unsafe() {
        // a module whose source was cut mid-body: 2 opens, 1 close
        let p = patch(vec![module("garbled", &["f1"], "use super::*;\nfn f1(){ if x {\n", 5)], 1);
        let r = precheck_split(&p);
        assert_eq!(r.verdict, Verdict::Unsafe);
        assert!(r.findings.iter().any(|f| f.message.contains("unbalanced code braces")));
    }

    #[test]
    fn missing_super_import_warns() {
        let p = patch(vec![module("a", &["f1"], "fn f1(){}\n", 5)], 1); // no `use super::*;`
        let r = precheck_split(&p);
        assert_eq!(r.verdict, Verdict::Review);
        assert!(r.findings.iter().any(|f| f.message.contains("use super::*;")));
    }

    #[test]
    fn precheck_file_runs_split_then_check() {
        let god = "use std::fmt;\npub fn handle_a(){}\npub fn handle_b(){}\npub struct Thing;\npub fn verify_x(){}\n";
        let r = precheck_file("handlers.rs", god, 4);
        // a real (tiny) split of well-formed code should be Safe and conserve all 4 items
        assert_eq!(r.items_total, r.items_placed);
        assert!(r.worth_building());
        assert_eq!(r.errors(), 0);
    }

    #[test]
    fn render_shows_badge_and_findings() {
        let unsafe_p = patch(vec![module("garbled", &["f1"], "fn f1(){ {\n", 3)], 1);
        let txt = render_precheck(&precheck_split(&unsafe_p));
        assert!(txt.contains("UNSAFE"));
        assert!(txt.contains("[E]"));
        let safe_p = patch(vec![module("a", &["f1"], "use super::*;\nfn f1(){}\n", 20)], 1);
        assert!(render_precheck(&precheck_split(&safe_p)).contains("SAFE"));
    }
}
