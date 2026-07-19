// fluxc prune-report — SEMANTIC DEPENDENCY ELIMINATION, report-only (v0.36 task, DeepSeek adopt).
//
// Walks the workspace's cargo dep edges (path-deps incl. dev/build/target deps) starting from
// the `default-members` roots in the root Cargo.toml and reports every member crate the
// load-bearing core can NOT reach — with LOC and last-commit-touch date — into
// docs/PRUNE_REPORT.md. It deletes NOTHING: the report is the input to a human/DeepSeek
// decision about `git rm` / archiving, never the executor.
//
// Reachability definition: a crate is "reachable" iff it appears in the transitive path-dep
// closure of the default-members set. Since default-members is itself maintained as the closure
// of the 10 core roots, unreachable == "no core crate can ever link this in".

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

pub fn run(args: &[String]) -> i32 {
    let verbose = args.iter().any(|a| a == "-v" || a == "--verbose");
    let root = match find_workspace_root() {
        Some(r) => r,
        None => {
            eprintln!("prune-report: no workspace Cargo.toml found above {:?}", std::env::current_dir().ok());
            return 1;
        }
    };
    let manifest = match std::fs::read_to_string(root.join("Cargo.toml")) {
        Ok(s) => s,
        Err(e) => { eprintln!("prune-report: read root Cargo.toml: {e}"); return 1; }
    };
    let ws: toml::Value = match manifest.parse() {
        Ok(v) => v,
        Err(e) => { eprintln!("prune-report: parse root Cargo.toml: {e}"); return 1; }
    };
    let ws_tbl = ws.get("workspace").and_then(|w| w.as_table());
    let members_spec: Vec<String> = ws_tbl
        .and_then(|w| w.get("members")).and_then(|m| m.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let default_members: Vec<String> = ws_tbl
        .and_then(|w| w.get("default-members")).and_then(|m| m.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    if default_members.is_empty() {
        eprintln!("prune-report: root Cargo.toml has no [workspace] default-members — nothing to walk from");
        return 1;
    }

    // Expand members globs (`crates/*`) into concrete crate dirs (rel paths from root).
    let mut member_dirs: BTreeSet<String> = BTreeSet::new();
    for spec in &members_spec {
        if let Some(prefix) = spec.strip_suffix("/*") {
            if let Ok(rd) = std::fs::read_dir(root.join(prefix)) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.join("Cargo.toml").is_file() {
                        member_dirs.insert(format!("{}/{}", prefix, e.file_name().to_string_lossy()));
                    }
                }
            }
        } else if root.join(spec).join("Cargo.toml").is_file() {
            member_dirs.insert(spec.clone());
        }
    }

    // BFS the path-dep closure from the default-members roots.
    let mut reachable: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<String> = default_members.iter().cloned().collect();
    while let Some(dir) = queue.pop_front() {
        if !reachable.insert(dir.clone()) { continue; }
        for dep_dir in path_deps(&root, &dir) {
            if !reachable.contains(&dep_dir) { queue.push_back(dep_dir); }
        }
    }

    // External consumers: sibling workspaces that path-dep into our members.
    // An "unreachable" member with an external consumer is NOT prunable.
    let scan_roots = default_scan_roots(&root);
    let ext = external_consumers(&root, &member_dirs, &scan_roots);

    // Unreachable = members - reachable. Collect LOC + last-touch for each,
    // split by external consumption.
    let mut rows: Vec<(String, u64, String)> = Vec::new(); // prune-eligible
    let mut keep_ext: Vec<(String, u64, String, Vec<String>)> = Vec::new(); // externally consumed
    let mut reachable_loc: u64 = 0;
    for dir in &member_dirs {
        let loc = rust_loc(&root.join(dir));
        if reachable.contains(dir) {
            reachable_loc += loc;
            continue;
        }
        let last = git_last_touch(&root, dir).unwrap_or_else(|| "untracked".into());
        if let Some(consumers) = ext.get(dir) {
            if verbose { eprintln!("KEEP-EXTERNAL: {dir} ({loc} LOC, {} consumer(s))", consumers.len()); }
            keep_ext.push((dir.clone(), loc, last, consumers.clone()));
            continue;
        }
        if verbose { eprintln!("unreachable: {dir} ({loc} LOC, last touch {last})"); }
        rows.push((dir.clone(), loc, last));
    }
    // Stalest first, then biggest first.
    rows.sort_by(|a, b| a.2.cmp(&b.2).then(b.1.cmp(&a.1)));
    keep_ext.sort_by(|a, b| b.1.cmp(&a.1));

    let total_unreachable_loc: u64 = rows.iter().map(|r| r.1).sum();
    let mut md = String::new();
    md.push_str("# PRUNE REPORT — semantic dependency elimination (report only)\n\n");
    md.push_str(&format!("Generated by `fluxc prune-report` on {}.\n\n", today_utc()));
    md.push_str(&format!(
        "- Workspace members: **{}**\n- Reachable from `default-members` ({} roots): **{}** crates ({} LOC)\n- KEEP-EXTERNAL (unreachable here, consumed by sibling workspaces): **{}** crates\n- **Prune-eligible: {} crates, {} LOC**\n\n",
        member_dirs.len(), default_members.len(), reachable.len(), reachable_loc, keep_ext.len(), rows.len(), total_unreachable_loc
    ));
    md.push_str(
        "Reachability = transitive path-dep closure (deps + dev-deps + build-deps + target deps)\n\
         of the root `Cargo.toml` `default-members` set, PLUS an external pass over sibling\n\
         workspaces' manifests (path-deps into this tree ⇒ KEEP-EXTERNAL, never prunable —\n\
         see docs/PRUNE_REVIEW_rocky.md FINDING 1). Scan roots: $FLUX_PRUNE_EXTERNAL_ROOTS or\n\
         every sibling dir of the workspace root. THIS REPORT DELETES NOTHING — it is the input\n\
         to an explicit prune/archive decision (swarm + DeepSeek review), never the executor.\n\n",
    );
    if !keep_ext.is_empty() {
        md.push_str("## KEEP-EXTERNAL — do not prune (external path-dep consumers)\n\n");
        md.push_str("| crate | LOC | last commit touch | consumers |\n|---|---:|---|---|\n");
        for (dir, loc, last, consumers) in &keep_ext {
            let shown = consumers.first().map(String::as_str).unwrap_or("");
            let more = if consumers.len() > 1 { format!(" (+{} more)", consumers.len() - 1) } else { String::new() };
            md.push_str(&format!("| `{}` | {} | {} | `{}`{} |\n", dir, loc, last, shown, more));
        }
        md.push('\n');
    }
    md.push_str("## Prune-eligible\n\n");
    md.push_str("| crate | LOC | last commit touch |\n|---|---:|---|\n");
    for (dir, loc, last) in &rows {
        md.push_str(&format!("| `{}` | {} | {} |\n", dir, loc, last));
    }
    md.push('\n');

    let out = root.join("docs").join("PRUNE_REPORT.md");
    if let Err(e) = std::fs::create_dir_all(out.parent().unwrap()) {
        eprintln!("prune-report: mkdir docs/: {e}");
        return 1;
    }
    if let Err(e) = std::fs::write(&out, &md) {
        eprintln!("prune-report: write {}: {e}", out.display());
        return 1;
    }
    println!(
        "prune-report: {} members, {} reachable from {} default-members roots, {} KEEP-EXTERNAL, {} prune-eligible ({} LOC) -> {}",
        member_dirs.len(), reachable.len(), default_members.len(), keep_ext.len(), rows.len(), total_unreachable_loc, out.display()
    );
    0
}

