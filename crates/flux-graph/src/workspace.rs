// workspace.rs — Discover workspace members from root Cargo.toml.
//
// Parses [workspace] members, resolves glob patterns like "crates/*",
// and returns the list of member crate directories.

use std::fs;
use std::path::PathBuf;

/// Discover all workspace member directories from the root Cargo.toml.
pub fn discover_members(root: &PathBuf) -> Result<Vec<PathBuf>, String> {
    let cargo_toml = root.join("Cargo.toml");
    let content = fs::read_to_string(&cargo_toml)
        .map_err(|e| format!("Cannot read {}: {}", cargo_toml.display(), e))?;

    let doc: toml::Value = toml::from_str(&content)
        .map_err(|e| format!("Invalid TOML in {}: {}", cargo_toml.display(), e))?;

    let members = doc
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
        .ok_or_else(|| format!("No [workspace] members found in {}", cargo_toml.display()))?;

    let mut paths = Vec::new();
    for member in members {
        let pattern = member.as_str()
            .ok_or_else(|| "workspace member is not a string".to_string())?;
        let resolved = resolve_glob(root, pattern)?;
        paths.extend(resolved);
    }

    if paths.is_empty() {
        return Err("No workspace members found".to_string());
    }

    Ok(paths)
}

/// Resolve a glob pattern like "crates/*" to concrete directories.
fn resolve_glob(root: &PathBuf, pattern: &str) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();

    if pattern.contains('*') {
        // Split on the glob: "crates/*" → ("crates/", "")
        let (prefix, _suffix) = match pattern.split_once('*') {
            Some((p, s)) => (p, s),
            None => return Err(format!("Invalid glob pattern: {}", pattern)),
        };

        let search_dir = root.join(prefix);
        if !search_dir.is_dir() {
            return Ok(paths); // non-existent dir, no matches
        }

        let entries = fs::read_dir(&search_dir)
            .map_err(|e| format!("Cannot read {}: {}", search_dir.display(), e))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("Cargo.toml").exists() {
                // ABSOLUTE member path — manifests must be readable independent of CWD.
                // The MCP server runs from a different cwd than the workspace root, so a
                // relative "crates/foo" got resolved against the wrong cwd and hard-failed
                // the whole graph (the flux_api_generate "Cannot read crates/.../Cargo.toml").
                paths.push(path);
            }
        }
    } else {
        // Literal path — push the absolute (root-joined) candidate, not the bare pattern.
        let candidate = root.join(pattern);
        if candidate.join("Cargo.toml").exists() {
            paths.push(candidate);
        }
    }

    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discover_members() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().parent().unwrap().to_path_buf();
        let members = discover_members(&root);
        assert!(members.is_ok(), "discover_members failed: {:?}", members.err());
        let members = members.unwrap();
        assert!(!members.is_empty(), "no members found");
        // flux-graph itself should be in the list
        let has_self = members.iter().any(|p| p.to_string_lossy().contains("flux-graph"));
        assert!(has_self, "flux-graph not found in workspace members");
    }
}
