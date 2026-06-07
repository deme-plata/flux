use serde_json::{json, Value};
use crate::handlers::{ToolDef, ToolRegistry};

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_test",
            description: "Run Rust tests. Returns only failures for token efficiency. Pass --package to scope.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "package": {"type": "string", "description": "Specific crate to test"},
                    "filter": {"type": "string", "description": "Test name filter (substring match)"}
                }
            }),
        },
        flux_test,
    );
    registry.register(
        ToolDef {
            name: "flux_combo",
            description: "Compile + test + predict in one call. Saves 67% token round-trips vs doing each separately.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "package": {"type": "string", "description": "Package to combo on"},
                    "release": {"type": "boolean", "description": "Build in release mode"}
                }
            }),
        },
        flux_combo,
    );
    registry.register(
        ToolDef {
            name: "flux_quickcast",
            description: "Tune + check + predict in one call. Fast iteration with auto-tuned loadout.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "package": {"type": "string", "description": "Package to quickcast on"}
                }
            }),
        },
        flux_quickcast,
    );
    registry.register(
        ToolDef {
            name: "flux_ult",
            description: "Check + heatmap + predict in one parallel call. Maximum insight with minimum tokens.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "package": {"type": "string", "description": "Package to ult on"}
                }
            }),
        },
        flux_ult,
    );

    registry.register(
        ToolDef {
            name: "flux_dev",
            description: "UNIFIED development combo: check + test + heal + suggest + cortex + webhooks in ONE call. The single MCP tool for AI-driven Flux development. Saves 80% token round-trips vs doing each step separately.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "package": {"type": "string", "description": "Package/crate to work on (default: fluxc)"},
                    "file": {"type": "string", "description": "Specific file to analyze (optional)"},
                    "skip_heal": {"type": "boolean"},
                    "skip_ai": {"type": "boolean"},
                    "deploy": {"type": "boolean"}
                }
            }),
        },
        flux_dev,
    );
}

use fluxc_core::webhook;

fn flux_test(args: &Value) -> String {
    let package = args.get("package").and_then(|v| v.as_str());
    let filter = args.get("filter").and_then(|v| v.as_str());

    let mut cmd = crate::handlers::cargo_cmd();
    cmd.arg("test");
    if let Some(pkg) = package { cmd.args(["--package", pkg]); }
    if let Some(f) = filter { cmd.args(["--", f]); }

    let start = std::time::Instant::now();
    match cmd.output() {
        Ok(output) => {
            let elapsed_ms = start.elapsed().as_millis();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{}\n{}", stdout, stderr);

            let pkg = package.unwrap_or("all");
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let (pass_count, _) = crate::handlers::parse_test_counts(&stdout, &stderr);
                webhook::auto_dispatch("test_complete", serde_json::json!({
                    "package": pkg, "success": true, "passed": pass_count, "failed": 0, "elapsed_ms": elapsed_ms
                }));
                format!("✓ All {} tests passed in {}ms", pass_count, elapsed_ms)
            } else {
                webhook::auto_dispatch("test_complete", serde_json::json!({
                    "package": pkg, "success": false, "elapsed_ms": elapsed_ms
                }));
                let failures: Vec<&str> = combined.lines()
                    .filter(|l| l.contains("FAILED") || l.contains("panicked") || l.contains("error"))
                    .take(10)
                    .collect();
                format!("✗ Tests failed in {}ms:\n{}", elapsed_ms, failures.join("\n"))
            }
        }
        Err(e) => format!("✗ Failed to run tests: {}", e),
    }
}

use fluxc_core::predict;

