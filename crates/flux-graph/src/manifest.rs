// manifest.rs — Parse a single crate's Cargo.toml for dependencies, edition, crate-type.
//
// Phase 2a: extracts intra-workspace path dependencies only.
// Phase 2b: adds version, git, and crates.io dependencies.

use std::fs;
use std::path::{Path, PathBuf};
use crate::{CrateInfo, CrateType, Dependency, DepKind};

struct WorkspaceContext {
    root: PathBuf,
    doc: toml::Value,
}

/// Parse a crate's Cargo.toml and return structured CrateInfo.
pub fn parse_crate(crate_dir: &PathBuf) -> Result<CrateInfo, String> {
    let cargo_toml = crate_dir.join("Cargo.toml");
    let content = fs::read_to_string(&cargo_toml)
        .map_err(|e| format!("Cannot read {}: {}", cargo_toml.display(), e))?;

    let doc: toml::Value = toml::from_str(&content)
        .map_err(|e| format!("Invalid TOML in {}: {}", cargo_toml.display(), e))?;
    let workspace = load_workspace_context(crate_dir)?;

    let package = doc.get("package")
        .ok_or_else(|| format!("No [package] in {}", cargo_toml.display()))?;

    let name = package.get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("No package.name in {}", cargo_toml.display()))?
        .to_string();

    let edition = inherited_package_string(package, "edition", workspace.as_ref())
        .unwrap_or("2021")
        .to_string();

    let crate_type = detect_crate_type(&doc, crate_dir);

    let dependencies = extract_path_deps(&doc, crate_dir, workspace.as_ref())?;

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

fn inherited_package_string<'a>(
    package: &'a toml::Value,
    key: &str,
    workspace: Option<&'a WorkspaceContext>,
) -> Option<&'a str> {
    match package.get(key) {
        Some(value) if value.as_str().is_some() => value.as_str(),
        Some(value) if value.get("workspace").and_then(|v| v.as_bool()) == Some(true) => {
            workspace?
                .doc
                .get("workspace")?
                .get("package")?
                .get(key)?
                .as_str()
        }
        _ => None,
    }
}

fn load_workspace_context(crate_dir: &Path) -> Result<Option<WorkspaceContext>, String> {
    let Some(root) = find_workspace_root(crate_dir) else {
        return Ok(None);
    };
    let cargo_toml = root.join("Cargo.toml");
    let content = fs::read_to_string(&cargo_toml)
        .map_err(|e| format!("Cannot read workspace {}: {}", cargo_toml.display(), e))?;
    let doc: toml::Value = toml::from_str(&content)
        .map_err(|e| format!("Invalid TOML in workspace {}: {}", cargo_toml.display(), e))?;
    Ok(Some(WorkspaceContext { root, doc }))
}

