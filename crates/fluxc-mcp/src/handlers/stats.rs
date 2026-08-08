use serde_json::{json, Value};
use crate::handlers::{ToolDef, ToolRegistry};

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_stats",
            description: "Show Flux build statistics: cache hit rate, build times, project info.",
            input_schema: json!({"type": "object", "properties": {}}),
        },
        flux_stats,
    );
    registry.register(
        ToolDef {
            name: "flux_version",
            description: "Get fluxc version and system info.",
            input_schema: json!({"type": "object", "properties": {}}),
        },
        flux_version,
    );
    registry.register(
        ToolDef {
            name: "flux_benchmark",
            description: "Comprehensive multi-crate benchmark with Q-Spec + X-Algo scoring.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "format": {"type": "string", "description": "Output format: text (default) or json"},
                    "crates": {"type": "array", "items": {"type": "string"}, "description": "Specific crates to benchmark"}
                }
            }),
        },
        flux_benchmark,
    );
    registry.register(
        ToolDef {
            name: "flux_benchmark_history",
            description: "Historical benchmark trends across all crates.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "crate_name": {"type": "string", "description": "Filter to a specific crate"},
                    "limit": {"type": "integer", "description": "Max history entries (default: 20)"}
                }
            }),
        },
        flux_benchmark_history,
    );
    registry.register(
        ToolDef {
            name: "flux_health_report",
            description: "Comprehensive health report: architecture score + SWOT + prediction + cache + tune. Returns text or JSON.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "format": {"type": "string", "description": "Output format: text (default) or json"},
                    "package": {"type": "string", "description": "Package to predict for (default: fluxc)"}
                }
            }),
        },
        flux_health_report,
    );
    // ── Version Management ──
    registry.register(
        ToolDef {
            name: "flux_version_status",
            description: "Audit all crate versions vs workspace. Reports inconsistencies.",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        flux_version_status,
    );
    registry.register(
        ToolDef {
            name: "flux_version_bump",
            description: "Bump workspace version (major, minor, patch). Updates Cargo.toml.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "component": {"type": "string", "description": "Which component to bump: major, minor, or patch"},
                    "dry_run": {"type": "boolean", "description": "If true, show what would change without writing"}
                },
                "required": ["component"]
            }),
        },
        flux_version_bump,
    );
    registry.register(
        ToolDef {
            name: "flux_version_sync",
            description: "Sync all crate Cargo.toml files to use version.workspace = true.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "dry_run": {"type": "boolean", "description": "If true, show what would change without writing"}
                }
            }),
        },
        flux_version_sync,
    );
}

use fluxc_webhooks::webhook;
use fluxc_core::quantum_architect;
use fluxc_analytics::{predict, tune};

fn flux_stats(_args: &Value) -> String {
    let hits = fluxc_core::CACHE_HITS.load(std::sync::atomic::Ordering::Relaxed);
    let misses = fluxc_core::CACHE_MISSES.load(std::sync::atomic::Ordering::Relaxed);
    let builds = fluxc_core::BUILD_COUNT.load(std::sync::atomic::Ordering::Relaxed);
    let total_time = fluxc_core::TOTAL_BUILD_TIME_MS.load(std::sync::atomic::Ordering::Relaxed);
    let total = hits + misses;
    let rate = if total > 0 { (hits as f64 / total as f64) * 100.0 } else { 0.0 };
    let avg_time = if builds > 0 { total_time / builds } else { 0 };

    let project_type = fluxc_core::detect_project();

    let (cu_hits, cu_misses) = fluxc_core::cache_event_counts();
    let cu_total = cu_hits + cu_misses;
    let cu_rate = if cu_total > 0 { (cu_hits as f64 / cu_total as f64) * 100.0 } else { 0.0 };

    format!(
        "⚡ Flux Statistics\n  Project: {:?}\n  Builds: {}\n  Cache: {}/{} ({:.1}%)\n  Compile cache: {}/{} units ({:.1}%)\n  Avg time: {}ms\n  Total time: {}ms",
        project_type, builds, hits, total, rate, cu_hits, cu_total, cu_rate, avg_time, total_time
    )
}

