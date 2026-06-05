//! `flux_cross_compile` — build a package for every platform + glibc version.
//!
//! The portability fix the glibc-2.39-vs-2.35 wall demanded: a **musl-static**
//! binary has ZERO libc dependency, so ONE build runs on any Linux — 2.31,
//! 2.35, 2.39, Alpine, a rented Vast box, anything. This tool wraps that
//! (proven with sigil-scaling) plus ARM and Windows cross-targets, with honest
//! per-target guidance when a cross-toolchain isn't installed.
//!
//! Friendly target names → triples:
//!   linux-portable → x86_64-unknown-linux-musl   (static, any glibc — DEFAULT)
//!   linux-arm      → aarch64-unknown-linux-musl   (static ARM64)
//!   linux-gnu      → x86_64-unknown-linux-gnu     (native dynamic)
//!   windows        → x86_64-pc-windows-gnu        (needs mingw-w64)
//!   macos-intel    → x86_64-apple-darwin          (needs osxcross)
//!   macos-arm      → aarch64-apple-darwin         (needs osxcross)

use std::path::PathBuf;
use std::process::Command;

use serde_json::{json, Value};

use crate::handlers::{ws, ToolDef, ToolRegistry};

/// (friendly, triple, is_musl, cross_cc_apt_hint)
const TARGETS: &[(&str, &str, bool, &str)] = &[
    ("linux-portable", "x86_64-unknown-linux-musl", true, "musl-tools"),
    ("linux-arm", "aarch64-unknown-linux-musl", true, "gcc-aarch64-linux-gnu musl-tools"),
    ("linux-gnu", "x86_64-unknown-linux-gnu", false, ""),
    ("windows", "x86_64-pc-windows-gnu", false, "mingw-w64"),
    ("macos-intel", "x86_64-apple-darwin", false, "osxcross (manual)"),
    ("macos-arm", "aarch64-apple-darwin", false, "osxcross (manual)"),
];

fn resolve(name: &str) -> Option<&'static (&'static str, &'static str, bool, &'static str)> {
    TARGETS.iter().find(|t| t.0 == name || t.1 == name)
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_cross_compile",
            description: "Build a package for every platform + glibc version. The headline target 'linux-portable' is musl-static — one binary that runs on ANY Linux regardless of glibc (2.31/2.35/2.39/Alpine), which is what makes deploying to rented Vast boxes work without per-box builds. Also does ARM and Windows when the cross-toolchain is present; reports exactly what to install otherwise.",
            input_schema: json!({
                "type":"object",
                "properties":{
                    "package":{"type":"string","description":"Cargo package to build (-p)."},
                    "bin":{"type":"string","description":"Specific binary name (--bin). Optional."},
                    "targets":{"type":"array","items":{"type":"string"},"description":"Friendly names or triples. Default ['linux-portable']. Use ['linux-portable','linux-arm','windows'] for a full fan-out."},
                    "workspace_dir":{"type":"string","description":"Workspace to build in. Default the Flux workspace."},
                    "release":{"type":"boolean","description":"Release build. Default true."}
                },
                "required":["package"]
            }),
        },
        flux_cross_compile,
    );
}

fn flux_cross_compile(args: &Value) -> String {
    let package = match args.get("package").and_then(|v| v.as_str()) {
        Some(p) => p.to_string(),
        None => return "❌ 'package' is required.".into(),
    };
    let bin = args.get("bin").and_then(|v| v.as_str()).map(String::from);
    let release = args.get("release").and_then(|v| v.as_bool()).unwrap_or(true);
    let workspace = args
        .get("workspace_dir")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(ws);
    let targets: Vec<String> = args
        .get("targets")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .filter(|v: &Vec<String>| !v.is_empty())
        .unwrap_or_else(|| vec!["linux-portable".into()]);

    let mut out = format!("🛠  flux_cross_compile · package '{package}'\n  workspace: {}\n", workspace.display());
    let mut ok = 0;

    for name in &targets {
        let t = match resolve(name) {
            Some(t) => t,
            None => {
                out.push_str(&format!("  ✗ {name}: unknown target\n"));
                continue;
            }
        };
        let (friendly, triple, is_musl, hint) = (t.0, t.1, t.2, t.3);

        // Ensure the rustc std for this target is installed (best-effort).
        let _ = Command::new("rustup").args(["target", "add", triple]).output();

        let mut cmd = Command::new("cargo");
        cmd.current_dir(&workspace).arg("build");
        if release {
            cmd.arg("--release");
        }
        cmd.args(["-p", &package, "--target", triple]);
        if let Some(b) = &bin {
            cmd.args(["--bin", b]);
        }
        // musl + ring/openssl need a musl C compiler; wire it for x86_64.
        if is_musl && triple.starts_with("x86_64") {
            cmd.env("CC_x86_64_unknown_linux_musl", "musl-gcc");
            cmd.env("CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER", "musl-gcc");
        }

        let res = cmd.output();
        let profile = if release { "release" } else { "debug" };
        match res {
            Ok(o) if o.status.success() => {
                let stem = bin.clone().unwrap_or_else(|| package.clone());
                let ext = if triple.contains("windows") { ".exe" } else { "" };
                // Search the common output locations (workspace target + shared dir).
                let cand = [
                    workspace.join(format!("target/{triple}/{profile}/{stem}{ext}")),
                    PathBuf::from(format!("/home/storage/deepseek-codewhale/.target-shared/{triple}/{profile}/{stem}{ext}")),
                ];
                let found = cand.iter().find(|p| p.exists());
                if let Some(p) = found {
                    let sz = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0) as f64 / 1024.0;
                    let tag = if is_musl { " [static — any glibc]" } else { "" };
                    out.push_str(&format!("  ✓ {friendly} ({triple}): {:.0} KB{tag}\n      {}\n", sz, p.display()));
                    ok += 1;
                } else {
                    out.push_str(&format!("  ✓ {friendly} ({triple}): built (binary path not auto-located)\n"));
                    ok += 1;
                }
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                let reason = if err.contains("linker") || err.contains("not found") || err.contains("cc") {
                    if hint.is_empty() {
                        "build error".to_string()
                    } else {
                        format!("cross-toolchain missing — install: {hint}")
                    }
                } else {
                    let last = err.lines().rev().take(2).collect::<Vec<_>>().join(" | ");
                    format!("build failed: {last}")
                };
                out.push_str(&format!("  ✗ {friendly} ({triple}): {reason}\n"));
            }
            Err(e) => out.push_str(&format!("  ✗ {friendly} ({triple}): cargo spawn failed: {e}\n")),
        }
    }

    out.push_str(&format!(
        "\n  {ok}/{} target(s) built. 'linux-portable' (musl-static) is the one that runs on every Linux + every rented box.",
        targets.len()
    ));
    out
}
