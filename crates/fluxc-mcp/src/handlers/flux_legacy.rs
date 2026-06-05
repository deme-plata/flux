//! P11 — the `flux_legacy_*` MCP surface. Wraps the flux-legacy pipeline so the swarm + cockpit
//! drive it over MCP, not just the CLI. This first cut is the READ-ONLY / propose-only trio
//! (analyze · plan · stability) — safe, no writes. The write ops (split --stage, sync --confirm)
//! are a gated follow-up: they must require an explicit `confirm` and branch `refactor/*`, never
//! the baseline (the flux-legacy operating discipline).

use serde_json::{json, Value};

use crate::handlers::{ToolDef, ToolRegistry};

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_legacy_analyze",
            description: "Analyze a BROWNFIELD cargo workspace (e.g. the Quillon Graph node): per-crate real metrics (LOC, files, pub items, god-files >800 LOC, fan-in) + coupling hubs. Read-only. `json=true` for structured output.",
            input_schema: json!({"type":"object","properties":{
                "root":{"type":"string","description":"Workspace root (default: /home/orobit/qnk)"},
                "json":{"type":"boolean","description":"Structured JSON instead of text"}
            }}),
        },
        flux_legacy_analyze,
    );
    registry.register(
        ToolDef {
            name: "flux_legacy_plan",
            description: "Ranked refactor plan for a brownfield workspace (impact ÷ effort, worst-first). Read-only / propose-only — names what to refactor, changes nothing. `json=true` for structured output.",
            input_schema: json!({"type":"object","properties":{
                "root":{"type":"string","description":"Workspace root (default: /home/orobit/qnk)"},
                "json":{"type":"boolean","description":"Structured JSON instead of text"}
            }}),
        },
        flux_legacy_plan,
    );
    registry.register(
        ToolDef {
            name: "flux_legacy_stability",
            description: "P10 live-node STABILITY audit of a RUNNING node: process/db/serving/root-disk/RAM/sync-gap/peers → OK/WATCH/DANGER verdict. Read-only (probes journald + the status endpoint). Defaults target the local Quillon q-api-server.",
            input_schema: json!({"type":"object","properties":{
                "service":{"type":"string","description":"systemd service (default: q-api-server)"},
                "process_match":{"type":"string","description":"pgrep -f pattern (default: q-api-server)"},
                "db_marker":{"type":"string","description":"authoritative DB path marker (default: data-mainnet-genesis)"},
                "endpoint":{"type":"string","description":"status endpoint (default: http://127.0.0.1:8080/api/v1/status)"}
            }}),
        },
        flux_legacy_stability,
    );
}

fn flux_legacy_analyze(args: &Value) -> String {
    let root = args.get("root").and_then(|v| v.as_str()).unwrap_or("/home/orobit/qnk");
    let report = flux_legacy::analyze_workspace_legacy(root);
    if args.get("json").and_then(|v| v.as_bool()).unwrap_or(false) {
        flux_legacy::render::render_json(&report)
    } else {
        flux_legacy::render::render_text(&report)
    }
}

fn flux_legacy_plan(args: &Value) -> String {
    let root = args.get("root").and_then(|v| v.as_str()).unwrap_or("/home/orobit/qnk");
    let report = flux_legacy::analyze_workspace_legacy(root);
    let tasks = flux_legacy::plan::refactor_plan(&report);
    if args.get("json").and_then(|v| v.as_bool()).unwrap_or(false) {
        flux_legacy::render::render_plan_json(&tasks)
    } else {
        flux_legacy::render::render_plan(&tasks)
    }
}

fn flux_legacy_stability(args: &Value) -> String {
    let service = args.get("service").and_then(|v| v.as_str()).unwrap_or("q-api-server");
    let process_match = args.get("process_match").and_then(|v| v.as_str()).unwrap_or("q-api-server");
    let db_marker = args.get("db_marker").and_then(|v| v.as_str()).unwrap_or("data-mainnet-genesis");
    let endpoint = args.get("endpoint").and_then(|v| v.as_str()).unwrap_or("http://127.0.0.1:8080/api/v1/status");
    let signals = flux_legacy::stability::probe(service, process_match, db_marker, endpoint);
    flux_legacy::stability::render(&flux_legacy::stability::assess(&signals))
}
