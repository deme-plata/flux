//! `fix` — drive Flux builds to ZERO warnings, owned by Flux itself.
//!
//! The engine behind `fluxc fix` and the `flux_fix` MCP combo. It parses
//! rustc/cargo JSON diagnostics (`--message-format=json`) and applies every
//! **machine-applicable** compiler suggestion straight to the source — the
//! deterministic ~90% of warnings (unused imports, `_`-prefix for unused vars,
//! needless `return`, redundant clones, `mut` removal, …). Whatever rustc marks
//! non-machine-applicable is returned as [`UnfixedWarning`] for the AI path
//! ([`crate::qspec`] / `flux_qspec`) to propose. Re-running the build then
//! confirms zero. This is the rustfix algorithm, native to Flux.
//!
//! The combo: `flux_combo` (detect) → `fix::plan_from_json` + `apply_plan` (auto)
//! → `flux_qspec` (AI for the remainder) → re-`flux_combo` (verify zero).

use serde::Deserialize;
use std::collections::BTreeMap;

/// One machine-applicable edit: replace bytes `[start, end)` of `file` with `replacement`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    pub file: String,
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}

/// A warning rustc could not auto-fix — hand to the AI path (`flux_qspec`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnfixedWarning {
    pub file: String,
    pub line: usize,
    pub message: String,
}

/// The result of analysing a build: deterministic [`Fix`]es + the AI remainder.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FixPlan {
    pub fixes: Vec<Fix>,
    pub unfixed: Vec<UnfixedWarning>,
}

impl FixPlan {
    /// Group fixes by file (so each file is read/written once).
    pub fn by_file(&self) -> BTreeMap<String, Vec<Fix>> {
        let mut m: BTreeMap<String, Vec<Fix>> = BTreeMap::new();
        for f in &self.fixes {
            m.entry(f.file.clone()).or_default().push(f.clone());
        }
        m
    }
}

// ── JSON shapes (subset of the rustc diagnostic format) ──────────────────────

#[derive(Deserialize)]
struct CargoLine {
    #[serde(default)]
    reason: String,
    message: Option<Diag>,
}

#[derive(Deserialize)]
struct Diag {
    #[serde(default)]
    level: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    spans: Vec<Span>,
    #[serde(default)]
    children: Vec<Diag>,
}

#[derive(Deserialize)]
struct Span {
    file_name: String,
    byte_start: usize,
    byte_end: usize,
    #[serde(default)]
    line_start: usize,
    #[serde(default)]
    is_primary: bool,
    suggested_replacement: Option<String>,
    #[serde(default)]
    suggestion_applicability: Option<String>,
}

fn collect_fixes(diag: &Diag, out: &mut Vec<Fix>) {
    for s in &diag.spans {
        if let Some(rep) = &s.suggested_replacement {
            if s.suggestion_applicability.as_deref() == Some("MachineApplicable") {
                out.push(Fix {
                    file: s.file_name.clone(),
                    start: s.byte_start,
                    end: s.byte_end,
                    replacement: rep.clone(),
                });
            }
        }
    }
    for c in &diag.children {
        collect_fixes(c, out);
    }
}

/// Build a [`FixPlan`] from cargo `--message-format=json` output (one JSON
/// object per line). Only `warning`-level diagnostics are considered.
pub fn plan_from_json(json: &str) -> FixPlan {
    let mut plan = FixPlan::default();
    for line in json.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<CargoLine>(line) else {
            continue;
        };
        if parsed.reason != "compiler-message" {
            continue;
        }
        let Some(diag) = parsed.message else { continue };
        if diag.level != "warning" {
            continue;
        }
        let before = plan.fixes.len();
        collect_fixes(&diag, &mut plan.fixes);
        if plan.fixes.len() == before {
            // no machine-applicable fix — route to the AI path
            let primary = diag.spans.iter().find(|s| s.is_primary).or(diag.spans.first());
            plan.unfixed.push(UnfixedWarning {
                file: primary.map(|s| s.file_name.clone()).unwrap_or_default(),
                line: primary.map(|s| s.line_start).unwrap_or(0),
                message: diag.message.clone(),
            });
        }
    }
    plan
}

