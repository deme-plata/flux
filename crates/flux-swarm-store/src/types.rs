//! Domain types — a 1:1 mirror of the JSON shapes the fluxc-mcp swarm handlers
//! already persist to `/tmp/flux-swarm*.json[l]`. Field names match the on-disk
//! JSON exactly so the importer is lossless.

use serde::{Deserialize, Serialize};

/// A registered swarm agent (from `flux-swarm.json` → `agents`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub wallet_address: String,
    pub registered_at: u64,
    /// "Idle" | "Working" — kept as a String to be lossless against the hot state.
    pub status: String,
    #[serde(default)]
    pub current_crates: Vec<String>,
    #[serde(default)]
    pub total_earned_qug: f64,
}

/// An active lane claim (from `flux-swarm.json` → `claims`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    pub task_id: String,
    #[serde(default)]
    pub crates: Vec<String>,
    pub agent: String,
    pub claimed_at: u64,
    #[serde(default)]
    pub priority: u32,
    #[serde(default)]
    pub estimated_qug: f64,
}

/// A settled task — the DURABLE QUG ledger (from `flux-swarm-completed.jsonl`).
/// This is real money attribution; it is never TTL'd.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Completed {
    pub task_id: String,
    pub agent_id: String,
    #[serde(default)]
    pub crates: Vec<String>,
    pub success: bool,
    pub qug_earned: f64,
    pub completed_at: u64,
}

/// An inter-agent message (from `flux-swarm-messages.jsonl`). `to == "*"` = broadcast.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: u64,
    pub from: String,
    pub to: String,
    pub ts_ms: u64,
    pub payload: String,
    #[serde(default)]
    pub reply_to: Option<u64>,
}

/// An activity-log entry (from `flux-swarm-activity.jsonl`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Activity {
    pub at: u64,
    pub agent: String,
    pub kind: String,
    pub detail: String,
}

/// A file-level lease (from `flux-swarm-files.json` → `claims`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileClaim {
    pub path: String,
    pub agent: String,
    pub claimed_at: u64,
    #[serde(default)]
    pub note: String,
}
