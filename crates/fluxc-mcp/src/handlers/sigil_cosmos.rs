//! Quantum Cosmos × SigilGraph κ-phase MCP combos.

use super::{ToolDef, ToolRegistry};
use serde_json::{json, Value};

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_sigil_cosmos_measure",
            description: "Measure Kristensen κ-phase state for a SigilGraph agent node (classical/transitional/quantum_coherent, merit aura, citizenship tier).",
            input_schema: json!({
                "type":"object",
                "properties":{
                    "agent_id":{"type":"string"},
                    "mass_kg":{"type":"number"},
                    "radius_m":{"type":"number"},
                    "temperature_k":{"type":"number"},
                    "observer_entropy_bits":{"type":"number"},
                    "euler_chi":{"type":"number"}
                },
                "required":["agent_id"]
            }),
        },
        flux_sigil_cosmos_measure,
    );
    registry.register(
        ToolDef {
            name: "flux_sigil_cosmos_citizenship_ritual",
            description: "ProbationeKappa ritual: stabilize swarm toward κ=1 boundary; returns Sigil digest + 1000 QUG IOU proposal (proposal-first, no auto-spend).",
            input_schema: json!({
                "type":"object",
                "properties":{
                    "agent_id":{"type":"string"},
                    "peers_json":{"type":"string","description":"optional JSON array of CosmicNode peers"}
                },
                "required":["agent_id"]
            }),
        },
        flux_sigil_cosmos_citizenship_ritual,
    );
    registry.register(
        ToolDef {
            name: "flux_sigil_cosmos_beta_bridge",
            description: "Hint path to live Quantum Cosmos on beta (185) via epsilon SSH.",
            input_schema: json!({"type":"object","properties":{}}),
        },
        |_| format!("=== flux_sigil_cosmos_beta_bridge ===\n{}", sigil_cosmos_mcp::flux_sigil_cosmos_beta_bridge_hint()),
    );
}

fn flux_sigil_cosmos_measure(args: &Value) -> String {
    let agent = args.get("agent_id").and_then(|v| v.as_str()).unwrap_or("");
    if agent.is_empty() { return "agent_id required".into(); }
    let f = |k: &str| args.get(k).and_then(|v| v.as_f64());
    format!(
        "=== flux_sigil_cosmos_measure ===\n{}",
        sigil_cosmos_mcp::flux_sigil_cosmos_measure(
            agent, f("mass_kg"), f("radius_m"), f("temperature_k"),
            f("observer_entropy_bits"), f("euler_chi"),
        )
    )
}

fn flux_sigil_cosmos_citizenship_ritual(args: &Value) -> String {
    let agent = args.get("agent_id").and_then(|v| v.as_str()).unwrap_or("");
    if agent.is_empty() { return "agent_id required".into(); }
    let peers = args.get("peers_json").and_then(|v| v.as_str());
    format!(
        "=== flux_sigil_cosmos_citizenship_ritual ===\n{}",
        sigil_cosmos_mcp::flux_sigil_cosmos_citizenship_ritual(agent, peers)
    )
}
