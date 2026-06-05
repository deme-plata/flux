//! import.rs - flux-legacy Beta-2 (increment 2): SAFE GitHub import.
//!
//! Beta 1 modernizes a workspace already on disk. Beta 2 onboards "a billion projects" - so first we
//! must FETCH an arbitrary GitHub repo, and fetching from the open internet is a security minefield
//! (clone bombs, huge repos that fill a near-full disk, credential leakage, malicious build scripts,
//! path traversal). This module is the fail-closed FRONT DOOR: every step is a gate.
//!
//! Pairs with `lang` (detection): `import -> lang::detect -> analyze`. It NEVER runs the repo's build,
//! hooks, or scripts - it clones SOURCE only. Pure validators (parse_github_url, dest_is_safe, vet)
//! are unit-tested; `import` does the I/O behind those gates.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// How + where to import. Fail-closed defaults: shallow, capped, sandboxed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportSpec {
    /// a github.com http(s) URL or `owner/repo` shorthand.
    pub url: String,
    /// sandbox destination dir (must be absolute + under /home, NOT / or /tmp on Epsilon).
    pub dest_root: PathBuf,
    /// shallow clone depth (>= 1).
    pub depth: u32,
    /// refuse if the cloned tree exceeds this many MB.
    pub max_mb: u64,
    /// hard wall-clock timeout for the clone, in seconds.
    pub timeout_s: u64,
    /// refuse to clone if the dest filesystem has fewer than this many MB free (disk guard).
    pub min_free_mb: u64,
}

impl ImportSpec {
    /// Sane fail-closed defaults: shallow depth 1, 500 MB cap, 120s timeout.
    pub fn new(url: impl Into<String>, dest_root: impl Into<PathBuf>) -> Self {
        ImportSpec { url: url.into(), dest_root: dest_root.into(), depth: 1, max_mb: 500, timeout_s: 120, min_free_mb: 1000 }
    }
}

/// Parse + validate a GitHub source - the FIRST gate. Only github.com over http(s) (or `owner/repo`
/// shorthand), strict shape, NO shell metacharacters, NO embedded credentials (`user@`), NO `..`.
/// Returns `(owner, repo, https_clone_url)`. Owners are alnum/-/_ (no dot); repos may carry a dot.
pub fn parse_github_url(raw: &str) -> Result<(String, String, String), String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("empty url".into());
    }
    if s.contains(char::is_whitespace)
        || s.contains(['@', ';', '|', '&', '$', '`', '\\', '<', '>', '"', '\'', '(', ')', '\n', '*', '?'])
    {
        return Err(format!("unsafe characters in url: {s}"));
    }
    if s.contains("..") {
        return Err("path traversal in url".into());
    }
    let rest = if let Some(r) = s.strip_prefix("https://github.com/") {
        r
    } else if let Some(r) = s.strip_prefix("http://github.com/") {
        r
    } else if s.contains("://") {
        return Err(format!("only github.com is allowed: {s}"));
    } else if s.contains('/') {
        s // owner/repo shorthand
    } else {
        return Err("not owner/repo or a github.com url".into());
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() != 2 {
        return Err(format!("expected owner/repo, got: {rest}"));
    }
    let (owner, repo) = (parts[0], parts[1]);
    let owner_ok = !owner.is_empty() && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    let repo_ok = !repo.is_empty() && repo.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.');
    if !owner_ok || !repo_ok {
        return Err(format!("invalid owner/repo: {owner}/{repo}"));
    }
    Ok((owner.to_string(), repo.to_string(), format!("https://github.com/{owner}/{repo}.git")))
}

/// Is `dest` a safe sandbox? Must be ABSOLUTE, contain no `..` segment, and live under `/home`
/// (Epsilon: the root partition is tiny + /tmp is wiped + /root,/etc,/usr are off-limits).
pub fn dest_is_safe(dest: &Path) -> bool {
    if !dest.is_absolute() {
        return false;
    }
    let s = dest.to_string_lossy();
    if s.split('/').any(|c| c == "..") {
        return false;
    }
    s.starts_with("/home/")
}

/// Pure pre-flight: would this spec be ALLOWED to run? No I/O - the fail-closed gate `import` calls first.
pub fn vet(spec: &ImportSpec) -> Result<(), String> {
    parse_github_url(&spec.url)?;
    if !dest_is_safe(&spec.dest_root) {
        return Err(format!("unsafe dest (must be absolute under /home, never / or /tmp): {:?}", spec.dest_root));
    }
    if spec.depth < 1 {
        return Err("depth must be >= 1 (shallow)".into());
    }
    if spec.max_mb == 0 || spec.max_mb > 5000 {
        return Err("max_mb must be in 1..=5000".into());
    }
    if spec.timeout_s == 0 || spec.timeout_s > 1800 {
        return Err("timeout_s must be in 1..=1800".into());
    }
    Ok(())
}

