// build_order.rs — Emit rustc compiler flags for each crate.
//
// Given the resolved workspace graph and a crate index, produces the
// rustc command-line arguments needed to compile that crate without cargo.
//
// Phase 2a: Assumes cargo has populated target/debug/deps at least once.
// Uses -L dependency= for crates.io deps and --extern for workspace deps.

use std::fs;
use std::path::PathBuf;
use crate::{WorkspaceGraph, CrateInfo, CrateType, DepKind};

/// Build flags for a single crate at the given index.
pub fn rustc_flags(ws: &WorkspaceGraph, idx: usize, release: bool) -> Result<Vec<String>, String> {
    let ci = &ws.crates[idx];
    let profile = if release { "release" } else { "debug" };
    let target_dir = ws.root.join("target").join(profile);
    let deps_dir = target_dir.join("deps");
    let incr_dir = ws.root.join("target").join(profile).join("incremental");

    let mut args: Vec<String> = Vec::new();

    // --- Edition ---
    args.push("--edition".into());
    args.push(ci.edition.clone());

    // --- Crate name ---
    args.push("--crate-name".into());
    args.push(ci.name.clone());

    // --- Crate type ---
    match ci.crate_type {
        CrateType::Lib => {
            args.push("--crate-type".into());
            args.push("lib".into());
        }
        CrateType::Bin => {
            args.push("--crate-type".into());
            args.push("bin".into());
        }
        CrateType::ProcMacro => {
            args.push("--crate-type".into());
            args.push("proc-macro".into());
        }
    }

    // --- Emit ---
    args.push("--emit".into());
    args.push("dep-info,link,metadata".into());

    // --- Output directory ---
    args.push("--out-dir".into());
    args.push(deps_dir.to_string_lossy().to_string());

    // --- Optimization / Debug ---
    if release {
        args.push("-C".into());
        args.push("opt-level=3".into());
        args.push("-C".into());
        args.push("debuginfo=0".into());
    } else {
        args.push("-C".into());
        args.push("opt-level=1".into());
        args.push("-C".into());
        args.push("debuginfo=2".into());
        args.push("-C".into());
        args.push("debug-assertions=on".into());
    }

    // --- Embed bitcode + metadata ---
    args.push("-C".into());
    args.push("embed-bitcode=no".into());

    let meta_hash = simple_hash(&ci.name);
    args.push("-C".into());
    args.push(format!("metadata={}", meta_hash));
    args.push("-C".into());
    args.push(format!("extra-filename=-{}", meta_hash));

    // --- Check-cfg (Phase 2a: skip, fragile arg splitting) ---

    // --- Incremental ---
    args.push("-C".into());
    args.push(format!("incremental={}", incr_dir.display()));

    // --- Dependency search paths ---
    // Cargo-built artifacts (crates.io + workspace)
    args.push("-L".into());
    args.push(format!("dependency={}", deps_dir.display()));

    // Native library paths from build scripts
    discover_native_libs(&deps_dir, &mut args);

    // --- Extern crates ---
    let mut seen_externs: std::collections::HashSet<String> = std::collections::HashSet::new();

    for dep in &ci.dependencies {
        let extern_name = dep.name.replace('-', "_");
        if seen_externs.contains(&extern_name) { continue; }

        match dep.kind {
            DepKind::Path => {
                // Workspace dep — find its artifact in deps/
                if let Some(artifact) = find_artifact(&deps_dir, &dep.name) {
                    args.push("--extern".into());
                    args.push(format!("{}={}", extern_name, artifact.display()));
                    seen_externs.insert(extern_name);
                }
            }
            DepKind::CratesIo | DepKind::Git => {
                // External dep — find its artifact in deps/
                if let Some(artifact) = find_artifact(&deps_dir, &dep.name) {
                    args.push("--extern".into());
                    args.push(format!("{}={}", extern_name, artifact.display()));
                    seen_externs.insert(extern_name);
                }
            }
        }
    }

    // --- Source file ---
    let src_file = source_file(ci);
    args.push(src_file.to_string_lossy().to_string());

    Ok(args)
}