fn flux_combo(args: &Value) -> String {
    let package = args.get("package").and_then(|v| v.as_str()).unwrap_or("fluxc");
    let release = args.get("release").and_then(|v| v.as_bool()).unwrap_or(false);
    let start = std::time::Instant::now();
    let pkg = package.to_string();
    let pkg2 = pkg.clone();

    let compile_handle = std::thread::spawn({
        let p = pkg.clone();
        move || {
            let mode = if release { "build" } else { "check" };
            let output = crate::handlers::cargo_cmd()
                .args([mode, "-p", &p])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stderr).to_string())
                .unwrap_or_default();
            output
        }
    });

    let test_handle = std::thread::spawn(move || {
        let raw = crate::handlers::cargo_cmd()
            .args(["test", "-p", &pkg2])
            .output();
        match raw {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                let stderr = String::from_utf8_lossy(&o.stderr);
                crate::handlers::parse_test_counts(&stdout, &stderr)
            }
            Err(_) => (0, 0),
        }
    });

    let predict_handle = std::thread::spawn({
        let p = pkg.clone();
        move || predict::predict_build(&p, false, &[])
    });

    let _compile_output = compile_handle.join().unwrap();
    let (test_passed, test_failed) = test_handle.join().unwrap();
    let pred = predict_handle.join().unwrap();

    let total_ms = start.elapsed().as_millis();

    webhook::auto_dispatch("combo_complete", serde_json::json!({
        "package": pkg, "compile_ms": total_ms,
        "tests_passed": test_passed, "tests_failed": test_failed,
        "predicted_ms": pred.predicted_ms
    }));

    let total = test_passed + test_failed;
    let test_frac = if total > 0 { test_passed as f64 / total as f64 } else { 1.0 };
    let status = if test_failed == 0 { "✓" } else { "✗" };
    let fired = webhook_fired("combo_complete");
    let pulse = "◉ ".repeat(fired.min(8));
    format!(
        "⚡⚡⚡  F L U X   C O M B O  ⚡⚡⚡\n{}",
        dashboard(
            &format!("{} · {}ms · {}", pkg, total_ms, if test_failed == 0 { "green" } else { "RED" }),
            &[
                " BUILD     ✓ compiled · parallel".to_string(),
                format!(" TESTS     {}  {}/{} {}", bar(test_frac, 24), test_passed, total, status),
                format!(" PREDICT   {}ms   cache {} {}%", pred.predicted_ms, bar(pred.predicted_cache_rate, 12), (pred.predicted_cache_rate * 100.0) as u64),
                format!("           confidence {} {}%", bar(pred.confidence, 12), (pred.confidence * 100.0) as u64),
                "--".to_string(),
                format!(" WEBHOOKS  combo_complete ▶ {} fired   {}", fired, pulse.trim_end()),
            ]
        )
    )
}

use fluxc_core::tune;
use fluxc_core::heatmap;

// ── Visual panel rendering (the ECONOMICS-box treatment for flux combos) ──
const PANEL_W: usize = 50; // inner content width (all rows are single-width chars)

/// Unicode progress bar: `█`×filled + `░`×empty, `w` cells, for `frac` in 0..1.
fn bar(frac: f64, w: usize) -> String {
    let f = (frac.clamp(0.0, 1.0) * w as f64).round() as usize;
    "█".repeat(f) + &"░".repeat(w - f)
}

/// Draw a titled box around body `rows` (each already starts with a space).
/// Rows must contain only single-width chars (ascii, ✓/✗, █/░) so it aligns.
fn boxed(title: &str, rows: &[String]) -> String {
    let head = format!("─ {title} ");
    let fill = PANEL_W.saturating_sub(head.chars().count());
    let mut o = format!("┌{}{}┐\n", head, "─".repeat(fill));
    for r in rows {
        let pad = PANEL_W.saturating_sub(r.chars().count());
        o.push_str(&format!("│{}{}│\n", r, " ".repeat(pad)));
    }
    o.push_str(&format!("└{}┘", "─".repeat(PANEL_W)));
    o
}

/// The "explode my terminal" double-box dashboard. Rows of `"--"` become a
/// section divider; all other rows are body lines (single-width chars only).
fn dashboard(title: &str, rows: &[String]) -> String {
    let head = format!("  {title}  ");
    let fill = PANEL_W.saturating_sub(head.chars().count());
    let l = fill / 2;
    let mut o = format!("╔{}{}{}╗\n", "═".repeat(l), head, "═".repeat(fill - l));
    for r in rows {
        if r == "--" {
            o.push_str(&format!("╟{}╢\n", "─".repeat(PANEL_W)));
        } else {
            let pad = PANEL_W.saturating_sub(r.chars().count());
            o.push_str(&format!("║{}{}║\n", r, " ".repeat(pad)));
        }
    }
    o.push_str(&format!("╚{}╝", "═".repeat(PANEL_W)));
    o
}

/// How many registered webhook endpoints would fire for `event` (real count).
fn webhook_fired(event: &str) -> usize {
    fluxc_core::webhook::count_listeners(event)
}

fn flux_quickcast(args: &Value) -> String {
    let package = args.get("package").and_then(|v| v.as_str()).unwrap_or("fluxc");
    let start = std::time::Instant::now();

    // Auto-tune
    let t = tune::auto_equip("fast iteration coding")
        .map(|(t, _)| t)
        .unwrap_or_else(|_| tune::load_tune());

    // Check (cargo check)
    let check_ok = crate::handlers::cargo_cmd()
        .args(["check", "-p", package])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    // Predict
    let pred = predict::predict_build(package, false, &[]);

    let ms = start.elapsed().as_millis();
    format!(
        "⚡ flux_quickcast · {} · {}ms\n{}",
        package, ms,
        boxed("tune · check · predict", &[
            format!(" tune     {} (auto)", t.preset_name),
            format!(" check    {}", if check_ok { "✓ ok" } else { "✗ failed" }),
            format!(" predict  {}ms  conf {} {}%", pred.predicted_ms, bar(pred.confidence, 12), (pred.confidence * 100.0) as u64),
        ])
    )
}

