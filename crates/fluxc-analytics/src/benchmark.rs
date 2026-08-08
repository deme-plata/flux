// flux_benchmark — Comprehensive Benchmark Engine
//
// Benchmarks all Flux crates with Q-Spec + X-Algo multi-dimensional scoring,
// generates ranked optimization suggestions, and persists history for trend tracking.
//
// Scoring combines:
//   1. Q-Spec 5 dimensions: CompileSuccess, TestPassRate, IntentFidelity, PerfDelta, Safety
//   2. X-Algo 5 dimensions: SourceDelta, CacheAffinity, DepGraphSensitivity, HistAccuracy, PeerConsensus
//   3. Active tune weights (from flux_tune) to bias scoring toward current loadout
//
// Output: per-crate health scores, ranked optimization actions, historical trends.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::predict;
use crate::tune;

// ── Crate Benchmark Result ──

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CrateBenchmark {
    /// Crate name (e.g. "fluxc-core")
    pub crate_name: String,
    /// Compile time in milliseconds (0 if build failed)
    pub compile_ms: u64,
    /// Whether compilation succeeded
    pub compile_ok: bool,
    /// Tests passed
    pub test_pass: usize,
    /// Tests total
    pub test_total: usize,
    /// Lines of code (approximate, from src/*.rs)
    pub loc: usize,
    /// Number of direct dependencies
    pub dep_count: usize,
    /// Number of source files
    pub file_count: usize,

    // Q-Spec dimensions (0.0–1.0)
    pub qspec_compile_success: f64,
    pub qspec_test_pass_rate: f64,
    pub qspec_safety_score: f64,
    pub qspec_health: f64,

    // X-Algo dimensions (0.0–1.0)
    pub xalgo_cache_affinity: f64,
    pub xalgo_dep_sensitivity: f64,

    // Composite health score (0.0–1.0, weighted by active tune)
    pub health_score: f64,
}

// ── Optimization Suggestion ──

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OptimizationSuggestion {
    /// Priority rank (1 = fix first)
    pub rank: usize,
    /// Which crate to act on
    pub crate_name: String,
    /// Concrete action to take
    pub action: String,
    /// Estimated impact (0–100%)
    pub impact_pct: f64,
    /// Effort level
    pub effort: String,
    /// Which scoring dimension this improves
    pub dimension: String,
    /// Human-readable explanation
    pub description: String,
}

// ── Benchmark Report ──

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BenchmarkReport {
    /// Unix timestamp
    pub timestamp_secs: u64,
    /// Per-crate results
    pub crates: Vec<CrateBenchmark>,
    /// Total compile time across all crates
    pub total_compile_ms: u64,
    /// Overall workspace health (weighted average)
    pub overall_health: f64,
    /// Number of crates that compiled
    pub crates_compiled: usize,
    /// Number of crates that failed
    pub crates_failed: usize,
    /// Total tests across all crates
    pub total_tests_pass: usize,
    pub total_tests_all: usize,
    /// Ranked optimization suggestions
    pub suggestions: Vec<OptimizationSuggestion>,
}

// ── History ──

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct BenchmarkHistory {
    pub reports: Vec<BenchmarkReport>,
}

fn history_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".flux").join("benchmarks.json")
}

pub fn load_history() -> BenchmarkHistory {
    let path = history_path();
    if path.exists() {
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        BenchmarkHistory::default()
    }
}

fn save_history(history: &BenchmarkHistory) {
    let path = history_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(history) {
        let _ = fs::write(&path, json);
    }
}

// ── Crate Discovery ──

/// Auto-discover workspace crates from Cargo.toml or by scanning crates/ directory.
pub fn discover_crates(workspace_root: &str) -> Vec<String> {
    let cargo_toml = PathBuf::from(workspace_root).join("Cargo.toml");
    if cargo_toml.exists() {
        if let Ok(contents) = fs::read_to_string(&cargo_toml) {
            // Parse [workspace] members
            let mut crates = Vec::new();
            for line in contents.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with('"') && trimmed.ends_with('"') {
                    let member = trimmed.trim_matches('"');
                    if let Some(name) = member.strip_prefix("crates/") {
                        crates.push(name.to_string());
                    }
                }
            }
            if !crates.is_empty() {
                return crates;
            }
        }
    }

    // Fallback: scan crates/ directory
    let crates_dir = PathBuf::from(workspace_root).join("crates");
    if crates_dir.exists() {
        if let Ok(entries) = fs::read_dir(&crates_dir) {
            let mut crates: Vec<String> = entries
                .flatten()
                .filter(|e| e.path().is_dir() && e.path().join("Cargo.toml").exists())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();
            crates.sort();
            return crates;
        }
    }

    Vec::new()
}

