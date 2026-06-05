//! project.rs — **BETA 2: analyze ANY repo**, not just a Rust cargo workspace.
//!
//! The hospital (triage/psych/plan/render) and the 1M bridge (corpus/ask) all consume a
//! [`LegacyReport`](crate::LegacyReport) or a packed bundle — both originally Rust-cargo-shaped.
//! This module produces the SAME shapes from any language tree (grouping by top-level directory as
//! a "module"), so an imported Python/Go/TS repo flows through the whole pipeline unchanged.
//!
//! [`analyze_auto`] / [`bundle_auto`] route: a Rust cargo workspace keeps the precise crate path;
//! anything else uses the generic walker built on [`lang`](crate::lang).

use crate::{GodFile, LegacyCrate, LegacyReport};
use flux_context::est_tokens;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Is this a Rust cargo workspace (so the precise `analyze_workspace_legacy` applies)?
pub fn is_rust_workspace(root: &str) -> bool {
    let p = PathBuf::from(root);
    p.join("Cargo.toml").is_file() && p.join("crates").is_dir()
}

/// Analyze any repo → a LegacyReport whose "crates" are top-level source directories.
pub fn analyze_project(root: &str) -> LegacyReport {
    let start = std::time::Instant::now();
    let root_path = PathBuf::from(root);
    let survey = crate::lang::survey(root);

    // group source files by their top-level directory under root (the "module")
    let mut modules: BTreeMap<String, Vec<(String, usize)>> = BTreeMap::new();
    for f in walk_source(&root_path) {
        let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("");
        if crate::lang::Language::from_ext(ext) == crate::lang::Language::Other {
            continue;
        }
        let rel = match f.strip_prefix(&root_path) {
            Ok(r) => r.to_string_lossy().to_string(),
            Err(_) => continue,
        };
        let top = rel.split('/').next().unwrap_or(".").to_string();
        let module = if rel.contains('/') { top } else { ".".to_string() };
        let loc = fs::read_to_string(&f).map(|c| c.lines().count()).unwrap_or(0);
        modules.entry(module).or_default().push((rel, loc));
    }

    let mut crates: Vec<LegacyCrate> = modules.into_iter().map(|(name, files)| {
        let loc: usize = files.iter().map(|(_, l)| l).sum();
        let (biggest_file, biggest_file_loc) = files.iter().max_by_key(|(_, l)| *l)
            .map(|(p, l)| (p.clone(), *l)).unwrap_or_default();
        let has_tests = files.iter().any(|(p, _)| {
            let pl = p.to_lowercase();
            pl.contains("test") || pl.contains("spec") || pl.contains("__tests__")
        });
        LegacyCrate {
            name,
            path: root_path.to_string_lossy().to_string(),
            loc,
            file_count: files.len(),
            biggest_file,
            biggest_file_loc,
            pub_fns: 0, // not computed generically (language-specific)
            pub_types: 0,
            has_tests,
            deps: Vec::new(),       // cross-module deps not parsed generically (yet)
            dependents: Vec::new(),
            ..Default::default()
        }
    }).collect();
    crates.sort_by(|a, b| b.loc.cmp(&a.loc));

    let god_files = survey.god_files.iter().map(|(file, loc)| {
        let module = file.split('/').next().unwrap_or(".").to_string();
        GodFile { crate_name: module, file: file.clone(), loc: *loc }
    }).collect();

    LegacyReport {
        root: root.to_string(),
        workspace_name: root_path.file_name().and_then(|n| n.to_str()).unwrap_or("project").to_string(),
        crate_count: crates.len(),
        total_loc: survey.total_loc,
        crates,
        god_files,
        analyze_ms: start.elapsed().as_millis(),
    }
}

/// Route: Rust workspace → the precise analyzer; anything else → the generic one.
pub fn analyze_auto(root: &str) -> LegacyReport {
    if is_rust_workspace(root) {
        crate::analyze_workspace_legacy(root)
    } else {
        analyze_project(root)
    }
}

