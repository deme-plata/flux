// v0.14-D: SDK compile-each tests.
//
// For each generator, write the produced source to a tempfile and invoke the
// language's compiler / parser. Tests soft-skip when the toolchain is missing
// (Epsilon currently has python3 + node + rustc; lacks tsc, go, kotlinc) so
// the suite stays green on a stripped-down box and starts validating the
// missing langs automatically once compilers are installed.
//
// Skips print `[skip] reason: <reason>` on stdout — captured by `cargo test`
// behind `--nocapture`, surfaced via the test summary anyway.

use flux_api::{
    discover_endpoints, generate_go_sdk, generate_kotlin_sdk, generate_python_sdk,
    generate_rust_client_sdk, generate_typescript_sdk,
};
use std::path::PathBuf;
use std::process::Command;

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

fn tool_present(bin: &str) -> bool {
    Command::new("which")
        .arg(bin)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn write_temp(prefix: &str, ext: &str, body: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let pid = std::process::id();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    p.push(format!("flux_api_sdk_{prefix}_{pid}_{ts}{ext}"));
    std::fs::write(&p, body).expect("write tempfile");
    p
}

fn run_check(cmd: &mut Command, label: &str) {
    let out = cmd.output().unwrap_or_else(|e| panic!("{label}: spawn: {e}"));
    if !out.status.success() {
        panic!(
            "{label}: compile failed (exit {:?})\nstdout:\n{}\nstderr:\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }
}

#[test]
fn python_sdk_parses_with_py_compile() {
    if !tool_present("python3") {
        println!("[skip] python3 not installed");
        return;
    }
    let eps = discover_endpoints(&ws(&["wickes-cms", "flux-ue-bridge"]));
    let sdk = generate_python_sdk(&eps, "http://localhost:8080");
    let path = write_temp("py", ".py", &sdk);
    let mut cmd = Command::new("python3");
    cmd.arg("-m").arg("py_compile").arg(&path);
    run_check(&mut cmd, "python");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn typescript_sdk_parses_with_tsc_if_available() {
    if !tool_present("tsc") {
        println!("[skip] tsc not installed — skipping TS compile check (generated source still validated by generator unit tests)");
        // Still sanity-check the generator produces a non-empty string.
        let eps = discover_endpoints(&ws(&["wickes-cms"]));
        let sdk = generate_typescript_sdk(&eps, "http://localhost:8080");
        assert!(!sdk.is_empty() && sdk.contains("export class"));
        return;
    }
    let eps = discover_endpoints(&ws(&["wickes-cms", "flux-ue-bridge"]));
    let sdk = generate_typescript_sdk(&eps, "http://localhost:8080");
    let path = write_temp("ts", ".ts", &sdk);
    let mut cmd = Command::new("tsc");
    cmd.args(["--noEmit", "--target", "ES2020", "--strict", "false"])
        .arg(&path);
    run_check(&mut cmd, "tsc");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn go_sdk_compiles_with_go_if_available() {
    if !tool_present("go") {
        println!("[skip] go not installed — skipping Go compile check");
        let eps = discover_endpoints(&ws(&["wickes-cms"]));
        let sdk = generate_go_sdk(&eps, "http://localhost:8080", "wickescms");
        assert!(sdk.contains("package wickescms"));
        return;
    }
    let eps = discover_endpoints(&ws(&["wickes-cms"]));
    let sdk = generate_go_sdk(&eps, "http://localhost:8080", "wickescms");

    // Go needs the source inside a folder with a matching package + go.mod.
    let mut dir = std::env::temp_dir();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    dir.push(format!("flux_api_go_{}_{ts}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("client.go"), &sdk).unwrap();
    std::fs::write(
        dir.join("go.mod"),
        "module example.com/wickescms\n\ngo 1.21\n",
    )
    .unwrap();

    let mut cmd = Command::new("go");
    cmd.arg("vet").arg("./...").current_dir(&dir);
    run_check(&mut cmd, "go vet");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rust_sdk_passes_syntax_check_with_rustc() {
    // We use rustc's parse-only mode (no reqwest dep present, but we don't
    // need it — only proving the file *parses* as valid Rust). `--emit=metadata`
    // would require deps; `--cfg test` won't help. The trick: drop a stub
    // shim that defines the reqwest types we use, then rustc on the combined
    // file with `--crate-type lib --edition 2021 -Z parse-only` (nightly
    // only). Easiest portable check: write the file and call
    // `rustc --crate-type lib --emit metadata` with a stub-reqwest scope.
    if !tool_present("rustc") {
        println!("[skip] rustc not installed");
        return;
    }
    let eps = discover_endpoints(&ws(&["wickes-cms"]));
    let sdk = generate_rust_client_sdk(&eps, "http://localhost:8080");

    // Stub reqwest types so rustc can parse + type-check without the real
    // crate. We only stub what the generated SDK uses.
    let stub = r#"
        pub mod reqwest {
            pub struct Client;
            impl Client {
                pub fn new() -> Self { Client }
                pub fn get(&self, _: String) -> RequestBuilder { RequestBuilder }
                pub fn post(&self, _: String) -> RequestBuilder { RequestBuilder }
                pub fn put(&self, _: String) -> RequestBuilder { RequestBuilder }
                pub fn delete(&self, _: String) -> RequestBuilder { RequestBuilder }
                pub fn patch(&self, _: String) -> RequestBuilder { RequestBuilder }
            }
            pub struct RequestBuilder;
            impl RequestBuilder {
                pub async fn send(self) -> Result<Response, Error> { Ok(Response) }
            }
            pub struct Response;
            #[derive(Debug)]
            pub struct Error;
        }
    "#;
    let path = write_temp("rs", ".rs", &format!("{stub}\n{sdk}"));
    let out_dir = std::env::temp_dir();
    let mut cmd = Command::new("rustc");
    cmd.args([
        "--edition",
        "2021",
        "--crate-type",
        "lib",
        "--emit=metadata",
        "--out-dir",
    ])
    .arg(&out_dir)
    .arg(&path);
    run_check(&mut cmd, "rustc");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn kotlin_sdk_compiles_with_kotlinc_if_available() {
    if !tool_present("kotlinc") {
        println!("[skip] kotlinc not installed — skipping Kotlin compile check");
        let eps = discover_endpoints(&ws(&["flux-ue-bridge"]));
        let sdk = generate_kotlin_sdk(&eps, "http://localhost:9989", "io.flux.ue");
        assert!(sdk.contains("class FluxUeBridgeClient"));
        return;
    }
    let eps = discover_endpoints(&ws(&["flux-ue-bridge"]));
    let sdk = generate_kotlin_sdk(&eps, "http://localhost:9989", "io.flux.ue");
    let path = write_temp("kt", ".kt", &sdk);
    let mut cmd = Command::new("kotlinc");
    cmd.args(["-script", "-"]).arg(&path);
    run_check(&mut cmd, "kotlinc");
    let _ = std::fs::remove_file(&path);
}