fn flux_version(_args: &Value) -> String {
    // Dynamic version from workspace Cargo.toml
    let vi = fluxc_core::version::VersionInfo::load(
        &fluxc_core::version::workspace_root(),
    );
    let version_banner = vi
        .as_ref()
        .map(|v| v.banner())
        .unwrap_or_else(|_| "fluxc (version unknown)".into());

    format!(
        "{}\n  Rust: {}\n  Target: {}\n  Profile: {}\n  Cache: content-hash (SHA-256 + BLAKE3)\n  Self-hosting: dogfooding (fluxc self)\n  Prediction: X-Algo 5-dimension build prediction + feedback loop\n  Webhooks: HMAC-SHA256 signed POSTs on build/test/bench/prediction events (non-blocking dispatch)",
        version_banner,
        option_env!("CARGO_PKG_VERSION").unwrap_or("unknown"),
        std::env::consts::ARCH,
        if cfg!(debug_assertions) { "debug" } else { "release" },
    )
}

fn flux_benchmark(args: &Value) -> String {
    let format = args.get("format").and_then(|v| v.as_str()).unwrap_or("text");
    let crates: Option<Vec<String>> = args.get("crates").and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect());

    let all_crates: &[&str] = &[
        "flux-cache", "flux-db", "flux-driver", "flux-gpu", "flux-gui",
        "flux-hotswap", "flux-mempool", "flux-p2p", "flux-science",
        "flux-search", "flux-sniff", "flux-zk", "fluxc-core", "fluxc-mcp", "fluxc",
    ];

    let targets: Vec<&str> = if let Some(ref c) = crates {
        all_crates.iter().filter(|n| c.iter().any(|c| c == **n)).copied().collect()
    } else {
        all_crates.to_vec()
    };

    let start = std::time::Instant::now();
    let mut results: Vec<(String, u64, f64, f64)> = Vec::new();

    for crate_name in &targets {
        let p = predict::predict_build(crate_name, false, &[]);
        results.push((crate_name.to_string(), p.predicted_ms, p.predicted_cache_rate, p.confidence));
    }

    let total_ms = start.elapsed().as_millis();
    let total_pred: u64 = results.iter().map(|(_, ms, _, _)| *ms).sum();
    let avg_conf: f64 = results.iter().map(|(_, _, _, c)| c).sum::<f64>() / results.len() as f64;

    if format == "json" {
        let items: Vec<Value> = results.iter().map(|(name, ms, cache, conf)| json!({
            "crate": name, "predicted_ms": ms,
            "cache_rate_pct": (cache * 100.0).round(),
            "confidence_pct": (conf * 100.0).round(),
        })).collect();
        serde_json::to_string_pretty(&json!({
            "crates_benchmarked": results.len(),
            "total_predicted_ms": total_pred,
            "avg_confidence_pct": (avg_conf * 100.0).round(),
            "compute_ms": total_ms,
            "results": items,
        })).unwrap_or_else(|e| format!("json: {}", e))
    } else {
        let mut lines = vec![format!(
            "⚡ Flux Benchmark — {} crates in {}ms\n  Total predicted: {}ms · Avg conf: {:.0}%",
            results.len(), total_ms, total_pred, avg_conf * 100.0
        )];
        for (name, ms, cache, conf) in &results {
            lines.push(format!(
                "  {} — {}ms · {:.0}% cache · {:.0}% conf",
                name, ms, cache * 100.0, conf * 100.0,
            ));
        }
        lines.join("\n")
    }
}

fn flux_benchmark_history(args: &Value) -> String {
    let crate_name = args.get("crate_name").and_then(|v| v.as_str()).unwrap_or("fluxc");
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

    let pred = predict::predict_build(crate_name, false, &[]);
    format!(
        "📈 Benchmark History: {} (last {})\n  Current: {}ms · {:.0}% cache · {:.0}% conf\n  History: {} predictions recorded · model calibrated",
        crate_name, limit,
        pred.predicted_ms,
        pred.predicted_cache_rate * 100.0,
        pred.confidence * 100.0,
        predict::load_history().total_predictions,
    )
}

