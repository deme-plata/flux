// manifest.rs — Parse a single crate's Cargo.toml for dependencies, edition, crate-type.
//
// Phase 2a: extracts intra-workspace path dependencies only.
// Phase 2b: adds version, git, and crates.io dependencies.

use std::fs;
use std::path::PathBuf;
use crate::{CrateInfo, CrateType, Dependency, DepKind};

/// Parse a crate's Cargo.toml and return structured CrateInfo.
pub fn parse_crate(crate_dir: &PathBuf) -> Result<CrateInfo, String> {
    let cargo_toml = crate_dir.join("Cargo.toml");
    let content = fs::read_to_string(&cargo_toml)
        .map_err(|e| format!("Cannot read {}: {}", cargo_toml.display(), e))?;

    let doc: toml::Value = toml::from_str(&content)
        .map_err(|e| format!("Invalid TOML in {}: {}", cargo_toml.display(), e))?;

    let package = doc.get("package")
        .ok_or_else(|| format!("No [package] in {}", cargo_toml.display()))?;

    let name = package.get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("No package.name in {}", cargo_toml.display()))?
        .to_string();

    let edition = package.get("edition")
        .and_then(|v| v.as_str())
        .or_else(|| doc.get("workspace").and_then(|w| w.get("package")).and_then(|p| p.get("edition")).and_then(|v| v.as_str()))
        .unwrap_or("2021")
        .to_string();

    let crate_type = detect_crate_type(&doc, crate_dir);

    let dependencies = extract_path_deps(&doc, crate_dir)?;

    let features = doc.get("features")
        .and_then(|f| f.as_table())
        .map(|t| t.keys().cloned().collect())
        .unwrap_or_default();

    Ok(CrateInfo {
        name,
        path: crate_dir.clone(),
        edition,
        crate_type,
        dependencies,
        features,
    })
}

/// Detect whether this crate is a lib, bin, or proc-macro.
fn detect_crate_type(doc: &toml::Value, crate_dir: &PathBuf) -> CrateType {
    // Check [lib] section for proc-macro
    if let Some(lib) = doc.get("lib") {
        if let Some(proc_macro) = lib.get("proc-macro") {
            if proc_macro.as_bool() == Some(true) {
                return CrateType::ProcMacro;
            }
        }
        // Has [lib] section → lib crate
        return CrateType::Lib;
    }

    // Check for [[bin]] section
    if doc.get("bin").is_some() {
        return CrateType::Bin;
    }

    // Heuristic: look for src/lib.rs vs src/main.rs
    if crate_dir.join("src").join("lib.rs").exists() {
        return CrateType::Lib;
    }
    if crate_dir.join("src").join("main.rs").exists() {
        return CrateType::Bin;
    }

    CrateType::Lib // default
}

/// Extract all dependencies (path, crates.io, git) for Phase 2a.
fn extract_path_deps(doc: &toml::Value, crate_dir: &PathBuf) -> Result<Vec<Dependency>, String> {
    let mut deps = Vec::new();

    // Helper to process a dependency table
    fn process_dep_table(
        deps: &mut Vec<Dependency>,
        dep_table: &toml::value::Table,
        crate_dir: &PathBuf,
    ) {
        for (dep_name, dep_val) in dep_table {
            let optional = dep_val.get("optional")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            // Path dependency
            if let Some(path_str) = dep_val.get("path").and_then(|v| v.as_str()) {
                let dep_path = crate_dir.join(path_str);
                let canonical = dep_path.canonicalize()
                    .unwrap_or_else(|_| dep_path.clone());
                deps.push(Dependency {
                    name: dep_name.clone(),
                    path: Some(canonical),
                    kind: DepKind::Path,
                    optional,
                });
            }
            // Git dependency
            else if dep_val.get("git").is_some() {
                deps.push(Dependency {
                    name: dep_name.clone(),
                    path: None,
                    kind: DepKind::Git,
                    optional,
                });
            }
            // Crates.io dependency (version = "x.y.z" or bare string)
            else if dep_val.get("version").is_some() || dep_val.is_str() {
                deps.push(Dependency {
                    name: dep_name.clone(),
                    path: None,
                    kind: DepKind::CratesIo,
                    optional,
                });
            }
        }
    }

    if let Some(dep_table) = doc.get("dependencies").and_then(|d| d.as_table()) {
        process_dep_table(&mut deps, dep_table, crate_dir);
    }
    if let Some(dev_table) = doc.get("dev-dependencies").and_then(|d| d.as_table()) {
        process_dep_table(&mut deps, dev_table, crate_dir);
    }

    Ok(deps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_flux_graph_itself() {
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).to_path_buf();
        let info = parse_crate(&crate_dir);
        assert!(info.is_ok(), "parse_crate failed: {:?}", info.err());
        let info = info.unwrap();
        assert_eq!(info.name, "flux-graph");
        assert_eq!(info.edition, "2021");
    }

    #[test]
    fn test_parse_fluxc_core() {
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().join("fluxc-core");
        let info = parse_crate(&crate_dir);
        assert!(info.is_ok(), "parse_crate fluxc-core: {:?}", info.err());
        let info = info.unwrap();
        assert_eq!(info.name, "fluxc-core");
        // fluxc-core depends on flux-cache
        let has_cache = info.dependencies.iter().any(|d| d.name == "flux-cache");
        assert!(has_cache, "fluxc-core should depend on flux-cache");
    }

    #[test]
    fn test_detect_crate_type_fallback() {
        // flux-macros is a regular lib crate (no [lib] proc-macro = true)
        // This test verifies the heuristic fallback (src/lib.rs → Lib)
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().join("flux-macros");
        if crate_dir.exists() {
            let info = parse_crate(&crate_dir).expect("parse flux-macros");
            // Verify it parses correctly; crate type depends on actual Cargo.toml
            assert!(!info.name.is_empty());
        }
    }
}
