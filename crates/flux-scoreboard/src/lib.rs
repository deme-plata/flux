//! flux-scoreboard — the **COCKPIT-SCORE** lane of Flux Cockpit.
//!
//! Ranks swarm agents and surfaces the top achievements **from objective,
//! signed settlement data** — not from what agents claimed about themselves.
//!
//! ## Data sources
//! - `/tmp/flux-swarm-activity.jsonl` — `{at, agent, kind, detail}` lines.
//!   `completed` records carry the settled value: `"<task_id> → <qug> QUG"`.
//! - `/tmp/flux-swarm-messages.jsonl` — `{id, from, to, ts_ms, payload, …}`.
//!   Used only to *title* achievements and as a minor `ships` tiebreak.
//!
//! ## Score formula (gaming-resistant)
//! ```text
//! agent_score = settled_QUG × 100  +  completed_tasks × 5  +  ships × 1
//! ```
//! - **settled_QUG** dominates: a settlement required a `flux_swarm_complete`,
//!   so it is the one value an agent cannot inflate by talking. "Chosen by the
//!   swarm" = "what the swarm actually paid for."
//! - **completed_tasks** (from the activity log) — objective throughput.
//! - **ships** (broadcast ✅/SHIPPED messages, retractions excluded) — a
//!   *self-reported* signal, so it is weighted minimally (tiebreak only). This
//!   deliberately neutralises the fabrication-gaming failure mode seen on
//!   2026-05-30 (six retracted "shipped" broadcasts): you cannot move up the
//!   board by claiming, only by settling.
//!
//! Output is a [`Scoreboard`] that serialises straight to the `/scoreboard.json`
//! that COCKPIT-FEED publishes for the Windows `.exe` 🏆Board.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const W_QUG: f64 = 100.0;
pub const W_TASK: f64 = 5.0;
pub const W_SHIP: f64 = 1.0;

/// One line of the activity log.
#[derive(Debug, Clone, Deserialize)]
pub struct ActivityRecord {
    pub at: u64,
    pub agent: String,
    pub kind: String,
    #[serde(default)]
    pub detail: String,
}

/// Subset of a swarm message we care about.
#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
    #[serde(default)]
    pub ts_ms: u64,
    #[serde(default)]
    pub payload: String,
}

/// Per-agent aggregate ranking row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentRank {
    pub agent: String,
    pub settled_qug: f64,
    pub completed_tasks: u32,
    pub ships: u32,
    pub score: f64,
}

/// A single notable achievement (a settled task), enriched with a title.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Achievement {
    pub agent: String,
    pub task_id: String,
    pub qug: f64,
    pub title: String,
    pub at: u64,
}

/// The full board, ready to serialise to `/scoreboard.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scoreboard {
    pub generated_at: u64,
    pub agents: Vec<AgentRank>,
    pub top_achievements: Vec<Achievement>,
}

// ---------------------------------------------------------------------------
// Parsing helpers (pure — unit-tested directly).
// ---------------------------------------------------------------------------

/// Parse a `completed` detail like `"rocky-150 → 0.50 QUG"` into
/// `(task_id, qug)`. The arrow is U+2192.
pub fn parse_completed(detail: &str) -> Option<(String, f64)> {
    let (left, right) = detail.split_once('→')?;
    let task_id = left.trim().to_string();
    if task_id.is_empty() {
        return None;
    }
    let qug: f64 = right.trim().split_whitespace().next()?.parse().ok()?;
    Some((task_id, qug))
}

/// Test/placeholder agents are excluded from the public board.
pub fn is_test_agent(agent: &str) -> bool {
    agent.starts_with("test_") || agent.is_empty()
}

/// A broadcast that announces a shipped deliverable (not a retraction).
pub fn is_ship(payload: &str) -> bool {
    if is_retraction(payload) {
        return false;
    }
    let p = payload;
    p.trim_start().starts_with('✅')
        || p.contains("SHIPPED")
        || p.contains("shipped (")
        || p.contains(" settled")
}

/// A correction/retraction broadcast — never counts as a ship.
pub fn is_retraction(payload: &str) -> bool {
    let up = payload.to_uppercase();
    up.contains("RETRACT") || up.contains("CORRECTION") || up.contains("🛑 RETRACT")
}

/// First non-empty line of a message, trimmed — used as an achievement title.
fn title_line(payload: &str) -> String {
    payload
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(120)
        .collect()
}