/// Walk up from CWD to the first Cargo.toml containing a [workspace] table.
fn find_workspace_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let m = dir.join("Cargo.toml");
        if m.is_file() {
            if let Ok(s) = std::fs::read_to_string(&m) {
                if s.contains("[workspace]") {
                    return Some(dir);
                }
            }
        }
        if !dir.pop() { return None; }
    }
}

/// Path-deps of the crate at `root/dir`, normalized to workspace-relative dirs.
/// Deps escaping the workspace root are ignored (reported once by the caller
/// via reachability anyway).
fn path_deps(root: &Path, dir: &str) -> Vec<String> {
    let rootn = normalize(root);
    manifest_path_deps(&root.join(dir).join("Cargo.toml"))
        .into_iter()
        .filter_map(|abs| {
            abs.strip_prefix(&rootn)
                .ok()
                .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        })
        .collect()
}

/// Absolute (lexically normalized) path-dep dirs declared by the manifest at
/// `manifest`. Covers [dependencies], [dev-dependencies], [build-dependencies]
/// and every [target.'cfg'.…] variant. Shared by the internal closure walk
/// (`path_deps`) and the external-consumer scan so both see identical edges.
fn manifest_path_deps(manifest: &Path) -> Vec<PathBuf> {
    let Some(dir) = manifest.parent() else { return Vec::new(); };
    let Ok(s) = std::fs::read_to_string(manifest) else { return Vec::new(); };
    let Ok(v) = s.parse::<toml::Value>() else { return Vec::new(); };
    let mut sections: Vec<&toml::value::Table> = Vec::new();
    for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(t) = v.get(key).and_then(|t| t.as_table()) { sections.push(t); }
    }
    if let Some(targets) = v.get("target").and_then(|t| t.as_table()) {
        for tgt in targets.values() {
            for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
                if let Some(t) = tgt.get(key).and_then(|t| t.as_table()) { sections.push(t); }
            }
        }
    }
    // Workspace ROOT manifests declare shared path-deps under
    // [workspace.dependencies] (how sigil's root consumes flux-arxiv-latex —
    // the miss that initially hid 1 of the 4 KEEP-EXTERNAL crates).
    if let Some(t) = v.get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(|t| t.as_table())
    {
        sections.push(t);
    }
    let mut out = Vec::new();
    for sec in sections {
        for spec in sec.values() {
            let Some(p) = spec.get("path").and_then(|p| p.as_str()) else { continue; };
            out.push(normalize(&dir.join(p)));
        }
    }
    out
}

