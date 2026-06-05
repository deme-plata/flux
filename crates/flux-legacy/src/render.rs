//! LEGACY-3 — render a [`LegacyReport`] (+ the [`plan`](crate::plan) lane's [`RefactorTask`]s)
//! for human eyes. Viktor is a visual learner, so the text view leads with aligned tables, a
//! one-glance header, comma'd numbers and impact bars; `render_json` is the machine view (MCP / CI).
//! Pure: no I/O, no network, `std` + `serde_json` only — feed it a report, get a `String`.
//!
//! (Enhanced from rocky-vision's baseline: comma formatting, impact bars, coupling-hubs section,
//! explicit LOC sort so output doesn't depend on analyze.rs ordering, `render_report` combiner,
//! `render_plan_json`. Agreed signatures `render_text`/`render_json`/`render_plan` are preserved.)

use crate::{LegacyCrate, LegacyReport, RefactorTask, GOD_FILE_LOC};

// ───────────────────────── small formatting helpers ─────────────────────────

/// Thousands-separated integer: `162967` → `"162,967"`.
fn commas(n: usize) -> String {
    let s = n.to_string();
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in b.iter().enumerate() {
        if i > 0 && (b.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*c as char);
    }
    out
}

/// Truncate to `n` chars with an ellipsis so columns never blow out.
fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else if n <= 1 {
        "…".to_string()
    } else {
        let keep: String = s.chars().take(n - 1).collect();
        format!("{keep}…")
    }
}

/// A 0–1 impact rendered as a `width`-cell bar: `0.75, 8` → `"██████··"`.
fn bar(impact: f64, width: usize) -> String {
    let f = impact.clamp(0.0, 1.0);
    let filled = (f * width as f64).round() as usize;
    let mut s = String::with_capacity(width * 3);
    for i in 0..width {
        s.push_str(if i < filled { "█" } else { "·" });
    }
    s
}

/// Just the file name (drop the dir) for compact tables.
fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

// ───────────────────────── text report ─────────────────────────

/// The full human report: header + biggest crates + god-files + coupling hubs. If you also have the
/// plan lane's tasks, prefer [`render_report`] (it appends the ranked targets).
pub fn render_text(report: &LegacyReport) -> String {
    let mut o = String::new();
    render_header(report, &mut o);
    render_biggest_crates(report, &mut o, 20);
    render_god_files(report, &mut o, 15);
    render_fan_in(report, &mut o, 12);
    o
}

/// Header + everything in [`render_text`] + the ranked refactor targets from the plan lane.
/// This is what the `flux-legacy` bin (LEGACY-4) pastes to the bus.
pub fn render_report(report: &LegacyReport, tasks: &[RefactorTask]) -> String {
    let mut o = render_text(report);
    o.push_str(&render_plan(tasks));
    o
}

fn render_header(r: &LegacyReport, o: &mut String) {
    let untested = r.crates.iter().filter(|c| !c.has_tests).count();
    let name = if r.workspace_name.is_empty() { "(workspace)" } else { &r.workspace_name };
    o.push_str("╔══════════════════════════════════════════════════════════════════════╗\n");
    o.push_str(&format!("  FLUX-LEGACY · {name}\n"));
    o.push_str(&format!("  root: {}\n", trunc(&r.root, 66)));
    o.push_str(&format!(
        "  {} crates · {} LOC · {} god-files (>{} LOC) · {} untested · {}ms\n",
        commas(r.crate_count),
        commas(r.total_loc),
        commas(r.god_files.len()),
        GOD_FILE_LOC,
        commas(untested),
        r.analyze_ms,
    ));
    o.push_str("╚══════════════════════════════════════════════════════════════════════╝\n");
}

fn render_biggest_crates(r: &LegacyReport, o: &mut String, top: usize) {
    o.push_str("\n▼ BIGGEST CRATES — by LOC (the surface area to tame)\n");
    o.push_str(&format!(
        "  {:<3} {:<24} {:>9} {:>5} {:>9} {:>5} {:>5} {:>5} {:>6}\n",
        "#", "crate", "LOC", "files", "biggest", "pfn", "pty", "test", "fan-in",
    ));
    let mut crates: Vec<&LegacyCrate> = r.crates.iter().collect();
    crates.sort_by(|a, b| b.loc.cmp(&a.loc));
    for (i, c) in crates.iter().take(top).enumerate() {
        o.push_str(&format!(
            "  {:<3} {:<24} {:>9} {:>5} {:>9} {:>5} {:>5} {:>5} {:>6}\n",
            i + 1,
            trunc(&c.name, 24),
            commas(c.loc),
            c.file_count,
            commas(c.biggest_file_loc),
            c.pub_fns,
            c.pub_types,
            if c.has_tests { "✓" } else { "✗" },
            c.dependents.len(),
        ));
    }
}