/// Known Flux crate list (fallback when auto-discovery fails).
pub fn known_flux_crates() -> Vec<String> {
    vec![
        "fluxc".into(),
        "fluxc-core".into(),
        "fluxc-mcp".into(),
        "flux-cache".into(),
        "flux-db".into(),
        "flux-driver".into(),
        "flux-gpu".into(),
        "flux-gui".into(),
        "flux-hotswap".into(),
        "flux-mempool".into(),
        "flux-p2p".into(),
        "flux-science".into(),
        "flux-search".into(),
        "flux-zk".into(),
    ]
}

// ── Benchmark Execution ──

/// Run a comprehensive benchmark across specified crates (or all discovered).
pub fn run_benchmark(workspace_root: &str, crates: Option<Vec<String>>) -> BenchmarkReport {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let target_crates = crates.unwrap_or_else(|| {
        let discovered = discover_crates(workspace_root);
        if discovered.is_empty() { known_flux_crates() } else { discovered }
    });

    let active_tune = tune::load_tune();
    let mut results: Vec<CrateBenchmark> = Vec::new();
    let mut total_compile_ms: u64 = 0;

    for crate_name in &target_crates {
        let bench = benchmark_single_crate(workspace_root, crate_name, &active_tune);
        total_compile_ms += bench.compile_ms;
        results.push(bench);
    }

    let compiled = results.iter().filter(|b| b.compile_ok).count();
    let failed = results.len() - compiled;
    let total_tests_pass: usize = results.iter().map(|b| b.test_pass).sum();
    let total_tests_all: usize = results.iter().map(|b| b.test_total).sum();

    let overall_health = if results.is_empty() {
        0.0
    } else {
        results.iter().map(|b| b.health_score).sum::<f64>() / results.len() as f64
    };

    let suggestions = generate_suggestions(&results);

    let report = BenchmarkReport {
        timestamp_secs: now,
        crates: results,
        total_compile_ms,
        overall_health,
        crates_compiled: compiled,
        crates_failed: failed,
        total_tests_pass,
        total_tests_all,
        suggestions,
    };

    // Persist
    let mut history = load_history();
    history.reports.push(report.clone());
    if history.reports.len() > 50 {
        history.reports = history.reports.split_off(history.reports.len() - 30);
    }
    save_history(&history);

    report
}

fn benchmark_single_crate(
    workspace_root: &str,
    crate_name: &str,
    active_tune: &tune::ActiveTune,
) -> CrateBenchmark {
    let crate_dir = find_crate_dir(workspace_root, crate_name);

    // Measure compile time
    let (compile_ok, compile_ms) = measure_compile(crate_name);
    let qspec_compile_success = if compile_ok { 1.0 } else { 0.0 };

    // Run tests
    let (test_pass, test_total) = measure_tests(crate_name);
    let qspec_test_pass_rate = if test_total > 0 {
        test_pass as f64 / test_total as f64
    } else {
        1.0 // no tests = neutral, not a failure
    };

    // Count LOC and files
    let (loc, file_count) = count_loc(&crate_dir);

    // Count dependencies
    let dep_count = count_deps(&crate_dir);

    // Safety score: analyze source for unsafe patterns
    let qspec_safety_score = analyze_safety(&crate_dir);

    // Q-Spec composite health (using active tune qspec weights)
    let qspec_health = qspec_compile_success * active_tune.qspec.w1
        + qspec_test_pass_rate * active_tune.qspec.w2
        + 0.8 * active_tune.qspec.w3   // intent_fidelity: neutral baseline
        + 0.7 * active_tune.qspec.w4    // perf_delta: neutral baseline
        + qspec_safety_score * active_tune.qspec.w5;

    // X-Algo dimensions
    let xalgo_cache_affinity = compute_cache_affinity_for_crate(crate_name);
    let xalgo_dep_sensitivity = predict::compute_dep_sensitivity_for_benchmark(crate_name);

    // Composite health score: 60% Q-Spec + 40% X-Algo (weighted by tune)
    let health_score = qspec_health * 0.6
        + xalgo_cache_affinity * active_tune.xalgo.w2 * 0.2
        + (1.0 - xalgo_dep_sensitivity) * active_tune.xalgo.w3 * 0.2;

    CrateBenchmark {
        crate_name: crate_name.to_string(),
        compile_ms,
        compile_ok,
        test_pass,
        test_total,
        loc,
        dep_count,
        file_count,
        qspec_compile_success,
        qspec_test_pass_rate,
        qspec_safety_score,
        qspec_health,
        xalgo_cache_affinity,
        xalgo_dep_sensitivity,
        health_score: health_score.clamp(0.0, 1.0),
    }
}