/// A generic token-budgeted bundle of ANY repo's source (entry files + biggest first, outline the
/// tail), for feeding DeepSeek's 1M window. Returns (bundle, files_included, tokens).
pub fn project_bundle(root: &str, window: u32) -> (String, usize, u32) {
    let root_path = PathBuf::from(root);
    let mut files: Vec<(PathBuf, String, u32, f64)> = Vec::new();
    for f in walk_source(&root_path) {
        let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("");
        if crate::lang::Language::from_ext(ext) == crate::lang::Language::Other {
            continue;
        }
        if let Ok(content) = fs::read_to_string(&f) {
            let toks = est_tokens(&content);
            let rel = f.strip_prefix(&root_path).unwrap_or(&f).to_string_lossy().to_string();
            let prio = file_priority(&rel, toks);
            files.push((f, content, toks, prio));
        }
    }
    files.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal).then(a.2.cmp(&b.2)));

    let mut bundle = format!("// flux-legacy project bundle — {root}\n\n");
    let mut used: u32 = 0;
    let mut included = 0usize;
    for (path, content, toks, _) in &files {
        let rel = path.strip_prefix(&root_path).unwrap_or(path).to_string_lossy();
        if used + toks <= window {
            bundle.push_str(&format!("// ==== {rel} ====\n{content}\n\n"));
            used += toks;
            included += 1;
        } else {
            let o = crate::context::outline(content, 1500);
            let ot = est_tokens(&o);
            if used + ot <= window {
                bundle.push_str(&format!("// ==== {rel} (OUTLINE) ====\n{o}\n\n"));
                used += ot;
                included += 1;
            }
        }
    }
    (bundle, included, used)
}

/// Route: Rust workspace → the ranked corpus bundle; anything else → the generic project bundle.
pub fn bundle_auto(root: &str, window: u32) -> String {
    if is_rust_workspace(root) {
        let pack = crate::corpus::build_corpus(&crate::analyze_workspace_legacy(root), window);
        crate::corpus::bundle_string(&pack)
    } else {
        project_bundle(root, window).0
    }
}

/// Entry/central files first (main/index/lib/mod/__init__/app + smaller = more get in).
fn file_priority(rel: &str, toks: u32) -> f64 {
    let name = Path::new(rel).file_stem().and_then(|n| n.to_str()).unwrap_or("");
    let mut p = 1.0;
    for (stem, boost) in [("lib", 6.0), ("main", 5.0), ("index", 5.0), ("mod", 3.0), ("__init__", 4.0), ("app", 3.0)] {
        if name == stem { p += boost; }
    }
    if rel.to_lowercase().contains("test") { p -= 2.0; } // tests are lower signal for arch review
    p - (toks as f64 / 50_000.0) // mild preference for smaller files so more fit
}

fn walk_source(root: &Path) -> Vec<PathBuf> {
    const SKIP: &[&str] = &[
        ".git", "target", "node_modules", "vendor", "dist", "build", ".venv", "venv",
        "__pycache__", ".gradle", "bin", "obj", ".next", "out", ".cargo",
    ];
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(entries) = fs::read_dir(&d) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !SKIP.contains(&name) { stack.push(p); }
                } else {
                    out.push(p);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, s: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }

    #[test]
    fn analyzes_a_python_repo_into_modules_and_god_files() {
        let tmp = std::env::temp_dir().join(format!("flux-proj-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        write(&tmp.join("pyproject.toml"), "[project]\nname='x'\n");
        write(&tmp.join("src/models.py"), &"def f(): pass\n".repeat(900)); // god-file
        write(&tmp.join("src/utils.py"), &"def g(): pass\n".repeat(100));
        write(&tmp.join("tests/test_x.py"), "def test_a(): pass\n");

        assert!(!is_rust_workspace(tmp.to_str().unwrap()));
        let report = analyze_auto(tmp.to_str().unwrap());
        // modules grouped by top dir: src, tests
        let names: Vec<&str> = report.crates.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"src") && names.contains(&"tests"), "{names:?}");
        // the src module carries the god-file; tests module has_tests
        let tests = report.crates.iter().find(|c| c.name == "tests").unwrap();
        assert!(tests.has_tests);
        assert!(report.god_files.iter().any(|g| g.file.ends_with("models.py")));

        // triage works on the generic report (the whole hospital does)
        let ward = crate::triage::triage(&report);
        assert!(!ward.patients.is_empty());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn project_bundle_packs_and_respects_window() {
        let tmp = std::env::temp_dir().join(format!("flux-projb-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        write(&tmp.join("main.go"), &"package main\n".repeat(30));
        write(&tmp.join("big.go"), &"func helper() {}\n".repeat(500));
        let (bundle, n, toks) = project_bundle(tmp.to_str().unwrap(), 200);
        assert!(bundle.contains("project bundle"));
        assert!(n >= 1);
        assert!(toks <= 200, "respects window: {toks}");
        // generous window includes both
        let (b2, n2, _) = project_bundle(tmp.to_str().unwrap(), 1_000_000);
        assert_eq!(n2, 2);
        assert!(b2.contains("main.go") && b2.contains("big.go"));
        let _ = fs::remove_dir_all(&tmp);
    }
}
