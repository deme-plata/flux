//! MCP-facing helpers for flux-agora-stargate.

use flux_agora_stargate::{testnet_deploy_bundle, TestnetDeployBundle};

pub fn flux_agora_stargate_bundle(deployer: &str) -> String {
    let b = testnet_deploy_bundle(deployer);
    serde_json::to_string_pretty(&b).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

pub fn flux_agora_stargate_registry_json(deployer: &str) -> String {
    let b = testnet_deploy_bundle(deployer);
    serde_json::to_string_pretty(&serde_json::json!({
        "deployed_at": chrono_now(),
        "network_id": b.network_id,
        "testnet_url": b.testnet_url,
        "contract": b.record,
        "txs_preview": b.txs,
        "status": "testnet-registry-v0.2",
        "honest": {
            "measured": "provenance hash bundle + Stargate ingest profile",
            "pretend": "VM execute() not wired — ContractDeploy is event-only until sigil-vm VM-1",
            "rollback": "remove agora-stargate-registry.json from dist-fluxapp"
        }
    })).unwrap_or_default()
}

fn chrono_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
