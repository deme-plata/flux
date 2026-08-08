// fluxc-core/goals.rs — multi-agent goal stack for the third in-game player.
//
// Three Claude Code terminals (rocky, rocky-arena-N, rocky-update, …) share
// control of one character in flux-arena. Each terminal `flux_goal_post`'s a
// behaviour token ("shoot viktor 3x", "pause", "patrol bridge"). The game
// client polls `/api/goal/current` every tick and executes the top-priority
// active goal.
//
// Priorities (lower = more urgent):
//   0  — emergency (dodge, take cover)
//   1  — focused action (shoot specific target N times)
//   2  — tactical (move to position)
//   3  — strategic ("hunt Torbjørn")
//   5  — idle behaviour ("move around and shoot")
//
// TTL lets goals self-expire. The default behaviour goal can post with
// ttl=0 (= never expires) and a high priority number, so any other goal
// preempts it the moment it's added.
//
// State on disk: /tmp/flux-goals.json — a single JSON object holding the
// stack. Written under flux_swarm_tools::with_locked so concurrent MCP
// processes never clobber each other.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const GOALS_PATH: &str = "/tmp/flux-goals.json";
pub const GOALS_LOCK: &str = "/tmp/flux-goals.lock";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Goal {
    pub id: String,
    pub agent: String,
    pub text: String,
    pub priority: u8,
    pub posted_at: u64,
    pub ttl_secs: u64,
    /// Free-form metadata bag, e.g. {"target":"viktor", "count":3}.
    #[serde(default)]
    pub meta: BTreeMap<String, serde_json::Value>,
}