/// Result of an import attempt. `Rejected` (never a panic) on any gate failure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ImportOutcome {
    Imported { path: PathBuf, size_mb: u64 },
    Rejected(String),
}

impl ImportOutcome {
    pub fn ok(&self) -> bool {
        matches!(self, ImportOutcome::Imported { .. })
    }
}

/// Clone the repo into `dest_root/<owner>__<repo>`: shallow + single-branch + no-tags + hooks
/// disabled + no credential prompt + hard timeout, then enforce the size cap. NEVER executes the
/// repo's build, scripts, or hooks - it clones source only. I/O (gated by `vet`).
pub fn import(spec: &ImportSpec) -> ImportOutcome {
    if let Err(e) = vet(spec) {
        return ImportOutcome::Rejected(e);
    }
    if let Some(free) = disk_free_mb(&spec.dest_root) {
        if free < spec.min_free_mb {
            return ImportOutcome::Rejected(format!("insufficient disk: {free} MB free < {} MB required", spec.min_free_mb));
        }
    }
    let (owner, repo, clone_url) = match parse_github_url(&spec.url) {
        Ok(t) => t,
        Err(e) => return ImportOutcome::Rejected(e),
    };
    let dest = spec.dest_root.join(format!("{owner}__{repo}"));
    if dest.exists() {
        return ImportOutcome::Rejected(format!("dest already exists (refusing to clobber): {dest:?}"));
    }
    if let Some(parent) = dest.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return ImportOutcome::Rejected(format!("cannot create sandbox parent {parent:?}"));
        }
    }
    let status = std::process::Command::new("timeout")
        .arg(spec.timeout_s.to_string())
        .args([
            "git", "-c", "core.hooksPath=/dev/null", "clone",
            "--depth", &spec.depth.to_string(), "--single-branch", "--no-tags",
            &clone_url, dest.to_string_lossy().as_ref(),
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "true")
        .status();
    match status {
        Ok(st) if st.success() => {}
        Ok(st) => {
            let _ = std::fs::remove_dir_all(&dest);
            return ImportOutcome::Rejected(format!("git clone failed (exit {:?}) - removed", st.code()));
        }
        Err(e) => return ImportOutcome::Rejected(format!("could not run git/timeout: {e}")),
    }
    let size_mb = dir_size_mb(&dest);
    if size_mb > spec.max_mb {
        let _ = std::fs::remove_dir_all(&dest);
        return ImportOutcome::Rejected(format!("clone {size_mb} MB exceeds cap {} MB - removed", spec.max_mb));
    }
    let escapes = scan_escapes(&dest);
    if !escapes.is_empty() {
        let _ = std::fs::remove_dir_all(&dest);
        return ImportOutcome::Rejected(format!("{} symlink(s) escape the sandbox (e.g. {:?}) - removed", escapes.len(), escapes.first()));
    }
    ImportOutcome::Imported { path: dest, size_mb }
}

/// Recursive on-disk size in MB (skips nothing; follows no symlinks via metadata, not symlink_metadata).
fn dir_size_mb(p: &Path) -> u64 {
    fn walk(p: &Path) -> u64 {
        let mut total = 0u64;
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                match e.metadata() {
                    Ok(md) if md.is_dir() => total += walk(&e.path()),
                    Ok(md) => total += md.len(),
                    Err(_) => {}
                }
            }
        }
        total
    }
    walk(p) / (1024 * 1024)
}

/// Free space (MB) on the filesystem holding `path`, via POSIX `df -Pk`. None if undeterminable.
pub fn disk_free_mb(path: &Path) -> Option<u64> {
    let out = std::process::Command::new("df").arg("-Pk").arg(path).output().ok()?;
    if !out.status.success() { return None; }
    let s = String::from_utf8_lossy(&out.stdout);
    let avail_kb: u64 = s.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()?;
    Some(avail_kb / 1024)
}

