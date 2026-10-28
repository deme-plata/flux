//! `flux_stardust` — read the trail a build left in a binary, without running it.
//!
//! See `fluxc_core::stardust` for the why (the 10.7.4 g2-build-on-g3 incident, the
//! `systemctl cat` lie, "built but not restarted"). This is the MCP surface: same engine
//! as `fluxc stardust`, plus a `stardust` webhook so dashboards and other agents see it.

use crate::handlers::{ToolDef, ToolRegistry};
use fluxc_core::stardust;
use fluxc_webhooks::webhook;
use serde_json::{json, Value};

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_stardust",
            description: "✦ STARDUST — what IS this binary, really? Reads the dust a build leaves inside an artifact WITHOUT executing it: the network it was BUILT FOR — read from the compile-time gossip wire topics (/sigil/g3/blocks, /txs, …), never from how often an id is mentioned (a g3 sigil-top mentions sigil-g2 66× and sigil-g3 4×; measured 2026-10-28, counts are worthless) — plus the flux-rev source stamp, rustc version, product/version strings, blake3 + sha256. target = a file path, a systemd unit (adds the EFFECTIVE ExecStart vs /proc/<MainPID>/exe check — the `systemctl cat` lie — and whether the file on disk is newer than the running process = built, not restarted), a pid, or a release-manifest URL (downloads the named target, verifies blake3+size, reads the network — the 10.7.4 g2-build-shipped-to-g3 check in one call). Never runs the target unless exec=true (then `<bin> network-id`, 5 s timeout). Fires a `stardust` webhook.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {"type": "string", "description": "File path | systemd unit (e.g. sigil-node, q-api-server) | pid | https://…/sigil-top-latest.json"},
                    "exec": {"type": "boolean", "description": "Also run `<bin> network-id` for the authoritative network (default false — a binary is never executed to find out what it is)"},
                    "triple": {"type": "string", "description": "Manifest target to pick for a URL (default linux-x64)"},
                    "json": {"type": "boolean", "description": "Raw JSON only"}
                },
                "required": ["target"]
            }),
        },
        flux_stardust,
    );
}

fn flux_stardust(args: &Value) -> String {
    let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if target.is_empty() {
        return "flux_stardust: `target` is required (path | unit | pid | manifest url)".into();
    }
    let opts = stardust::Opts {
        exec: args.get("exec").and_then(|v| v.as_bool()).unwrap_or(false),
        triple: args.get("triple").and_then(|v| v.as_str()).unwrap_or("linux-x64").to_string(),
    };
    let d = stardust::read(&target, &opts);
    let data = stardust::to_json(&d);
    let listeners = webhook::count_listeners("stardust");
    webhook::auto_dispatch("stardust", data.clone());
    if args.get("json").and_then(|v| v.as_bool()).unwrap_or(false) {
        return serde_json::to_string_pretty(&data).unwrap_or_default();
    }
    format!("{}  stardust webhooks: {listeners}\n```json\n{}\n```", stardust::render(&d),
        serde_json::to_string_pretty(&data).unwrap_or_default())
}