impl Goal {
    pub fn is_active(&self, now: u64) -> bool {
        self.ttl_secs == 0 || now.saturating_sub(self.posted_at) < self.ttl_secs
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GoalStore {
    pub goals: Vec<Goal>,
    /// Monotonically increasing — each post gets a fresh id.
    #[serde(default)]
    pub next_seq: u64,
}

impl GoalStore {
    pub fn load(bytes: &[u8]) -> Self {
        if bytes.is_empty() {
            return Self::default();
        }
        serde_json::from_slice(bytes).unwrap_or_default()
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).unwrap_or_else(|_| b"{}".to_vec())
    }
    pub fn cleanup(&mut self, now: u64) {
        self.goals.retain(|g| g.is_active(now));
    }
    /// Stable ranking: lower priority first, then most recent post wins ties.
    pub fn sorted(&self) -> Vec<&Goal> {
        let mut out: Vec<&Goal> = self.goals.iter().collect();
        out.sort_by(|a, b| {
            a.priority.cmp(&b.priority)
                .then(b.posted_at.cmp(&a.posted_at))
        });
        out
    }
    pub fn consensus(&self, now: u64) -> Option<&Goal> {
        self.sorted().into_iter().find(|g| g.is_active(now))
    }
}

// ── Public API ──────────────────────────────────────────────────────────────

pub fn post_goal(
    agent: &str,
    text: &str,
    priority: u8,
    ttl_secs: u64,
) -> Result<Goal, String> {
    if agent.is_empty() || text.is_empty() {
        return Err("agent and text required".into());
    }
    let agent = agent.to_string();
    let text = text.to_string();
    let now = now_secs();
    let mut posted: Option<Goal> = None;
    flux_swarm_tools::with_locked(GOALS_LOCK, GOALS_PATH, |cur| {
        let mut store = GoalStore::load(cur);
        store.cleanup(now);
        store.next_seq += 1;
        let goal = Goal {
            id: format!("g-{}", store.next_seq),
            agent: agent.clone(),
            text: text.clone(),
            priority,
            posted_at: now,
            ttl_secs,
            meta: parse_meta(&text),
        };
        posted = Some(goal.clone());
        store.goals.push(goal);
        store.to_bytes()
    }).map_err(|e| format!("goals lock: {}", e))?;
    posted.ok_or_else(|| "post lost".into())
}

pub fn list_goals() -> Result<Vec<Goal>, String> {
    let mut goals: Vec<Goal> = Vec::new();
    flux_swarm_tools::with_locked(GOALS_LOCK, GOALS_PATH, |cur| {
        let mut store = GoalStore::load(cur);
        store.cleanup(now_secs());
        goals = store.sorted().into_iter().cloned().collect();
        store.to_bytes()
    }).map_err(|e| format!("goals lock: {}", e))?;
    Ok(goals)
}

pub fn consensus_goal() -> Result<Option<Goal>, String> {
    let goals = list_goals()?;
    Ok(goals.into_iter().next())
}

pub fn clear_goals() -> Result<usize, String> {
    let mut cleared = 0usize;
    flux_swarm_tools::with_locked(GOALS_LOCK, GOALS_PATH, |cur| {
        let store = GoalStore::load(cur);
        cleared = store.goals.len();
        GoalStore::default().to_bytes()
    }).map_err(|e| format!("goals lock: {}", e))?;
    Ok(cleared)
}

/// Render the consensus as a single line — what the game client will see.
pub fn render_consensus(g: &Goal) -> serde_json::Value {
    serde_json::json!({
        "id": g.id,
        "agent": g.agent,
        "text": g.text,
        "priority": g.priority,
        "posted_at": g.posted_at,
        "ttl_secs": g.ttl_secs,
        "meta": g.meta,
    })
}

// ── Heuristic parsing ───────────────────────────────────────────────────────
// Cheap NLP for the goal text → structured meta. Recognises:
//   "shoot X N times"           → {action:"shoot", target:X, count:N}
//   "shoot X and Y N times each" → adds count:N (game spawns two sub-tasks)
//   "pause", "wait", "hold"     → {action:"pause"}
//   "move around", "patrol"     → {action:"patrol"}

fn parse_meta(text: &str) -> BTreeMap<String, serde_json::Value> {
    let mut m = BTreeMap::new();
    let t = text.to_lowercase();
    let action = if t.contains("shoot") {
        "shoot"
    } else if t.contains("pause") || t.contains("hold") || t.contains("wait") {
        "pause"
    } else if t.contains("patrol") || t.contains("move around") || t.contains("wander") {
        "patrol"
    } else if t.contains("hide") || t.contains("cover") {
        "take_cover"
    } else if t.contains("hunt") {
        "hunt"
    } else {
        "free"
    };
    m.insert("action".into(), serde_json::json!(action));

    // Targets — substring match on known names. Cheap, good enough for
    // 2-player matches.
    let mut targets: Vec<String> = Vec::new();
    for name in &["viktor", "torbjørn", "torbjorn", "brother", "rocky"] {
        if t.contains(name) {
            targets.push((*name).to_string());
        }
    }
    if !targets.is_empty() {
        m.insert("targets".into(), serde_json::json!(targets));
    }

    // Count — "N times" / "N times each".
    if let Some(n) = parse_count(&t) {
        m.insert("count".into(), serde_json::json!(n));
    }

    m
}

fn parse_count(t: &str) -> Option<u32> {
    // Look for "N times" — N is the digits before " time"/" times".
    for needle in &[" times", " time"] {
        if let Some(idx) = t.find(needle) {
            let prefix = &t[..idx];
            let n: String = prefix
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            let n: String = n.chars().rev().collect();
            if let Ok(v) = n.parse::<u32>() {
                return Some(v);
            }
        }
    }
    None
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// Re-export so other modules don't have to import the parking-lot path.
pub use std::path::PathBuf;
pub fn goals_path() -> PathBuf {
    PathBuf::from(GOALS_PATH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_shoot_3_times() {
        let m = parse_meta("shoot viktor and torbjørn 3 times each");
        assert_eq!(m.get("action"), Some(&serde_json::json!("shoot")));
        let targets = m.get("targets").unwrap().as_array().unwrap();
        assert!(targets.iter().any(|v| v == "viktor"));
        assert!(targets.iter().any(|v| v == "torbjørn"));
        assert_eq!(m.get("count"), Some(&serde_json::json!(3)));
    }

    #[test]
    fn parse_pause_default() {
        let m = parse_meta("then pause");
        assert_eq!(m.get("action"), Some(&serde_json::json!("pause")));
    }

    #[test]
    fn parse_patrol() {
        let m = parse_meta("move around and shoot");
        // "shoot" wins over "move around" because shoot is checked first.
        assert_eq!(m.get("action"), Some(&serde_json::json!("shoot")));
        // But the patrol fallback also gets caught by .contains() check.
    }

    #[test]
    fn store_consensus_orders_by_priority() {
        let mut s = GoalStore::default();
        s.goals.push(Goal { id:"g-1".into(), agent:"a".into(), text:"patrol".into(),
            priority:5, posted_at:100, ttl_secs:0, meta:Default::default() });
        s.goals.push(Goal { id:"g-2".into(), agent:"b".into(), text:"shoot".into(),
            priority:1, posted_at:200, ttl_secs:0, meta:Default::default() });
        assert_eq!(s.consensus(300).unwrap().id, "g-2");
    }

    #[test]
    fn ttl_expires() {
        let g = Goal { id:"g-1".into(), agent:"a".into(), text:"x".into(),
            priority:1, posted_at:100, ttl_secs:30, meta:Default::default() };
        assert!(g.is_active(120));
        assert!(!g.is_active(200));
        let g2 = Goal { ttl_secs: 0, ..g.clone() };
        assert!(g2.is_active(9_999_999));
    }
}
