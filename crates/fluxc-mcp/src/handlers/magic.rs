//! `flux_magic` — the guarded build as one MCP call.
//!
//! guard (MemAvailable ≥ 12 GB / 20 GB release, root disk, IO pressure, swap, load) →
//! receipt of the production units (MainPID + ActiveEnterTimestamp) → the one wrapper-cached
//! `cargo test -p` + predictor, spawned as the kernel's FIRST OOM victim → receipt again
//! (RESTARTED is the first line if a unit moved) → stardust on the built binary →
//! `magic_complete` webhook. See `fluxc_core::magic` for the incidents behind every rule.

use crate::handlers::{ToolDef, ToolRegistry};
use fluxc_core::magic;
use serde_json::{json, Value};

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_magic",
            description: "✨ MAGIC — the guarded build, one word. Pre-flight the box (refuses when MemAvailable < 12 GB, or < 20 GB for release; refuses when root disk < 2 GB; warns on IO pressure > 20 %, swap nearly full, load > 2× cpus — every threshold is a measured incident that restarted a production node), snapshot q-api-server + sigil-node (MainPID + ActiveEnterTimestamp), run the wrapper-cached `cargo test -p <package>` ∥ predictor with the cargo child marked as the kernel's FIRST OOM victim (oom_score_adj 1000), snapshot again and put RESTARTED in the first line if either unit moved, read the stardust off the built binary (network id, flux-rev), fire `magic_complete`. Env: FLUX_GUARD=warn|0, FLUX_GUARD_MEM_GB, FLUX_GUARD_UNITS, FLUX_GUARD_OOM=0. Use instead of flux_combo whenever the box also runs production.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "package": {"type": "string", "description": "Crate to build+test (default fluxc-util)"},
                    "release": {"type": "boolean", "description": "Release build (floor 20 GB instead of 12)"},
                    "dust": {"type": "boolean", "description": "Read stardust off target/<profile>/<package> if this run built it (default true)"},
                    "json": {"type": "boolean", "description": "Raw JSON only"}
                }
            }),
        },
        flux_magic,
    );
}

fn flux_magic(args: &Value) -> String {
    let package = args.get("package").and_then(|v| v.as_str()).unwrap_or("fluxc-util").to_string();
    let release = args.get("release").and_then(|v| v.as_bool()).unwrap_or(false);
    let dust = args.get("dust").and_then(|v| v.as_bool()).unwrap_or(true);
    // cast() fires `magic_complete` itself through the wired hooks sink — do not fire twice here.
    let r = magic::cast(&package, release, dust);
    let data = magic::to_json(&r);
    if args.get("json").and_then(|v| v.as_bool()).unwrap_or(false) {
        return serde_json::to_string_pretty(&data).unwrap_or_default();
    }
    format!("{}```json\n{}\n```", magic::render(&r), serde_json::to_string_pretty(&data).unwrap_or_default())
}
