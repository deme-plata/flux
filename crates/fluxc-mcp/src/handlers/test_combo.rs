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
            description: "PRIMARY dev tool for new Flux users: compile + test + predict in ONE call. This is the 'flux way' — never call cargo directly. Use flux_combo package=your-crate after every edit. Saves ~67% tokens vs separate steps. The heart of flux-dev skill combos.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "package": {"type": "string", "description": "Package/crate to combo on (e.g. fluxc-mcp, fluxc-core)"},
                    "release": {"type": "boolean", "description": "Build in release mode (slower, for final checks)"},
                    "incremental": {"type": "boolean", "description": "FIP-0003 unit gate: skip the test run entirely when the package's test binaries are byte-identical to its last green run (probe + content-addressed keys, fail-open). Promotes the gate after a green run."}
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

    registry.register(
        ToolDef {
            name: "flux_develop",
            description: "THE unified cortex-driven development cycle: flux-rev snapshot → cortex AI review → auto-heal → test → provenance-sign → deploy. The single MCP tool for the complete AI-native dev loop. Replaces 7 separate tool calls.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "package": {"type": "string", "description": "Package/crate (default: fluxc)"},
                    "message": {"type": "string", "description": "Revision message for flux-rev"},
                    "skip_ai": {"type": "boolean"},
                    "skip_heal": {"type": "boolean"},
                    "deploy": {"type": "boolean"},
                    "webhook": {"type": "string", "description": "Webhook URL for CI notification"}
                }
            }),
        },
        flux_develop,
    );
}

use fluxc_core::webhook;

