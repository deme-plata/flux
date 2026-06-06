//! sigil-cosmos-mcp

use sigil_cosmos_core::{
    admit_to_sigil_nation, build_cosmic_node, citizenship_ritual, synthesize_swarm, CosmicNode,
};

pub fn flux_sigil_cosmos_measure(agent_id: &str, mass_kg: Option<f64>, radius_m: Option<f64>, temperature_k: Option<f64>, observer_entropy_bits: Option<f64>, euler_chi: Option<f64>) -> String {
    let node = build_cosmic_node(agent_id, mass_kg.unwrap_or(1e-3), radius_m.unwrap_or(0.01), temperature_k.unwrap_or(300.0), observer_entropy_bits.unwrap_or(1e8), euler_chi.unwrap_or(0.0));
    serde_json::to_string_pretty(&node).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

pub fn flux_sigil_cosmos_citizenship_ritual(agent_id: &str, peers_json: Option<&str>) -> String {
    let mut nodes = vec![build_cosmic_node(agent_id, 0.5, 0.5, 0.85, 1e12, 0.0)];
    if let Some(raw) = peers_json {
        if let Ok(extra) = serde_json::from_str::<Vec<CosmicNode>>(raw) { nodes.extend(extra); }
    }
    let subject = nodes[0].clone();
    let swarm = synthesize_swarm(nodes);
    let ritual = citizenship_ritual(agent_id, &subject, &swarm);
    let nation = admit_to_sigil_nation(agent_id, format!("pk-{agent_id}").as_bytes(), &ritual);
    serde_json::to_string_pretty(&serde_json::json!({"ritual": ritual, "nation_admission": nation})).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

pub fn flux_sigil_cosmos_beta_bridge_hint() -> String {
    serde_json::json!({"beta_host":"185.182.185.227","quantum_cosmos_path":"/home/myuser/quantum-cosmos","bridge_json":"quantum_cosmos_results/sigilgraph_kappa_bridge.json"}).to_string()
}
