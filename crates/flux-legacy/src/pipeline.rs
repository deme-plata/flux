//! pipeline.rs — flux-legacy PROTOTYPE 6 — the git-aware APPLY + SYNC capstone (owner: rocky-vision).
//!
//! P1 measures, P2 splits, P3 build-plans, P4 sandbox-verifies, P5 prechecks. **P6 LANDS** a verified
//! refactor through the sync topology: it creates a feature branch in the synced clone, applies the
//! edits, commits, and pushes to the bare hub — so a refactor flows Epsilon → hub → Beta.
//!
//! REVERSIBLE BY CONSTRUCTION:
//!   * a NEW branch every time (`refactor/<crate>-<file>-split`), NEVER the baseline;
//!   * `confirm` gates every write — dry-run by default reports exactly what WOULD happen;
//!   * the caller is expected to have a GREEN P4 verify before calling with `confirm=true`
//!     (the bin wires that gate; this module refuses to fabricate a verdict).
//!
//! Pure git mechanics over `std::process::Command` — tested against a LOCAL bare hub that mirrors the
//! Beta `q-narwhalknight-hub.git` topology, so the push round-trip is proven without any network.

use crate::split::SplitPatch;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::process::Command;

/// How to land the refactor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncOpts {
    /// the NEW branch to create (must differ from the baseline — enforced)
    pub branch: String,
    /// git remote to push to (the bare hub; e.g. "origin")
    pub remote: String,
    /// push to the hub after commit (else local commit only)
    pub push: bool,
    /// gate: nothing is written/committed/pushed unless true
    pub confirm: bool,
    pub commit_message: String,
}

