//! `flux_combo_supersonic` — the EV-fast onboarding combo for AI agents.
//!
//! The FIRST tool a freshly-onboarded agent (Grok et al. picking up the flux-dev
//! skill) should call. One zero-config MCP call that:
//!   * runs a SINGLE wrapper-cached `cargo test -p X` (no redundant parallel
//!     `cargo check` → no cargo build-dir lock contention) ∥ the in-process
//!     build predictor (pure CPU, no lock),
//!   * links via mold (.cargo/config.toml) so the test-binary LINK — the
//!     wall-clock bottleneck on big crates once compile is cached — is ~1s,
//!   * targets <10s warm ("0-100 km/h in 2s"),
//!   * emits a HIGH-SIGNAL, machine-actionable `combo_supersonic` webhook so
//!     other agents / dashboards know exactly what to do next.
//!
//! Co-designed with DeepSeek-reasoner (raw facts fed in; its draft was then
//! API-corrected here: `auto_dispatch` is SYNC fire-and-forget — not async; MCP
//! handlers are sync `fn(&Value)->String`; the wrapper is `current_exe()`, not a
//! hardcoded path; no `chrono`/`regex`/`tokio` deps). Verified-by-DeepSeek design,
//! Rust-real implementation.

use crate::handlers::{ToolDef, ToolRegistry};
use fluxc_core::{combo_v2, webhook};
use serde_json::{json, Value};

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_combo_supersonic",
            description: "🚀 SUPERSONIC combo — the first tool a new AI agent should run. ONE zero-config, wrapper-cached, mold-linked `cargo test` (<10s warm) in parallel with the build predictor. Emits a high-signal, machine-actionable `combo_supersonic` webhook (verdict + next_action + first_error{file,line,col,code,snippet}) for AI/MCP consumers. Use it to onboard, baseline a crate, or get an instant fix-or-ship verdict. Returns a one-line verdict followed by the structured JSON the webhook carries.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "package": {"type": "string", "description": "Crate to combo on (default: fluxc)"},
                    "release": {"type": "boolean", "description": "Optimized build (default: false)"}
                }
            }),
        },
        flux_combo_supersonic,
    );
}

fn flux_combo_supersonic(args: &Value) -> String {
    let package = args
        .get("package")
        .and_then(|v| v.as_str())
        .unwrap_or("fluxc")
        .to_string();
    let release = args.get("release").and_then(|v| v.as_bool()).unwrap_or(false);

    // Delegate to the shared engine in fluxc-core (factored; same code path as `fluxc combo`).
    // This does the ONE wrapper-cached cargo test || predict, mold, correct envs, full ComboResult.
    let res = combo_v2::run_combo(&package, release);
    let data = res.to_json();

    // Fire-and-forget high-signal webhook (MCP surface still emits it for other agents/dashboards).
    let listeners = webhook::count_listeners("combo_supersonic");
    webhook::auto_dispatch("combo_supersonic", data.clone());

    // AI-friendly one-line + full JSON (same contract as CLI).
    let speed = if res.warm { "🚀 supersonic" } else { "🐌 cold (cache warming)" };
    let p = res.tests_passed;
    let f = res.tests_failed;
    let pred = &res.prediction;
    let pms = pred.get("predicted_ms").and_then(|v| v.as_u64()).unwrap_or(0);
    let crate_pct = (pred.get("predicted_cache_rate").and_then(|v| v.as_f64()).unwrap_or(0.0) * 100.0) as u64;
    format!(
        "{speed}  {ver}  {pkg}  {tot}ms (cmd {cmd}ms + predict {pr}ms)  → next_action={na}\n\
         tests {p}✓/{f}✗ · predicted {pms}ms · cache {crate_pct}% · combo_supersonic webhooks: {listeners}\n\
         ```json\n{js}\n```",
        ver = res.verdict,
        pkg = res.package,
        tot = res.total_ms,
        cmd = res.cmd_ms,
        pr = res.predict_ms,
        na = res.next_action,
        js = serde_json::to_string_pretty(&data).unwrap_or_default(),
    )
}