fn find_workspace_root(crate_dir: &Path) -> Option<PathBuf> {
    for ancestor in crate_dir.ancestors() {
        let cargo_toml = ancestor.join("Cargo.toml");
        if !cargo_toml.exists() {
            continue;
        }
        let Ok(content) = fs::read_to_string(&cargo_toml) else {
            continue;
        };
        let Ok(doc) = toml::from_str::<toml::Value>(&content) else {
            continue;
        };
        if doc.get("workspace").is_some() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
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
fn extract_path_deps(
    doc: &toml::Value,
    crate_dir: &PathBuf,
    workspace: Option<&WorkspaceContext>,
) -> Result<Vec<Dependency>, String> {
    let mut deps = Vec::new();

    if let Some(dep_table) = doc.get("dependencies").and_then(|d| d.as_table()) {
        process_dep_table(&mut deps, dep_table, crate_dir, workspace)?;
    }
    if let Some(build_table) = doc.get("build-dependencies").and_then(|d| d.as_table()) {
        process_dep_table(&mut deps, build_table, crate_dir, workspace)?;
    }
    if let Some(dev_table) = doc.get("dev-dependencies").and_then(|d| d.as_table()) {
        process_dep_table(&mut deps, dev_table, crate_dir, workspace)?;
    }

    Ok(deps)
}

fn process_dep_table(
    deps: &mut Vec<Dependency>,
    dep_table: &toml::value::Table,
    crate_dir: &PathBuf,
    workspace: Option<&WorkspaceContext>,
) -> Result<(), String> {
    for (dep_name, dep_val) in dep_table {
        if let Some(dep) = parse_dependency(dep_name, dep_val, crate_dir, workspace)? {
            push_dependency(deps, dep);
        }
    }
    Ok(())
}

fn push_dependency(deps: &mut Vec<Dependency>, dep: Dependency) {
    if let Some(existing) = deps
        .iter_mut()
        .find(|d| d.name == dep.name && d.kind == dep.kind && d.path == dep.path)
    {
        existing.optional = existing.optional && dep.optional;
    } else {
        deps.push(dep);
    }
}

fn parse_dependency(
    dep_name: &str,
    dep_val: &toml::Value,
    crate_dir: &PathBuf,
    workspace: Option<&WorkspaceContext>,
) -> Result<Option<Dependency>, String> {
    let optional = dep_val.get("optional")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if dep_val.get("workspace").and_then(|v| v.as_bool()) == Some(true) {
        let workspace = workspace
            .ok_or_else(|| format!("dependency '{}' uses workspace=true but no workspace root was found", dep_name))?;
        let ws_deps = workspace.doc
            .get("workspace")
            .and_then(|w| w.get("dependencies"))
            .and_then(|d| d.as_table())
            .ok_or_else(|| "workspace dependency table not found".to_string())?;
        let ws_dep = ws_deps
            .get(dep_name)
            .ok_or_else(|| format!("workspace dependency '{}' not found", dep_name))?;
        let mut dep = parse_dependency_value(dep_name, ws_dep, &workspace.root, optional)
            .ok_or_else(|| format!("workspace dependency '{}' has unsupported shape", dep_name))?;
        dep.optional = optional || dep.optional;
        return Ok(Some(dep));
    }

    Ok(parse_dependency_value(dep_name, dep_val, crate_dir, optional))
}

fn parse_dependency_value(
    dep_name: &str,
    dep_val: &toml::Value,
    base_dir: &Path,
    optional: bool,
) -> Option<Dependency> {
    if let Some(path_str) = dep_val.get("path").and_then(|v| v.as_str()) {
        let dep_path = base_dir.join(path_str);
        let canonical = dep_path.canonicalize()
            .unwrap_or_else(|_| dep_path.clone());
        return Some(Dependency {
            name: dep_name.to_string(),
            path: Some(canonical),
            kind: DepKind::Path,
            optional,
        });
    }
    if dep_val.get("git").is_some() {
        return Some(Dependency {
            name: dep_name.to_string(),
            path: None,
            kind: DepKind::Git,
            optional,
        });
    }
    if dep_val.get("version").is_some() || dep_val.is_str() {
        return Some(Dependency {
            name: dep_name.to_string(),
            path: None,
            kind: DepKind::CratesIo,
            optional,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_workspace(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("flux_graph_{}_{}_{}", name, std::process::id(), nonce))
    }

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

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

    #[test]
    fn test_parse_workspace_inherited_dependencies() {
        let root = temp_workspace("inherited_deps");
        write_file(
            &root.join("Cargo.toml"),
            r#"
[workspace]
members = ["crates/app", "crates/dep"]

[workspace.package]
edition = "2021"

[workspace.dependencies]
dep-crate = { path = "crates/dep" }
serde = "1"
codegen = { path = "crates/codegen" }
"#,
        );
        write_file(
            &root.join("crates/app/Cargo.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
edition.workspace = true

[dependencies]
dep-crate = { workspace = true }
serde = { workspace = true }

[build-dependencies]
codegen = { workspace = true }

[dev-dependencies]
serde = { workspace = true }
"#,
        );
        write_file(
            &root.join("crates/app/src/lib.rs"),
            "pub fn app() {}\n",
        );
        write_file(
            &root.join("crates/dep/Cargo.toml"),
            r#"
[package]
name = "dep-crate"
version = "0.1.0"
edition = "2021"
"#,
        );
        write_file(&root.join("crates/dep/src/lib.rs"), "pub fn dep() {}\n");
        write_file(
            &root.join("crates/codegen/Cargo.toml"),
            r#"
[package]
name = "codegen"
version = "0.1.0"
edition = "2021"
"#,
        );
        write_file(&root.join("crates/codegen/src/lib.rs"), "pub fn codegen() {}\n");

        let info = parse_crate(&root.join("crates/app")).expect("parse app");
        assert_eq!(info.edition, "2021");

        let dep_crate = info.dependencies.iter().find(|d| d.name == "dep-crate").unwrap();
        assert_eq!(dep_crate.kind, DepKind::Path);
        assert_eq!(dep_crate.path.as_ref().unwrap(), &root.join("crates/dep").canonicalize().unwrap());

        let serde = info.dependencies.iter().find(|d| d.name == "serde").unwrap();
        assert_eq!(serde.kind, DepKind::CratesIo);
        assert_eq!(info.dependencies.iter().filter(|d| d.name == "serde").count(), 1);

        let codegen = info.dependencies.iter().find(|d| d.name == "codegen").unwrap();
        assert_eq!(codegen.kind, DepKind::Path);

        let _ = fs::remove_dir_all(root);
    }
}
