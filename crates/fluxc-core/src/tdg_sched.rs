// FIP-0003 read side, first cut — the CRATE-granularity incremental scheduler.
//
// The FIP's payoff query: "give me the set of nodes whose identity keys
// changed, plus their transitive dependents — everything else is provably
// reusable." This module answers it at crate granularity, which is the finest
// unit we can re-derive INDEPENDENTLY of cargo today: a crate's identity is
// the BLAKE3 over its manifest + every source file. (Unit-granularity needs
// cargo's unit graph + the wrapper's per-unit keys — that refinement layers on
// top of this without changing the workflow.)
//
// Workflow:
//   fluxc tdg baseline        record every crate's source key ("current tree
//                             is the reference point"; proven_green=false)
//   fluxc tdg plan            diff keys → dirty crates → reverse-dep cone →
//                             print what WOULD run and what's reusable
//   fluxc tdg run [--check]   execute test (or check) for ONLY the cone, one
//                             cargo invocation; on green, promote the cone's
//                             stored keys to proven_green=true
//
// Honesty rules inherited from the FIP + FIP-0002:
//   - key-equality only, no mtimes;
//   - a missing/corrupt TDG degrades to "everything dirty" = today's full run;
//   - keys are only promoted AFTER the run comes back green (run_cargo exits
//     the process on failure, so a red run can never promote);
//   - "clean" is reported with its proof state: proven-green vs baseline-only.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateState {
    pub source_key: String,
    pub recorded_unix: u64,
    /// true only when a scheduler-driven run came back green for this key.
    /// baseline-recorded keys are "seen, not proven".
    pub proven_green: bool,
    pub last_wall_ms: u64,
}

