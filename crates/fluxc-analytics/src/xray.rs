// fluxc-core/xray.rs — Workspace x-ray.
//
// One rich snapshot of the workspace state for AI agents with large context windows.
// Combines flux-graph's crate resolution, the swarm ledger, and basic git state into
// a single structured JSON document. Designed for the 1M-context-aware AI coding
// loop (see FLUX_AI_CODING_V2_1M_CONTEXT.md): one call at session start absorbs
// everything; the rest of the session is fine-grained.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XrayReport {
    pub workspace_root: String,
    pub flux_version: String,
    pub crates: Vec<CrateSnapshot>,
    pub batches: Vec<Vec<String>>,
    pub swarm: SwarmSnapshot,
    pub generated_at_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateSnapshot {
    pub name: String,
    pub path: String,
    pub crate_type: String,
    pub edition: String,
    pub path_deps: Vec<String>,
    pub external_deps: Vec<String>,
    pub source_loc: Option<usize>,
    pub claimed_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SwarmSnapshot {
    pub agents: Vec<AgentRow>,
    pub active_claims: Vec<ClaimRow>,
    pub completed: Vec<CompletedRow>,
    pub total_qug_paid: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRow {
    pub id: String,
    pub wallet: String,
    pub status: String,
    pub total_earned_qug: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimRow {
    pub task_id: String,
    pub agent: String,
    pub crates: Vec<String>,
    pub priority: i64,
    pub estimated_qug: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletedRow {
    pub task_id: String,
    pub agent_id: String,
    pub crates: Vec<String>,
    pub qug_earned: f64,
}

/// Build the workspace x-ray from the current working directory.
pub fn xray() -> Result<XrayReport, String> {
    let root = std::env::current_dir().map_err(|e| e.to_string())?;
    xray_from(&root)
}

pub fn xray_from(root: &PathBuf) -> Result<XrayReport, String> {
    let ws = flux_graph::resolve_workspace(root)
        .map_err(|e| format!("flux-graph: {}", e))?;
    let swarm = read_swarm_state();
    let claimed_lookup: std::collections::HashMap<String, String> = swarm.active_claims.iter()
        .flat_map(|c| c.crates.iter().map(move |k| (k.clone(), c.agent.clone())))
        .collect();
    let crates: Vec<CrateSnapshot> = ws.crates.iter().map(|c| {
        let path_deps: Vec<String> = c.dependencies.iter()
            .filter(|d| matches!(d.kind, flux_graph::DepKind::Path))
            .map(|d| d.name.clone()).collect();
        let external_deps: Vec<String> = c.dependencies.iter()
            .filter(|d| !matches!(d.kind, flux_graph::DepKind::Path))
            .map(|d| d.name.clone()).collect();
        let source_loc = count_loc(&c.path);
        CrateSnapshot {
            name: c.name.clone(),
            path: c.path.display().to_string(),
            crate_type: format!("{:?}", c.crate_type),
            edition: c.edition.clone(),
            path_deps,
            external_deps,
            source_loc,
            claimed_by: claimed_lookup.get(&c.name).cloned(),
        }
    }).collect();
    let batches: Vec<Vec<String>> = ws.batches.iter()
        .map(|b| b.iter().map(|&i| ws.crates[i].name.clone()).collect())
        .collect();
    let now_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64).unwrap_or(0);
    Ok(XrayReport {
        workspace_root: ws.root.display().to_string(),
        flux_version: env!("CARGO_PKG_VERSION").to_string(),
        crates,
        batches,
        swarm,
        generated_at_us: now_us,
    })
}

fn count_loc(crate_path: &std::path::Path) -> Option<usize> {
    let src = crate_path.join("src");
    if !src.exists() { return None; }
    let mut total = 0usize;
    walk_rs(&src, &mut total);
    Some(total)
}

fn walk_rs(dir: &std::path::Path, total: &mut usize) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk_rs(&p, total);
            } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
                if let Ok(s) = std::fs::read_to_string(&p) {
                    *total += s.lines().count();
                }
            }
        }
    }
}