fn render_god_files(r: &LegacyReport, o: &mut String, top: usize) {
    if r.god_files.is_empty() {
        return;
    }
    o.push_str(&format!("\n▼ GOD FILES — single .rs > {GOD_FILE_LOC} LOC (split candidates)\n"));
    o.push_str(&format!("  {:>9}  {:<22} {}\n", "LOC", "crate", "file"));
    let mut g: Vec<&crate::GodFile> = r.god_files.iter().collect();
    g.sort_by(|a, b| b.loc.cmp(&a.loc));
    for f in g.iter().take(top) {
        o.push_str(&format!(
            "  {:>9}  {:<22} {}\n",
            commas(f.loc),
            trunc(&f.crate_name, 22),
            trunc(basename(&f.file), 40),
        ));
    }
}

fn render_fan_in(r: &LegacyReport, o: &mut String, top: usize) {
    let mut hubs: Vec<&LegacyCrate> = r.crates.iter().filter(|c| !c.dependents.is_empty()).collect();
    if hubs.is_empty() {
        return;
    }
    hubs.sort_by(|a, b| b.dependents.len().cmp(&a.dependents.len()));
    o.push_str("\n▼ COUPLING HUBS — most-depended-on (change-risk: edits ripple)\n");
    o.push_str(&format!("  {:>6}  {:<24} {}\n", "fan-in", "crate", "dependents"));
    for c in hubs.iter().take(top) {
        o.push_str(&format!(
            "  {:>6}  {:<24} {}\n",
            c.dependents.len(),
            trunc(&c.name, 24),
            trunc(&c.dependents.join(", "), 40),
        ));
    }
}

/// The ranked refactor targets from the plan lane — top 20, the do-this-first list.
pub fn render_plan(tasks: &[RefactorTask]) -> String {
    let mut o = String::new();
    if tasks.is_empty() {
        o.push_str("\n▼ TOP REFACTOR TARGETS — (none: plan lane produced no tasks)\n");
        return o;
    }
    o.push_str("\n▼ TOP REFACTOR TARGETS — ranked by impact (do these first)\n");
    o.push_str(&format!(
        "  {:<3} {:<22} {:<15} {:<10} {:<7} {:>5}  {}\n",
        "#", "crate", "kind", "impact", "effort", "~min", "target",
    ));
    let mut t: Vec<&RefactorTask> = tasks.iter().collect();
    t.sort_by(|a, b| a.rank.cmp(&b.rank));
    for task in t.iter().take(20) {
        o.push_str(&format!(
            "  {:<3} {:<22} {:<15} {:<10} {:<7} {:>5}  {}\n",
            task.rank,
            trunc(&task.crate_name, 22),
            trunc(&task.kind, 15),
            bar(task.impact, 8),
            trunc(&task.effort, 7),
            task.est_minutes,
            trunc(&task.target, 34),
        ));
    }
    o
}

// ───────────────────────── json report ─────────────────────────

/// The machine view of the analysis (MCP / CI / dashboards). Pretty-printed for diff-ability.
pub fn render_json(report: &LegacyReport) -> String {
    serde_json::to_string_pretty(report).unwrap_or_else(|e| format!("{{\"error\":\"serialize report: {e}\"}}"))
}

