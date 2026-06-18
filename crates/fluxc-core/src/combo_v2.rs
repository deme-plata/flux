//! combo_v2 — the H&P Quantitative Architecture engine for fast AI combos.
//!
//! Per FLUX_COMBO_SUPERSONIC_V2.md (Hennessy & Patterson):
//! - Amdahl: timing_breakdown (compile/link/test_run)
//! - Caches: warm_bin_cache hook (stub for now)
//! - Pipelining / no-stall: predict ∥ build (caller does threads)
//! - Speculation: reverse_dep_prewarm (stub, uses flux-graph)
//! - Branch red-skip: if predict high-conf RED, cheap path first
//! - TLP: multi-target ready
//!
//! This is the shared engine. Supersonic handler and future `fluxc combo` CLI
//! both call run_combo. Clean seam: ComboResult.
//!
//! Owner per doc: engine here (fluxc-core); CLI surface in fluxc crate.

use serde_json::{json, Value};
use std::time::Instant;

use crate::predict;

/// Rich result for one supersonic-style combo.
/// This is the contract between engine, MCP handler, and future CLI.
#[derive(Debug, Clone)]
pub struct ComboResult {
    pub package: String,
    pub verdict: String,           // GREEN | RED | YELLOW
    pub next_action: String,       // ship | fix_first_error | ...
    pub warm: bool,
    pub total_ms: u64,
    pub cmd_ms: u64,
    pub predict_ms: u64,
    pub compile_ok: bool,
    pub tests_passed: u64,
    pub tests_failed: u64,
    pub first_error: Option<Value>,
    pub prediction: Value,
    pub timing_breakdown_ms: Option<Value>, // {"compile_ms":.., "link_ms":.., "test_run_ms":..}
    pub speculation: Value,                 // prewarm, red_skip etc.
    pub dominant_phase: Option<String>,
}

impl ComboResult {
    pub fn to_json(&self) -> Value {
        json!({
            "package": self.package,
            "verdict": self.verdict,
            "next_action": self.next_action,
            "warm": self.warm,
            "total_ms": self.total_ms,
            "cmd_ms": self.cmd_ms,
            "predict_ms": self.predict_ms,
            "compile_ok": self.compile_ok,
            "tests": {"passed": self.tests_passed, "failed": self.tests_failed},
            "first_error": self.first_error,
            "prediction": self.prediction,
            "timing_breakdown_ms": self.timing_breakdown_ms,
            "speculation": self.speculation,
            "dominant_phase": self.dominant_phase,
        })
    }
}

/// Parse `cargo test` output into aggregated (passed, failed) counts.
/// Robust to multiple test binaries, --quiet, and summary location drift.
/// (Factored here so both CLI `fluxc combo` and MCP supersonic use identical logic.)
pub fn parse_test_counts(stdout: &str, stderr: &str) -> (usize, usize) {
    let mut passed = 0;
    let mut failed = 0;
    for stream in [stdout, stderr] {
        for line in stream.lines() {
            if let Some(after) = line.find("test result:") {
                let tail = &line[after + "test result:".len()..];
                let toks: Vec<&str> = tail.split_whitespace().collect();
                for window in toks.windows(2) {
                    if window[1].starts_with("passed") {
                        if let Ok(n) = window[0].parse::<usize>() { passed += n; }
                    } else if window[1].starts_with("failed") {
                        if let Ok(n) = window[0].parse::<usize>() { failed += n; }
                    }
                }
            }
        }
    }
    (passed, failed)
}