pub struct Plan {
    /// crates whose source key changed (or was never recorded)
    pub dirty: Vec<(String, &'static str)>, // (name, reason)
    /// dirty ∪ transitive dependents — what must run
    pub cone: Vec<String>,
    /// unchanged crates — reusable, with their proof state
    pub clean_proven: Vec<String>,
    pub clean_baseline: Vec<String>,
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Walk up from `start` to the enclosing workspace root (a Cargo.toml
/// containing `[workspace]`).
pub fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        let manifest = dir.join("Cargo.toml");
        if manifest.is_file() {
            if let Ok(txt) = std::fs::read_to_string(&manifest) {
                if txt.contains("[workspace]") {
                    return Some(dir);
                }
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// A crate's re-derivable identity: BLAKE3 over (relpath, content-hash) of its
/// Cargo.toml + every `.rs` file under the crate dir, sorted by relpath so the
/// walk order can't leak in. `target/` and dotdirs are skipped.
pub fn crate_source_key(crate_dir: &Path) -> String {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_sources(crate_dir, crate_dir, &mut files);
    files.sort();
    let mut h = blake3::Hasher::new();
    for rel in &files {
        h.update(rel.to_string_lossy().as_bytes());
        h.update(b"\0");
        let content_hash = std::fs::read(crate_dir.join(rel))
            .map(|c| blake3::hash(&c).to_hex().to_string())
            .unwrap_or_default();
        h.update(content_hash.as_bytes());
        h.update(b"\n");
    }
    h.finalize().to_hex().to_string()
}

fn collect_sources(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if p.is_dir() {
            if name == "target" || name.starts_with('.') {
                continue;
            }
            collect_sources(root, &p, out);
        } else if name == "Cargo.toml" || name.ends_with(".rs") {
            if let Ok(rel) = p.strip_prefix(root) {
                out.push(rel.to_path_buf());
            }
        }
    }
}

fn state_key(name: &str) -> String {
    format!("c/{}", name)
}

/// Diff the workspace against stored crate keys and expand the dirty set to
/// its reverse-dependency cone.
pub fn compute_plan(root: &PathBuf) -> Result<Plan, String> {
    let ws = flux_graph::resolve_workspace(root)?;
    let dag = flux_graph::graph::build_dag(&ws.crates)?;
    let db = flux_db::Database::open(crate::tdg::tdg_dir())
        .map_err(|e| format!("TDG open failed ({}) — run without --incremental", e))?;

    let mut dirty: Vec<(usize, &'static str)> = Vec::new();
    let mut states: Vec<Option<CrateState>> = Vec::with_capacity(ws.crates.len());
    for (i, ci) in ws.crates.iter().enumerate() {
        let key = crate_source_key(&ci.path);
        let stored: Option<CrateState> = db
            .get(state_key(&ci.name).as_bytes())
            .ok()
            .flatten()
            .and_then(|v| bincode::deserialize(&v).ok());
        match &stored {
            Some(st) if st.source_key == key => {}
            Some(_) => dirty.push((i, "source changed")),
            None => dirty.push((i, "never recorded")),
        }
        states.push(stored);
    }

    // Cone = dirty ∪ transitive dependents (BFS over reverse edges).
    let mut in_cone = vec![false; ws.crates.len()];
    let mut queue: std::collections::VecDeque<usize> = dirty.iter().map(|(i, _)| *i).collect();
    for (i, _) in &dirty {
        in_cone[*i] = true;
    }
    while let Some(i) = queue.pop_front() {
        for &dep_by in &dag.depended_by[i] {
            if !in_cone[dep_by] {
                in_cone[dep_by] = true;
                queue.push_back(dep_by);
            }
        }
    }

    let mut plan = Plan {
        dirty: dirty.iter().map(|(i, r)| (ws.crates[*i].name.clone(), *r)).collect(),
        cone: Vec::new(),
        clean_proven: Vec::new(),
        clean_baseline: Vec::new(),
    };
    for (i, ci) in ws.crates.iter().enumerate() {
        if in_cone[i] {
            plan.cone.push(ci.name.clone());
        } else if states[i].as_ref().map(|s| s.proven_green).unwrap_or(false) {
            plan.clean_proven.push(ci.name.clone());
        } else {
            plan.clean_baseline.push(ci.name.clone());
        }
    }
    Ok(plan)
}

/// Persist current source keys for every workspace crate. `proven` is false
/// for a baseline ("current tree is the reference, correctness not asserted")
/// and true when promoting a cone after a green run.
fn store_keys(root: &PathBuf, only: Option<&[String]>, proven: bool, wall_ms: u64) -> Result<usize, String> {
    let ws = flux_graph::resolve_workspace(root)?;
    let db = flux_db::Database::open(crate::tdg::tdg_dir()).map_err(|e| e.to_string())?;
    let mut n = 0;
    let mut entries: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for ci in &ws.crates {
        if let Some(filter) = only {
            if !filter.contains(&ci.name) {
                continue;
            }
        }
        let st = CrateState {
            source_key: crate_source_key(&ci.path),
            recorded_unix: now_unix(),
            proven_green: proven,
            last_wall_ms: wall_ms,
        };
        if let Ok(v) = bincode::serialize(&st) {
            entries.push((state_key(&ci.name).into_bytes(), v));
            n += 1;
        }
    }
    db.put_many(&entries).map_err(|e| e.to_string())?;
    Ok(n)
}

pub fn cmd_baseline(root: &PathBuf) {
    match store_keys(root, None, false, 0) {
        Ok(n) => println!("  ✓ baseline recorded for {} crates (reference point; proven_green=false)", n),
        Err(e) => eprintln!("  ✗ baseline failed: {}", e),
    }
}

pub fn print_plan(plan: &Plan) {
    println!("  dirty ({}):", plan.dirty.len());
    for (name, reason) in plan.dirty.iter().take(20) {
        println!("    ~ {:<30} {}", name, reason);
    }
    if plan.dirty.len() > 20 {
        println!("    … and {} more", plan.dirty.len() - 20);
    }
    let dependents = plan.cone.len().saturating_sub(plan.dirty.len());
    println!("  cone: {} crates to run ({} dirty + {} dependents)",
        plan.cone.len(), plan.dirty.len(), dependents);
    println!("  reusable: {} proven-green, {} baseline-only (skipped)",
        plan.clean_proven.len(), plan.clean_baseline.len());
}

/// `fluxc tdg run [--check] [--dry]` — execute ONLY the cone.
pub fn cmd_run(check_only: bool, dry: bool) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some(root) = find_workspace_root(&cwd) else {
        eprintln!("  ✗ no enclosing [workspace] found from {}", cwd.display());
        return;
    };
    println!("⚡ fluxc tdg — incremental schedule for {}", root.display());
    let plan = match compute_plan(&root) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("  ✗ {}", e);
            return;
        }
    };
    print_plan(&plan);
    if plan.cone.is_empty() {
        println!("  ✓ nothing dirty — whole workspace reusable, 0 units scheduled");
        return;
    }
    if dry {
        println!("  (dry run — nothing executed)");
        return;
    }
    let action = if check_only { "check" } else { "test" };
    println!("  running: cargo {} for {} crates…", action, plan.cone.len());
    let mut extra: Vec<String> = Vec::new();
    for c in &plan.cone {
        extra.push("--package".to_string());
        extra.push(c.clone());
    }
    let config = crate::BuildConfig::default();
    let t0 = std::time::Instant::now();
    // run_cargo exits the process on a red run — the promotion below is
    // therefore only reachable on green, which is exactly the contract.
    crate::run_cargo(action, &config, &extra);
    let wall = t0.elapsed().as_millis() as u64;
    match store_keys(&root, Some(&plan.cone), true, wall) {
        Ok(n) => println!("  ✓ green — promoted {} cone crates to proven ({} ms)", n, wall),
        Err(e) => eprintln!("  green, but key promotion failed: {}", e),
    }
}

pub fn cmd_plan() {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some(root) = find_workspace_root(&cwd) else {
        eprintln!("  ✗ no enclosing [workspace] found from {}", cwd.display());
        return;
    };
    println!("⚡ fluxc tdg — incremental plan for {}", root.display());
    match compute_plan(&root) {
        Ok(p) => print_plan(&p),
        Err(e) => eprintln!("  ✗ {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, content: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    /// Two-crate workspace: `app` depends on `dep`; `leaf` stands alone.
    fn fake_ws(root: &Path) {
        write(&root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/dep\", \"crates/app\", \"crates/leaf\"]\n");
        write(&root.join("crates/dep/Cargo.toml"),
            "[package]\nname = \"dep\"\nversion = \"0.1.0\"\n");
        write(&root.join("crates/dep/src/lib.rs"), "pub fn dep() {}\n");
        write(&root.join("crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ndep = { path = \"../dep\" }\n");
        write(&root.join("crates/app/src/lib.rs"), "pub fn app() {}\n");
        write(&root.join("crates/leaf/Cargo.toml"),
            "[package]\nname = \"leaf\"\nversion = \"0.1.0\"\n");
        write(&root.join("crates/leaf/src/lib.rs"), "pub fn leaf() {}\n");
    }

    #[test]
    fn plan_scopes_to_the_dependency_cone() {
        let _guard = crate::tdg::TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let base = std::env::temp_dir().join(format!("flux-sched-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let ws = base.join("ws");
        fake_ws(&ws);
        std::env::set_var("FLUX_TDG_DIR", base.join("tdg"));

        // Before any baseline: everything is dirty ("never recorded").
        let p0 = compute_plan(&ws).expect("plan");
        assert_eq!(p0.cone.len(), 3, "no baseline → all crates dirty");

        // Baseline, then a no-op tree: nothing dirty, all reusable.
        assert_eq!(store_keys(&ws, None, false, 0).unwrap(), 3);
        let p1 = compute_plan(&ws).expect("plan");
        assert!(p1.dirty.is_empty() && p1.cone.is_empty(), "clean tree → empty cone");
        assert_eq!(p1.clean_baseline.len(), 3, "baseline keys are seen-not-proven");

        // Edit dep → cone = {dep, app}; leaf stays reusable. THE payoff assert:
        // the schedule scales with the edit's cone, not the workspace.
        write(&ws.join("crates/dep/src/lib.rs"), "pub fn dep() { /* edited */ }\n");
        let p2 = compute_plan(&ws).expect("plan");
        assert_eq!(p2.dirty.len(), 1);
        assert_eq!(p2.dirty[0].0, "dep");
        let mut cone = p2.cone.clone();
        cone.sort();
        assert_eq!(cone, vec!["app".to_string(), "dep".to_string()],
            "cone = edited crate + its dependents, nothing else");
        assert_eq!(p2.clean_baseline, vec!["leaf".to_string()], "leaf is untouched → reusable");

        // Promote the cone as green → clean tree now reports proven.
        assert_eq!(store_keys(&ws, Some(&p2.cone), true, 42).unwrap(), 2);
        let p3 = compute_plan(&ws).expect("plan");
        assert!(p3.cone.is_empty());
        let mut proven = p3.clean_proven.clone();
        proven.sort();
        assert_eq!(proven, vec!["app".to_string(), "dep".to_string()]);

        std::env::remove_var("FLUX_TDG_DIR");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn source_key_is_content_not_mtime() {
        let base = std::env::temp_dir().join(format!("flux-sched-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        write(&base.join("Cargo.toml"), "[package]\nname=\"x\"\n");
        write(&base.join("src/lib.rs"), "pub fn x() {}\n");
        let k1 = crate_source_key(&base);
        // Rewrite identical content (fresh mtime) → same key.
        write(&base.join("src/lib.rs"), "pub fn x() {}\n");
        assert_eq!(k1, crate_source_key(&base), "identical content must produce identical keys");
        // Change content → different key.
        write(&base.join("src/lib.rs"), "pub fn x() { let _ = 1; }\n");
        assert_ne!(k1, crate_source_key(&base));
        let _ = std::fs::remove_dir_all(&base);
    }
}