fn flux_health_report(args: &Value) -> String {
    let format = args.get("format").and_then(|v| v.as_str()).unwrap_or("text");
    let package = args.get("package").and_then(|v| v.as_str()).unwrap_or("fluxc");

    let start = std::time::Instant::now();
    let arch = crate::handlers::analyze_ws();
    let swot = quantum_architect::generate_swot(&arch);
    let pred = predict::predict_build(package, false, &[]);
    let tune = tune::load_tune();

    let hits = fluxc_core::CACHE_HITS.load(std::sync::atomic::Ordering::Relaxed);
    let misses = fluxc_core::CACHE_MISSES.load(std::sync::atomic::Ordering::Relaxed);
    let total = hits + misses;
    let cache_rate = if total > 0 { (hits as f64 / total as f64) * 100.0 } else { 0.0 };
    let builds = fluxc_core::BUILD_COUNT.load(std::sync::atomic::Ordering::Relaxed);
    let total_time = fluxc_core::TOTAL_BUILD_TIME_MS.load(std::sync::atomic::Ordering::Relaxed);
    let avg_time = if builds > 0 { total_time / builds } else { 0 };
    let total_loc: usize = arch.crates.iter().map(|b| b.loc).sum();

    let health_score = arch.architecture_score * 100.0;
    let ms = start.elapsed().as_millis();

    if format == "json" {
        let report = json!({
            "tool": "flux_health_report",
            "version": fluxc_core::version::VersionInfo::load(
                &fluxc_core::version::workspace_root(),
            ).map(|vi| vi.raw).unwrap_or_else(|_| "unknown".into()),
            "health_score": health_score,
            "architecture": {
                "score_pct": health_score,
                "crates": arch.crates.len(),
                "loc": total_loc,
                "top_actions": arch.priority_actions.iter().take(3).map(|a| json!({
                    "rank": a.rank, "crate": a.crate_name, "action": a.action,
                    "impact_pct": a.impact * 100.0, "effort": a.effort,
                })).collect::<Vec<_>>(),
            },
            "swot": {
                "strengths": swot.strengths.len(), "weaknesses": swot.weaknesses.len(),
                "opportunities": swot.opportunities.len(), "threats": swot.threats.len(),
                "top_priority": swot.top_priority,
            },
            "prediction": {
                "predicted_ms": pred.predicted_ms,
                "cache_rate_pct": pred.predicted_cache_rate * 100.0,
                "test_pass_pct": pred.predicted_test_pass * 100.0,
                "confidence_pct": pred.confidence * 100.0,
            },
            "cache": {
                "hits": hits, "misses": misses, "rate_pct": cache_rate,
                "builds": builds, "avg_time_ms": avg_time, "total_time_ms": total_time,
            },
            "tune": { "preset": tune.preset_name },
            "compute_ms": ms,
        });
        serde_json::to_string_pretty(&report).unwrap_or_else(|e| format!("json error: {}", e))
    } else {
        let vstr = fluxc_core::version::VersionInfo::load(
            &fluxc_core::version::workspace_root(),
        ).map(|vi| vi.display()).unwrap_or_else(|_| "v?.?.?".into());
        format!(
            "🏥 Flux Health Report — {}\n\n  ⚛️  Architecture: {:.1}% ideal · {} crates · {} LOC\n\n  📊 SWOT: {} strengths · {} weaknesses · {} opportunities · {} threats\n     Top priority: {}\n\n  🔮 Prediction ({}): {}ms · {:.0}% cache · {:.0}% test pass · {:.0}% confidence\n\n  💾 Cache: {:.1}% hit rate ({}/{} hits) · {} builds · {}ms avg\n\n  🎮 Tune: {}\n\n  ⏱ Report generated in {}ms",
            vstr,
            health_score, arch.crates.len(), total_loc,
            swot.strengths.len(), swot.weaknesses.len(), swot.opportunities.len(), swot.threats.len(),
            swot.top_priority,
            package, pred.predicted_ms,
            pred.predicted_cache_rate * 100.0, pred.predicted_test_pass * 100.0, pred.confidence * 100.0,
            cache_rate, hits, total, builds, avg_time,
            tune.preset_name,
            ms,
        )
    }
}

// ── flux_version_status ──
fn flux_version_status(_args: &Value) -> String {
    // Use workspace_root() — exe-anchored — so this works when the MCP server's
    // cwd is /home/storage/claude-code or any other non-workspace dir.
    let ws_dir = fluxc_core::version::workspace_root();
    let vi = match fluxc_core::version::VersionInfo::load(&ws_dir) {
        Ok(v) => v,
        Err(e) => return format!("✗ Cannot load version: {}", e),
    };

    let crates = match fluxc_core::version::audit_crate_versions(&ws_dir) {
        Ok(c) => c,
        Err(e) => return format!("✗ Cannot audit crates: {}", e),
    };

    let consistent: Vec<_> = crates.iter().filter(|c| c.consistent).collect();
    let inconsistent: Vec<_> = crates.iter().filter(|c| !c.consistent).collect();

    let mut lines = vec![
        format!("📦 Flux Version Status"),
        format!("  Workspace: {}", vi.display()),
        format!("  Crates: {} total · {} consistent · {} stale", crates.len(), consistent.len(), inconsistent.len()),
    ];

    if !inconsistent.is_empty() {
        lines.push("".into());
        lines.push("⚠ Stale crate versions (hardcoded, not using workspace):".into());
        for c in &inconsistent {
            lines.push(format!("  {} — {} (should be {})", c.name, c.source, vi.raw));
        }
    }

    if !consistent.is_empty() {
        lines.push("".into());
        lines.push(format!("✅ Consistent crates ({}/{}):", consistent.len(), crates.len()));
        for c in &consistent {
            lines.push(format!("  {} — {}", c.name, c.version));
        }
    }

    lines.join("\n")
}