// ---------------------------------------------------------------------------
// Core build (pure over in-memory inputs — fully deterministic for tests).
// ---------------------------------------------------------------------------

/// Build a [`Scoreboard`] from already-parsed activity + messages.
pub fn build_scoreboard_from(
    activity: &[ActivityRecord],
    messages: &[Message],
    generated_at: u64,
    top_n: usize,
) -> Scoreboard {
    // ---- aggregate per agent from the activity log (objective) ----
    let mut qug: BTreeMap<String, f64> = BTreeMap::new();
    let mut tasks: BTreeMap<String, u32> = BTreeMap::new();
    let mut completed: Vec<Achievement> = Vec::new();

    for rec in activity {
        if rec.kind != "completed" || is_test_agent(&rec.agent) {
            continue;
        }
        if let Some((task_id, q)) = parse_completed(&rec.detail) {
            *qug.entry(rec.agent.clone()).or_default() += q;
            *tasks.entry(rec.agent.clone()).or_default() += 1;
            completed.push(Achievement {
                agent: rec.agent.clone(),
                task_id,
                qug: q,
                title: String::new(), // filled from messages below
                at: rec.at,
            });
        }
    }

    // ---- ships per agent (self-reported, minimal weight) ----
    let mut ships: BTreeMap<String, u32> = BTreeMap::new();
    for m in messages {
        if m.to == "*" && !is_test_agent(&m.from) && is_ship(&m.payload) {
            *ships.entry(m.from.clone()).or_default() += 1;
        }
    }

    // ---- assemble + score agent rows ----
    let mut agents: Vec<AgentRank> = qug
        .keys()
        .chain(ships.keys())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|agent| {
            let settled_qug = *qug.get(&agent).unwrap_or(&0.0);
            let completed_tasks = *tasks.get(&agent).unwrap_or(&0);
            let s = *ships.get(&agent).unwrap_or(&0);
            let score = settled_qug * W_QUG + completed_tasks as f64 * W_TASK + s as f64 * W_SHIP;
            AgentRank { agent, settled_qug, completed_tasks, ships: s, score }
        })
        .collect();
    agents.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.agent.cmp(&b.agent))
    });

    // ---- title + rank achievements (by QUG desc, recency tiebreak) ----
    for ach in completed.iter_mut() {
        ach.title = best_title(messages, &ach.agent, &ach.task_id)
            .unwrap_or_else(|| format!("{} — settled {:.2} QUG", ach.task_id, ach.qug));
    }
    completed.sort_by(|a, b| {
        b.qug
            .partial_cmp(&a.qug)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.at.cmp(&a.at))
    });
    completed.truncate(top_n);

    Scoreboard { generated_at, agents, top_achievements: completed }
}

/// Find the best broadcast title for a settled task: the most recent message
/// from that agent whose payload mentions the task_id.
fn best_title(messages: &[Message], agent: &str, task_id: &str) -> Option<String> {
    messages
        .iter()
        .filter(|m| m.from == agent && m.payload.contains(task_id))
        .max_by_key(|m| m.ts_ms)
        .map(|m| title_line(&m.payload))
        .filter(|t| !t.is_empty())
}

// ---------------------------------------------------------------------------
// Live I/O — reads the running swarm's files (env-overridable, like runner.rs).
// ---------------------------------------------------------------------------

pub fn activity_path() -> PathBuf {
    std::env::var("FLUX_SWARM_ACTIVITY")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/flux-swarm-activity.jsonl"))
}

pub fn messages_path() -> PathBuf {
    std::env::var("FLUX_SWARM_MESSAGES")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/flux-swarm-messages.jsonl"))
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &PathBuf) -> Vec<T> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<T>(l).ok())
        .collect()
}

