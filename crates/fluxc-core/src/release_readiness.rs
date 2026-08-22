// release_readiness.rs — machine-checkable gate for docs/VERSION_LEDGER.md's release rule.
//
// The ledger states the rule in prose: "a release = a commit that bumps [the
// workspace version] + an annotated tag `vX.Y.0` on that commit + a CHANGELOG
// entry. No tag without all three." That rule has been broken before (v0.33.0
// was tagged without a bump or changelog, backfilled after the fact — see the
// ledger's Track B notes). This module makes the same three checks a single
// callable fact instead of something an agent has to remember to do by hand.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ReleaseReadiness {
    pub version: String,
    pub changelog_entry: bool,
    pub ledger_row: bool,
    pub git_tag_exists: bool,
    pub tag_matches_head: bool,
    pub commits_since_tag: u32,
    pub dirty_release_files: Vec<String>,
    pub missing: Vec<String>,
    /// One of: "released", "stale_tag", "ready_to_tag", "not_ready".
    pub verdict: String,
}

impl ReleaseReadiness {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }
}

/// `root` should be the workspace root (the directory containing the
/// workspace `Cargo.toml`). Read-only: never mutates the tree or the repo.
pub fn check(root: &Path) -> ReleaseReadiness {
    let cargo_toml = read(&root.join("Cargo.toml"));
    let version = workspace_version(&cargo_toml).unwrap_or_else(|| "unknown".to_string());

    let changelog = read(&root.join("CHANGELOG.md"));
    let changelog_entry = changelog_has_entry(&changelog, &version);

    let ledger = read(&root.join("docs/VERSION_LEDGER.md"));
    let ledger_row = ledger_has_row(&ledger, &version);

    let tag = format!("v{version}");
    let git_tag_exists = git_tag_exists(root, &tag);
    let tag_matches_head = git_tag_exists && git_tag_matches_head(root, &tag);
    let commits_since_tag = if git_tag_exists && !tag_matches_head {
        commits_since(root, &tag)
    } else {
        0
    };
    let dirty_release_files = dirty_release_files(root);

    let mut missing = Vec::new();
    if !changelog_entry {
        missing.push(format!("CHANGELOG.md has no heading for v{version}"));
    }
    if !ledger_row {
        missing.push(format!("docs/VERSION_LEDGER.md has no Track B row for v{version}"));
    }
    if !dirty_release_files.is_empty() {
        missing.push(format!(
            "uncommitted changes in release files: {}",
            dirty_release_files.join(", ")
        ));
    }

    let verdict = if git_tag_exists && tag_matches_head {
        "released"
    } else if git_tag_exists {
        missing.push(format!(
            "tag {tag} exists but HEAD is {commits_since_tag} commit(s) ahead of it — bump the workspace version before cutting the next release"
        ));
        "stale_tag"
    } else if changelog_entry && ledger_row && dirty_release_files.is_empty() {
        "ready_to_tag"
    } else {
        "not_ready"
    };

    ReleaseReadiness {
        version,
        changelog_entry,
        ledger_row,
        git_tag_exists,
        tag_matches_head,
        commits_since_tag,
        dirty_release_files,
        missing,
        verdict: verdict.to_string(),
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn workspace_version(cargo_toml: &str) -> Option<String> {
    let mut in_workspace_package = false;
    for raw in cargo_toml.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_workspace_package = line == "[workspace.package]";
            continue;
        }
        if !in_workspace_package || !line.starts_with("version") {
            continue;
        }
        let (_, value) = line.split_once('=')?;
        return Some(value.trim().trim_matches('"').to_string());
    }
    None
}

/// A CHANGELOG heading looks like `## v0.40.0 — Receipts everywhere (2026-08-08)`.
/// Match on `## v{version}` at a word boundary so `v0.4.0` doesn't false-match
/// a search for `v0.40.0` (or vice versa).
fn changelog_has_entry(changelog: &str, version: &str) -> bool {
    let needle = format!("v{version}");
    changelog.lines().any(|raw| {
        let line = raw.trim();
        let Some(rest) = line.trim_start_matches('#').trim_start().strip_prefix(&needle) else {
            return false;
        };
        rest.chars().next().is_none_or(|c| !c.is_ascii_digit() && c != '.')
    })
}