/// The plan lane's ranked tasks as JSON.
pub fn render_plan_json(tasks: &[RefactorTask]) -> String {
    serde_json::to_string_pretty(tasks).unwrap_or_else(|e| format!("{{\"error\":\"serialize tasks: {e}\"}}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GodFile, LegacyCrate};

    /// Fixture grounded in the REAL measured numbers from the 100-crate Quillon Graph node
    /// (q-narwhalknight-src, read-only `find | wc -l` pass).
    fn quillon_fixture() -> LegacyReport {
        let mk = |name: &str, loc: usize, big: usize, tests: bool, deps: &[&str], depd: &[&str]| LegacyCrate {
            name: name.into(),
            path: format!("crates/{name}"),
            loc,
            file_count: 12,
            biggest_file: format!("crates/{name}/src/lib.rs"),
            biggest_file_loc: big,
            pub_fns: 40,
            pub_types: 12,
            has_tests: tests,
            deps: deps.iter().map(|s| s.to_string()).collect(),
            dependents: depd.iter().map(|s| s.to_string()).collect(),
        };
        LegacyReport {
            root: "/home/orobit/q-narwhalknight-src".into(),
            workspace_name: "q-narwhalknight".into(),
            crate_count: 100,
            total_loc: 941_087,
            crates: vec![
                mk("q-api-server", 162_967, 14_000, false, &["q-types", "q-storage"], &[]),
                mk("q-storage", 99_139, 9_000, true, &["q-types"], &["q-api-server", "q-vm"]),
                mk("q-vm", 60_000, 7_500, false, &["q-types", "q-storage"], &["q-api-server"]),
                mk("q-network", 52_024, 5_000, true, &["q-types"], &["q-api-server"]),
                mk("q-types", 22_020, 1_200, true, &[], &["q-api-server", "q-storage", "q-vm", "q-network"]),
            ],
            god_files: vec![
                GodFile { crate_name: "q-api-server".into(), file: "crates/q-api-server/src/handlers.rs".into(), loc: 14_000 },
                GodFile { crate_name: "q-storage".into(), file: "crates/q-storage/src/turbo_sync.rs".into(), loc: 9_000 },
            ],
            analyze_ms: 1234,
        }
    }

    #[test]
    fn text_report_has_sections_and_formatted_numbers() {
        let out = render_text(&quillon_fixture());
        assert!(out.contains("FLUX-LEGACY · q-narwhalknight"));
        assert!(out.contains("100 crates"));
        assert!(out.contains("941,087 LOC"), "thousands separator in header");
        assert!(out.contains("BIGGEST CRATES"));
        assert!(out.contains("GOD FILES"));
        assert!(out.contains("COUPLING HUBS"));
        assert!(out.contains("q-api-server"));
        assert!(out.contains("162,967"));
        assert!(out.contains("q-types"));
    }

    #[test]
    fn biggest_crates_sorted_desc_even_if_input_unsorted() {
        // input order deliberately NOT by LOC — render must still put the 163k crate first
        let mut r = quillon_fixture();
        r.crates.reverse();
        let out = render_text(&r);
        let api = out.find("q-api-server").unwrap();
        let storage = out.find("q-storage").unwrap();
        assert!(api < storage, "biggest crate should appear first regardless of input order");
    }

    #[test]
    fn plan_renders_ranked_targets_with_bars() {
        let tasks = vec![
            RefactorTask { rank: 1, crate_name: "q-api-server".into(), kind: "split-god-file".into(),
                target: "handlers.rs → 8 modules".into(), detail: "14k LOC handler god-file".into(),
                impact: 0.95, effort: "high".into(), est_minutes: 480 },
            RefactorTask { rank: 2, crate_name: "q-vm".into(), kind: "add-tests".into(),
                target: "q-vm".into(), detail: "60k LOC, no tests".into(),
                impact: 0.6, effort: "medium".into(), est_minutes: 180 },
        ];
        let out = render_plan(&tasks);
        assert!(out.contains("TOP REFACTOR TARGETS"));
        assert!(out.contains("split-god-file"));
        assert!(out.contains("█"), "impact bar rendered");
        assert!(out.find("q-api-server").unwrap() < out.find("q-vm").unwrap(), "rank order preserved");
    }

    #[test]
    fn render_report_combines_analysis_and_plan() {
        let out = render_report(&quillon_fixture(), &[]);
        assert!(out.contains("BIGGEST CRATES"));
        assert!(out.contains("TOP REFACTOR TARGETS"));
    }

    #[test]
    fn empty_plan_is_handled() {
        assert!(render_plan(&[]).contains("none"));
    }

    #[test]
    fn json_roundtrips() {
        let j = render_json(&quillon_fixture());
        let back: LegacyReport = serde_json::from_str(&j).expect("valid json");
        assert_eq!(back.crate_count, 100);
        assert_eq!(back.total_loc, 941_087);
        assert_eq!(back.crates.len(), 5);
    }

    #[test]
    fn commas_and_bar_helpers() {
        assert_eq!(commas(162_967), "162,967");
        assert_eq!(commas(0), "0");
        assert_eq!(commas(999), "999");
        assert_eq!(commas(1_000), "1,000");
        assert_eq!(bar(1.0, 8).chars().filter(|c| *c == '█').count(), 8);
        assert_eq!(bar(0.0, 8).chars().filter(|c| *c == '█').count(), 0);
    }
}