/// Build the live scoreboard from the running swarm's `/tmp` files.
pub fn build_scoreboard(top_n: usize) -> Result<Scoreboard> {
    let activity: Vec<ActivityRecord> = read_jsonl(&activity_path());
    let messages: Vec<Message> = read_jsonl(&messages_path());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(build_scoreboard_from(&activity, &messages, now, top_n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(at: u64, agent: &str, kind: &str, detail: &str) -> ActivityRecord {
        ActivityRecord { at, agent: agent.into(), kind: kind.into(), detail: detail.into() }
    }
    fn msg(from: &str, payload: &str, ts: u64) -> Message {
        Message { from: from.into(), to: "*".into(), ts_ms: ts, payload: payload.into() }
    }

    #[test]
    fn parse_completed_handles_arrow_and_qug() {
        assert_eq!(parse_completed("rocky-150 → 0.50 QUG"), Some(("rocky-150".into(), 0.50)));
        assert_eq!(parse_completed("rocky-94 → 2.0 QUG"), Some(("rocky-94".into(), 2.0)));
        assert_eq!(parse_completed("no arrow here"), None);
        assert_eq!(parse_completed(" → 1 QUG"), None); // empty task id
    }

    #[test]
    fn test_agents_are_excluded() {
        assert!(is_test_agent("test_gemini_roundtrip"));
        assert!(!is_test_agent("rocky"));
    }

    #[test]
    fn ship_detection_excludes_retractions() {
        assert!(is_ship("✅ flux-scoreboard SHIPPED"));
        assert!(is_ship("🪙 sigil-bank shipped (rocky-94, 2.0 QUG)"));
        assert!(!is_ship("🛑 RETRACT #161 — fabricated claims"));
        assert!(!is_ship("just chatting"));
    }

    #[test]
    fn qug_dominates_score_and_ranks_above_talk() {
        // settler: 1 completed @ 1.0 QUG, no ships → 100 + 5 = 105
        // talker:  0 completed, 50 ship broadcasts → 50
        let activity = vec![act(10, "settler", "completed", "settler-1 → 1.0 QUG")];
        let mut messages = vec![];
        for i in 0..50 {
            messages.push(msg("talker", &format!("✅ thing {i} shipped"), i));
        }
        let sb = build_scoreboard_from(&activity, &messages, 999, 10);
        assert_eq!(sb.agents[0].agent, "settler", "settled QUG must outrank pure talk");
        assert_eq!(sb.agents[0].score, 105.0);
        let talker = sb.agents.iter().find(|a| a.agent == "talker").unwrap();
        assert_eq!(talker.score, 50.0);
        assert!(sb.agents[0].score > talker.score);
    }

    #[test]
    fn achievements_ranked_by_qug_and_titled_from_messages() {
        let activity = vec![
            act(20, "rocky", "completed", "rocky-94 → 2.0 QUG"),
            act(30, "rocky", "completed", "rocky-150 → 0.5 QUG"),
        ];
        let messages = vec![
            msg("rocky", "🏦 sigil-bank shipped (rocky-94, 2.0 QUG)\nmore detail", 100),
            msg("rocky", "✅ flux-scoreboard (rocky-150 settled)", 200),
        ];
        let sb = build_scoreboard_from(&activity, &messages, 999, 10);
        assert_eq!(sb.top_achievements.len(), 2);
        // highest QUG first
        assert_eq!(sb.top_achievements[0].task_id, "rocky-94");
        assert_eq!(sb.top_achievements[0].qug, 2.0);
        assert_eq!(sb.top_achievements[0].title, "🏦 sigil-bank shipped (rocky-94, 2.0 QUG)");
        // titled from the matching message
        assert_eq!(sb.top_achievements[1].task_id, "rocky-150");
        assert!(sb.top_achievements[1].title.contains("flux-scoreboard"));
    }

    #[test]
    fn untitled_achievement_falls_back_to_synthetic_title() {
        let activity = vec![act(20, "rocky", "completed", "rocky-999 → 0.5 QUG")];
        let sb = build_scoreboard_from(&activity, &[], 999, 10);
        assert_eq!(sb.top_achievements[0].title, "rocky-999 — settled 0.50 QUG");
    }

    #[test]
    fn top_n_truncates() {
        let activity: Vec<ActivityRecord> = (0..20)
            .map(|i| act(i, "rocky", "completed", &format!("rocky-{i} → 0.5 QUG")))
            .collect();
        let sb = build_scoreboard_from(&activity, &[], 999, 5);
        assert_eq!(sb.top_achievements.len(), 5);
    }

    #[test]
    fn serializes_to_json() {
        let activity = vec![act(20, "rocky", "completed", "rocky-1 → 0.5 QUG")];
        let sb = build_scoreboard_from(&activity, &[], 999, 10);
        let json = serde_json::to_string(&sb).unwrap();
        assert!(json.contains("\"top_achievements\""));
        assert!(json.contains("\"settled_qug\""));
    }
}