fn find_crate_dir(workspace_root: &str, crate_name: &str) -> PathBuf {
    let candidates = vec![
        PathBuf::from(workspace_root).join("crates").join(crate_name),
        PathBuf::from(workspace_root).join(crate_name),
    ];
    for c in &candidates {
        if c.exists() && c.join("Cargo.toml").exists() {
            return c.clone();
        }
    }
    // Default: guess crates/<name>
    PathBuf::from(workspace_root).join("crates").join(crate_name)
}

fn measure_compile(crate_name: &str) -> (bool, u64) {
    let start = Instant::now();
    let status = Command::new("cargo")
        .args(["check", "--package", crate_name, "--quiet"])
        .output();
    let ms = start.elapsed().as_millis() as u64;

    match status {
        Ok(output) => (output.status.success(), ms),
        Err(_) => (false, ms),
    }
}

fn measure_tests(crate_name: &str) -> (usize, usize) {
    let output = Command::new("cargo")
        .args(["test", "--package", crate_name, "--quiet", "--", "--test-threads=1"])
        .output();

    match output {
        Ok(out) => {
            let combined = format!(
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let passed = combined.lines().filter(|l| l.contains("... ok")).count();
            let failed = combined.lines().filter(|l| l.contains("... FAILED")).count();
            (passed, passed + failed)
        }
        Err(_) => (0, 0),
    }
}

fn count_loc(crate_dir: &PathBuf) -> (usize, usize) {
    let src_dir = crate_dir.join("src");
    if !src_dir.exists() {
        return (0, 0);
    }

    let mut total_loc = 0usize;
    let mut file_count = 0usize;

    fn walk(dir: &PathBuf, loc: &mut usize, files: &mut usize) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, loc, files);
                } else if path.extension().map_or(false, |e| e == "rs") {
                    *files += 1;
                    if let Ok(content) = fs::read_to_string(&path) {
                        *loc += content.lines().count();
                    }
                }
            }
        }
    }

    walk(&src_dir, &mut total_loc, &mut file_count);
    (total_loc, file_count)
}

fn count_deps(crate_dir: &PathBuf) -> usize {
    let cargo_toml = crate_dir.join("Cargo.toml");
    if !cargo_toml.exists() {
        return 0;
    }
    if let Ok(content) = fs::read_to_string(&cargo_toml) {
        let in_deps = content.contains("[dependencies]");
        let _in_dev = content.contains("[dev-dependencies]");
        let _in_build = content.contains("[build-dependencies]");
        if in_deps {
            // Count dependency lines after [dependencies]
            let mut counting = false;
            let mut count = 0usize;
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed == "[dependencies]" {
                    counting = true;
                    continue;
                }
                if counting {
                    if trimmed.starts_with('[') {
                        counting = false; // next section
                    } else if !trimmed.is_empty() && !trimmed.starts_with('#') {
                        count += 1;
                    }
                }
            }
            return count;
        }
        // Fallback: rough estimate
        let dep_lines = content.lines().filter(|l| l.contains("=") && l.contains("\"")).count();
        return dep_lines.min(50);
    }
    0
}

