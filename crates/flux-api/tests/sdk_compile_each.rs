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
    discover_endpoints, discover_schemas, generate_go_sdk, generate_kotlin_sdk,
    generate_python_sdk, generate_python_sdk_with_types, generate_rust_client_sdk,
    generate_typescript_sdk,
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

/// Resolve a language toolchain binary, preferring an explicit env override.
///
/// Boxes like Epsilon deliberately keep `node`/`tsc` off the global PATH (Flux
/// dogfooding), so a bare `which tsc` reports "missing" even when a perfectly
/// good compiler is installed — and the compile check silently degrades to a
/// string assertion. The env hook (`FLUX_TSC`, `FLUX_GO`, `FLUX_KOTLINC`) lets
/// CI and local runs point at the real binary and get genuine validation.
fn resolve_tool(bin: &str, env_var: &str) -> Option<String> {
    if let Ok(p) = std::env::var(env_var) {
        if !p.is_empty() && std::path::Path::new(&p).exists() {
            return Some(p);
        }
    }
    tool_present(bin).then(|| bin.to_string())
}

/// An endpoint carrying whatever middleware the caller wants to exercise.
fn mw_endpoint(path: &str, op: &str, mw: flux_api::MiddlewareSpec) -> flux_api::ApiEndpoint {
    flux_api::ApiEndpoint {
        crate_name: "demo".into(),
        method: flux_api::HttpMethod::GET,
        path: path.into(),
        operation_id: op.into(),
        summary: "demo".into(),
        parameters: vec![],
        request_body: None,
        responses: vec![flux_api::ApiResponse {
            status: 200,
            description: String::new(),
            schema: None,
        }],
        tags: vec!["demo".into()],
        middleware: Some(mw),
    }
}

/// The three pagination styles + SSE, all on one client — the surface added in
/// v0.15-B. Used by the per-language compile checks so generated paginators
/// and stream readers are validated by a real compiler, not just by
/// string-contains assertions in the generator's own unit tests.
fn middleware_endpoints() -> Vec<flux_api::ApiEndpoint> {
    use flux_api::{MiddlewareSpec, PaginationStyle, RetryPolicy, StreamKind};
    vec![
        mw_endpoint(
            "/cursor",
            "list_cursor",
            MiddlewareSpec::bearer_auth()
                .with_retry(RetryPolicy::standard())
                .with_pagination(PaginationStyle::Cursor {
                    cursor_param: "after".into(),
                    response_field: "next_cursor".into(),
                }),
        ),
        mw_endpoint(
            "/paged",
            "list_paged",
            MiddlewareSpec::default()
                .with_pagination(PaginationStyle::Page { page_param: "page".into() })
                .with_items_field("results"),
        ),
        mw_endpoint(
            "/offset",
            "list_offset",
            MiddlewareSpec::default().with_pagination(PaginationStyle::Offset {
                offset_param: "offset".into(),
                limit_param: "limit".into(),
            }),
        ),
        mw_endpoint(
            "/events",
            "watch_events",
            MiddlewareSpec::default().with_streaming(StreamKind::Sse),
        ),
    ]
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
fn typed_python_sdk_parses_with_py_compile() {
    // The typed variant emits `TypedDict` definitions + annotated body params;
    // prove the richer output still compiles under py_compile.
    if !tool_present("python3") {
        println!("[skip] python3 not installed");
        return;
    }
    let g = ws(&["wickes-cms", "wickes-erp", "wickes-finance", "flux-ue-bridge"]);
    let eps = discover_endpoints(&g);
    let sdk = generate_python_sdk_with_types(&eps, "http://localhost:8080", &discover_schemas(&g));
    let path = write_temp("py_typed", ".py", &sdk);
    let mut cmd = Command::new("python3");
    cmd.arg("-m").arg("py_compile").arg(&path);
    run_check(&mut cmd, "typed python");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn typescript_sdk_parses_with_tsc_if_available() {
    let Some(tsc) = resolve_tool("tsc", "FLUX_TSC") else {
        println!("[skip] tsc not installed — skipping TS compile check (set FLUX_TSC=/path/to/tsc to enable)");
        // Still sanity-check the generator produces a non-empty string.
        let eps = discover_endpoints(&ws(&["wickes-cms"]));
        let sdk = generate_typescript_sdk(&eps, "http://localhost:8080");
        assert!(!sdk.is_empty() && sdk.contains("export class"));
        return;
    };
    let eps = discover_endpoints(&ws(&["wickes-cms", "flux-ue-bridge"]));
    let sdk = generate_typescript_sdk(&eps, "http://localhost:8080");
    let path = write_temp("ts", ".ts", &sdk);
    let mut cmd = Command::new(&tsc);
    cmd.args(["--noEmit", "--target", "ES2020", "--strict", "false"])
        .arg(&path);
    run_check(&mut cmd, "tsc");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn typescript_middleware_sdk_compiles_with_tsc_if_available() {
    // v0.15-B: the paginators emit async generators and the SSE reader drives a
    // ReadableStream — both are far more likely to be malformed than the plain
    // fetch client, so they get their own compiler pass.
    let Some(tsc) = resolve_tool("tsc", "FLUX_TSC") else {
        println!("[skip] tsc not installed — set FLUX_TSC=/path/to/tsc to compile-check paginators");
        let sdk = generate_typescript_sdk(&middleware_endpoints(), "http://localhost:8080");
        assert!(sdk.contains("IterPages") && sdk.contains("Stream("));
        return;
    };
    let sdk = generate_typescript_sdk(&middleware_endpoints(), "http://localhost:8080");
    let path = write_temp("ts_mw", ".ts", &sdk);
    let mut cmd = Command::new(&tsc);
    cmd.args(["--noEmit", "--target", "ES2020", "--strict", "false"])
        .arg(&path);
    run_check(&mut cmd, "tsc (middleware)");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn python_middleware_sdk_parses_with_py_compile() {
    // Same reasoning as the TS middleware check: generated `yield`-based
    // paginators and the SSE reader are the parts most likely to be malformed.
    if !tool_present("python3") {
        println!("[skip] python3 not installed");
        return;
    }
    let sdk = generate_python_sdk(&middleware_endpoints(), "http://localhost:8080");
    let path = write_temp("py_mw", ".py", &sdk);
    let mut cmd = Command::new("python3");
    cmd.arg("-m").arg("py_compile").arg(&path);
    run_check(&mut cmd, "python (middleware)");
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
                pub fn query<T: ?Sized>(self, _: &T) -> Self { self }
                pub fn json<T: ?Sized>(self, _: &T) -> Self { self }
                pub fn bearer_auth<T>(self, _: T) -> Self { self }
                pub async fn send(self) -> Result<Response, Error> { Ok(Response) }
            }
            pub struct Response;
            #[derive(Debug)]
            pub struct Error;
        }
        // Minimal serde_json shim — the generated SDK names `serde_json::Value`
        // as the request-body type. A real consumer depends on the crate; the
        // parse/type-check here only needs the path to resolve.
        pub mod serde_json {
            #[derive(Default)]
            pub struct Value;
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