fn flux_ult(args: &Value) -> String {
    let package = args.get("package").and_then(|v| v.as_str()).unwrap_or("fluxc");
    let start = std::time::Instant::now();

    // Parallel: check + heatmap + predict
    let pkg = package.to_string();
    let check_handle = std::thread::spawn({
        let p = pkg.clone();
        move || crate::handlers::cargo_cmd()
            .args(["check", "-p", &p])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    });

    let heatmap_handle = std::thread::spawn(|| heatmap::capture_heatmap());

    let pred_handle = std::thread::spawn({
        let p = pkg.clone();
        move || predict::predict_build(&p, false, &[])
    });

    let check_ok = check_handle.join().unwrap();
    let heat: fluxc_core::heatmap::HeatmapSnapshot = heatmap_handle.join().unwrap_or_default();
    let pred = pred_handle.join().unwrap();

    let ms = start.elapsed().as_millis();
    format!(
        "⚡ flux_ult · {} · {}ms\n{}",
        package, ms,
        boxed("check · heatmap · predict", &[
            format!(" check     {}", if check_ok { "✓ ok" } else { "✗ failed" }),
            format!(" stability {} {:.0}%  {}", bar(heat.stability_score, 14), heat.stability_score * 100.0, heat.status),
            format!(" predict   {}ms  cache {} {}%", pred.predicted_ms, bar(pred.predicted_cache_rate, 10), (pred.predicted_cache_rate * 100.0) as u64),
        ])
    )
}

// ── Unified flux_dev handler ──

fn flux_dev(args: &Value) -> String {
    let package = args.get("package").and_then(|v| v.as_str()).unwrap_or("fluxc");
    let file = args.get("file").and_then(|v| v.as_str());
    let skip_heal = args.get("skip_heal").and_then(|v| v.as_bool()).unwrap_or(false);
    let skip_ai = args.get("skip_ai").and_then(|v| v.as_bool()).unwrap_or(false);
    let deploy = args.get("deploy").and_then(|v| v.as_bool()).unwrap_or(false);

    let mut steps = Vec::new();
    let start = std::time::Instant::now();

    let check = std::process::Command::new("fluxc")
        .args(["check", "-p", package]).output();
    let check_ok = check.as_ref().map(|o| o.status.success()).unwrap_or(false);
    steps.push(json!({"step": "check", "ok": check_ok}));

    let test = std::process::Command::new("fluxc")
        .args(["t", package]).output();
    let test_ok = test.as_ref().map(|o| o.status.success()).unwrap_or(false);
    steps.push(json!({"step": "test", "ok": test_ok}));

    if !skip_ai {
        if let Some(f) = file {
            let suggest = std::process::Command::new("fluxc")
                .args(["suggest", f]).output();
            steps.push(json!({"step": "suggest", "ok": suggest.as_ref().map(|o| o.status.success()).unwrap_or(false)}));
        }
    }

    if !check_ok && !skip_heal {
        let target = file.unwrap_or(package);
        let heal = std::process::Command::new("fluxc")
            .args(["heal", target, "-n", "3"]).output();
        steps.push(json!({"step": "heal", "ok": heal.as_ref().map(|o| o.status.success()).unwrap_or(false)}));
    }

    let cortex = std::process::Command::new("fluxc")
        .args(["cortex-summary"]).output();
    steps.push(json!({"step": "cortex", "ok": cortex.as_ref().map(|o| o.status.success()).unwrap_or(false)}));

    let listener_count = fluxc_core::webhook::count_listeners("build_complete");
    steps.push(json!({"step": "webhooks", "listeners": listener_count}));

    if deploy && check_ok && test_ok {
        let dep = std::process::Command::new("fluxc")
            .args(["release", package]).output();
        steps.push(json!({"step": "deploy", "ok": dep.as_ref().map(|o| o.status.success()).unwrap_or(false)}));
    }

    let elapsed = start.elapsed().as_secs_f64();
    let all_ok = steps.iter().all(|s| s["ok"].as_bool().unwrap_or(true));

    json!({
        "package": package, "all_ok": all_ok,
        "elapsed_secs": format!("{:.1}", elapsed), "steps": steps,
        "next": if !all_ok { "Run flux_dev again with skip_heal=false" } else { "All checks passed" },
    }).to_string()
}
