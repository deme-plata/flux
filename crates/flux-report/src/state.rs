// Snapshot the live Flux state the report needs: workspace version, swarm
// history (agents / claims / completed / total QUG), and synthesized SAP
// chart values. All reads are local — no network calls — so report generation
// stays deterministic and offline-safe.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportState {
    pub generated_at_utc: String,
    pub workspace_version: String,
    pub swarm: SwarmSnapshot,
    pub sap: SapChart,
    pub flux_ai: AiSnapshot,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SwarmSnapshot {
    pub agents: Vec<SwarmAgentRow>,
    pub active_claims: Vec<SwarmClaimRow>,
    pub completed: Vec<SwarmCompletedRow>,
    pub total_qug_paid: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmAgentRow {
    pub id: String,
    pub wallet: String,
    pub status: String,
    pub current_crates: Vec<String>,
    pub total_earned_qug: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmClaimRow {
    pub task_id: String,
    pub agent: String,
    pub crates: Vec<String>,
    pub priority: u32,
    pub estimated_qug: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmCompletedRow {
    pub task_id: String,
    pub agent: String,
    pub crates: Vec<String>,
    pub success: bool,
    pub qug_earned: f64,
}

/// Synthetic SAP-style chart for the report. SAP today is mostly P2P scoring
/// (contribution/latency/stake) — for a project report the more useful axes
/// are derived: how fast Flux is building, how warm the cache is, and how
/// much swarm activity happened. Each is 0.0–1.0 so they plot together.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SapChart {
    pub compile_velocity: f64,
    pub cache_health: f64,
    pub swarm_utilization: f64,
    pub agent_diversity: f64,
    pub settlement_throughput: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiSnapshot {
    pub lifetime_hints: usize,
    pub send_sync_hints: usize,
    pub race_hints: usize,
    pub unsafe_hints: usize,
    pub ownership_hints: usize,
    pub deadlock_hints: usize,
}

#[derive(Serialize, Deserialize)]
struct SwarmFileShape {
    agents: std::collections::HashMap<String, SwarmAgentFile>,
    claims: Vec<SwarmClaimFile>,
    completed: Vec<SwarmCompletedFile>,
    qug_paid: f64,
}

#[derive(Serialize, Deserialize)]
struct SwarmAgentFile {
    id: String,
    wallet_address: String,
    status: serde_json::Value,
    current_crates: Vec<String>,
    total_earned_qug: f64,
}

#[derive(Serialize, Deserialize)]
struct SwarmClaimFile {
    task_id: String,
    agent: String,
    crates: Vec<String>,
    priority: u32,
    estimated_qug: f64,
}

#[derive(Serialize, Deserialize)]
struct SwarmCompletedFile {
    task_id: String,
    agent_id: String,
    crates: Vec<String>,
    success: bool,
    qug_earned: f64,
}

/// Read `/tmp/flux-swarm.json` (or `swarm_path` override) into a normalized
/// snapshot. Returns an empty snapshot if the file is missing — report can
/// still render with a "swarm idle" section.
pub fn load_swarm(swarm_path: &Path) -> SwarmSnapshot {
    let raw = match std::fs::read_to_string(swarm_path) {
        Ok(s) => s,
        Err(_) => return SwarmSnapshot::default(),
    };
    let parsed: SwarmFileShape = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(_) => return SwarmSnapshot::default(),
    };
    SwarmSnapshot {
        agents: parsed
            .agents
            .into_values()
            .map(|a| SwarmAgentRow {
                id: a.id,
                wallet: a.wallet_address,
                status: status_label(&a.status),
                current_crates: a.current_crates,
                total_earned_qug: a.total_earned_qug,
            })
            .collect(),
        active_claims: parsed
            .claims
            .into_iter()
            .map(|c| SwarmClaimRow {
                task_id: c.task_id,
                agent: c.agent,
                crates: c.crates,
                priority: c.priority,
                estimated_qug: c.estimated_qug,
            })
            .collect(),
        completed: parsed
            .completed
            .into_iter()
            .map(|c| SwarmCompletedRow {
                task_id: c.task_id,
                agent: c.agent_id,
                crates: c.crates,
                success: c.success,
                qug_earned: c.qug_earned,
            })
            .collect(),
        total_qug_paid: parsed.qug_paid,
    }
}

fn status_label(v: &serde_json::Value) -> String {
    // Status is serialized as either a bare enum name ("Idle") or a tagged
    // object — collapse both into a short label.
    v.as_str()
        .map(str::to_string)
        .or_else(|| v.as_object().and_then(|o| o.keys().next().cloned()))
        .unwrap_or_else(|| "Unknown".into())
}

/// Compute the synthesized SAP chart from the swarm snapshot. The five axes
/// are normalized to 0.0–1.0 so they can plot on the same radial.
pub fn compute_sap(swarm: &SwarmSnapshot) -> SapChart {
    // Compile velocity: today this is a placeholder until we drain
    // /tmp/flux-events for real per-build durations. 0.7 ≈ "good".
    let compile_velocity = 0.70;

    // Cache health is read from the fluxc-core atomics. They reset on
    // process restart so on a fresh MCP the value is 0 — that's correct,
    // we shouldn't pretend.
    let hits = fluxc_core::CACHE_HITS.load(std::sync::atomic::Ordering::Relaxed) as f64;
    let misses = fluxc_core::CACHE_MISSES.load(std::sync::atomic::Ordering::Relaxed) as f64;
    let cache_health = if hits + misses == 0.0 {
        0.0
    } else {
        hits / (hits + misses)
    };

    // Swarm utilization = fraction of registered agents that are currently
    // working. A clean swarm with everyone Idle reads 0.0; one with every
    // agent on a claim reads 1.0.
    let total = swarm.agents.len().max(1) as f64;
    let working = swarm
        .agents
        .iter()
        .filter(|a| a.status == "Working")
        .count() as f64;
    let swarm_utilization = working / total;

    // Diversity: distinct agents that earned QUG this period. Caps at 3.
    let earners = swarm
        .agents
        .iter()
        .filter(|a| a.total_earned_qug > 0.0)
        .count() as f64;
    let agent_diversity = (earners / 3.0).min(1.0);

    // Settlement throughput: completed-vs-active ratio, capped at 1.0 with
    // a soft scale so an idle period with 0 active reads as "fully settled".
    let active = swarm.active_claims.len() as f64;
    let completed = swarm.completed.len() as f64;
    let settlement_throughput = if active + completed == 0.0 {
        0.0
    } else {
        completed / (active + completed)
    };

    SapChart {
        compile_velocity,
        cache_health,
        swarm_utilization,
        agent_diversity,
        settlement_throughput,
    }
}

pub fn snapshot_flux_ai(workspace_root: &Path) -> AiSnapshot {
    // resolve_workspace builds the same WorkspaceGraph fluxc uses; if it
    // fails (e.g. report is run outside the workspace), the audit returns
    // zeros — the report just shows an empty audit section.
    let root_pb = workspace_root.to_path_buf();
    let ws = match flux_graph::resolve_workspace(&root_pb) {
        Ok(ws) => ws,
        Err(_) => return AiSnapshot::default(),
    };
    let r = flux_ai::full_ai_audit(&ws);
    AiSnapshot {
        lifetime_hints: r.lifetime_suggestions.len(),
        send_sync_hints: r.send_sync_suggestions.len(),
        race_hints: r.race_detection_findings.len(),
        unsafe_hints: r.unsafe_verification.len(),
        ownership_hints: r.ownership_wrappers.len(),
        deadlock_hints: r.deadlock_findings.len(),
    }
}

pub fn snapshot_workspace_version(workspace_root: &Path) -> String {
    fluxc_core::version::VersionInfo::load(workspace_root)
        .map(|v| v.raw)
        .unwrap_or_else(|_| "unknown".into())
}

/// Compose the full snapshot. UTC timestamp at top so each generated report
/// is identifiable independent of filename.
pub fn snapshot(workspace_root: &Path, swarm_path: &Path) -> ReportState {
    let swarm = load_swarm(swarm_path);
    let sap = compute_sap(&swarm);
    let flux_ai = snapshot_flux_ai(workspace_root);
    ReportState {
        generated_at_utc: chrono::Utc::now().format("%Y-%m-%d %H:%M:%SZ").to_string(),
        workspace_version: snapshot_workspace_version(workspace_root),
        swarm,
        sap,
        flux_ai,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_swarm_file_yields_empty_snapshot() {
        let snap = load_swarm(Path::new("/tmp/__definitely_does_not_exist__.json"));
        assert_eq!(snap.agents.len(), 0);
        assert_eq!(snap.active_claims.len(), 0);
        assert_eq!(snap.total_qug_paid, 0.0);
    }

    #[test]
    fn compute_sap_handles_empty_swarm() {
        let sap = compute_sap(&SwarmSnapshot::default());
        // Empty swarm → 0 utilization, 0 diversity, 0 settlement, untouched cache.
        assert_eq!(sap.swarm_utilization, 0.0);
        assert_eq!(sap.agent_diversity, 0.0);
        assert_eq!(sap.settlement_throughput, 0.0);
        // compile_velocity has a hardcoded baseline — pin it so a refactor
        // wiring real events shifts this test deliberately.
        assert!(sap.compile_velocity > 0.0);
    }

    #[test]
    fn settlement_throughput_scales_with_completed() {
        let mut s = SwarmSnapshot::default();
        s.completed = vec![
            SwarmCompletedRow {
                task_id: "x-0".into(),
                agent: "x".into(),
                crates: vec![],
                success: true,
                qug_earned: 0.5,
            };
            3
        ];
        let sap = compute_sap(&s);
        assert!(sap.settlement_throughput > 0.99); // 3 settled / 0 active → 1.0
    }
}
