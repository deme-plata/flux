//! Agora × Stargate testnet deploy combos.

use super::{ToolDef, ToolRegistry};
use serde_json::{json, Value};
use std::process::Command;

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_agora_stargate_combo",
            description: "flux_combo on flux-agora-stargate — compile + test the Agora×Stargate v0.2 iteration.",
            input_schema: json!({"type":"object","properties":{"release":{"type":"boolean"}}}),
        },
        flux_agora_stargate_combo,
    );
    registry.register(
        ToolDef {
            name: "flux_agora_stargate_deploy_testnet",
            description: "Build testnet deploy bundle (AGORA token + AgoraStargateRegistry ContractDeploy) and write registry JSON to sigilgraph.fluxapp.xyz dist.",
            input_schema: json!({
                "type":"object",
                "properties":{
                    "deployer_hex":{"type":"string","description":"32-byte wallet hex (64 chars)"},
                    "write_registry":{"type":"boolean","description":"Write to dist-final (default true)"}
                }
            }),
        },
        flux_agora_stargate_deploy_testnet,
    );
}

fn flux_agora_stargate_combo(args: &Value) -> String {
    let release = args.get("release").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut cmd = Command::new("/usr/local/bin/flux-cargo-wrapper");
    cmd.args(["test", "-p", "flux-agora-stargate"]);
    if release { cmd.arg("--release"); }
    cmd.current_dir("/home/storage/deepseek-codewhale/flux");
    match cmd.output() {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            format!(
                "=== flux_agora_stargate_combo ===\nexit={}\n{}\n{}",
                o.status.code().unwrap_or(-1),
                stdout,
                stderr
            )
        }
        Err(e) => format!("flux_agora_stargate_combo failed: {e}"),
    }
}

fn flux_agora_stargate_deploy_testnet(args: &Value) -> String {
    let deployer = args.get("deployer_hex").and_then(|v| v.as_str())
        .unwrap_or("4242424242424242424242424242424242424242424242424242424242424242");
    let write = args.get("write_registry").and_then(|v| v.as_bool()).unwrap_or(true);
    let json_body = flux_agora_stargate_mcp::flux_agora_stargate_registry_json(deployer);
    if write {
        let path = "/home/orobit/q-narwhalknight/dist-fluxapp/agora-stargate-registry.json";
        if let Err(e) = std::fs::write(path, &json_body) {
            return format!("write registry failed: {e}");
        }
    }
    format!(
        "=== flux_agora_stargate_deploy_testnet ===\nregistry: https://sigilgraph.fluxapp.xyz/agora-stargate-registry.json\nui: https://sigilgraph.fluxapp.xyz/agora-stargate.html\n\n{}",
        json_body
    )
}
