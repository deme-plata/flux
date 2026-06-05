// v0.17-A — publish bundler.
//
// Assemble per-language SDK bundles (source + minimal manifest) and write
// them to a directory. Real uploads to npm/PyPI/crates/Maven/Go-module-proxy
// are deferred to v0.17.x — for now `publish_dry_run` is the proof that the
// pipeline produces the right artifacts. Each language's manifest is the
// minimum that the target registry CLI accepts:
//   * TypeScript → package.json
//   * Python     → pyproject.toml
//   * Go         → go.mod
//   * Rust       → Cargo.toml
//   * Kotlin     → build.gradle.kts (Maven-publishable)

use crate::{
    generate_go_sdk, generate_kotlin_sdk, generate_python_sdk, generate_rust_client_sdk,
    generate_typescript_sdk, ApiEndpoint,
};
use std::path::{Path, PathBuf};

/// One package ready to ship to a language registry.
#[derive(Debug, Clone)]
pub struct SdkBundle {
    pub language: Language,
    pub package_name: String,
    pub version: String,
    /// File name + body pairs that compose the bundle. Layout differs per
    /// language: TS = `package.json` + `index.ts`; Python = `pyproject.toml`
    /// + `src/<pkg>/__init__.py` + `src/<pkg>/client.py`; etc.
    pub files: Vec<(PathBuf, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    TypeScript,
    Python,
    Go,
    Rust,
    Kotlin,
}

impl Language {
    pub fn slug(self) -> &'static str {
        match self {
            Language::TypeScript => "ts",
            Language::Python => "py",
            Language::Go => "go",
            Language::Rust => "rust",
            Language::Kotlin => "kotlin",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PublishPlan {
    pub package_name: String,
    pub version: String,
    pub base_url: String,
    pub bundles: Vec<SdkBundle>,
}

/// Build bundles for every language from a single endpoint list. The caller
/// can opt out of any language by filtering `plan.bundles` before calling
/// `publish_dry_run`.
pub fn plan_publish(
    package_name: &str,
    version: &str,
    base_url: &str,
    endpoints: &[ApiEndpoint],
) -> PublishPlan {
    let bundles = vec![
        ts_bundle(package_name, version, base_url, endpoints),
        py_bundle(package_name, version, base_url, endpoints),
        go_bundle(package_name, version, base_url, endpoints),
        rust_bundle(package_name, version, base_url, endpoints),
        kotlin_bundle(package_name, version, base_url, endpoints),
    ];
    PublishPlan {
        package_name: package_name.into(),
        version: version.into(),
        base_url: base_url.into(),
        bundles,
    }
}

/// Write every bundle's files under `<out_dir>/<lang-slug>/`. Returns the
/// full set of paths created so a CI step can `tar`/`zip` them.
pub fn publish_dry_run(plan: &PublishPlan, out_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut all = vec![];
    for b in &plan.bundles {
        let lang_dir = out_dir.join(b.language.slug());
        std::fs::create_dir_all(&lang_dir)?;
        for (rel, body) in &b.files {
            let p = lang_dir.join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&p, body)?;
            all.push(p);
        }
    }
    Ok(all)
}

fn ts_bundle(pkg: &str, ver: &str, base: &str, eps: &[ApiEndpoint]) -> SdkBundle {
    let manifest = format!(
        "{{\n  \"name\": \"{pkg}\",\n  \"version\": \"{ver}\",\n  \"main\": \"index.ts\",\n  \"types\": \"index.ts\"\n}}\n"
    );
    let src = generate_typescript_sdk(eps, base);
    SdkBundle {
        language: Language::TypeScript,
        package_name: pkg.into(),
        version: ver.into(),
        files: vec![
            (PathBuf::from("package.json"), manifest),
            (PathBuf::from("index.ts"), src),
        ],
    }
}

fn py_bundle(pkg: &str, ver: &str, base: &str, eps: &[ApiEndpoint]) -> SdkBundle {
    let manifest = format!(
        "[project]\nname = \"{pkg}\"\nversion = \"{ver}\"\nrequires-python = \">=3.10\"\ndependencies = [\"httpx>=0.27\"]\n"
    );
    let src = generate_python_sdk(eps, base);
    let py_pkg = pkg.replace('-', "_");
    SdkBundle {
        language: Language::Python,
        package_name: pkg.into(),
        version: ver.into(),
        files: vec![
            (PathBuf::from("pyproject.toml"), manifest),
            (
                PathBuf::from(format!("src/{py_pkg}/__init__.py")),
                "from .client import *  # noqa: F401,F403\n".into(),
            ),
            (PathBuf::from(format!("src/{py_pkg}/client.py")), src),
        ],
    }
}

fn go_bundle(pkg: &str, ver: &str, base: &str, eps: &[ApiEndpoint]) -> SdkBundle {
    // Go modules look like `example.com/owner/name`; we synthesize a
    // placeholder path that callers override before `go publish`.
    let module_path = format!("flux.example/{pkg}");
    let manifest = format!("module {module_path}\n\ngo 1.21\n");
    let go_pkg = pkg.replace('-', "");
    let src = generate_go_sdk(eps, base, &go_pkg);
    SdkBundle {
        language: Language::Go,
        package_name: module_path,
        version: ver.into(),
        files: vec![
            (PathBuf::from("go.mod"), manifest),
            (PathBuf::from("client.go"), src),
        ],
    }
}

fn rust_bundle(pkg: &str, ver: &str, base: &str, eps: &[ApiEndpoint]) -> SdkBundle {
    let manifest = format!(
        "[package]\nname = \"{pkg}\"\nversion = \"{ver}\"\nedition = \"2021\"\n\n[dependencies]\nreqwest = {{ version = \"0.12\", features = [\"json\"] }}\ntokio = {{ version = \"1\", features = [\"rt-multi-thread\", \"macros\"] }}\n"
    );
    let src = generate_rust_client_sdk(eps, base);
    SdkBundle {
        language: Language::Rust,
        package_name: pkg.into(),
        version: ver.into(),
        files: vec![
            (PathBuf::from("Cargo.toml"), manifest),
            (PathBuf::from("src/lib.rs"), src),
        ],
    }
}

fn kotlin_bundle(pkg: &str, ver: &str, base: &str, eps: &[ApiEndpoint]) -> SdkBundle {
    let group = "io.flux.generated";
    let manifest = format!(
        "plugins {{\n    kotlin(\"jvm\") version \"1.9.20\"\n    `maven-publish`\n}}\n\ngroup = \"{group}\"\nversion = \"{ver}\"\n\ndependencies {{\n    implementation(\"com.squareup.okhttp3:okhttp:4.12.0\")\n    implementation(\"org.jetbrains.kotlinx:kotlinx-coroutines-core:1.7.3\")\n}}\n\npublishing {{\n    publications {{\n        create<MavenPublication>(\"maven\") {{\n            groupId = \"{group}\"\n            artifactId = \"{pkg}\"\n            version = \"{ver}\"\n            from(components[\"java\"])\n        }}\n    }}\n}}\n"
    );
    let kt_pkg = format!("{group}.{}", pkg.replace('-', ""));
    let src = generate_kotlin_sdk(eps, base, &kt_pkg);
    SdkBundle {
        language: Language::Kotlin,
        package_name: format!("{group}:{pkg}:{ver}"),
        version: ver.into(),
        files: vec![
            (PathBuf::from("build.gradle.kts"), manifest),
            (PathBuf::from("src/main/kotlin/Client.kt"), src),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::discover_endpoints;

    fn ci(name: &str) -> flux_graph::CrateInfo {
        flux_graph::CrateInfo {
            name: name.into(),
            path: PathBuf::from("/tmp"),
            dependencies: vec![],
            edition: "2021".into(),
            crate_type: flux_graph::CrateType::Lib,
            features: vec![],
        }
    }
    fn ws(names: &[&str]) -> flux_graph::WorkspaceGraph {
        flux_graph::WorkspaceGraph {
            root: PathBuf::from("/tmp"),
            crates: names.iter().map(|n| ci(n)).collect(),
            batches: vec![],
        }
    }
    fn fixture_plan() -> PublishPlan {
        let eps = discover_endpoints(&ws(&["flux-ue-bridge"]));
        plan_publish("flux-ue-bridge-sdk", "0.17.0", "http://localhost:9989", &eps)
    }

    #[test]
    fn plan_includes_every_supported_language() {
        let plan = fixture_plan();
        let langs: Vec<_> = plan.bundles.iter().map(|b| b.language).collect();
        for l in [
            Language::TypeScript,
            Language::Python,
            Language::Go,
            Language::Rust,
            Language::Kotlin,
        ] {
            assert!(langs.contains(&l), "missing language {l:?}");
        }
    }

    #[test]
    fn ts_bundle_has_package_json_and_index() {
        let plan = fixture_plan();
        let ts = plan.bundles.iter().find(|b| b.language == Language::TypeScript).unwrap();
        let names: Vec<String> = ts.files.iter().map(|(p, _)| p.display().to_string()).collect();
        assert!(names.iter().any(|n| n == "package.json"));
        assert!(names.iter().any(|n| n == "index.ts"));
        let manifest = &ts.files.iter().find(|(p, _)| p.ends_with("package.json")).unwrap().1;
        assert!(manifest.contains("\"name\": \"flux-ue-bridge-sdk\""));
        assert!(manifest.contains("\"version\": \"0.17.0\""));
    }

    #[test]
    fn python_bundle_layout_is_src_pkg_init_client() {
        let plan = fixture_plan();
        let py = plan.bundles.iter().find(|b| b.language == Language::Python).unwrap();
        let paths: Vec<String> = py.files.iter().map(|(p, _)| p.display().to_string()).collect();
        assert!(paths.iter().any(|p| p == "pyproject.toml"));
        // dashes in pkg name become underscores in the python module path
        assert!(paths.iter().any(|p| p.starts_with("src/flux_ue_bridge_sdk/")));
        assert!(paths.iter().any(|p| p.ends_with("client.py")));
    }

    #[test]
    fn rust_bundle_has_cargo_toml_and_lib_rs() {
        let plan = fixture_plan();
        let rs = plan.bundles.iter().find(|b| b.language == Language::Rust).unwrap();
        let cargo = &rs.files.iter().find(|(p, _)| p.ends_with("Cargo.toml")).unwrap().1;
        assert!(cargo.contains("name = \"flux-ue-bridge-sdk\""));
        assert!(cargo.contains("reqwest"));
        assert!(rs.files.iter().any(|(p, _)| p == &PathBuf::from("src/lib.rs")));
    }

    #[test]
    fn dry_run_writes_every_file_under_lang_dir() {
        let plan = fixture_plan();
        let tmp = std::env::temp_dir().join(format!(
            "flux_api_publish_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        let written = publish_dry_run(&plan, &tmp).expect("dry run");
        assert!(!written.is_empty());
        for p in &written {
            assert!(p.exists(), "expected {} to exist", p.display());
        }
        // every lang has its own dir
        for lang in [
            Language::TypeScript,
            Language::Python,
            Language::Go,
            Language::Rust,
            Language::Kotlin,
        ] {
            let dir = tmp.join(lang.slug());
            assert!(dir.exists(), "missing dir for {}", lang.slug());
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