/// Walk a cloned tree and return any SYMLINK whose target escapes `root` (resolves outside it, or
/// dangles). A malicious repo can ship a symlink to /etc/passwd or out of the sandbox; reject those.
pub fn scan_escapes(root: &Path) -> Vec<PathBuf> {
    let root_can = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut bad = Vec::new();
    fn walk(dir: &Path, root_can: &Path, bad: &mut Vec<PathBuf>) {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                let md = match std::fs::symlink_metadata(&p) { Ok(m) => m, Err(_) => continue };
                if md.file_type().is_symlink() {
                    match std::fs::canonicalize(&p) {
                        Ok(t) if t.starts_with(root_can) => {}
                        _ => bad.push(p),
                    }
                } else if md.is_dir() {
                    walk(&p, root_can, bad);
                }
            }
        }
    }
    walk(root, &root_can, &mut bad);
    bad
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_real_github() {
        let (o, r, u) = parse_github_url("https://github.com/torvalds/linux").unwrap();
        assert_eq!((o.as_str(), r.as_str()), ("torvalds", "linux"));
        assert_eq!(u, "https://github.com/torvalds/linux.git");
        assert_eq!(parse_github_url("https://github.com/a/b.git").unwrap().2, "https://github.com/a/b.git");
        assert_eq!(parse_github_url("https://github.com/a/b/").unwrap().1, "b");
        // shorthand + dotted repo name
        assert_eq!(parse_github_url("owner/repo").unwrap().0, "owner");
        assert_eq!(parse_github_url("babel/babel.js").unwrap().1, "babel.js");
    }

    #[test]
    fn parse_rejects_unsafe_and_non_github() {
        assert!(parse_github_url("https://gitlab.com/a/b").is_err()); // not github
        assert!(parse_github_url("https://github.com/a/b; rm -rf /").is_err()); // shell meta
        assert!(parse_github_url("https://user:tok@github.com/a/b").is_err()); // creds (@)
        assert!(parse_github_url("https://github.com/../../etc").is_err()); // traversal
        assert!(parse_github_url("https://github.com/onlyone").is_err()); // not owner/repo
        assert!(parse_github_url("https://github.com/a/b$(whoami)").is_err()); // command sub
        assert!(parse_github_url("evil.com/x").is_err()); // dotted owner via shorthand
        assert!(parse_github_url("").is_err());
    }

    #[test]
    fn dest_must_be_sandboxed_under_home() {
        assert!(dest_is_safe(Path::new("/home/storage/imports/x")));
        assert!(!dest_is_safe(Path::new("/tmp/x")));
        assert!(!dest_is_safe(Path::new("/root/x")));
        assert!(!dest_is_safe(Path::new("/")));
        assert!(!dest_is_safe(Path::new("relative/x")));
        assert!(!dest_is_safe(Path::new("/home/../etc/x")));
    }

    #[test]
    fn vet_is_fail_closed() {
        let good = ImportSpec::new("owner/repo", "/home/storage/imports");
        assert!(vet(&good).is_ok());
        let mut d0 = good.clone();
        d0.depth = 0;
        assert!(vet(&d0).is_err());
        let mut big = good.clone();
        big.max_mb = 0;
        assert!(vet(&big).is_err());
        let mut to = good.clone();
        to.timeout_s = 9999;
        assert!(vet(&to).is_err());
        assert!(vet(&ImportSpec::new("owner/repo", "/tmp/x")).is_err());
        assert!(vet(&ImportSpec::new("evil.com/x", "/home/storage/imports")).is_err());
    }

    #[test]
    #[ignore]
    fn live_clone_octocat() {
        let sandbox = std::path::PathBuf::from("/home/storage/flux-legacy-import-test");
        let _ = std::fs::remove_dir_all(sandbox.join("octocat__Hello-World"));
        let spec = ImportSpec::new("octocat/Hello-World", sandbox.clone());
        match import(&spec) {
            ImportOutcome::Imported { path, size_mb } => {
                assert!(path.join(".git").is_dir(), "should be a real git clone");
                assert!(size_mb < 50, "tiny repo should be small, got {size_mb} MB");
                println!("LIVE-CLONE OK: {path:?} ({size_mb} MB)");
                let _ = std::fs::remove_dir_all(&path);
            }
            ImportOutcome::Rejected(e) => panic!("expected Imported, got Rejected: {e}"),
        }
    }

    #[test]
    fn disk_free_is_readable_for_home() {
        assert!(disk_free_mb(Path::new("/home")).map_or(false, |m| m > 0));
    }

    #[test]
    fn scan_escapes_flags_a_symlink_out_of_sandbox() {
        use std::os::unix::fs::symlink;
        let base = std::env::temp_dir().join("flux_legacy_escape_test");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("sub")).unwrap();
        std::fs::write(base.join("sub/ok.txt"), b"hi").unwrap();
        let _ = symlink("/etc/passwd", base.join("sub/evil"));
        let escapes = scan_escapes(&base);
        assert!(escapes.iter().any(|x| x.ends_with("evil")), "symlink to /etc must be flagged");
        assert!(!escapes.iter().any(|x| x.ends_with("ok.txt")), "a normal file is fine");
        let _ = std::fs::remove_dir_all(&base);
    }
}