/// Apply a single file's fixes to its source. Fixes are applied by descending
/// byte offset so earlier edits never shift later spans; overlapping fixes are
/// skipped (keep the later-in-file one). Operates on bytes, so it never panics
/// on a char boundary.
pub fn apply(src: &str, mut fixes: Vec<Fix>) -> String {
    fixes.sort_by(|a, b| b.start.cmp(&a.start));
    let mut bytes = src.as_bytes().to_vec();
    let mut boundary = bytes.len(); // lowest start already applied
    for f in fixes {
        if f.start > f.end || f.end > bytes.len() || f.end > boundary {
            continue; // malformed or overlaps an already-applied edit
        }
        bytes.splice(f.start..f.end, f.replacement.bytes());
        boundary = f.start;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Apply a whole plan to the filesystem. Returns `(file, fixes_applied)` per
/// touched file. Files are read once, fixed, and written back.
pub fn apply_plan(plan: &FixPlan) -> std::io::Result<Vec<(String, usize)>> {
    let mut report = Vec::new();
    for (file, fixes) in plan.by_file() {
        let n = fixes.len();
        let src = std::fs::read_to_string(&file)?;
        let fixed = apply(&src, fixes);
        if fixed != src {
            std::fs::write(&file, fixed)?;
        }
        report.push((file, n));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    // a real-shape cargo warning for an unused import, with a MachineApplicable
    // child suggestion that deletes the import (replacement "").
    const UNUSED_IMPORT_JSON: &str = r#"{"reason":"compiler-artifact","target":{"name":"x"}}
{"reason":"compiler-message","message":{"level":"warning","message":"unused import: `std::io`","spans":[{"file_name":"src/x.rs","byte_start":0,"byte_end":12,"line_start":1,"is_primary":true,"suggested_replacement":null,"suggestion_applicability":null}],"children":[{"level":"help","message":"remove the unused import","spans":[{"file_name":"src/x.rs","byte_start":0,"byte_end":13,"line_start":1,"is_primary":true,"suggested_replacement":"","suggestion_applicability":"MachineApplicable"}],"children":[]}]}}
{"reason":"build-finished","success":true}"#;

    #[test]
    fn parses_machine_applicable_fix() {
        let plan = plan_from_json(UNUSED_IMPORT_JSON);
        assert_eq!(plan.fixes.len(), 1, "one machine-applicable fix");
        assert_eq!(plan.unfixed.len(), 0);
        let f = &plan.fixes[0];
        assert_eq!(f.file, "src/x.rs");
        assert_eq!((f.start, f.end), (0, 13));
        assert_eq!(f.replacement, "");
    }

    #[test]
    fn applies_fix_to_source() {
        let src = "use std::io;\nfn main() {}\n";
        let fixed = apply(src, vec![Fix { file: "x".into(), start: 0, end: 13, replacement: String::new() }]);
        assert!(!fixed.contains("use std::io"), "import should be removed: {fixed:?}");
        assert!(fixed.contains("fn main()"));
    }

    #[test]
    fn applies_multiple_fixes_without_offset_corruption() {
        // replace "aaaa" (0..4) and "bbbb" (8..12) — descending apply keeps both correct
        let src = "aaaa--bbbb"; // note: bbbb at 6..10 actually; use exact spans
        let fixed = apply(
            src,
            vec![
                Fix { file: "x".into(), start: 0, end: 4, replacement: "X".into() },
                Fix { file: "x".into(), start: 6, end: 10, replacement: "Y".into() },
            ],
        );
        assert_eq!(fixed, "X--Y");
    }

    #[test]
    fn skips_overlapping_fixes() {
        let src = "0123456789";
        let fixed = apply(
            src,
            vec![
                Fix { file: "x".into(), start: 2, end: 6, replacement: "A".into() },
                Fix { file: "x".into(), start: 4, end: 8, replacement: "B".into() }, // overlaps → skipped
            ],
        );
        // higher start (4..8 → B) applied first, then 2..6 overlaps boundary 4 → skipped
        assert_eq!(fixed, "0123B89");
    }

    #[test]
    fn non_machine_applicable_routes_to_ai() {
        let json = r#"{"reason":"compiler-message","message":{"level":"warning","message":"unreachable pattern","spans":[{"file_name":"src/y.rs","byte_start":40,"byte_end":50,"line_start":7,"is_primary":true,"suggested_replacement":null,"suggestion_applicability":null}],"children":[]}}"#;
        let plan = plan_from_json(json);
        assert_eq!(plan.fixes.len(), 0);
        assert_eq!(plan.unfixed.len(), 1);
        assert_eq!(plan.unfixed[0].file, "src/y.rs");
        assert_eq!(plan.unfixed[0].line, 7);
        assert!(plan.unfixed[0].message.contains("unreachable"));
    }

    #[test]
    fn ignores_errors_and_non_messages() {
        let json = r#"{"reason":"build-finished","success":false}
{"reason":"compiler-message","message":{"level":"error","message":"mismatched types","spans":[],"children":[]}}"#;
        let plan = plan_from_json(json);
        assert!(plan.fixes.is_empty() && plan.unfixed.is_empty());
    }
}
