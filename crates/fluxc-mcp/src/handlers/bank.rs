//! Flux Bank + Bifrost MCP.

use super::{ToolDef, ToolFn, ToolRegistry};
use serde_json::{json, Value};

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_bank_status",
            description: "Read-only Flux Bank status: Quillon Graph bank metrics + Flux policy (proposal-first). endpoint: quillon|epsilon|delta.",
            input_schema: json!({"type":"object","properties":{"endpoint":{"type":"string"}}}),
        },
        flux_bank_status,
    );
    registry.register(
        ToolDef {
            name: "flux_bank_propose_transfer",
            description: "Dry-run transfer proposal. NEVER executes.",
            input_schema: json!({
                "type":"object",
                "properties":{
                    "from":{"type":"string"},"to":{"type":"string"},
                    "amount_uqug":{"type":"integer"},"token":{"type":"string"},"memo":{"type":"string"}
                },
                "required":["from","to","amount_uqug"]
            }),
        },
        flux_bank_propose_transfer,
    );
    registry.register(
        ToolDef {
            name: "flux_bifrost_run",
            description: "Bifrost loop: posts goal to flux_goal stack, returns orchestration plan + consensus. Does NOT auto-rent GPU or spend QUG.",
            input_schema: json!({
                "type":"object",
                "properties":{
                    "goal":{"type":"string"},
                    "agent_id":{"type":"string"},
                    "priority":{"type":"integer","description":"0=emergency..5=idle, default 3"},
                    "ttl_secs":{"type":"integer","description":"default 3600"},
                    "execute_goal_post":{"type":"boolean","description":"default true — actually flux_goal_post"}
                },
                "required":["goal","agent_id"]
            }),
        },
        flux_bifrost_run,
    );
}

fn flux_bank_status(args: &Value) -> String {
    let endpoint = args.get("endpoint").and_then(|v| v.as_str());
    format!("=== flux_bank_status ===\n{}", flux_bank_mcp::flux_bank_status(endpoint))
}

fn flux_bank_propose_transfer(args: &Value) -> String {
    let from = args.get("from").and_then(|v| v.as_str()).unwrap_or("");
    let to = args.get("to").and_then(|v| v.as_str()).unwrap_or("");
    if from.is_empty() || to.is_empty() { return "from and to required".into(); }
    let amount = args.get("amount_uqug").and_then(|v| v.as_u64()).unwrap_or(0) as u128;
    let token = args.get("token").and_then(|v| v.as_str());
    let memo = args.get("memo").and_then(|v| v.as_str());
    format!(
        "=== flux_bank_propose_transfer ===\n{}",
        flux_bank_mcp::flux_bank_propose_transfer(from, to, amount, token, memo)
    )
}

fn flux_bifrost_run(args: &Value) -> String {
    use flux_moe::goalroute::{route_from_consensus, route_from_goal_text};

    let goal = args.get("goal").and_then(|v| v.as_str()).unwrap_or("");
    let agent = args.get("agent_id").and_then(|v| v.as_str()).unwrap_or("agent");
    if goal.is_empty() {
        return "goal required".into();
    }
    let priority = args.get("priority").and_then(|v| v.as_u64()).unwrap_or(3) as u8;
    let ttl = args.get("ttl_secs").and_then(|v| v.as_u64()).unwrap_or(3600);
    let exec = args.get("execute_goal_post").and_then(|v| v.as_bool()).unwrap_or(true);
    let bank_ep = args
        .get("bank_endpoint")
        .and_then(|v| v.as_str())
        .unwrap_or("epsilon");

    let mut lines = vec!["=== flux_bifrost_run (alle lanes) ===".into()];
    if exec {
        match fluxc_core::goals::post_goal(agent, goal, priority, ttl) {
            Ok(g) => lines.push(format!(
                "goal_posted: id={} priority={} ttl={}",
                g.id, g.priority, g.ttl_secs
            )),
            Err(e) => lines.push(format!("goal_post_error: {e}")),
        }
    }
    if let Ok(Some(g)) = fluxc_core::goals::consensus_goal() {
        lines.push(format!("consensus: [{}] {} — {}", g.agent, g.id, g.text));
    }

    let route = route_from_consensus().unwrap_or_else(|| route_from_goal_text(goal));
    if let Ok(j) = serde_json::to_string_pretty(&route) {
        lines.push(format!("moe_route:\n{j}"));
    }

    lines.push(format!(
        "bank_status ({bank_ep}):\n{}",
        flux_bank_mcp::flux_bank_status(Some(bank_ep))
    ));
    lines.push(flux_bank_mcp::flux_bifrost_run(goal, agent));
    lines.join("\n")
}


#[cfg(test)]
mod bifrost_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bifrost_run_alle_lanes_smoke() {
        let out = flux_bifrost_run(&json!({
            "goal": "port Quillon bank + bifrost alle lanes",
            "agent_id": "grok-viktor",
            "execute_goal_post": false,
            "bank_endpoint": "epsilon"
        }));
        assert!(out.contains("flux_bifrost_run"));
        assert!(out.contains("moe_route"));
        assert!(out.contains("bank_status"));
        assert!(out.contains("flux_vast_recommend"));
    }
}