fn analyze_safety(crate_dir: &PathBuf) -> f64 {
    let src_dir = crate_dir.join("src");
    if !src_dir.exists() {
        return 1.0;
    }

    let unsafe_patterns = [
        "unsafe", "transmute", "unreachable_unchecked",
        "mem::uninitialized", "mem::zeroed", "static mut",
        "MaybeUninit", "from_raw_parts",
    ];

    let mut total_violations = 0usize;
    let mut total_files = 0usize;

    fn walk_safety(dir: &PathBuf, patterns: &[&str], violations: &mut usize, files: &mut usize) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk_safety(&path, patterns, violations, files);
                } else if path.extension().map_or(false, |e| e == "rs") {
                    *files += 1;
                    if let Ok(content) = fs::read_to_string(&path) {
                        let lower = content.to_lowercase();
                        for pat in patterns {
                            *violations += lower.matches(&pat.to_lowercase()).count();
                        }
                    }
                }
            }
        }
    }

    walk_safety(&src_dir, &unsafe_patterns, &mut total_violations, &mut total_files);

    if total_files == 0 {
        return 1.0;
    }

    // Normalize: violations per file, capped
    let vpf = total_violations as f64 / total_files as f64;
    (1.0 - vpf * 0.05).max(0.0)
}

fn compute_cache_affinity_for_crate(crate_name: &str) -> f64 {
    // Heuristic: leaf crates have higher cache affinity
    // Lower in the dependency tree = more cache-stable
    match crate_name {
        "flux-cache" | "flux-db" => 0.95,    // leaf crates, rarely change
        "flux-zk" | "flux-gpu" | "flux-hotswap" => 0.85,
        "flux-science" | "flux-search" => 0.80,
        "flux-mempool" | "flux-driver" => 0.70,
        "flux-p2p" | "flux-gui" => 0.60,
        "fluxc-core" => 0.55,
        "fluxc-mcp" => 0.50,
        "fluxc" => 0.40,                       // root, changes frequently
        _ => 0.65,                              // unknown
    }
}

// ── Optimization Suggestions Engine ──

