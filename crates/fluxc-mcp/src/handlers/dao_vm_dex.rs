//! SigilGraph DAO x VM x DEX integration MCP combos.

use super::{ToolDef, ToolRegistry};
use serde_json::{json, Value};
use std::process::Command;

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_sigil_dao_vm_dex_combo",
            description: "flux_combo on sigil-dao-bridge - compile + test DAO VM DEX integration crate.",
            input_schema: json!({"type":"object","properties":{"release":{"type":"boolean"}}}),
        },
        flux_sigil_dao_vm_dex_combo,
    );
    registry.register(
        ToolDef {
            name: "flux_sigil_dao_vm_dex_bundle",
            description: "Return testnet DAO/VM/DEX integration bundle JSON (honest live vs scaffold flags).",
            input_schema: json!({
                "type":"object",
                "properties":{
                    "deployer_hex":{"type":"string","description":"32-byte wallet hex (64 chars)"}
                }
            }),
        },
        flux_sigil_dao_vm_dex_bundle,
    );
    registry.register(
        ToolDef {
            name: "flux_sigil_dao_vm_dex_registry_json",
            description: "Generate dao-vm-dex-registry.json for sigilgraph.fluxapp.xyz publish.",
            input_schema: json!({
                "type":"object",
                "properties":{
                    "deployer_hex":{"type":"string","description":"32-byte wallet hex (64 chars)"}
                }
            }),
        },
        flux_sigil_dao_vm_dex_registry_json,
    );
    registry.register(
        ToolDef {
            name: "flux_sigil_dao_vm_dex_demo",
            description: "In-memory governance + VM scaffold demo; returns JSON outcome with dao_roots.",
            input_schema: json!({
                "type":"object",
                "properties":{
                    "franchise":{"type":"integer","description":"Total franchise weight (default 100)"}
                }
            }),
        },
        flux_sigil_dao_vm_dex_demo,
    );
    registry.register(
        ToolDef {
            name: "flux_sigil_dao_vm_dex_bridge_hint",
            description: "Operator hint: crate map + stargate examples + chokepoint for DAO/VM/DEX wiring.",
            input_schema: json!({"type":"object","properties":{}}),
        },
        |_| format!(
            "=== flux_sigil_dao_vm_dex_bridge_hint ===\n{}",
            sigil_dao_bridge_mcp::flux_sigil_dao_vm_dex_bridge_hint()
        ),
    );
    registry.register(
        ToolDef {
            name: "flux_sigil_dao_vm_dex_deploy_testnet",
            description: "Write dao-vm-dex-registry.json to sigilgraph.fluxapp.xyz dist-final.",
            input_schema: json!({
                "type":"object",
                "properties":{
                    "deployer_hex":{"type":"string","description":"32-byte wallet hex (64 chars)"},
                    "write_registry":{"type":"boolean","description":"Write to dist-final (default true)"}
                }
            }),
        },
        flux_sigil_dao_vm_dex_deploy_testnet,
    );
}

fn deployer_hex(args: &Value) -> &str {
    args.get("deployer_hex")
        .and_then(|v| v.as_str())
        .unwrap_or("4242424242424242424242424242424242424242424242424242424242424242")
}

fn flux_sigil_dao_vm_dex_combo(args: &Value) -> String {
    let release = args.get("release").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut cmd = Command::new("/usr/local/bin/flux-cargo-wrapper");
    cmd.args(["test", "-p", "sigil-dao-bridge", "-p", "sigil-dao-bridge-mcp"]);
    if release { cmd.arg("--release"); }
    cmd.current_dir("/home/storage/deepseek-codewhale/sigil");
    match cmd.output() {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            format!(
                "=== flux_sigil_dao_vm_dex_combo ===\nexit={}\n{}\n{}",
                o.status.code().unwrap_or(-1), stdout, stderr
            )
        }
        Err(e) => format!("flux_sigil_dao_vm_dex_combo failed: {e}"),
    }
}

fn flux_sigil_dao_vm_dex_bundle(args: &Value) -> String {
    format!(
        "=== flux_sigil_dao_vm_dex_bundle ===\n{}",
        sigil_dao_bridge_mcp::flux_sigil_dao_vm_dex_bundle(deployer_hex(args))
    )
}

fn flux_sigil_dao_vm_dex_registry_json(args: &Value) -> String {
    format!(
        "=== flux_sigil_dao_vm_dex_registry_json ===\n{}",
        sigil_dao_bridge_mcp::flux_sigil_dao_vm_dex_registry_json(deployer_hex(args))
    )
}

fn flux_sigil_dao_vm_dex_demo(args: &Value) -> String {
    let franchise = args.get("franchise").and_then(|v| v.as_u64());
    format!(
        "=== flux_sigil_dao_vm_dex_demo ===\n{}",
        sigil_dao_bridge_mcp::flux_sigil_dao_vm_dex_demo(franchise)
    )
}

fn flux_sigil_dao_vm_dex_deploy_testnet(args: &Value) -> String {
    let deployer = deployer_hex(args);
    let write = args.get("write_registry").and_then(|v| v.as_bool()).unwrap_or(true);
    let json_body = sigil_dao_bridge_mcp::flux_sigil_dao_vm_dex_registry_json(deployer);
    if write {
        let path = "/home/orobit/q-narwhalknight/dist-fluxapp/dao-vm-dex-registry.json";
        if let Err(e) = std::fs::write(path, &json_body) {
            return format!("write registry failed: {e}");
        }
    }
    format!(
        "=== flux_sigil_dao_vm_dex_deploy_testnet ===\nregistry: https://sigilgraph.fluxapp.xyz/dao-vm-dex-registry.json\ndocs: flux/docs/SIGILGRAPH_DAO_VM_DEX_INTEGRATION.md\n\n{}",
        json_body
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dao_vm_dex_demo_smoke() {
        let out = flux_sigil_dao_vm_dex_demo(&json!({}));
        assert!(out.contains("flux_sigil_dao_vm_dex_demo"));
        assert!(out.contains("dao-vm-dex-scaffold"));
    }

    #[test]
    fn dao_vm_dex_bridge_hint_smoke() {
        let out = format!(
            "=== flux_sigil_dao_vm_dex_bridge_hint ===\n{}",
            sigil_dao_bridge_mcp::flux_sigil_dao_vm_dex_bridge_hint()
        );
        assert!(out.contains("sigil-dao-bridge"));
        assert!(out.contains("commit_state_transition"));
    }
}