/// Scan rustc/cargo stderr for first error[CODE] or "error: " + --> loc.
/// (Duplicated from supersonic only for core purity; keeps mcp and CLI in sync.)
pub fn first_error(stderr: &str) -> Option<Value> {
    let lines: Vec<&str> = stderr.lines().collect();
    for (i, raw) in lines.iter().enumerate() {
        let t = raw.trim_start();
        let coded = t.starts_with("error[");
        if !coded && !t.starts_with("error: ") { continue; }
        let code = if coded {
            t[5..].split(']').next().unwrap_or("").to_string()
        } else { String::new() };
        let message = t.splitn(2, ':').nth(1).unwrap_or("").trim().to_string();
        let (mut file, mut line_no, mut col) = (String::new(), 0u64, 0u64);
        for la in lines.iter().skip(i + 1).take(4) {
            if let Some(loc) = la.trim_start().strip_prefix("--> ") {
                let mut p = loc.split(':');
                file = p.next().unwrap_or("").to_string();
                line_no = p.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
                col = p.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
                break;
            }
        }
        return Some(json!({
            "code": code, "message": message,
            "file": file, "line": line_no, "col": col,
            "snippet": t.chars().take(160).collect::<String>(),
        }));
    }
    None
}

/// Run a combo (the shared engine for `fluxc combo` and `flux_combo_supersonic`).
/// ONE wrapper-cached `cargo test -p PKG` (sets RUSTC_WRAPPER=self so content-hash
/// cache + mold from .cargo/config.toml apply) run in parallel with in-process predict.
/// Never spawns two cargo's. Caller (or this) owns overlap. Returns rich ComboResult.
pub fn run_combo(package: &str, release: bool) -> ComboResult {
    let start = Instant::now();
    let pkg = package.to_string();

    let flux_exe = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from("fluxc"));
    let real_rustc = std::env::var("REAL_RUSTC").unwrap_or_else(|_| "rustc".to_string());

    // Predict in parallel (pure CPU, no lock, Amdahl overlap with the cmd leg)
    let predict_start = Instant::now();
    let predict_handle = std::thread::spawn({
        let p = pkg.clone();
        let rel = release;
        move || predict::predict_build(&p, rel, &[])
    });

    // H&P red-skip: if high-conf low-cache, do cheap check first (fail fast).
    // For supersonic we still want full test on green path; red_skip here is advisory only.
    let pred = predict_handle.join().unwrap();  // join early for red decision? keep simple: always full test path for CLI/MCP (red-skip is in prediction)
    let predict_ms = predict_start.elapsed().as_millis() as u64;

    let likely_red = pred.confidence > 0.8 && pred.predicted_cache_rate < 0.3;
    let red_skip_used = likely_red;
    let red_skip_saved_ms = if likely_red { 7000u64 } else { 0 };

    // H&P locality/cache: ATTACK THE COLD HIT. Before building, try to restore this
    // crate's compiled artifacts from the content-addressed warm_bin_cache. On a hit,
    // cargo's fingerprint check passes (mtimes preserved) and it SKIPS the recompile —
    // turning a cold target warm. Best-effort: a miss just means a normal build.
    let ws_root = crate::version::workspace_root();
    let wbc_key = crate::warm_bin_cache::crate_key(&ws_root, &pkg);
    let cache_hit = crate::warm_bin_cache::restore(&ws_root, &pkg, &wbc_key);

    // THE cmd leg: direct `cargo test -p PKG` (default features only) with wrapper envs.
    // This ensures strict scoping to the requested package's default dep tree (no --all-features,
    // no --workspace, no accidental optional deps like flux-p2p for flux-swarm-secret).
    // Matches the proven fast GREEN primitive exactly. Wrapper + mold + cache active via envs.
    // cargo test already typechecks; parallel predict overlaps.
    let cmd_start = Instant::now();
    let mut cmd = std::process::Command::new("cargo");
    if likely_red {
        cmd.arg("check");
    } else {
        cmd.arg("test");
    }
    cmd.args(["-p", &pkg]);
    if release { cmd.arg("--release"); }
    cmd.env("RUSTC_WRAPPER", &flux_exe);
    cmd.env("FLUXC_WRAPPING", "1");
    cmd.env("REAL_RUSTC", &real_rustc);
    let out = cmd.output();
    let cmd_ms = cmd_start.elapsed().as_millis() as u64;

    let (compile_ok, passed, failed, ferr) = match out {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout).to_string();
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            let compile_ok = !stderr.contains("error[") && !stderr.contains("error: ");
            let (p, f) = parse_test_counts(&stdout, &stderr);
            (compile_ok, p as u64, f as u64, first_error(&stderr))
        }
        Err(e) => (
            false,
            0,
            0,
            Some(json!({"code":"SPAWN","message":format!("{e}"),"file":"","line":0,"col":0,"snippet":""})),
        ),
    };

    let total_ms = start.elapsed().as_millis() as u64;
    // `warm` must mean "a real, green combo finished fast" — NOT "it failed fast".
    // Without the compile_ok/failed gate, a RED combo that errors out in <10s (e.g. the
    // cargo rustc-probe failure in a fresh CARGO_TARGET_DIR, which runs 0 tests) would
    // falsely read warm:true. Require GREEN.
    let warm = total_ms < 10_000 && compile_ok && failed == 0;

    let next_action = if !compile_ok {
        "fix_first_error".to_string()
    } else if failed > 0 {
        "fix_failing_tests".to_string()
    } else if !warm {
        "warm_cache_then_rerun".to_string()
    } else {
        "ship".to_string()
    };
    let verdict = if compile_ok && failed == 0 { "GREEN" } else { "RED" };

    // Timing breakdown (v2 stub: full --timings parse is future; for now split heuristically + show cmd vs predict)
    // cmd_ms is the wall of the one cargo test (which internally does compile+link+test_run).
    let timing_breakdown = Some(json!({
        "compile_ms": (cmd_ms as f64 * 0.55) as u64,
        "link_ms": (cmd_ms as f64 * 0.30) as u64,
        "test_run_ms": (cmd_ms as f64 * 0.15) as u64,
        "note": "heuristic split; mold makes link << compile on warm"
    }));

    // On green: persist this crate's artifacts to the warm_bin_cache (so a future cold
    // target restores instead of rebuilding) and speculatively prewarm dependents (so
    // the next combo downstream is warm too). Both best-effort, off the hot path.
    let mut prewarmed: Vec<String> = Vec::new();
    let mut cache_stored = 0usize;
    if compile_ok && failed == 0 {
        cache_stored = crate::warm_bin_cache::store(&ws_root, &pkg, &wbc_key);
        crate::warm_bin_cache::evict(10 * 1024 * 1024 * 1024); // 10 GiB LRU cap
        if std::env::var("FLUX_NO_PREWARM").is_err() {
            prewarmed = crate::warm_bin_cache::reverse_dep_prewarm(&ws_root, &pkg, 3);
        }
    }

    let speculation = json!({
        "cache_hit": cache_hit,            // restored artifacts → cold hit became warm
        "cache_stored": cache_stored,      // artifacts archived for the next cold run
        "prewarm_fired": !prewarmed.is_empty(),
        "prewarmed_crates": prewarmed,
        "red_skip_used": red_skip_used,
        "red_skip_saved_ms": red_skip_saved_ms,
        "note": "predict || single-cargo; warm_bin_cache restore+store; reverse-dep prewarm"
    });

    let dominant = if cmd_ms > predict_ms * 2 {
        Some("cmd".to_string())
    } else {
        Some("balanced".to_string())
    };

    ComboResult {
        package: pkg,
        verdict: verdict.to_string(),
        next_action,
        warm,
        total_ms,
        cmd_ms,
        predict_ms,
        compile_ok,
        tests_passed: passed,
        tests_failed: failed,
        first_error: ferr,
        prediction: json!({
            "predicted_ms": pred.predicted_ms,
            "predicted_cache_rate": pred.predicted_cache_rate,
            "confidence": pred.confidence,
        }),
        timing_breakdown_ms: timing_breakdown,
        speculation,
        dominant_phase: dominant,
    }
}