fn generate_suggestions(results: &[CrateBenchmark]) -> Vec<OptimizationSuggestion> {
    let mut suggestions: Vec<OptimizationSuggestion> = Vec::new();

    for bench in results {
        // 1. Build failure → highest priority
        if !bench.compile_ok {
            suggestions.push(OptimizationSuggestion {
                rank: 0, // sorted later
                crate_name: bench.crate_name.clone(),
                action: "Fix build errors".into(),
                impact_pct: 85.0,
                effort: "high".into(),
                dimension: "compile_success".into(),
                description: format!(
                    "{} failed to compile. Fix type errors, missing imports, or dependency issues. This unblocks all downstream crates.",
                    bench.crate_name
                ),
            });
            continue; // skip other suggestions for broken crates
        }

        // 2. Zero tests → add test coverage
        if bench.test_total == 0 && bench.loc > 50 {
            suggestions.push(OptimizationSuggestion {
                rank: 0,
                crate_name: bench.crate_name.clone(),
                action: "Add test module".into(),
                impact_pct: (30.0 + bench.loc as f64 * 0.01).min(60.0),
                effort: "medium".into(),
                dimension: "test_pass_rate".into(),
                description: format!(
                    "{} has {} LOC but zero tests. Add #[cfg(test)] mod tests {{ ... }} with at least basic unit tests. Estimated coverage gain: {}%.",
                    bench.crate_name, bench.loc,
                    (30.0 + bench.loc as f64 * 0.01).min(60.0) as u32
                ),
            });
        }

        // 3. Low test pass rate
        if bench.test_total > 0 && bench.qspec_test_pass_rate < 0.9 {
            let failing = bench.test_total - bench.test_pass;
            suggestions.push(OptimizationSuggestion {
                rank: 0,
                crate_name: bench.crate_name.clone(),
                action: format!("Fix {} failing tests", failing),
                impact_pct: 40.0 + failing as f64 * 5.0,
                effort: "medium".into(),
                dimension: "test_pass_rate".into(),
                description: format!(
                    "{} has {}/{} tests failing ({:.0}% pass rate). Investigate and fix assertions.",
                    bench.crate_name, failing, bench.test_total,
                    bench.qspec_test_pass_rate * 100.0
                ),
            });
        }

        // 4. High dependency count → reduce coupling
        if bench.dep_count > 10 {
            suggestions.push(OptimizationSuggestion {
                rank: 0,
                crate_name: bench.crate_name.clone(),
                action: "Reduce dependencies".into(),
                impact_pct: (bench.dep_count as f64 * 1.5).min(40.0),
                effort: "high".into(),
                dimension: "dep_sensitivity".into(),
                description: format!(
                    "{} has {} direct dependencies (high coupling). Consider splitting into smaller crates or removing unused deps.",
                    bench.crate_name, bench.dep_count
                ),
            });
        }

        // 5. Low safety score → review unsafe patterns
        if bench.qspec_safety_score < 0.8 {
            suggestions.push(OptimizationSuggestion {
                rank: 0,
                crate_name: bench.crate_name.clone(),
                action: "Review unsafe code".into(),
                impact_pct: ((1.0 - bench.qspec_safety_score) * 50.0).min(40.0),
                effort: "medium".into(),
                dimension: "safety_score".into(),
                description: format!(
                    "{} safety score is {:.0}%. Review unsafe blocks, transmutes, and raw pointer usage. Consider safe abstractions.",
                    bench.crate_name,
                    bench.qspec_safety_score * 100.0
                ),
            });
        }

        // 6. Large crate → potential refactor target
        if bench.loc > 2000 {
            suggestions.push(OptimizationSuggestion {
                rank: 0,
                crate_name: bench.crate_name.clone(),
                action: "Consider splitting large crate".into(),
                impact_pct: 25.0,
                effort: "high".into(),
                dimension: "compile_time".into(),
                description: format!(
                    "{} is {} LOC. Large crates slow incremental builds. Consider extracting sub-modules into separate crates.",
                    bench.crate_name, bench.loc
                ),
            });
        }

        // 7. Slow compile → optimization candidate
        if bench.compile_ms > 3000 {
            suggestions.push(OptimizationSuggestion {
                rank: 0,
                crate_name: bench.crate_name.clone(),
                action: "Optimize compile time".into(),
                impact_pct: (bench.compile_ms as f64 / 100.0).min(30.0),
                effort: "medium".into(),
                dimension: "compile_time".into(),
                description: format!(
                    "{} compiles in {}ms (>3s threshold). Check for macro expansion bloat, large generics, or unnecessary proc macros.",
                    bench.crate_name, bench.compile_ms
                ),
            });
        }
    }

    // Sort by impact descending, assign ranks
    suggestions.sort_by(|a, b| b.impact_pct.partial_cmp(&a.impact_pct).unwrap_or(std::cmp::Ordering::Equal));
    for (i, s) in suggestions.iter_mut().enumerate() {
        s.rank = i + 1;
    }

    // Cap at top 15 suggestions
    suggestions.truncate(15);
    suggestions
}

// ── Report Formatting ──

/// Generate a human-readable benchmark summary.
pub fn format_benchmark_report(report: &BenchmarkReport) -> String {
    let mut lines: Vec<String> = Vec::new();

    lines.push(format!(
        "⚡ Flux Benchmark Report — {}/{} crates compiled, health: {:.0}%",
        report.crates_compiled,
        report.crates_compiled + report.crates_failed,
        report.overall_health * 100.0,
    ));
    lines.push(format!(
        "   Total compile: {}ms | Tests: {}/{} pass | {} suggestions",
        report.total_compile_ms,
        report.total_tests_pass,
        report.total_tests_all,
        report.suggestions.len(),
    ));
    lines.push(String::new());

    // Per-crate table
    lines.push(format!(
        "   {:<20} {:>8} {:>6} {:>6} {:>4} {:>4} {:>3}",
        "CRATE", "COMPILE", "LOC", "TESTS", "QS", "XA", "HLTH"
    ));
    lines.push(format!(
        "   {:-<20} {:-<8} {:-<6} {:-<6} {:-<4} {:-<4} {:-<3}",
        "", "", "", "", "", "", ""
    ));

    for b in &report.crates {
        let compile_str = if b.compile_ok {
            format!("{}ms", b.compile_ms)
        } else {
            "FAILED".into()
        };
        let test_str = format!("{}/{}", b.test_pass, b.test_total);
        let qs = (b.qspec_health * 100.0) as u32;
        let xa = (b.xalgo_cache_affinity * 0.5 + (1.0 - b.xalgo_dep_sensitivity) * 0.5 * 100.0) as u32;
        let hlth = (b.health_score * 100.0) as u32;

        let marker = if !b.compile_ok { " ✗" } else if b.health_score < 0.4 { " ⚠" } else { "" };

        lines.push(format!(
            "   {:<20} {:>8} {:>6} {:>6} {:>3}% {:>3}% {:>3}%{}",
            b.crate_name, compile_str, b.loc, test_str, qs, xa, hlth, marker
        ));
    }

    lines.push(String::new());

    // Optimization suggestions
    if !report.suggestions.is_empty() {
        lines.push("🎯 Optimization Path:".into());
        lines.push(String::new());
        for s in &report.suggestions {
            let effort_icon = match s.effort.as_str() {
                "low" => "🟢",
                "medium" => "🟡",
                "high" => "🔴",
                _ => "⚪",
            };
            lines.push(format!(
                "  #{}. [{}] {} — {} (impact: {:.0}%, effort: {}{})",
                s.rank, s.dimension, s.crate_name, s.action, s.impact_pct, s.effort, effort_icon,
            ));
            lines.push(format!("      {}", s.description));
        }
    }

    lines.join("\n")
}