/// Simple deterministic hash from a string (for -C metadata / extra-filename).
fn simple_hash(s: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    format!("{:x}", h.finish())
}

/// Find a compiled artifact for a crate in the deps directory.
/// Matches lib<cratename>-<hash>.rmeta or .rlib exactly (not prefix-substring).
fn find_artifact(deps_dir: &PathBuf, crate_name: &str) -> Option<PathBuf> {
    let normalized = crate_name.replace('-', "_");
    let prefix = format!("lib{}-", normalized);
    if let Ok(entries) = fs::read_dir(deps_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let fname = path.file_name().unwrap_or_default().to_string_lossy();
            if fname.starts_with(&prefix) && (fname.ends_with(".rmeta") || fname.ends_with(".rlib")) {
                return Some(path);
            }
        }
    }
    None
}

/// Discover native library paths from cargo build script output directories.
fn discover_native_libs(deps_dir: &PathBuf, args: &mut Vec<String>) {
    // Cargo's build scripts produce output in target/debug/build/<crate>-<hash>/out/
    let build_dir = deps_dir.parent().unwrap().join("build");
    if let Ok(entries) = fs::read_dir(&build_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let out_dir = path.join("out");
                if out_dir.is_dir() {
                    args.push("-L".into());
                    args.push(format!("native={}", out_dir.display()));
                }
            }
        }
    }
}

/// Find the source file for a crate.
fn source_file(ci: &CrateInfo) -> PathBuf {
    match ci.crate_type {
        CrateType::Bin => {
            let main_rs = ci.path.join("src").join("main.rs");
            if main_rs.exists() { main_rs } else { ci.path.join("src").join("lib.rs") }
        }
        _ => ci.path.join("src").join("lib.rs"),
    }
}

/// Build the full rustc command for a crate, including cache check.
/// Returns None if the cache hit means we can skip compilation.
pub fn build_command(ws: &WorkspaceGraph, idx: usize, release: bool) -> Result<Option<Vec<String>>, String> {
    let flags = rustc_flags(ws, idx, release)?;

    // Check flux-cache
    let source_file: Option<&str> = flags.iter()
        .find(|a| a.ends_with(".rs"))
        .map(|s| s.as_str());

    let hash = flux_cache::compute_hash(source_file, &flags);

    if flux_cache::lookup(&hash).is_some() {
        return Ok(None);
    }

    let mut cmd: Vec<String> = vec!["rustc".into()];
    cmd.extend(flags);
    Ok(Some(cmd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CrateInfo, CrateType, Dependency};

    fn make_test_ws() -> WorkspaceGraph {
        let crates = vec![
            CrateInfo {
                name: "leaf".into(),
                path: PathBuf::from("/ws/crates/leaf"),
                edition: "2021".into(),
                crate_type: CrateType::Lib,
                dependencies: vec![],
                features: vec![],
            },
            CrateInfo {
                name: "mid".into(),
                path: PathBuf::from("/ws/crates/mid"),
                edition: "2021".into(),
                crate_type: CrateType::Lib,
                dependencies: vec![
                    Dependency { name: "leaf".into(), path: Some(PathBuf::from("/ws/crates/leaf")), kind: DepKind::Path, optional: false },
                ],
                features: vec![],
            },
        ];
        WorkspaceGraph {
            root: PathBuf::from("/ws"),
            crates,
            batches: vec![vec![0], vec![1]],
        }
    }

    #[test]
    fn test_rustc_flags_has_edition_and_crate_name() {
        let ws = make_test_ws();
        let flags = rustc_flags(&ws, 0, false).unwrap();
        assert!(flags.contains(&"--crate-name".into()));
        assert!(flags.contains(&"leaf".into()));
        assert!(flags.contains(&"--edition".into()));
        assert!(flags.contains(&"2021".into()));
    }

    #[test]
    fn test_rustc_flags_has_dependency_path() {
        let ws = make_test_ws();
        let flags = rustc_flags(&ws, 0, false).unwrap();
        let has_l_flag = flags.windows(2).any(|w| w[0] == "-L" && w[1].contains("dependency="));
        assert!(has_l_flag, "should have -L dependency= flag");
    }
}