/// FINDING 1 of docs/PRUNE_REVIEW_rocky.md: reachability from THIS workspace's
/// default-members is blind to sibling workspaces that path-dep into our
/// crates (the live sigil tree consumes flux-history / flux-market /
/// flux-uint / flux-arxiv-latex — all four sat on the 2026-07-03 kill list).
/// Scan `scan_roots` for Cargo.toml manifests whose path-deps resolve inside
/// this workspace, and map member-dir → consumer manifest paths.
/// `target`/`.git`/`node_modules` subtrees are skipped; symlinked dirs are
/// not followed (DirEntry::file_type doesn't traverse).
fn external_consumers(
    root: &Path,
    member_dirs: &BTreeSet<String>,
    scan_roots: &[PathBuf],
) -> BTreeMap<String, Vec<String>> {
    let rootn = normalize(root);
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for scan in scan_roots {
        let mut stack = vec![scan.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue; };
            for e in rd.flatten() {
                let p = e.path();
                let Ok(ft) = e.file_type() else { continue; };
                if ft.is_dir() {
                    if p.file_name().map_or(false, |n| n == "target" || n == ".git" || n == "node_modules") {
                        continue;
                    }
                    stack.push(p);
                } else if p.file_name().map_or(false, |n| n == "Cargo.toml") {
                    for dep in manifest_path_deps(&p) {
                        if let Ok(rel) = dep.strip_prefix(&rootn) {
                            let rel = rel.to_string_lossy().replace('\\', "/");
                            if member_dirs.contains(&rel) {
                                out.entry(rel).or_default().push(p.display().to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// Scan roots for the external-consumer pass: `$FLUX_PRUNE_EXTERNAL_ROOTS`
/// (colon-separated) when set, else every sibling directory of the workspace
/// root (the layout that bit us: /…/deepseek-codewhale/{flux,sigil,…}).
fn default_scan_roots(root: &Path) -> Vec<PathBuf> {
    if let Ok(s) = std::env::var("FLUX_PRUNE_EXTERNAL_ROOTS") {
        return s.split(':').filter(|p| !p.is_empty()).map(PathBuf::from).collect();
    }
    let rootn = normalize(root);
    let Some(parent) = root.parent() else { return Vec::new(); };
    let Ok(rd) = std::fs::read_dir(parent) else { return Vec::new(); };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && normalize(p) != rootn)
        .collect()
}

/// Lexical `..`/`.` normalization (no symlink resolution — keeps missing paths workable).
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => { out.pop(); }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Total lines across every .rs file under the crate dir (src, tests, benches, examples).
fn rust_loc(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue; };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue; };
            if ft.is_dir() {
                if p.file_name().map_or(false, |n| n == "target" || n == ".git") { continue; }
                stack.push(p);
            } else if p.extension().map_or(false, |x| x == "rs") {
                if let Ok(s) = std::fs::read_to_string(&p) {
                    total += s.lines().count() as u64;
                }
            }
        }
    }
    total
}

/// `git log -1 --format=%cs -- <dir>` — the last commit date (YYYY-MM-DD) that touched the crate.
fn git_last_touch(root: &Path, dir: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(["log", "-1", "--format=%cs", "--", dir])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture: ws/crates/{used-ext,truly-dead} as members; a sibling
    /// workspace consumer path-deps into used-ext (through a [target] section
    /// too, to prove those edges count). external_consumers must find exactly
    /// used-ext, with the consumer manifest recorded, and ignore truly-dead.
    #[test]
    fn external_scan_finds_sibling_consumers_only() {
        let base = std::env::temp_dir().join("fluxc-prune-ext-test");
        let _ = std::fs::remove_dir_all(&base);
        let ws = base.join("ws");
        for member in ["used-ext", "truly-dead", "ws-root-consumed"] {
            let d = ws.join("crates").join(member);
            std::fs::create_dir_all(d.join("src")).unwrap();
            std::fs::write(d.join("Cargo.toml"),
                format!("[package]\nname = \"{member}\"\nversion = \"0.1.0\"\n")).unwrap();
            std::fs::write(d.join("src/lib.rs"), "\n").unwrap();
        }
        let consumer = base.join("sibling").join("crates").join("consumer");
        std::fs::create_dir_all(&consumer).unwrap();
        std::fs::write(consumer.join("Cargo.toml"),
            "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\n\
             [dependencies]\nused-ext = { path = \"../../../ws/crates/used-ext\" }\n\
             [target.'cfg(unix)'.dev-dependencies]\nused-ext2 = { path = \"../../../ws/crates/used-ext\" }\n").unwrap();
        // A sibling workspace ROOT manifest consuming via [workspace.dependencies]
        // (the sigil/flux-arxiv-latex shape that a [dependencies]-only parse missed).
        std::fs::write(base.join("sibling").join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n\
             [workspace.dependencies]\nws-root-consumed = { path = \"../ws/crates/ws-root-consumed\" }\n").unwrap();
        // Noise that must be skipped: a manifest under target/.
        let junk = base.join("sibling").join("target").join("junk");
        std::fs::create_dir_all(&junk).unwrap();
        std::fs::write(junk.join("Cargo.toml"),
            "[package]\nname=\"junk\"\nversion=\"0.1.0\"\n\
             [dependencies]\ntruly-dead = { path = \"../../../ws/crates/truly-dead\" }\n").unwrap();

        let members: BTreeSet<String> =
            ["crates/used-ext", "crates/truly-dead", "crates/ws-root-consumed"]
                .iter().map(|s| s.to_string()).collect();
        let got = external_consumers(&ws, &members, &[base.join("sibling")]);

        assert_eq!(got.keys().collect::<Vec<_>>(), vec!["crates/used-ext", "crates/ws-root-consumed"],
            "externally consumed members only — incl. the [workspace.dependencies] edge; never truly-dead");
        let consumers = &got["crates/used-ext"];
        assert!(consumers.iter().all(|c| c.ends_with("Cargo.toml")));
        assert!(consumers.iter().any(|c| c.contains("consumer")),
            "the real consumer manifest must be recorded");
        assert!(!consumers.iter().any(|c| c.contains("target")),
            "manifests under target/ must be skipped");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// path_deps (the internal closure walk) must see the same edges through
    /// the shared manifest_path_deps helper — including [target] sections.
    #[test]
    fn internal_path_deps_reuse_shared_parser() {
        let base = std::env::temp_dir().join("fluxc-prune-int-test");
        let _ = std::fs::remove_dir_all(&base);
        let a = base.join("crates").join("a");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(a.join("Cargo.toml"),
            "[package]\nname=\"a\"\nversion=\"0.1.0\"\n\
             [dependencies]\nb = { path = \"../b\" }\n\
             [target.'cfg(unix)'.build-dependencies]\nc = { path = \"../c\" }\n").unwrap();
        let mut deps = path_deps(&base, "crates/a");
        deps.sort();
        assert_eq!(deps, vec!["crates/b", "crates/c"]);
        let _ = std::fs::remove_dir_all(&base);
    }
}

fn today_utc() -> String {
    // Date from epoch days — avoids pulling a chrono dep for one line.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = secs / 86_400;
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}", y, m, d)
}