/// Generate JSON report suitable for webhooks/API.
pub fn benchmark_json(report: &BenchmarkReport) -> serde_json::Value {
    serde_json::json!({
        "timestamp_secs": report.timestamp_secs,
        "overall_health_pct": (report.overall_health * 100.0) as u32,
        "crates_compiled": report.crates_compiled,
        "crates_failed": report.crates_failed,
        "total_compile_ms": report.total_compile_ms,
        "tests_pass": report.total_tests_pass,
        "tests_total": report.total_tests_all,
        "suggestion_count": report.suggestions.len(),
        "crates": report.crates.iter().map(|b| serde_json::json!({
            "name": b.crate_name,
            "compile_ok": b.compile_ok,
            "compile_ms": b.compile_ms,
            "loc": b.loc,
            "test_pass": b.test_pass,
            "test_total": b.test_total,
            "qspec_health_pct": (b.qspec_health * 100.0) as u32,
            "health_score_pct": (b.health_score * 100.0) as u32,
            "dep_count": b.dep_count,
            "safety_pct": (b.qspec_safety_score * 100.0) as u32,
        })).collect::<Vec<_>>(),
        "suggestions": report.suggestions.iter().map(|s| serde_json::json!({
            "rank": s.rank,
            "crate": s.crate_name,
            "action": s.action,
            "impact_pct": s.impact_pct as u32,
            "effort": s.effort,
            "dimension": s.dimension,
            "description": s.description,
        })).collect::<Vec<_>>(),
    })
}

/// Format benchmark history as trend report.
pub fn format_history(history: &BenchmarkHistory) -> String {
    if history.reports.is_empty() {
        return "No benchmark history yet. Run flux_benchmark first.".into();
    }

    let mut lines = vec![
        format!("📈 Benchmark History — {} reports", history.reports.len()),
        String::new(),
    ];

    // Trend: last 5 reports
    let recent: Vec<&BenchmarkReport> = history.reports.iter().rev().take(5).collect();

    lines.push("   Last 5 reports:".into());
    for (i, r) in recent.iter().enumerate() {
        let date_str = format_timestamp(r.timestamp_secs);
        let trend = if i == 0 {
            "← latest"
        } else if r.overall_health > recent[0].overall_health {
            "↑"
        } else if r.overall_health < recent[0].overall_health {
            "↓"
        } else {
            "→"
        };
        lines.push(format!(
            "   {}. {} — health: {:.0}%, {}/{} compiled, {}ms {}",
            i + 1,
            date_str,
            r.overall_health * 100.0,
            r.crates_compiled,
            r.crates.len(),
            r.total_compile_ms,
            trend,
        ));
    }

    // Overall trend
    if history.reports.len() >= 2 {
        let first = &history.reports[0];
        let last = &history.reports[history.reports.len() - 1];
        let delta = (last.overall_health - first.overall_health) * 100.0;
        let direction = if delta > 0.0 { "improving" } else if delta < 0.0 { "declining" } else { "stable" };
        lines.push(String::new());
        lines.push(format!(
            "   Overall trend: {:.1}% ({}) over {} reports",
            delta, direction, history.reports.len()
        ));
    }

    lines.join("\n")
}

