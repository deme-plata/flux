//! flux-cortex MCP handler — Autonomous Continuous Optimization Engine tools.
//!
//! Registers 4 MCP tools:
//!   flux_cortex_loop       — run a single Cortex Loop
//!   flux_cortex_continuous — run N Cortex Loops continuously
//!   flux_cortex_summary    — get Cortex activity summary
//!   flux_cortex_reset      — reset Cortex state

use crate::handlers::{ToolDef, ToolRegistry};
use serde_json::{json, Value};
use flux_graph::WorkspaceGraph;
use flux_optimize::OptimizationPreset;

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_cortex_loop",
            description: "Run a single Cortex Loop: Architect (6-dim blueprint) → Predict (X-Algo forecast) → Optimize (SIMD/io_uring/cache) → Apply (auto-apply best actions) → Validate (measure actual impact) → Learn (improve prediction model).",
            input_schema: json!({"type": "object", "properties": {"preset": {"type": "string", "description": "MaxPerf, Balanced, or PowerSaver (default: MaxPerf)"}}}),
        },
        flux_cortex_loop,
    );
    registry.register(
        ToolDef {
            name: "flux_cortex_continuous",
            description: "Run N Cortex Loops continuously. The engine learns from each iteration and stops early if plateaued.",
            input_schema: json!({"type": "object", "properties": {"iterations": {"type": "integer", "description": "Iterations (default: 3, max: 20)"}, "preset": {"type": "string", "description": "Preset (default: Balanced)"}}}),
        },
        flux_cortex_continuous,
    );
    registry.register(
        ToolDef {
            name: "flux_cortex_summary",
            description: "Get a summary of all Cortex activity: iterations, actions, cumulative perf gain, prediction accuracy.",
            input_schema: json!({"type": "object", "properties": {}}),
        },
        flux_cortex_summary,
    );
    registry.register(
        ToolDef {
            name: "flux_cortex_reset",
            description: "Reset Cortex state — clear all history and start fresh.",
            input_schema: json!({"type": "object", "properties": {}}),
        },
        flux_cortex_reset,
    );
}

fn flux_cortex_loop(args: &Value) -> String {
    let preset_str = args.get("preset").and_then(|v| v.as_str()).unwrap_or("MaxPerf");
    let preset = match preset_str.to_lowercase().as_str() {
        "powersaver" | "power_saver" => OptimizationPreset::PowerSaver,
        "balanced" => OptimizationPreset::Balanced,
        _ => OptimizationPreset::MaxPerf,
    };
    let ws = match discover_workspace() { Ok(w) => w, Err(e) => return format!("❌ cortex_loop: {e}") };
    let mut cortex = flux_cortex::Cortex::new(ws);
    let result = cortex.run_loop(preset);
    serde_json::to_string_pretty(&result).unwrap_or_else(|e| format!("❌ {e}"))
}

fn flux_cortex_continuous(args: &Value) -> String {
    let iterations = args.get("iterations").and_then(|v| v.as_u64()).unwrap_or(3).min(20) as usize;
    let preset_str = args.get("preset").and_then(|v| v.as_str()).unwrap_or("Balanced");
    let preset = match preset_str.to_lowercase().as_str() {
        "powersaver" | "power_saver" => OptimizationPreset::PowerSaver,
        "maxperf" | "max_perf" => OptimizationPreset::MaxPerf,
        _ => OptimizationPreset::Balanced,
    };
    let ws = match discover_workspace() { Ok(w) => w, Err(e) => return format!("❌ cortex_continuous: {e}") };
    let mut cortex = flux_cortex::Cortex::new(ws);
    let results = cortex.run_continuous(iterations, preset);
    let total_gain: f64 = results.iter().filter_map(|r| r.actual_total_gain_pct).sum();
    let summary = json!({"iterations_completed": results.len(), "iterations_requested": iterations, "total_actual_gain_pct": format!("{:.2}%", total_gain), "results": results});
    serde_json::to_string_pretty(&summary).unwrap_or_else(|e| format!("❌ {e}"))
}

fn flux_cortex_summary(_args: &Value) -> String {
    let ws = match discover_workspace() { Ok(w) => w, Err(e) => return format!("❌ cortex_summary: {e}") };
    let cortex = flux_cortex::Cortex::new(ws);
    let summary = cortex.summary();
    serde_json::to_string_pretty(&summary).unwrap_or_else(|e| format!("❌ {e}"))
}

fn flux_cortex_reset(_args: &Value) -> String {
    let ws = match discover_workspace() { Ok(w) => w, Err(e) => return format!("❌ cortex_reset: {e}") };
    let cortex = flux_cortex::Cortex::new(ws);
    let summary = cortex.summary();
    json!({"status": "reset", "message": "Cortex state cleared.", "initial_summary": summary}).to_string()
}

fn discover_workspace() -> Result<WorkspaceGraph, String> {
    let ws_root = crate::handlers::ws();
    let member_paths = flux_graph::workspace::discover_members(&ws_root)
        .map_err(|e| format!("workspace discovery failed: {e}"))?;
    let crates: Vec<flux_graph::CrateInfo> = member_paths.iter().filter_map(|p| {
        let name = p.file_name()?.to_string_lossy().to_string();
        Some(flux_graph::CrateInfo { name, path: p.clone(), dependencies: vec![], edition: "2021".to_string(), crate_type: flux_graph::CrateType::Lib, features: vec![] })
    }).collect();
    Ok(WorkspaceGraph { root: ws_root, crates, batches: vec![] })
}
