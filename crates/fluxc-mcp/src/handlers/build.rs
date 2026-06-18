use serde_json::{json, Value};
use crate::handlers::{ToolDef, ToolRegistry};

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_compile",
            description: "Compile a Rust project with Flux content-hash cache. 50ms incremental builds.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "release": {"type": "boolean", "description": "Build in release mode"},
                    "package": {"type": "string", "description": "Specific package to build"}
                }
            }),
        },
        flux_compile,
    );
    registry.register(
        ToolDef {
            name: "flux_iterate",
            description: "AI iteration loop: compile + test + stats in one call. Returns build time, test results, and cache hit rate. Optimized for AI agent coding loops.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "package": {"type": "string", "description": "Package to iterate on"},
                    "release": {"type": "boolean", "description": "Build in release mode"}
                }
            }),
        },
        flux_iterate,
    );
    registry.register(
        ToolDef {
            name: "flux_format",
            description: "Format Rust source code with rustfmt. Returns diff or success.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "check": {"type": "boolean", "description": "Check only, don't modify (default: false)"},
                    "file": {"type": "string", "description": "Specific file to format (default: entire project)"}
                }
            }),
        },
        flux_format,
    );
    registry.register(
        ToolDef {
            name: "flux_batch_compile",
            description: "Compile multiple packages in parallel using Flux cache. Returns per-package timing and status.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "packages": {"type": "array", "items": {"type": "string"}, "description": "List of package names to compile in parallel"},
                    "release": {"type": "boolean", "description": "Build in release mode"}
                },
                "required": ["packages"]
            }),
        },
        flux_batch_compile,
    );
    registry.register(
        ToolDef {
            name: "flux_self_build",
            description: "Dogfooding: build the entire Flux workspace using itself as the compiler wrapper (RUSTC_WRAPPER=self). The ultimate self-hosting proof.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "release": {"type": "boolean", "description": "Build in release mode (default: false)"}
                }
            }),
        },
        flux_self_build,
    );
}

use fluxc_core::{webhook, predict};

fn flux_compile(args: &Value) -> String {
    let release = args.get("release").and_then(|v| v.as_bool()).unwrap_or(false);
    let package = args.get("package").and_then(|v| v.as_str()).unwrap_or("");
    let pkg_name = if package.is_empty() { "all" } else { package };

    let pred = predict::predict_build(pkg_name, false, &[]);
    webhook::auto_dispatch("prediction_made", predict::prediction_webhook_data(&pred));

    let start = std::time::Instant::now();
    let mut cmd = crate::handlers::cargo_cmd();
    cmd.arg("build");
    if release { cmd.arg("--release"); }
    if !package.is_empty() { cmd.args(["--package", package]); }

    match cmd.status() {
        Ok(status) if status.success() => {
            let elapsed_ms = start.elapsed().as_millis();
            webhook::auto_dispatch("build_complete", webhook::build_event_data(pkg_name, true, elapsed_ms, 0, 0));
            let fb = predict::record_feedback(&pred, elapsed_ms as u64, 0.85, true);
            let fevt = if fb.was_accurate { "prediction_accurate" } else { "prediction_deviation" };
            webhook::auto_dispatch(fevt, predict::feedback_webhook_data(&fb));
            format!("✓ Compilation successful in {}ms (Flux cache active)", elapsed_ms)
        }
        Ok(status) => {
            let elapsed_ms = start.elapsed().as_millis();
            webhook::auto_dispatch("build_failed", webhook::build_event_data(pkg_name, false, elapsed_ms, 0, 0));
            let fb = predict::record_feedback(&pred, elapsed_ms as u64, 0.0, false);
            webhook::auto_dispatch("prediction_deviation", predict::feedback_webhook_data(&fb));
            format!("✗ Compilation failed with exit code {}", status.code().unwrap_or(1))
        }
        Err(e) => format!("✗ Failed to run cargo: {}", e),
    }
}

fn flux_iterate(args: &Value) -> String {
    let package = args.get("package").and_then(|v| v.as_str());
    let release = args.get("release").and_then(|v| v.as_bool()).unwrap_or(false);

    let start = std::time::Instant::now();

    let mut cmd = crate::handlers::cargo_cmd();
    cmd.arg("build");
    if release { cmd.arg("--release"); }
    if let Some(pkg) = package { cmd.args(["--package", pkg]); }
    cmd.arg("--quiet");

    let compile_ok = cmd.status().map(|s| s.success()).unwrap_or(false);
    let compile_ms = start.elapsed().as_millis();

    let test_result = if compile_ok {
        let mut cmd = crate::handlers::cargo_cmd();
        cmd.arg("test");
        if let Some(pkg) = package { cmd.args(["--package", pkg]); }
        // Drop --quiet so per-binary "test result:" lines stay visible
        // for parse_test_counts. cargo's summary is one line regardless,
        // so output volume stays modest.
        match cmd.output() {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let (passed, failed) = crate::handlers::parse_test_counts(&stdout, &stderr);
                format!("{} passed, {} failed", passed, failed)
            }
            Err(e) => format!("error: {}", e),
        }
    } else {
        "skipped (compile failed)".into()
    };

    let hits = fluxc_core::CACHE_HITS.load(std::sync::atomic::Ordering::Relaxed);
    let misses = fluxc_core::CACHE_MISSES.load(std::sync::atomic::Ordering::Relaxed);
    let total = hits + misses;
    let cache_rate = if total > 0 { (hits as f64 / total as f64) * 100.0 } else { 0.0 };
    let builds = fluxc_core::BUILD_COUNT.load(std::sync::atomic::Ordering::Relaxed);

    let total_ms = start.elapsed().as_millis();

    let pkg = package.unwrap_or("all");
    webhook::auto_dispatch("iterate_complete", serde_json::json!({
        "package": pkg, "success": compile_ok, "compile_ms": compile_ms,
        "total_ms": total_ms, "cache_rate": cache_rate, "builds": builds
    }));

    format!(
        "⚡ flux_iterate complete in {}ms\n  Compile: {}ms ({})\n  Tests: {}\n  Cache: {:.1}% ({}/{} hits)\n  Builds: {}",
        total_ms,
        compile_ms,
        if compile_ok { "✓" } else { "✗ FAILED" },
        test_result,
        cache_rate, hits, total,
        builds
    )
}