// ── flux_version_bump ──
fn flux_version_bump(args: &Value) -> String {
    let component = args.get("component")
        .and_then(|v| v.as_str())
        .unwrap_or("patch");
    let dry_run = args.get("dry_run")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Use workspace_root() — exe-anchored — so this works when the MCP server's
    // cwd is /home/storage/claude-code or any other non-workspace dir.
    let ws_dir = fluxc_core::version::workspace_root();
    let vi_current = match fluxc_core::version::VersionInfo::load(&ws_dir) {
        Ok(v) => v,
        Err(e) => return format!("✗ Cannot load version: {}", e),
    };

    // Compute what the new version would be
    let new_raw = match component {
        "major" => format!("{}.0.0", vi_current.major + 1),
        "minor" => format!("{}.{}.0", vi_current.major, vi_current.minor + 1),
        "patch" => format!("{}.{}.{}", vi_current.major, vi_current.minor, vi_current.patch + 1),
        _ => return format!("✗ Unknown component: {}. Use major, minor, or patch.", component),
    };

    if dry_run {
        return format!(
            "🔍 Dry run — would bump {} from {} to {}\n  File: {}",
            component,
            vi_current.display(),
            format!("v{}", new_raw),
            vi_current.workspace_path.display(),
        );
    }

    let mut vi = vi_current;
    match vi.bump(component) {
        Ok(new_v) => format!(
            "✅ Bumped {}: {} → {}\n  File: {}",
            component,
            format!("v{}", new_raw.split('.').collect::<Vec<_>>().join(".")),
            format!("v{}", new_v),
            vi.workspace_path.display(),
        ),
        Err(e) => format!("✗ Bump failed: {}", e),
    }
}

// ── flux_version_sync ──
fn flux_version_sync(args: &Value) -> String {
    let dry_run = args.get("dry_run")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Use workspace_root() — exe-anchored — so this works when the MCP server's
    // cwd is /home/storage/claude-code or any other non-workspace dir.
    let ws_dir = fluxc_core::version::workspace_root();
    let crates = match fluxc_core::version::audit_crate_versions(&ws_dir) {
        Ok(c) => c,
        Err(e) => return format!("✗ Cannot audit crates: {}", e),
    };

    let inconsistent: Vec<_> = crates.iter().filter(|c| !c.consistent).collect();

    if inconsistent.is_empty() {
        return "✅ All crates already use version.workspace = true".into();
    }

    if dry_run {
        let mut lines = vec![
            format!("🔍 Dry run — {} crate(s) would be synced to workspace:", inconsistent.len())
        ];
        for c in &inconsistent {
            lines.push(format!("  {}: {} → workspace", c.name, c.source));
        }
        return lines.join("\n");
    }

    // Perform sync: replace hardcoded version with version.workspace = true
    let mut synced = 0;
    let mut errors = Vec::new();
    for c in &inconsistent {
        let toml_path = ws_dir.join("crates").join(&c.name).join("Cargo.toml");
        match std::fs::read_to_string(&toml_path) {
            Ok(content) => {
                let old_line = format!("version = \"{}\"", c.source);
                if content.contains(&old_line) {
                    let new_content = content.replace(
                        &old_line,
                        "version.workspace = true",
                    );
                    match std::fs::write(&toml_path, &new_content) {
                        Ok(_) => synced += 1,
                        Err(e) => errors.push(format!("{}: {}", c.name, e)),
                    }
                } else {
                    errors.push(format!("{}: pattern '{}' not found", c.name, old_line));
                }
            }
            Err(e) => errors.push(format!("{}: {}", c.name, e)),
        }
    }

    let mut lines = vec![format!("✅ Synced {}/{} crates to workspace", synced, inconsistent.len())];
    if !errors.is_empty() {
        lines.push("".into());
        lines.push("⚠ Errors:".into());
        for e in &errors {
            lines.push(format!("  {}", e));
        }
    }

    lines.join("\n")
}
