//! LEGACY-1 — the real per-crate metrics walker (owner: rocky-vision).
//!
//! Works on ANY external cargo workspace root, not the flux dogfood tree. Every number here is
//! observed from the source on disk; nothing is predicted or hardcoded.

use crate::{GodFile, LegacyCrate, LegacyReport, GOD_FILE_LOC};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Analyze a legacy workspace at `root` (expects `<root>/crates/*/src`). Returns measured metrics
/// for every crate plus the worst god-files across the whole tree.
pub fn analyze_workspace_legacy(root: &str) -> LegacyReport {
    let start = std::time::Instant::now();
    let root_path = PathBuf::from(root);
    let crates_dir = root_path.join("crates");

    let workspace_name = root_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace")
        .to_string();

    let mut crates: Vec<LegacyCrate> = Vec::new();
    let mut god_files: Vec<GodFile> = Vec::new();

    if let Ok(entries) = fs::read_dir(&crates_dir) {
        for entry in entries.flatten() {
            let cpath = entry.path();
            let cargo = cpath.join("Cargo.toml");
            let src = cpath.join("src");
            if !cpath.is_dir() || !cargo.exists() || !src.exists() {
                continue;
            }
            let name = crate_name(&cargo).unwrap_or_else(|| dir_name(&cpath));

            let rs_files = walk_rs(&src);
            let mut loc = 0usize;
            let mut pub_fns = 0usize;
            let mut pub_types = 0usize;
            let mut has_tests = false;
            let mut biggest_file = String::new();
            let mut biggest_file_loc = 0usize;

            for f in &rs_files {
                let content = match fs::read_to_string(f) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let file_loc = content.lines().count();
                loc += file_loc;
                let (pf, pt, tests) = scan_source(&content);
                pub_fns += pf;
                pub_types += pt;
                has_tests |= tests;

                let rel = rel_to(&cpath, f);
                if file_loc > biggest_file_loc {
                    biggest_file_loc = file_loc;
                    biggest_file = rel.clone();
                }
                if file_loc >= GOD_FILE_LOC {
                    god_files.push(GodFile { crate_name: name.clone(), file: rel, loc: file_loc });
                }
            }
            // a `tests/` integration dir also counts as having tests
            has_tests |= cpath.join("tests").is_dir();

            let deps = path_deps(&cargo);

            crates.push(LegacyCrate {
                name,
                path: cpath.to_string_lossy().to_string(),
                loc,
                file_count: rs_files.len(),
                biggest_file,
                biggest_file_loc,
                pub_fns,
                pub_types,
                has_tests,
                deps,
                dependents: Vec::new(),
            });
        }
    }

    // invert deps → dependents (fan-in). Only edges whose target is a crate in THIS workspace count.
    let names: std::collections::BTreeSet<String> = crates.iter().map(|c| c.name.clone()).collect();
    let mut dependents: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for c in &crates {
        for d in &c.deps {
            if names.contains(d) {
                dependents.entry(d.clone()).or_default().push(c.name.clone());
            }
        }
    }
    for c in &mut crates {
        if let Some(mut v) = dependents.remove(&c.name) {
            v.sort();
            v.dedup();
            c.dependents = v;
        }
    }

    crates.sort_by(|a, b| b.loc.cmp(&a.loc));
    god_files.sort_by(|a, b| b.loc.cmp(&a.loc));
    let total_loc = crates.iter().map(|c| c.loc).sum();

    LegacyReport {
        root: root.to_string(),
        workspace_name,
        crate_count: crates.len(),
        total_loc,
        crates,
        god_files,
        analyze_ms: start.elapsed().as_millis(),
    }
}

/// Count `pub fn`, `pub struct|enum|trait`, and detect a test module, ignoring matches in comments.
fn scan_source(content: &str) -> (usize, usize, bool) {
    let mut pub_fns = 0;
    let mut pub_types = 0;
    let mut has_tests = false;
    for raw in content.lines() {
        let line = raw.trim_start();
        if line.starts_with("//") || line.starts_with('*') {
            continue;
        }
        if line.contains("#[cfg(test)]") || line.contains("#[test]") {
            has_tests = true;
        }
        if line.starts_with("pub fn ") || line.starts_with("pub async fn ") || line.contains("pub fn ") {
            pub_fns += 1;
        }
        if line.starts_with("pub struct ")
            || line.starts_with("pub enum ")
            || line.starts_with("pub trait ")
        {
            pub_types += 1;
        }
    }
    (pub_fns, pub_types, has_tests)
}