impl SyncOpts {
    /// A sane default for splitting `crate_name`'s `file` (e.g. `handlers.rs`).
    pub fn for_split(crate_name: &str, file: &str, remote: &str) -> Self {
        let stem = Path::new(file).file_stem().and_then(|s| s.to_str()).unwrap_or("file");
        SyncOpts {
            branch: format!("refactor/{crate_name}-{stem}-split"),
            remote: remote.to_string(),
            push: true,
            confirm: false,
            commit_message: format!("refactor({crate_name}): split {file} into focused modules\n\nflux-legacy P6 — verified split, mechanical apply."),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub step: String,
    pub ok: bool,
    pub detail: String,
}

/// The outcome of a sync-apply run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncReport {
    pub repo_root: String,
    pub branch: String,
    pub dry_run: bool,
    pub steps: Vec<StepResult>,
    pub committed: bool,
    pub pushed: bool,
    /// short HEAD after commit
    pub head: Option<String>,
}

impl SyncReport {
    fn step(&mut self, step: &str, ok: bool, detail: impl Into<String>) {
        self.steps.push(StepResult { step: step.into(), ok, detail: detail.into() });
    }
    /// did every executed step pass?
    pub fn ok(&self) -> bool {
        self.steps.iter().all(|s| s.ok)
    }
}

/// Run a git command in `dir`, capturing stdout (trimmed) or a descriptive error.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("spawn git {args:?}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// The baseline branch P6 must NEVER write to directly.
pub const PROTECTED_BASELINE: &str = "agent/cross-shard-simd-validation";

/// Land `edits` (each `(repo-relative path, full new content)`) on a NEW branch and optionally push
/// to the hub. Dry-run unless `opts.confirm`. Never force-pushes, never touches the baseline.
pub fn sync_apply(repo_root: &str, edits: &[(String, String)], opts: &SyncOpts) -> SyncReport {
    let root = Path::new(repo_root);
    let mut r = SyncReport {
        repo_root: repo_root.to_string(),
        branch: opts.branch.clone(),
        dry_run: !opts.confirm,
        steps: Vec::new(),
        committed: false,
        pushed: false,
        head: None,
    };

    // guard 0: must be a git work tree
    if git(root, &["rev-parse", "--is-inside-work-tree"]).map(|s| s == "true").unwrap_or(false) == false {
        r.step("git-check", false, "not a git work tree");
        return r;
    }
    // guard 1: never write the baseline directly
    if opts.branch == PROTECTED_BASELINE || opts.branch.is_empty() {
        r.step("branch-guard", false, format!("refusing to target protected/empty branch '{}'", opts.branch));
        return r;
    }
    // guard 2: refuse to clobber uncommitted changes already in the tree
    match git(root, &["status", "--porcelain"]) {
        Ok(s) if !s.is_empty() => {
            r.step("clean-check", false, "work tree has uncommitted changes — commit/stash first");
            return r;
        }
        Ok(_) => r.step("clean-check", true, "work tree clean"),
        Err(e) => {
            r.step("clean-check", false, e);
            return r;
        }
    }

    if !opts.confirm {
        r.step(
            "dry-run",
            true,
            format!(
                "would: branch '{}' off HEAD · apply {} file(s) · commit · {}",
                opts.branch,
                edits.len(),
                if opts.push { format!("push to '{}'", opts.remote) } else { "no push".into() }
            ),
        );
        for (p, _) in edits {
            r.step("plan-write", true, p.clone());
        }
        return r;
    }

    // 1) create the feature branch off current HEAD
    match git(root, &["checkout", "-B", &opts.branch]) {
        Ok(_) => r.step("branch", true, opts.branch.clone()),
        Err(e) => {
            r.step("branch", false, e);
            return r;
        }
    }

    // 2) write the edits
    for (rel, content) in edits {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(e) = fs::write(&p, content) {
            r.step("apply", false, format!("{rel}: {e}"));
            return r;
        }
    }
    r.step("apply", true, format!("{} file(s)", edits.len()));

    // 3) stage + commit
    if let Err(e) = git(root, &["add", "-A"]) {
        r.step("stage", false, e);
        return r;
    }
    match git(root, &["commit", "-m", &opts.commit_message]) {
        Ok(_) => {
            r.committed = true;
            r.head = git(root, &["rev-parse", "--short", "HEAD"]).ok();
            r.step("commit", true, r.head.clone().unwrap_or_default());
        }
        Err(e) => {
            r.step("commit", false, e);
            return r;
        }
    }

    // 4) push to the hub (no force; sets upstream)
    if opts.push {
        match git(root, &["push", "-u", &opts.remote, &opts.branch]) {
            Ok(_) => {
                r.pushed = true;
                r.step("push", true, format!("{} → {}", opts.branch, opts.remote));
            }
            Err(e) => r.step("push", false, e),
        }
    }
    r
}

/// Map a [`SplitPatch`] to repo-relative edits: one file per module + the rewritten god-file
/// (preamble preserved, body replaced by the `mod` wiring). `crate_src_rel` is the crate's src dir
/// relative to the repo root, e.g. `crates/q-api-server/src`.
pub fn split_to_edits(patch: &SplitPatch, crate_src_rel: &str) -> Vec<(String, String)> {
    let orig = Path::new(&patch.original_file);
    let stem = orig.file_stem().and_then(|s| s.to_str()).unwrap_or("module");
    let god_name = orig.file_name().and_then(|s| s.to_str()).unwrap_or("mod.rs");

    let mut edits = Vec::new();
    for m in &patch.modules {
        edits.push((format!("{crate_src_rel}/{stem}_{}.rs", m.module), m.src.clone()));
    }
    // the god-file becomes the module index: keep its leading `use`/attr header, then the wiring.
    let header = leading_header(&patch_original_src(patch));
    let new_god = format!(
        "// flux-legacy P6 — {} split into {} modules; original items live in the sibling files.\n{}\n{}\n",
        god_name,
        patch.modules.len(),
        header,
        patch.mod_wiring,
    );
    edits.push((format!("{crate_src_rel}/{god_name}"), new_god));
    edits
}

/// Best-effort original source for header extraction (the patch may not carry it; fall back to "").
fn patch_original_src(patch: &SplitPatch) -> String {
    fs::read_to_string(&patch.original_file).unwrap_or_default()
}

/// The leading `use` / `#![..]` / `//!` lines of a file (its shared header), up to the first item.
fn leading_header(src: &str) -> String {
    let mut out = Vec::new();
    for line in src.lines() {
        let t = line.trim_start();
        if t.starts_with("use ")
            || t.starts_with("#![")
            || t.starts_with("#[")
            || t.starts_with("//!")
            || t.starts_with("//")
            || t.is_empty()
            || t.starts_with("extern crate")
        {
            out.push(line);
        } else {
            break;
        }
    }
    out.join("\n")
}

/// Human-readable run summary.
pub fn render_sync(r: &SyncReport) -> String {
    let mut s = format!(
        "🏁 P6 SYNC-APPLY — {}\n   branch {}{}\n",
        r.repo_root,
        r.branch,
        if r.dry_run { "  (DRY-RUN — pass --confirm to land it)" } else { "" },
    );
    for st in &r.steps {
        s.push_str(&format!("   {} {:<12} {}\n", if st.ok { "✓" } else { "✗" }, st.step, st.detail));
    }
    if r.committed {
        s.push_str(&format!(
            "   → committed {}{}\n",
            r.head.clone().unwrap_or_default(),
            if r.pushed { " · pushed to hub (Epsilon→hub→Beta)" } else { " · local only" },
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_ok(dir: &Path, args: &[&str]) {
        git(dir, args).unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    }

    fn init_repo(dir: &Path) {
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/lib.rs"), "pub fn a() {}\n").unwrap();
        git_ok(dir, &["init", "-q", "-b", "agent/cross-shard-simd-validation"]);
        git_ok(dir, &["config", "user.email", "rocky@flux.dev"]);
        git_ok(dir, &["config", "user.name", "rocky-vision"]);
        git_ok(dir, &["add", "-A"]);
        git_ok(dir, &["commit", "-q", "-m", "init"]);
    }

    #[test]
    fn dry_run_writes_nothing() {
        let base = std::env::temp_dir().join(format!("flux-p6-dry-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let work = base.join("work");
        init_repo(&work);

        let edits = vec![("src/lib_b.rs".to_string(), "pub fn b() {}\n".to_string())];
        let mut opts = SyncOpts::for_split("demo", "lib.rs", "hub");
        opts.confirm = false;
        let r = sync_apply(work.to_str().unwrap(), &edits, &opts);

        assert!(r.dry_run);
        assert!(!r.committed && !r.pushed);
        assert!(!work.join("src/lib_b.rs").exists(), "dry-run must not write");
        // still on baseline, untouched
        assert_eq!(git(&work, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap(), "agent/cross-shard-simd-validation");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn refuses_to_target_the_baseline() {
        let base = std::env::temp_dir().join(format!("flux-p6-guard-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let work = base.join("work");
        init_repo(&work);

        let mut opts = SyncOpts::for_split("demo", "lib.rs", "hub");
        opts.branch = PROTECTED_BASELINE.to_string();
        opts.confirm = true;
        let r = sync_apply(work.to_str().unwrap(), &[], &opts);
        assert!(!r.committed);
        assert!(r.steps.iter().any(|s| s.step == "branch-guard" && !s.ok));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn commits_and_pushes_to_a_bare_hub() {
        // mirrors the Beta topology: work tree → push to bare hub → (Beta would pull)
        let base = std::env::temp_dir().join(format!("flux-p6-sync-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let hub = base.join("hub.git");
        let work = base.join("work");
        fs::create_dir_all(&base).unwrap();
        git_ok(&base, &["init", "-q", "--bare", hub.to_str().unwrap()]);
        init_repo(&work);
        git_ok(&work, &["remote", "add", "hub", hub.to_str().unwrap()]);

        let edits = vec![
            ("src/lib_b.rs".to_string(), "pub fn b() {}\n".to_string()),
            ("src/lib.rs".to_string(), "pub fn a() {}\nmod lib_b;\n".to_string()),
        ];
        let mut opts = SyncOpts::for_split("demo", "lib.rs", "hub");
        opts.confirm = true;
        opts.push = true;
        let r = sync_apply(work.to_str().unwrap(), &edits, &opts);

        assert!(r.committed, "{:?}", r.steps);
        assert!(r.pushed, "{:?}", r.steps);
        assert!(r.ok());
        // the new file landed on a NEW branch, not the baseline
        assert_eq!(git(&work, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap(), "refactor/demo-lib-split");
        assert!(work.join("src/lib_b.rs").exists());
        // the bare hub RECEIVED the branch (the sync round-trip)
        let hub_branch = git(&hub, &["rev-parse", "--verify", "refactor/demo-lib-split"]);
        assert!(hub_branch.is_ok(), "hub must have the pushed branch: {hub_branch:?}");
        // baseline on the hub is untouched (we never pushed to it)
        let _ = fs::remove_dir_all(&base);
    }
}