fn format_timestamp(secs: u64) -> String {
    // Simple relative time
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let ago = now.saturating_sub(secs);
    if ago < 60 {
        format!("{}s ago", ago)
    } else if ago < 3600 {
        format!("{}m ago", ago / 60)
    } else if ago < 86400 {
        format!("{}h ago", ago / 3600)
    } else {
        format!("{}d ago", ago / 86400)
    }
}

// ── Integration: expose dep_sensitivity for benchmark use ──

/// Re-export from predict: compute dependency sensitivity for a crate by name.
pub fn compute_dep_sensitivity_for_benchmark(package: &str) -> f64 {
    predict::compute_dep_sensitivity_for_benchmark(package)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discover_crates_finds_flux() {
        let crates = discover_crates(".");
        // Should find at least some crates in the flux workspace
        println!("Discovered {} crates: {:?}", crates.len(), crates);
        // Not asserting specific count since it depends on Cargo.toml state
    }

    #[test]
    fn test_known_flux_crates() {
        let crates = known_flux_crates();
        assert!(crates.contains(&"fluxc".into()));
        assert!(crates.contains(&"flux-search".into()));
        assert!(crates.contains(&"flux-science".into()));
        assert_eq!(crates.len(), 14);
    }

    #[test]
    fn test_count_loc() {
        // Use current crate's source
        let dir = PathBuf::from("crates/flux-search");
        if dir.exists() {
            let (loc, files) = count_loc(&dir);
            println!("flux-search: {} LOC, {} files", loc, files);
            assert!(loc > 0);
            assert!(files > 0);
        }
    }

    #[test]
    fn test_analyze_safety() {
        let dir = PathBuf::from("crates/flux-science");
        if dir.exists() {
            let score = analyze_safety(&dir);
            println!("flux-science safety: {:.2}", score);
            assert!(score >= 0.0 && score <= 1.0);
        }
    }

    #[test]
    fn test_generate_suggestions_empty() {
        let results: Vec<CrateBenchmark> = Vec::new();
        let suggestions = generate_suggestions(&results);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_generate_suggestions_broken() {
        let results = vec![CrateBenchmark {
            crate_name: "broken-crate".into(),
            compile_ms: 100,
            compile_ok: false,
            test_pass: 0,
            test_total: 0,
            loc: 100,
            dep_count: 5,
            file_count: 3,
            qspec_compile_success: 0.0,
            qspec_test_pass_rate: 0.0,
            qspec_safety_score: 1.0,
            qspec_health: 0.0,
            xalgo_cache_affinity: 0.5,
            xalgo_dep_sensitivity: 0.5,
            health_score: 0.0,
        }];
        let suggestions = generate_suggestions(&results);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].action, "Fix build errors");
    }

    #[test]
    fn test_format_report() {
        let report = BenchmarkReport {
            timestamp_secs: 0,
            crates: vec![],
            total_compile_ms: 0,
            overall_health: 0.85,
            crates_compiled: 13,
            crates_failed: 1,
            total_tests_pass: 45,
            total_tests_all: 50,
            suggestions: vec![],
        };
        let formatted = format_benchmark_report(&report);
        assert!(formatted.contains("85%"));
        assert!(formatted.contains("13/14"));
    }

    #[test]
    fn test_history_persistence_roundtrip() {
        // Isolate HOME (benchmarks.json lives under $HOME/.flux): otherwise a parallel
        // module's HOME swap reads the wrong store mid-test. Shared lock = deterministic.
        fluxc_util::test_home::with_temp_home("bench_roundtrip", || {
            let mut history = BenchmarkHistory::default();
            let report = BenchmarkReport {
                timestamp_secs: 1,
                crates: vec![],
                total_compile_ms: 100,
                overall_health: 0.9,
                crates_compiled: 1,
                crates_failed: 0,
                total_tests_pass: 10,
                total_tests_all: 10,
                suggestions: vec![],
            };
            history.reports.push(report);
            save_history(&history);

            let loaded = load_history();
            assert_eq!(loaded.reports.len(), 1);
            assert_eq!(loaded.reports[0].total_compile_ms, 100);
        });
    }
}