fn read_swarm_state() -> SwarmSnapshot {
    let raw = std::fs::read_to_string("/tmp/flux-swarm.json").unwrap_or_default();
    if raw.is_empty() { return SwarmSnapshot::default(); }
    let v: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v, Err(_) => return SwarmSnapshot::default(),
    };
    let agents = v.get("agents").and_then(|a| a.as_object())
        .map(|m| m.iter().map(|(id, ag)| AgentRow {
            id: id.clone(),
            wallet: ag.get("wallet_address").and_then(|w| w.as_str()).unwrap_or("").to_string(),
            status: ag.get("status").and_then(|s| s.as_str()).unwrap_or("").to_string(),
            total_earned_qug: ag.get("total_earned_qug").and_then(|q| q.as_f64()).unwrap_or(0.0),
        }).collect())
        .unwrap_or_default();
    let active_claims = v.get("claims").and_then(|c| c.as_array())
        .map(|a| a.iter().filter_map(|c| Some(ClaimRow {
            task_id: c.get("task_id")?.as_str()?.to_string(),
            agent: c.get("agent")?.as_str()?.to_string(),
            crates: c.get("crates")?.as_array()?.iter()
                .filter_map(|s| s.as_str().map(String::from)).collect(),
            priority: c.get("priority").and_then(|p| p.as_i64()).unwrap_or(0),
            estimated_qug: c.get("estimated_qug").and_then(|q| q.as_f64()).unwrap_or(0.0),
        })).collect())
        .unwrap_or_default();
    let completed = v.get("completed").and_then(|c| c.as_array())
        .map(|a| a.iter().filter_map(|c| Some(CompletedRow {
            task_id: c.get("task_id")?.as_str()?.to_string(),
            agent_id: c.get("agent_id")?.as_str()?.to_string(),
            crates: c.get("crates")?.as_array()?.iter()
                .filter_map(|s| s.as_str().map(String::from)).collect(),
            qug_earned: c.get("qug_earned").and_then(|q| q.as_f64()).unwrap_or(0.0),
        })).collect())
        .unwrap_or_default();
    let total_qug_paid = v.get("qug_paid").and_then(|q| q.as_f64()).unwrap_or(0.0);
    SwarmSnapshot { agents, active_claims, completed, total_qug_paid }
}

/// Render the xray as plain text for human consumption.
pub fn render_text(r: &XrayReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("⚡ Flux workspace x-ray (v{}):\n", r.flux_version));
    out.push_str(&format!("  Root: {}\n", r.workspace_root));
    out.push_str(&format!("  Crates: {} | Batches: {}\n", r.crates.len(), r.batches.len()));
    out.push_str(&format!("\nCrates:\n"));
    for c in &r.crates {
        let loc = c.source_loc.map(|n| format!("{} LOC", n)).unwrap_or_else(|| "?".to_string());
        let claim = c.claimed_by.as_deref().map(|a| format!(" [claimed:{}]", a)).unwrap_or_default();
        out.push_str(&format!("  • {} ({}, {}, {}{})\n", c.name, c.crate_type, c.edition, loc, claim));
        if !c.path_deps.is_empty() {
            out.push_str(&format!("      path-deps: {}\n", c.path_deps.join(", ")));
        }
    }
    out.push_str(&format!("\nBuild batches: {} parallel groups\n", r.batches.len()));
    for (i, b) in r.batches.iter().enumerate() {
        out.push_str(&format!("  batch {}: {} ({})\n", i + 1, b.len(), b.join(", ")));
    }
    out.push_str(&format!("\nSwarm: {} agents, {} active claims, {} completed, {:.2} QUG paid\n",
        r.swarm.agents.len(), r.swarm.active_claims.len(),
        r.swarm.completed.len(), r.swarm.total_qug_paid));
    for a in &r.swarm.agents {
        out.push_str(&format!("  • {} ({}): {:.2} QUG earned\n", a.id, a.status, a.total_earned_qug));
    }
    for c in &r.swarm.active_claims {
        out.push_str(&format!("  claim {}: {} → {} ({} QUG est.)\n",
            c.task_id, c.agent, c.crates.join(","), c.estimated_qug));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_empty_report_doesnt_panic() {
        let r = XrayReport {
            workspace_root: "/tmp/foo".into(),
            flux_version: "0.10.1".into(),
            crates: vec![],
            batches: vec![],
            swarm: SwarmSnapshot::default(),
            generated_at_us: 0,
        };
        let s = render_text(&r);
        assert!(s.contains("Flux workspace x-ray"));
        assert!(s.contains("Crates: 0"));
    }

    #[test]
    fn json_roundtrip() {
        let r = XrayReport {
            workspace_root: "/tmp/foo".into(),
            flux_version: "0.10.1".into(),
            crates: vec![],
            batches: vec![vec!["a".into(), "b".into()]],
            swarm: SwarmSnapshot::default(),
            generated_at_us: 1234,
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: XrayReport = serde_json::from_str(&s).unwrap();
        assert_eq!(back.workspace_root, "/tmp/foo");
        assert_eq!(back.batches.len(), 1);
    }
}