/// Recursively collect every `.rs` file under `dir`.
fn walk_rs(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(entries) = fs::read_dir(&d) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

/// crate name from `[package] name = "..."`.
fn crate_name(cargo: &Path) -> Option<String> {
    let s = fs::read_to_string(cargo).ok()?;
    let mut in_package = false;
    for raw in s.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package {
            if let Some(rest) = line.strip_prefix("name") {
                if let Some(q1) = rest.find('"') {
                    let after = &rest[q1 + 1..];
                    if let Some(q2) = after.find('"') {
                        return Some(after[..q2].to_string());
                    }
                }
            }
        }
    }
    None
}

/// intra-workspace path dependencies: `foo = { path = "../foo" }` → crate name `foo`.
/// Resolved from the `path = "..."` directory name (kebab preserved) so it matches crate dir names.
fn path_deps(cargo: &Path) -> Vec<String> {
    let s = match fs::read_to_string(cargo) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut deps = Vec::new();
    for raw in s.lines() {
        let line = raw.trim();
        if line.starts_with('#') {
            continue;
        }
        if let Some(i) = line.find("path") {
            // only treat as a dep line if there's a `path = "..."` with ../ pointing at a sibling crate
            let after = &line[i..];
            if let Some(q1) = after.find('"') {
                let val = &after[q1 + 1..];
                if let Some(q2) = val.find('"') {
                    let p = &val[..q2];
                    if let Some(last) = Path::new(p).file_name().and_then(|n| n.to_str()) {
                        // skip self-ish / non-crate paths
                        if !last.is_empty() && p.contains("..") {
                            deps.push(last.to_string());
                        }
                    }
                }
            }
        }
    }
    deps.sort();
    deps.dedup();
    deps
}

fn dir_name(p: &Path) -> String {
    p.file_name().and_then(|n| n.to_str()).unwrap_or("unknown").to_string()
}

fn rel_to(base: &Path, f: &Path) -> String {
    f.strip_prefix(base).unwrap_or(f).to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(p: &Path, s: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, s).unwrap();
    }

    #[test]
    fn analyzes_a_synthetic_two_crate_workspace() {
        let tmp = std::env::temp_dir().join(format!("flux-legacy-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        // crate `core`: small, tested
        write(&tmp.join("crates/core/Cargo.toml"), "[package]\nname = \"core\"\n");
        write(
            &tmp.join("crates/core/src/lib.rs"),
            "pub fn a() {}\npub struct S;\n#[cfg(test)]\nmod t { #[test] fn x(){} }\n",
        );
        // crate `app`: a god-file, depends on core, no tests
        write(
            &tmp.join("crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\n[dependencies]\ncore = { path = \"../core\" }\n",
        );
        let big = "pub fn f() {}\n".repeat(900);
        write(&tmp.join("crates/app/src/main.rs"), &big);

        let r = analyze_workspace_legacy(tmp.to_str().unwrap());
        assert_eq!(r.crate_count, 2);
        // app is bigger → sorted first
        assert_eq!(r.crates[0].name, "app");
        assert!(r.crates[0].loc >= 900);
        assert!(!r.crates[0].has_tests);
        assert_eq!(r.crates[0].deps, vec!["core".to_string()]);
        // core has a god-file? no. app does.
        assert_eq!(r.god_files.len(), 1);
        assert_eq!(r.god_files[0].crate_name, "app");
        // fan-in: core is depended on by app
        let core = r.crates.iter().find(|c| c.name == "core").unwrap();
        assert_eq!(core.dependents, vec!["app".to_string()]);
        assert!(core.has_tests);
        assert!(core.pub_fns >= 1 && core.pub_types >= 1);

        let _ = fs::remove_dir_all(&tmp);
    }
}
