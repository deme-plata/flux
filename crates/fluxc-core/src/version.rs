// fluxc-core version management — centralized semver control
//
// Reads/writes workspace Cargo.toml. All crates should use version.workspace = true.
// This module is the single source of truth for Flux version numbers.

use std::fs;
use std::path::{Path, PathBuf};

/// Current workspace version, parsed from Cargo.toml at startup.
#[derive(Debug, Clone)]
pub struct VersionInfo {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub raw: String,
    pub workspace_path: PathBuf,
}

impl VersionInfo {
    /// Load version from the workspace Cargo.toml.
    /// Walks up from `base_dir` looking for a workspace Cargo.toml with `[workspace]`.
    pub fn load(base_dir: &Path) -> Result<Self, String> {
        let ws_toml = find_workspace_toml(base_dir)?;
        let content = fs::read_to_string(&ws_toml)
            .map_err(|e| format!("Cannot read {}: {}", ws_toml.display(), e))?;

        // Parse: version = "x.y.z" under [workspace.package]
        let version_str = extract_workspace_version(&content)?;
        let (major, minor, patch) = parse_semver(&version_str)?;

        Ok(VersionInfo {
            major,
            minor,
            patch,
            raw: version_str,
            workspace_path: ws_toml,
        })
    }

    /// Bump the version and write back to Cargo.toml.
    /// Returns the new version string.
    pub fn bump(&mut self, component: &str) -> Result<String, String> {
        match component {
            "major" => {
                self.major += 1;
                self.minor = 0;
                self.patch = 0;
            }
            "minor" => {
                self.minor += 1;
                self.patch = 0;
            }
            "patch" => {
                self.patch += 1;
            }
            _ => return Err(format!("Unknown component: {}. Use major, minor, or patch.", component)),
        };
        self.raw = format!("{}.{}.{}", self.major, self.minor, self.patch);

        let content = fs::read_to_string(&self.workspace_path)
            .map_err(|e| format!("Cannot read {}: {}", self.workspace_path.display(), e))?;

        // Replace the version line under [workspace.package]
        let old_pattern = format!("version = \"{}\"", find_current_version_in_content(&content)?);
        if !content.contains(&old_pattern) {
            return Err(format!("Version pattern '{}' not found in {}", old_pattern, self.workspace_path.display()));
        }

        let new_content = content.replace(
            &old_pattern,
            &format!("version = \"{}\"", self.raw),
        );

        fs::write(&self.workspace_path, &new_content)
            .map_err(|e| format!("Cannot write {}: {}", self.workspace_path.display(), e))?;

        Ok(self.raw.clone())
    }

    /// Report version as "v{major}.{minor}.{patch}"
    pub fn display(&self) -> String {
        format!("v{}.{}.{}", self.major, self.minor, self.patch)
    }

    /// Human-readable one-liner for the binary.
    pub fn banner(&self) -> String {
        format!(
            "fluxc {} — Universal Build Orchestrator (Rust + Frontend + Full-Stack + Supercluster + Persistent Stats + Self-Hosting)",
            self.raw
        )
    }
}

