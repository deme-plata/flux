//! sigil-dao-bridge-mcp — MCP-facing helpers for SigilGraph DAO/VM/DEX integration.

use sigil_dao_bridge::{
    dao_composite_root, testnet_dao_bundle, DaoAction, SigilGraphBridge,
};
use sigil_council::Risk;

/// Return the testnet DAO/VM/DEX bundle as pretty JSON.
pub fn flux_sigil_dao_vm_dex_bundle(deployer: &str) -> String {
    let b = testnet_dao_bundle(deployer);
    serde_json::to_string_pretty(&b).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

/// Generate registry JSON for sigilgraph.fluxapp.xyz publish.
pub fn flux_sigil_dao_vm_dex_registry_json(deployer: &str) -> String {
    let b = testnet_dao_bundle(deployer);
    serde_json::to_string_pretty(&serde_json::json!({
        "deployed_at": chrono_now(),
        "network_id": b.network_id,
        "testnet_url": b.testnet_url,
        "registry_path": b.registry_path,
        "version": b.version,
        "integration": b.integration,
        "honest": b.honest,
        "sample_proposals": b.sample_proposals,
        "status": "dao-vm-dex-bridge-v0.1",
        "related": {
            "agora_stargate": "flux-agora-stargate-mcp::flux_agora_stargate_registry_json",
            "cosmos_citizenship": "sigil-cosmos-mcp::flux_sigil_cosmos_citizenship_ritual",
            "stargate_examples": "sigil-state/examples/stargate_dag.rs"
        }
    }))
    .unwrap_or_default()
}

/// Run an in-memory governance + VM scaffold demo; returns JSON outcome.
pub fn flux_sigil_dao_vm_dex_demo(franchise: Option<u64>) -> String {
    let mut bridge = SigilGraphBridge::new(franchise.unwrap_or(100));
    bridge.accrue_treasury(5000);

    bridge.propose(1, "VM governance hook (scaffold)", Risk::LowRisk);
    let _ = bridge.sign(1);
    let _ = bridge.vote(1, 60, true);
    let _ = bridge.finalize(1);

    let vm_action = DaoAction::VmContractCall {
        proposal_id: 1,
        caller: [0x01; 32],
        contract: [0x02; 32],
        input_hex: "deadbeef".into(),
        gas_limit: 1_000_000,
    };

    let outcome = match bridge.execute_passed_action(vm_action) {
        Ok(o) => serde_json::json!({"ok": true, "outcome": o}),
        Err(e) => serde_json::json!({"ok": false, "error": e.to_string()}),
    };
    let roots = bridge.dao_roots();
    let composite = dao_composite_root(bridge.council(), bridge.treasury());

    serde_json::to_string_pretty(&serde_json::json!({
        "demo": "dao-vm-dex-scaffold",
        "result": outcome,
        "dao_roots": roots,
        "dao_composite_root_hex": hex::encode(composite),
        "height": bridge.height(),
    }))
    .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

/// Bridge hint for MCP operators connecting flux-agora-stargate + sigil-state stargate examples.
pub fn flux_sigil_dao_vm_dex_bridge_hint() -> String {
    serde_json::json!({
        "testnet_url": "https://sigilgraph.fluxapp.xyz",
        "network_id": "sigil-g0",
        "flux_crates": [
            "flux-agora-stargate",
            "flux-agora-stargate-mcp",
            "sigil-cosmos-core",
            "sigil-cosmos-mcp",
            "sigil-dao-bridge-mcp"
        ],
        "sigil_crates": [
            "sigil-council",
            "sigil-treasury",
            "sigil-vm",
            "sigil-dex",
            "sigil-state",
            "sigil-dao-bridge"
        ],
        "stargate_examples": [
            "sigil-state/examples/stargate_dag.rs",
            "sigil-state/examples/stargate_1m.rs",
            "sigil-state/examples/stargate_50m.rs",
            "sigil-state/examples/stargate_500m.rs"
        ],
        "chokepoint": "sigil-state::commit_state_transition",
        "honest_vm": "sigil-vm execute() returns NotImplemented until VM-1 (wasmi)"
    })
    .to_string()
}

fn chrono_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}