fn flux_format(args: &Value) -> String {
    let check = args.get("check").and_then(|v| v.as_bool()).unwrap_or(false);
    let file = args.get("file").and_then(|v| v.as_str());

    let mut cmd = std::process::Command::new("rustfmt");
    if check { cmd.arg("--check"); }
    cmd.arg("--edition").arg("2021");
    if let Some(f) = file { cmd.arg(f); }

    match cmd.output() {
        Ok(output) => {
            if output.status.success() {
                if check {
                    "✓ Code is correctly formatted (rustfmt check passed)".into()
                } else {
                    "✓ Code formatted successfully (rustfmt applied)".into()
                }
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                format!("✗ Formatting issues found:\n{}", stderr.lines().take(15).collect::<Vec<_>>().join("\n"))
            }
        }
        Err(e) => format!("⚠ rustfmt not available: {}", e),
    }
}

fn flux_batch_compile(args: &Value) -> String {
    let packages: Vec<String> = args.get("packages")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let release = args.get("release").and_then(|v| v.as_bool()).unwrap_or(false);

    if packages.is_empty() {
        return "Error: 'packages' must be a non-empty array of package names".into();
    }

    let start = std::time::Instant::now();
    let results: std::sync::Arc<std::sync::Mutex<Vec<(String, bool, u128)>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

    let mut handles = Vec::new();
    for pkg in &packages {
        let pkg = pkg.clone();
        let results = results.clone();
        let handle = std::thread::spawn(move || {
            let pkg_start = std::time::Instant::now();
            let mut cmd = crate::handlers::cargo_cmd();
            // batch_compile spawns parallel threads — cargo_cmd() is fine since
            // current_dir is captured at construction, not at spawn.
            cmd.arg("build");
            if release { cmd.arg("--release"); }
            cmd.args(["--package", &pkg]);
            let ok = cmd.status().map(|s| s.success()).unwrap_or(false);
            let ms = pkg_start.elapsed().as_millis();
            results.lock().unwrap().push((pkg, ok, ms));
        });
        handles.push(handle);
    }

    for h in handles {
        h.join().ok();
    }

    let results = results.lock().unwrap();
    let total_ms = start.elapsed().as_millis();
    let success_count = results.iter().filter(|(_, ok, _)| *ok).count();
    let fail_count = results.len() - success_count;

    webhook::auto_dispatch("batch_complete", serde_json::json!({
        "total_packages": results.len(), "success_count": success_count,
        "fail_count": fail_count, "total_ms": total_ms
    }));

    let mut lines = Vec::new();
    lines.push(format!(
        "⚡ flux_batch_compile: {} packages in {}ms ({} ok, {} failed)",
        results.len(), total_ms, success_count, fail_count
    ));
    for (pkg, ok, ms) in results.iter() {
        lines.push(format!(
            "  {} {} — {}ms",
            if *ok { "✓" } else { "✗" },
            pkg,
            ms
        ));
    }

    lines.join("\n")
}

fn flux_self_build(args: &Value) -> String {
    let release = args.get("release").and_then(|v| v.as_bool()).unwrap_or(false);
    let start = std::time::Instant::now();

    let mut cmd = crate::handlers::cargo_cmd();
    cmd.arg("build").arg("--package").arg("fluxc");
    if release { cmd.arg("--release"); }
    cmd.env("RUSTC_WRAPPER", std::env::current_exe().unwrap_or_else(|_| "fluxc".into()));
    cmd.env("FLUXC_WRAPPING", "1");
    let real = std::env::var("REAL_RUSTC").unwrap_or_else(|_| "rustc".to_string());
    cmd.env("REAL_RUSTC", &real);

    match cmd.output() {
        Ok(output) => {
            let elapsed_ms = start.elapsed().as_millis();
            if output.status.success() {
                format!(
                    "🐕 Self-build successful in {}ms\n  Flux compiled Flux — dogfooding loop closed.\n  RUSTC_WRAPPER=self active.",
                    elapsed_ms
                )
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                format!("✗ Self-build failed in {}ms\n{}", elapsed_ms, stderr.lines().take(10).collect::<Vec<_>>().join("\n"))
            }
        }
        Err(e) => format!("✗ Self-build error: {}", e),
    }
}