fn flux_test(args: &Value) -> String {
    let package = args.get("package").and_then(|v| v.as_str());
    let filter = args.get("filter").and_then(|v| v.as_str());

    let flux_exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("fluxc"));
    let mut cmd = std::process::Command::new(&flux_exe);
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
    let incremental = args.get("incremental").and_then(|v| v.as_bool()).unwrap_or(false);
    let start = std::time::Instant::now();
    let pkg = package.to_string();
    let pkg2 = pkg.clone();

    // FIP-0003 unit gate: when the package's test binaries hash identically to
    // the set that already passed, the prior green is provably still valid —
    // report the reuse instead of re-running. Fail-open by construction: any
    // gate uncertainty (no history, changed keys, red probe, broken TDG) falls
    // through to the normal combo below.
    if incremental {
        if let fluxc_core::tdg_sched::GateDecision::Skip { green_unix } =
            fluxc_core::tdg_sched::combo_gate(&pkg)
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let age_min = now.saturating_sub(green_unix) / 60;
            let probe_ms = start.elapsed().as_millis();
            webhook::auto_dispatch("combo_complete", serde_json::json!({
                "package": pkg, "compile_ms": probe_ms,
                "tests_passed": 0, "tests_failed": 0,
                "incremental_skip": true, "reused_green_unix": green_unix
            }));
            return format!(
                "⚡⚡⚡  F L U X   C O M B O  ⚡⚡⚡\n{}",
                dashboard(
                    &format!("{} · {}ms · green (reused)", pkg, probe_ms),
                    &[
                        " GATE      ⏭ INCREMENTAL SKIP (FIP-0003 unit gate)".to_string(),
                        " PROOF     test binaries byte-identical to last green".to_string(),
                        format!(" REUSED    green run from {}m ago · 0 test executions", age_min),
                        " PROBE     cargo test --no-run compiled only deltas".to_string(),
                    ]
                )
            );
        }
    }

    let flux_exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("fluxc"));

    // NOTE: no per-call tune spawn (per swarm feedback to avoid latency). Assume SPEED_BOOTS or set at boot.
    // Dogfood: use fluxc test (which already does the type-check, no redundant check thread to avoid lock contention).

    let test_handle = std::thread::spawn({
        let pkg2 = pkg2.clone();
        let exe = flux_exe.clone();
        // Returns (passed, failed, suites_seen, run_ok). `suites_seen == 0`
        // means no test harness printed a summary — the "0/0 green" lie this
        // handler used to tell; `run_ok == false` means the subcommand itself
        // failed to spawn or exited non-zero without any suite output.
        move || {
            let raw = std::process::Command::new(&exe)
                .args(["test", "-p", &pkg2])
                .output();
            match raw {
                Ok(o) => {
                    let stdout = String::from_utf8_lossy(&o.stdout);
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    let (p, f, suites) = crate::handlers::parse_test_outcome(&stdout, &stderr);
                    (p, f, suites, o.status.success() || suites > 0)
                }
                Err(_) => (0, 0, 0, false),
            }
        }
    });

    let predict_handle = std::thread::spawn({
        let p = pkg.clone();
        move || predict::predict_build(&p, false, &[])
    });

    let (test_passed, test_failed, suites_seen, run_ok) = test_handle.join().unwrap();
    let pred = predict_handle.join().unwrap();

    let total_ms = start.elapsed().as_millis();
    // Three-way verdict: tests executed and all green / something red /
    // NOTHING demonstrably ran (never report that as green).
    let unverified = suites_seen == 0;

    // Incremental promote: last-green := last-built, but ONLY on a green run
    // that demonstrably executed tests (0/0 is the known "test binary never
    // ran" ambiguity — never promote on it).
    if incremental && test_failed == 0 && test_passed > 0 {
        fluxc_core::tdg_sched::combo_promote(&pkg, total_ms as u64);
    }

    let verdict = if unverified {
        "UNVERIFIED"
    } else if test_failed == 0 {
        "green"
    } else {
        "RED"
    };

    webhook::auto_dispatch("combo_complete", serde_json::json!({
        "package": pkg, "compile_ms": total_ms,
        "tests_passed": test_passed, "tests_failed": test_failed,
        "suites_seen": suites_seen, "verdict": verdict,
        "predicted_ms": pred.predicted_ms
    }));

    let total = test_passed + test_failed;
    let test_frac = if total > 0 { test_passed as f64 / total as f64 } else { 1.0 };
    let fired = webhook_fired("combo_complete");
    let pulse = "◉ ".repeat(fired.min(8));

    if unverified {
        // The old behavior rendered this exact case as "0/0 ✓ green" — the
        // known trap that sent agents shipping unverified code. Fail loud:
        // no green claim, and a concrete next action.
        let build_line = if run_ok {
            " BUILD     ? subcommand exited 0 but NO test suite ran".to_string()
        } else {
            " BUILD     ✗ `fluxc test` failed before any suite ran".to_string()
        };
        return format!(
            "⚡⚡⚡  F L U X   C O M B O  ⚡⚡⚡\n{}",
            dashboard(
                &format!("{} · {}ms · UNVERIFIED", pkg, total_ms),
                &[
                    build_line,
                    " TESTS     ░░░░░░░░░░░░░░░░░░░░░░░░  0 suites executed".to_string(),
                    " VERDICT   ⚠ NOT green — no test harness produced output".to_string(),
                    format!(" NEXT      fluxc check -p {} · then run target/debug/deps/<crate>-<hash> directly", pkg),
                    "--".to_string(),
                    format!(" WEBHOOKS  combo_complete ▶ {} fired   {}", fired, pulse.trim_end()),
                ]
            )
        );
    }

    let status = if test_failed == 0 { "✓" } else { "✗" };
    let tests_line = if total == 0 {
        // Suites ran and reported zero tests: a real (empty-suite) green,
        // labeled so it can't be confused with the unverified case above.
        format!(" TESTS     {}  0 tests in crate ({} suite(s) ran) {}", bar(1.0, 24), suites_seen, status)
    } else {
        format!(" TESTS     {}  {}/{} {}", bar(test_frac, 24), test_passed, total, status)
    };
    format!(
        "⚡⚡⚡  F L U X   C O M B O  ⚡⚡⚡\n{}",
        dashboard(
            &format!("{} · {}ms · {}", pkg, total_ms, verdict),
            &[
                " BUILD     ✓ (via fluxc test, no redundant check)".to_string(),
                tests_line,
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

    // Check via fluxc (dogfood, no raw cargo)
    let flux_exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("fluxc"));
    let _ = std::process::Command::new(&flux_exe)
        .args(["tune", "--preset", "speed-boots"])
        .status();
    let check_ok = std::process::Command::new(&flux_exe)
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

    // Parallel: check + heatmap + predict (dogfood via fluxc + speed tune)
    let pkg = package.to_string();
    let flux_exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("fluxc"));
    let _ = std::process::Command::new(&flux_exe)
        .args(["tune", "--preset", "speed-boots"])
        .status();
    let check_handle = std::thread::spawn({
        let p = pkg.clone();
        let exe = flux_exe.clone();
        move || {
            std::process::Command::new(&exe)
                .args(["check", "-p", &p])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
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

// ── flux_develop: unified cortex-driven development cycle ──

fn flux_develop(args: &Value) -> String {
    let package = args.get("package").and_then(|v| v.as_str()).unwrap_or("fluxc");
    let message = args.get("message").and_then(|v| v.as_str()).unwrap_or("flux_develop auto-revision");
    let skip_ai = args.get("skip_ai").and_then(|v| v.as_bool()).unwrap_or(false);
    let skip_heal = args.get("skip_heal").and_then(|v| v.as_bool()).unwrap_or(false);
    let deploy = args.get("deploy").and_then(|v| v.as_bool()).unwrap_or(false);
    let webhook_url = args.get("webhook").and_then(|v| v.as_str());

    let mut steps = Vec::new();
    let start = std::time::Instant::now();

    // 1. Flux-rev snapshot
    let rev = std::process::Command::new("flux-rev")
        .args(["snapshot", "--message", message, "--author", "flux_develop"]).output();
    let rev_ok = rev.as_ref().map(|o| o.status.success()).unwrap_or(false);
    steps.push(json!({"step": "snapshot", "ok": rev_ok,
        "output": String::from_utf8_lossy(&rev.as_ref().map(|o| &o.stdout[..]).unwrap_or(&[])).lines().last().unwrap_or("").to_string()
    }));

    // 2. Build
    let build = std::process::Command::new("fluxc")
        .args(["build", "-p", package]).output();
    let build_ok = build.as_ref().map(|o| o.status.success()).unwrap_or(false);
    steps.push(json!({"step": "build", "ok": build_ok}));

    // 3. AI Cortex review
    if !skip_ai {
        let ai = std::process::Command::new("fluxc")
            .args(["cortex-ai", "review", "--json"]).output();
        steps.push(json!({"step": "cortex-ai", "ok": ai.as_ref().map(|o| o.status.success()).unwrap_or(false)}));
    }

    // 4. Auto-heal if build failed
    if !build_ok && !skip_heal {
        let heal = std::process::Command::new("fluxc")
            .args(["heal", &format!("crates/{}/src/lib.rs", package), "-n", "3"]).output();
        let heal_ok = heal.as_ref().map(|o| o.status.success()).unwrap_or(false);
        steps.push(json!({"step": "heal", "ok": heal_ok}));
        // Rebuild after heal
        if heal_ok {
            let rebuild = std::process::Command::new("fluxc")
                .args(["build", "-p", package]).output();
            steps.push(json!({"step": "rebuild", "ok": rebuild.as_ref().map(|o| o.status.success()).unwrap_or(false)}));
        }
    }

    // 5. Test
    let test = std::process::Command::new("fluxc")
        .args(["test", "-p", package]).output();
    let test_ok = test.as_ref().map(|o| o.status.success()).unwrap_or(false);
    steps.push(json!({"step": "test", "ok": test_ok}));

    // 6. Cortex summary
    let cortex = std::process::Command::new("fluxc")
        .args(["cortex-summary"]).output();
    steps.push(json!({"step": "cortex", "ok": cortex.as_ref().map(|o| o.status.success()).unwrap_or(false)}));

    // 7. Deploy
    if deploy {
        let dep = std::process::Command::new("fluxc")
            .args(["release", package]).output();
        steps.push(json!({"step": "deploy", "ok": dep.as_ref().map(|o| o.status.success()).unwrap_or(false)}));
    }

    // 8. Webhook
    if let Some(url) = webhook_url {
        let payload = json!({"event":"flux_develop","package":package,"steps":steps});
        let _ = std::process::Command::new("curl")
            .args(["-s","-X","POST",url,"-H","Content-Type: application/json","-d",&payload.to_string(),"--max-time","5"])
            .status();
        steps.push(json!({"step": "webhook", "url": url}));
    }

    let elapsed = start.elapsed().as_secs_f64();
    let all_ok = steps.iter().all(|s| s["ok"].as_bool().unwrap_or(true));

    // Fire internal webhook
    fluxc_core::webhook::auto_dispatch("flux_develop", json!({
        "package": package, "all_ok": all_ok, "elapsed_secs": elapsed, "steps": steps
    }));

    json!({
        "package": package, "all_ok": all_ok,
        "elapsed_secs": format!("{:.1}", elapsed), "steps": steps,
        "cycle": "snapshot → build → cortex AI → heal → test → cortex summary → deploy → webhook",
        "next": if !all_ok { "Check heal step — re-run to auto-fix" } else { "Development cycle complete ✓" },
    }).to_string()
}