/// Resolve the Flux workspace root robustly.
///
/// Order of precedence:
/// 1. `Q_FLUX_WORKSPACE` env var (explicit override — preferred for tests / CI).
/// 2. Walk up from `std::env::current_exe()` looking for `[workspace]` Cargo.toml.
///    The binary's location is stable; cwd isn't (MCP servers often run from a
///    random parent shell's cwd like `/home/storage/claude-code`).
/// 3. Walk up from `std::env::current_dir()` as a final fallback.
/// 4. `.` if nothing found, so callers that pass `&workspace_root().to_string_lossy()`
///    degrade to today's behavior rather than panicking.
pub fn workspace_root() -> PathBuf {
    if let Ok(p) = std::env::var("Q_FLUX_WORKSPACE") {
        let path = PathBuf::from(p);
        if path.exists() {
            return path;
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            if let Ok(toml) = find_workspace_toml(parent) {
                if let Some(root) = toml.parent() {
                    return root.to_path_buf();
                }
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(toml) = find_workspace_toml(&cwd) {
            if let Some(root) = toml.parent() {
                return root.to_path_buf();
            }
        }
    }

    PathBuf::from(".")
}

/// Find the workspace-level Cargo.toml by walking up from base_dir.
fn find_workspace_toml(base_dir: &Path) -> Result<PathBuf, String> {
    let mut current = base_dir.to_path_buf();
    loop {
        let candidate = current.join("Cargo.toml");
        if candidate.exists() {
            if let Ok(content) = fs::read_to_string(&candidate) {
                if content.contains("[workspace]") {
                    return Ok(candidate);
                }
            }
        }
        if !current.pop() {
            return Err("No workspace Cargo.toml found".into());
        }
    }
}

/// Extract the `version = "x.y.z"` from `[workspace.package]` section.
fn extract_workspace_version(content: &str) -> Result<String, String> {
    let in_ws_package = content
        .lines()
        .skip_while(|l| !l.trim().starts_with("[workspace.package]"))
        .skip(1)
        .take_while(|l| !l.trim().starts_with('['));

    for line in in_ws_package {
        let trimmed = line.trim();
        if trimmed.starts_with("version") {
            if let Some(start) = trimmed.find('"') {
                if let Some(end) = trimmed[start + 1..].find('"') {
                    return Ok(trimmed[start + 1..start + 1 + end].to_string());
                }
            }
        }
    }
    Err("No version = \"x.y.z\" found in [workspace.package]".into())
}

fn find_current_version_in_content(content: &str) -> Result<String, String> {
    extract_workspace_version(content)
}

fn parse_semver(s: &str) -> Result<(u32, u32, u32), String> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 3 {
        return Err(format!("Invalid semver: {}", s));
    }
    let major = parts[0].parse::<u32>().map_err(|_| format!("Invalid major: {}", parts[0]))?;
    let minor = parts[1].parse::<u32>().map_err(|_| format!("Invalid minor: {}", parts[1]))?;
    let patch = parts[2].parse::<u32>().map_err(|_| format!("Invalid patch: {}", parts[2]))?;
    Ok((major, minor, patch))
}

/// Scan all crates/*/Cargo.toml for version consistency.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CrateVersionStatus {
    pub name: String,
    pub version: String,
    pub source: String, // "workspace" or literal value
    pub consistent: bool,
}

pub fn audit_crate_versions(workspace_dir: &Path) -> Result<Vec<CrateVersionStatus>, String> {
    let vi = VersionInfo::load(workspace_dir)?;
    let crates_dir = workspace_dir.join("crates");
    let mut results = Vec::new();

    for entry in fs::read_dir(&crates_dir)
        .map_err(|e| format!("Cannot read {}: {}", crates_dir.display(), e))?
    {
        let entry = entry.map_err(|e| e.to_string())?;
        let toml_path = entry.path().join("Cargo.toml");
        if !toml_path.exists() {
            continue;
        }
        let content = fs::read_to_string(&toml_path).unwrap_or_default();
        let name = entry.file_name().to_string_lossy().to_string();

        let in_package = content
            .lines()
            .skip_while(|l| !l.trim().starts_with("[package]"))
            .take_while(|l| !l.trim().is_empty() || l == &"[package]");

        let mut crate_version = String::new();
        let mut is_workspace = false;
        for line in in_package {
            let t = line.trim();
            if t.starts_with("version.workspace") {
                is_workspace = true;
                crate_version = vi.raw.clone();
                break;
            }
            if t.starts_with("version") && !t.contains("workspace") {
                if let Some(start) = t.find('"') {
                    if let Some(end) = t[start + 1..].find('"') {
                        crate_version = t[start + 1..start + 1 + end].to_string();
                    }
                }
                break;
            }
        }

        results.push(CrateVersionStatus {
            consistent: is_workspace,
            source: if is_workspace {
                "workspace".into()
            } else {
                crate_version.clone()
            },
            version: if crate_version.is_empty() {
                "unknown".into()
            } else {
                crate_version
            },
            name,
        });
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_semver() {
        assert_eq!(parse_semver("0.9.17").unwrap(), (0, 9, 17));
        assert_eq!(parse_semver("1.0.0").unwrap(), (1, 0, 0));
        assert!(parse_semver("0.9").is_err());
        assert!(parse_semver("abc").is_err());
    }

    #[test]
    fn test_extract_version() {
        let toml = r#"
[workspace]
members = ["crates/*"]

[workspace.package]
version = "0.9.17"
edition = "2021"
"#;
        assert_eq!(extract_workspace_version(toml).unwrap(), "0.9.17");
    }

    #[test]
    fn test_version_display() {
        let vi = VersionInfo {
            major: 0,
            minor: 9,
            patch: 17,
            raw: "0.9.17".into(),
            workspace_path: PathBuf::from("/tmp/Cargo.toml"),
        };
        assert_eq!(vi.display(), "v0.9.17");
        assert!(vi.banner().starts_with("fluxc 0.9.17"));
    }
}
