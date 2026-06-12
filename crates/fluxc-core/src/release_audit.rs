// release_audit.rs - read-only release-lane split report.

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn print_release_audit() {
    let root = release_root();
    let cargo_toml = read(root.join("Cargo.toml"));
    let releases = read(root.join("RELEASES.md"));
    let plan_036 = root.join("docs/RELEASE_0.36.1_PLAN.md").exists();
    let fh3d = root.join("docs/flux-hunyuan3d-pipeline.md").exists();
    let saga_count = count_saga_files(&root.join("sagas"));
    let status = git_release_status(&root);

    let workspace_version = workspace_version(&cargo_toml).unwrap_or_else(|| "unknown".into());
    let release_head = first_heading(&releases).unwrap_or_else(|| "missing RELEASES.md heading".into());
    let current_drop = release_head.contains("v0.27") || workspace_version == "0.27.0";
    let future_plan_waiting = plan_036;

    println!("Flux Release Audit");
    println!("  workspace root:    {}", root.display());
    println!("  workspace version: {}", workspace_version);
    println!("  release ledger:    {}", release_head);
    println!("  0.36.1 plan:       {}", if plan_036 { "present" } else { "absent" });
    println!("  FH3D plan:         {}", if fh3d { "present" } else { "absent" });
    println!("  saga assets:       {}", saga_count);

    println!("\nRelease-file status:");
    if status.is_empty() {
        println!("  clean");
    } else {
        for line in &status {
            println!("  {}", line);
        }
    }

    println!("\nVerdict:");
    if current_drop && future_plan_waiting {
        println!("  HOLD: finish the v0.27.0 surprise-drop lane before any 0.36.1 bump.");
    } else if future_plan_waiting {
        println!("  READY: 0.36.1 planning docs are present; confirm tree cleanliness before bumping.");
    } else {
        println!("  READY: no future release plan detected in the standard docs path.");
    }

    println!("\nSuggested split:");
    println!("  A. v0.27 cut: Cargo.toml, Cargo.lock, RELEASES.md, and committed feature crates.");
    println!("  B. 0.36.1 plan: docs/RELEASE_0.36.1_PLAN.md only; keep future bump separate.");
    println!("  C. FH3D design: docs/flux-hunyuan3d-pipeline.md as design/proposal, not release-blocking.");
    println!("  D. sagas: narrative/media assets; stage separately from compiler or deploy changes.");
}

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn first_heading(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| line.starts_with('#'))
        .map(|line| line.trim_start_matches('#').trim().to_string())
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

fn count_saga_files(path: &Path) -> usize {
    std::fs::read_dir(path)
        .map(|entries| entries.filter_map(Result::ok).filter(|e| e.path().is_file()).count())
        .unwrap_or(0)
}

fn release_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_default();
    git_root(&cwd).unwrap_or(cwd)
}

fn git_root(cwd: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        None
    } else {
        Some(PathBuf::from(root))
    }
}

fn git_release_status(root: &Path) -> Vec<String> {
    let output = Command::new("git")
        .args([
            "status",
            "--short",
            "--",
            "Cargo.toml",
            "Cargo.lock",
            "RELEASES.md",
            "docs/RELEASE_0.36.1_PLAN.md",
            "docs/flux-hunyuan3d-pipeline.md",
            "sagas",
        ])
        .current_dir(root)
        .output();

    let Ok(output) = output else {
        return vec!["git status unavailable".into()];
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}