/// A ledger Track B row looks like `| v0.40.0 | ... | ... |`.
fn ledger_has_row(ledger: &str, version: &str) -> bool {
    let needle = format!("v{version}");
    ledger.lines().any(|raw| {
        let line = raw.trim();
        if !line.starts_with('|') {
            return false;
        }
        let Some(cell) = line.split('|').nth(1) else {
            return false;
        };
        cell.trim() == needle
    })
}

fn git_tag_exists(root: &Path, tag: &str) -> bool {
    Command::new("git")
        .args(["tag", "-l", tag])
        .current_dir(root)
        .output()
        .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(false)
}

fn git_tag_matches_head(root: &Path, tag: &str) -> bool {
    let tag_sha = git_rev_parse(root, &format!("{tag}^{{commit}}"));
    let head_sha = git_rev_parse(root, "HEAD");
    matches!((tag_sha, head_sha), (Some(a), Some(b)) if a == b)
}

fn git_rev_parse(root: &Path, rev: &str) -> Option<String> {
    let output = Command::new("git").args(["rev-parse", rev]).current_dir(root).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sha.is_empty() {
        None
    } else {
        Some(sha)
    }
}

fn commits_since(root: &Path, tag: &str) -> u32 {
    Command::new("git")
        .args(["rev-list", "--count", &format!("{tag}..HEAD")])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(0)
}

fn dirty_release_files(root: &Path) -> Vec<String> {
    let output = Command::new("git")
        .args([
            "status",
            "--short",
            "--",
            "Cargo.toml",
            "Cargo.lock",
            "CHANGELOG.md",
            "docs/VERSION_LEDGER.md",
        ])
        .current_dir(root)
        .output();
    let Ok(output) = output else {
        return vec!["git status unavailable".to_string()];
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    //! Pure-parse tests — same rationale as release_audit.rs's release_parse_tests:
    //! wrong parsing here silently lets a bad release through the gate.
    use super::*;

    #[test]
    fn changelog_has_entry_matches_exact_version_heading() {
        let cl = "# Flux\n\n## v0.40.0 — Receipts everywhere (2026-08-08)\n\nbody\n";
        assert!(changelog_has_entry(cl, "0.40.0"));
        assert!(!changelog_has_entry(cl, "0.41.0"));
    }

    #[test]
    fn changelog_has_entry_rejects_prefix_collision() {
        let cl = "## v0.40.0 — theme\n";
        // v0.4.0 must NOT match against a v0.40.0 heading, and vice versa.
        assert!(!changelog_has_entry(cl, "0.4.0"));
        let cl2 = "## v0.4.0 — theme\n";
        assert!(!changelog_has_entry(cl2, "0.40.0"));
    }

    #[test]
    fn changelog_has_entry_handles_missing_and_empty() {
        assert!(!changelog_has_entry("", "0.40.0"));
        assert!(!changelog_has_entry("no headings here\njust text", "0.40.0"));
    }

    #[test]
    fn ledger_has_row_matches_track_b_cell() {
        let ledger = "| Tag | Date | Theme |\n|---|---|---|\n| v0.40.0 | 2026-08-08 | Receipts |\n";
        assert!(ledger_has_row(ledger, "0.40.0"));
        assert!(!ledger_has_row(ledger, "0.41.0"));
    }

    #[test]
    fn ledger_has_row_ignores_non_table_lines() {
        let ledger = "v0.40.0 mentioned in prose, not a table row\n";
        assert!(!ledger_has_row(ledger, "0.40.0"));
    }

    #[test]
    fn workspace_version_reads_only_workspace_package_table() {
        assert_eq!(
            workspace_version("[workspace.package]\nversion = \"1.2.3\""),
            Some("1.2.3".into())
        );
        assert_eq!(workspace_version("[package]\nversion = \"9.9.9\""), None);
        assert_eq!(
            workspace_version("[package]\nversion=\"0.0.1\"\n[workspace.package]\nversion=\"4.5.6\""),
            Some("4.5.6".into())
        );
        assert_eq!(workspace_version(""), None);
    }

    #[test]
    fn check_on_missing_files_reports_not_ready_with_reasons() {
        // A directory with no Cargo.toml/CHANGELOG/ledger and no git repo:
        // every signal should fail closed (never silently "ready").
        let dir = std::env::temp_dir().join(format!(
            "flux-release-readiness-test-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let result = check(&dir);
        assert_eq!(result.version, "unknown");
        assert!(!result.changelog_entry);
        assert!(!result.ledger_row);
        assert!(!result.git_tag_exists);
        assert_eq!(result.verdict, "not_ready");
        assert!(!result.missing.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
