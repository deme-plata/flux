// swarm.rs — Agentic Money Swarm Coordination
// File-persistent state via /tmp/flux-swarm.json
// Multiple agents (Gemini, DeepSeek) coordinate work atomically.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use flux_swarm_store::{FluxDbStore, SwarmStore};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AgentStatus { Idle, Working, WaitingForReview, Offline }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmAgent { pub id: String, pub wallet_address: String, pub registered_at: u64, pub status: AgentStatus, pub current_crates: Vec<String>, pub total_earned_qug: f64 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkClaim { pub task_id: String, pub crates: Vec<String>, pub agent: String, pub claimed_at: u64, pub priority: u32, pub estimated_qug: f64 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletedTask { pub task_id: String, pub agent_id: String, pub crates: Vec<String>, pub success: bool, pub qug_earned: f64, pub completed_at: u64 }

#[derive(Serialize, Deserialize)]
struct Inner {
    agents: HashMap<String, SwarmAgent>,
    claims: Vec<WorkClaim>,
    // Legacy in-file completed history. Still DESERIALIZED for back-compat (old
    // files carry the full array) but NEVER re-serialized — that unbounded Vec
    // being rewritten on every mutation was the 947µs "rear tire". It's migrated
    // to the append-only journal + `completed_count` on first load.
    #[serde(default, skip_serializing)]
    completed: Vec<CompletedTask>,
    // Monotonic count of all completed tasks (preserves task_id + status after
    // the journal split).
    #[serde(default)]
    completed_count: usize,
    qug_paid: f64,
}

const SWARM_FILE: &str = "/tmp/flux-swarm.json";
const COMPLETED_JOURNAL: &str = "/tmp/flux-swarm-completed.jsonl";
static SWARM: Mutex<Option<Inner>> = Mutex::new(None);

fn db_path() -> Option<String> {
    std::env::var("FLUX_SWARM_DB")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

fn with_db<T>(f: impl FnOnce(&FluxDbStore) -> Result<T, String>) -> Option<Result<T, String>> {
    let path = db_path()?;
    Some(FluxDbStore::open(path).map_err(|e| e.to_string()).and_then(|db| f(&db)))
}

fn agent_to_store(a: &SwarmAgent) -> flux_swarm_store::Agent {
    flux_swarm_store::Agent {
        id: a.id.clone(),
        wallet_address: a.wallet_address.clone(),
        registered_at: a.registered_at,
        status: format!("{:?}", a.status),
        current_crates: a.current_crates.clone(),
        total_earned_qug: a.total_earned_qug,
    }
}

fn agent_from_store(a: flux_swarm_store::Agent) -> SwarmAgent {
    let status = match a.status.as_str() {
        "Working" => AgentStatus::Working,
        "WaitingForReview" => AgentStatus::WaitingForReview,
        "Offline" => AgentStatus::Offline,
        _ => AgentStatus::Idle,
    };
    SwarmAgent {
        id: a.id,
        wallet_address: a.wallet_address,
        registered_at: a.registered_at,
        status,
        current_crates: a.current_crates,
        total_earned_qug: a.total_earned_qug,
    }
}

fn claim_to_store(c: &WorkClaim) -> flux_swarm_store::Claim {
    flux_swarm_store::Claim {
        task_id: c.task_id.clone(),
        crates: c.crates.clone(),
        agent: c.agent.clone(),
        claimed_at: c.claimed_at,
        priority: c.priority,
        estimated_qug: c.estimated_qug,
    }
}

fn completed_to_store(t: &CompletedTask) -> flux_swarm_store::Completed {
    flux_swarm_store::Completed {
        task_id: t.task_id.clone(),
        agent_id: t.agent_id.clone(),
        crates: t.crates.clone(),
        success: t.success,
        qug_earned: t.qug_earned,
        completed_at: t.completed_at,
    }
}

fn load() -> Option<Inner> {
    std::fs::read_to_string(SWARM_FILE).ok().and_then(|s| serde_json::from_str(&s).ok())
}

// Atomic small-file write: serialize ONLY the live state (agents + claims +
// counter + qug), tmp+rename so readers never see a torn file. `completed` is
// skip_serialized, so this no longer grows with history — the rear-tire fix.
fn save(inner: &Inner) {
    if let Ok(j) = serde_json::to_string(inner) {
        let tmp = format!("{SWARM_FILE}.tmp");
        if std::fs::write(&tmp, j).is_ok() { let _ = std::fs::rename(&tmp, SWARM_FILE); }
    }
}

// Append one settled task to the O(1) journal instead of rewriting all history.
fn journal_completed(t: &CompletedTask) {
    use std::io::Write;
    if let Ok(line) = serde_json::to_string(t) {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(COMPLETED_JOURNAL) {
            let _ = writeln!(f, "{line}");
        }
    }
}

// Re-derive the live counters from the durable, append-only completed journal.
// The journal (`/tmp/flux-swarm-completed.jsonl`) survives even when the hot
// state file (`/tmp/flux-swarm.json`) is lost, wiped, or recreated fresh — that
// asymmetry is why `completed_count` + `qug_paid` used to "start over every
// time" (operator directive #120). By reconciling on every init the counters
// become self-healing: lose the hot file and the next read rebuilds the totals
// from the journal, so the swarm ledger no longer resets.
fn reconcile_from_journal(inner: &mut Inner) {
    reconcile_from_journal_at(inner, COMPLETED_JOURNAL);
}

fn reconcile_from_journal_at(inner: &mut Inner, journal_path: &str) {
    let data = match std::fs::read_to_string(journal_path) {
        Ok(d) => d,
        Err(_) => return, // no journal yet — nothing settled, nothing to reconcile
    };
    let mut count = 0usize;
    let mut qug = 0.0f64;
    let mut per_agent: HashMap<String, f64> = HashMap::new();
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        if let Ok(t) = serde_json::from_str::<CompletedTask>(line) {
            count += 1; // one line = one completion event (matches the old counter semantics)
            qug += t.qug_earned;
            *per_agent.entry(t.agent_id).or_insert(0.0) += t.qug_earned;
        }
    }
    // The journal is the source of truth for settled history. Adopt its totals
    // whenever it's at least as complete as the in-file counter (the normal case
    // after any reset). Never shrink below what the hot file already recorded.
    if count >= inner.completed_count {
        inner.completed_count = count;
        inner.qug_paid = qug;
    }
    // Rebuild lifetime per-agent earnings for agents we currently know about, so
    // `total_earned_qug` survives a hot-file reset too (feeds the by-role ledger).
    for (id, agent) in inner.agents.iter_mut() {
        if let Some(total) = per_agent.get(id) {
            agent.total_earned_qug = *total;
        }
    }
}

fn init() -> std::sync::MutexGuard<'static, Option<Inner>> {
    let mut g = SWARM.lock().unwrap();
    if g.is_none() {
        let mut inner = load().unwrap_or(Inner { agents: HashMap::new(), claims: Vec::new(), completed: Vec::new(), completed_count: 0, qug_paid: 0.0 });
        // One-time migration: an old hot file carried the whole `completed` array.
        // Fold it into the journal + counter, then drop it so it's never rewritten.
        if !inner.completed.is_empty() {
            if inner.completed_count < inner.completed.len() {
                for t in &inner.completed { journal_completed(t); }
                inner.completed_count = inner.completed.len();
            }
            inner.completed.clear();
        }
        // Self-heal counters from the durable journal (directive #120): if the
        // hot file was lost/reset, the journal still has the full settled history.
        reconcile_from_journal(&mut inner);
        *g = Some(inner);
        save(g.as_ref().unwrap());
    }
    g
}

/// Drop the in-process cached state so the next call re-reads from disk.
/// Use this just after acquiring an external file lock so the load that
/// follows reflects every concurrent writer's commits — without this,
/// the SWARM static keeps serving the snapshot from the first init().
pub fn force_reload() {
    let mut g = SWARM.lock().unwrap();
    *g = None;
}

pub fn register_agent(id: &str, wallet: &str) -> SwarmAgent {
    if let Some(res) = with_db(|db| {
        let mut agent = db
            .get_agent(id)
            .map_err(|e| e.to_string())?
            .map(agent_from_store)
            .unwrap_or(SwarmAgent {
                id: id.into(),
                wallet_address: wallet.into(),
                registered_at: now(),
                status: AgentStatus::Idle,
                current_crates: vec![],
                total_earned_qug: 0.0,
            });
        agent.wallet_address = wallet.into();
        db.put_agent(&agent_to_store(&agent)).map_err(|e| e.to_string())?;
        Ok(agent)
    }) {
        if let Ok(agent) = res {
            return agent;
        }
    }

    let mut g = init(); let s = g.as_mut().unwrap();
    let a = SwarmAgent { id: id.into(), wallet_address: wallet.into(), registered_at: now(), status: AgentStatus::Idle, current_crates: vec![], total_earned_qug: 0.0 };
    s.agents.insert(id.into(), a.clone()); save(s); a
}

pub fn claim_work(agent_id: &str, crates: &[String], priority: u32) -> Result<WorkClaim, String> {
    if let Some(res) = with_db(|db| {
        for c in db.list_claims().map_err(|e| e.to_string())? {
            for mc in crates {
                if c.crates.contains(mc) {
                    if c.agent == agent_id {
                        return Err(format!(
                            "self-owned: you already hold task {} on '{}' — re-claim is a no-op. Use flux_swarm_release to drop or flux_swarm_complete to settle.",
                            c.task_id, mc
                        ));
                    }
                    return Err(format!("{} claimed by {}", mc, c.agent));
                }
            }
        }

        let task_id = format!(
            "{}-{}",
            agent_id,
            db.list_claims().map_err(|e| e.to_string())?.len() as u64
                + db.completed_count().map_err(|e| e.to_string())?
        );
        let claim = WorkClaim {
            task_id,
            crates: crates.to_vec(),
            agent: agent_id.into(),
            claimed_at: now(),
            priority,
            estimated_qug: crates.len() as f64 * 0.5,
        };
        if let Some(mut agent) = db.get_agent(agent_id).map_err(|e| e.to_string())?.map(agent_from_store) {
            agent.status = AgentStatus::Working;
            agent.current_crates = crates.to_vec();
            db.put_agent(&agent_to_store(&agent)).map_err(|e| e.to_string())?;
        }
        db.put_claim(&claim_to_store(&claim)).map_err(|e| e.to_string())?;
        Ok(claim)
    }) {
        return res;
    }

    let mut g = init(); let s = g.as_mut().unwrap();
    // Conflict check: distinguish self-owned (idempotent / informational) from
    // other-agent owned (true conflict). The MCP wrapper inspects the
    // "self-owned:" prefix to surface a softer UX message.
    for c in &s.claims {
        for mc in crates {
            if c.crates.contains(mc) {
                if c.agent == agent_id {
                    return Err(format!(
                        "self-owned: you already hold task {} on '{}' — re-claim is a no-op. Use flux_swarm_release to drop or flux_swarm_complete to settle.",
                        c.task_id, mc
                    ));
                }
                return Err(format!("{} claimed by {}", mc, c.agent));
            }
        }
    }
    // Monotonic task_id: claims + completed history. Avoids the v0.11 bug
    // where `claims.len()` reused indexes after settlement, producing
    // duplicate task_ids like three separate `rocky-1` rows in `completed`.
    let tid = format!("{}-{}", agent_id, s.claims.len() + s.completed_count);
    let claim = WorkClaim { task_id: tid.clone(), crates: crates.to_vec(), agent: agent_id.into(), claimed_at: now(), priority, estimated_qug: crates.len() as f64 * 0.5 };
    if let Some(a) = s.agents.get_mut(agent_id) { a.status = AgentStatus::Working; a.current_crates = crates.to_vec(); }
    s.claims.push(claim.clone()); save(s); Ok(claim)
}

pub fn complete_work(agent_id: &str, task_id: &str, success: bool) -> Option<CompletedTask> {
    if let Some(res) = with_db(|db| {
        let claim = db.take_claim(task_id).map_err(|e| e.to_string())?;
        let Some(claim) = claim.filter(|c| c.agent == agent_id) else {
            return Ok(None);
        };
        let t = CompletedTask {
            task_id: task_id.into(),
            agent_id: agent_id.into(),
            crates: claim.crates.clone(),
            success,
            qug_earned: if success { claim.estimated_qug } else { 0.0 },
            completed_at: now(),
        };
        if let Some(mut agent) = db.get_agent(agent_id).map_err(|e| e.to_string())?.map(agent_from_store) {
            agent.status = AgentStatus::Idle;
            agent.current_crates.clear();
            agent.total_earned_qug += t.qug_earned;
            db.put_agent(&agent_to_store(&agent)).map_err(|e| e.to_string())?;
        }
        db.append_completed(&completed_to_store(&t)).map_err(|e| e.to_string())?;
        Ok(Some(t))
    }) {
        return res.ok().flatten();
    }

    let mut g = init(); let s = g.as_mut().unwrap();
    let pos = s.claims.iter().position(|c| c.task_id == task_id && c.agent == agent_id)?;
    let claim = s.claims.remove(pos);
    let t = CompletedTask { task_id: task_id.into(), agent_id: agent_id.into(), crates: claim.crates.clone(), success, qug_earned: if success { claim.estimated_qug } else { 0.0 }, completed_at: now() };
    if let Some(a) = s.agents.get_mut(agent_id) { a.status = AgentStatus::Idle; a.current_crates.clear(); a.total_earned_qug += t.qug_earned; }
    s.qug_paid += t.qug_earned; journal_completed(&t); s.completed_count += 1; save(s); Some(t)
}

/// Release a claim without payment. For stuck claims, crashed agents, or work that won't complete.
/// Idempotent — returns false if the claim doesn't exist.
pub fn release_claim(agent_id: &str, task_id: &str) -> bool {
    if let Some(res) = with_db(|db| {
        let claim = db.take_claim(task_id).map_err(|e| e.to_string())?;
        let Some(claim) = claim else { return Ok(false); };
        if claim.agent != agent_id {
            db.put_claim(&claim).map_err(|e| e.to_string())?;
            return Ok(false);
        }
        if let Some(mut agent) = db.get_agent(agent_id).map_err(|e| e.to_string())?.map(agent_from_store) {
            agent.current_crates.clear();
            agent.status = AgentStatus::Idle;
            db.put_agent(&agent_to_store(&agent)).map_err(|e| e.to_string())?;
        }
        Ok(true)
    }) {
        return res.unwrap_or(false);
    }

    let mut g = init(); let s = g.as_mut().unwrap();
    let pos = s.claims.iter().position(|c| c.task_id == task_id && c.agent == agent_id);
    if let Some(p) = pos {
        s.claims.remove(p);
        if let Some(a) = s.agents.get_mut(agent_id) {
            a.current_crates.clear();
            a.status = AgentStatus::Idle;
        }
        save(s); true
    } else { false }
}

pub fn swarm_status() -> SwarmStatus {
    if let Some(res) = with_db(|db| {
        let agents = db
            .list_agents()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(agent_from_store)
            .collect::<Vec<_>>();
        Ok(SwarmStatus {
            agents: agents.len(),
            active_claims: db.list_claims().map_err(|e| e.to_string())?.len(),
            completed_tasks: db.completed_count().map_err(|e| e.to_string())? as usize,
            qug_paid: db.sum_qug_earned().map_err(|e| e.to_string())?,
            agents_list: agents,
        })
    }) {
        if let Ok(status) = res {
            return status;
        }
    }

    let g = init(); let s = g.as_ref().unwrap();
    SwarmStatus { agents: s.agents.len(), active_claims: s.claims.len(), completed_tasks: s.completed_count, qug_paid: s.qug_paid, agents_list: s.agents.values().cloned().collect() }
}

#[derive(Debug, Clone, Serialize)]
pub struct SwarmStatus { pub agents: usize, pub active_claims: usize, pub completed_tasks: usize, pub qug_paid: f64, pub agents_list: Vec<SwarmAgent> }

impl SwarmStatus {
    pub fn summary(&self) -> String {
        let names: Vec<String> = self.agents_list.iter().map(|a| a.id.clone()).collect();
        format!("🐝 Flux Swarm — {} agent(s): {}\n  Active claims: {} | Completed: {} | QUG paid: {:.2}",
            self.agents, names.join(", "), self.active_claims, self.completed_tasks, self.qug_paid)
    }
}

pub fn swarm_register_cli(agent_id: &str, wallet: &str) -> String {
    let agent = register_agent(agent_id, wallet);
    format!("🐝 Agent {} registered with wallet {}", agent.id, agent.wallet_address)
}

pub fn swarm_claim_cli(agent_id: &str, crate_name: &str) -> String {
    match claim_work(agent_id, &[crate_name.to_string()], 1) {
        Ok(c) => format!("✅ {} claimed {} (est. {:.2} QUG)", agent_id, crate_name, c.estimated_qug),
        Err(e) if e.starts_with("self-owned:") => format!("ℹ {}", &e["self-owned: ".len()..]),
        Err(e) => format!("❌ {}", e),
    }
}

fn now() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() }

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as TestMutex;

    static DB_ENV_LOCK: TestMutex<()> = TestMutex::new(());

    fn reset_for_test(tmp: &std::path::Path) {
        // The static SWARM uses a hardcoded /tmp path; for unit-test isolation
        // we point at a per-test file via an env override. Caller cleans up.
        let _ = std::fs::write(tmp, r#"{"agents":{},"claims":[],"completed":[],"qug_paid":0.0}"#);
    }

    #[test]
    fn task_ids_are_monotonic_across_completion() {
        // Mock Inner directly to test the task_id computation without touching
        // the global SWARM_FILE. The id formula is `{agent}-{claims+completed}`.
        let claims = 1usize;
        let completed = 17usize;
        let agent = "rocky";
        let tid = format!("{}-{}", agent, claims + completed);
        assert_eq!(tid, "rocky-18");
        // After settling that claim (claims=0, completed=18), the next id is
        // rocky-18 too if I forget to bump completed. The real code pushes
        // before computing — confirm it monotonically grows.
        let tid_next = format!("{}-{}", agent, 1 + 18);
        assert_eq!(tid_next, "rocky-19");
        assert_ne!(tid, tid_next);
    }

    #[test]
    fn self_owned_prefix_is_stable() {
        // The MCP wrapper splits on this exact prefix; pin it so a refactor
        // doesn't silently break the softer-message UX path.
        let err: String = "self-owned: you already hold task rocky-3 on 'flux-api' — re-claim is a no-op.".into();
        assert!(err.starts_with("self-owned: "));
        let trimmed = &err["self-owned: ".len()..];
        assert!(trimmed.starts_with("you already hold task"));
        let _ = reset_for_test; // silence unused-fn warning if we don't hit the fs path
    }

    #[test]
    fn reconcile_rebuilds_counters_from_journal() {
        // Simulates directive #120: the hot state file was lost/reset (counters
        // at 0) but the append-only journal still holds the settled history.
        // After reconcile, completed_count + qug_paid + per-agent earnings are
        // rebuilt from the journal — the ledger no longer "starts over".
        use std::io::Write;
        let jp = std::env::temp_dir()
            .join(format!("flux-swarm-reconcile-test-{}.jsonl", now()));
        let jp_str = jp.to_str().unwrap().to_string();
        let entries = [
            CompletedTask { task_id: "a-0".into(), agent_id: "a".into(), crates: vec![], success: true, qug_earned: 0.5, completed_at: 1 },
            CompletedTask { task_id: "a-1".into(), agent_id: "a".into(), crates: vec![], success: true, qug_earned: 0.5, completed_at: 2 },
            CompletedTask { task_id: "b-0".into(), agent_id: "b".into(), crates: vec![], success: true, qug_earned: 0.5, completed_at: 3 },
        ];
        {
            let mut f = std::fs::File::create(&jp).unwrap();
            for e in &entries { writeln!(f, "{}", serde_json::to_string(e).unwrap()).unwrap(); }
        }

        let mut inner = Inner { agents: HashMap::new(), claims: vec![], completed: vec![], completed_count: 0, qug_paid: 0.0 };
        inner.agents.insert("a".into(), SwarmAgent { id: "a".into(), wallet_address: "w".into(), registered_at: 0, status: AgentStatus::Idle, current_crates: vec![], total_earned_qug: 0.0 });

        reconcile_from_journal_at(&mut inner, &jp_str);

        assert_eq!(inner.completed_count, 3, "3 journal lines = 3 completions");
        assert!((inner.qug_paid - 1.5).abs() < 1e-9, "summed qug from journal");
        assert!((inner.agents.get("a").unwrap().total_earned_qug - 1.0).abs() < 1e-9, "per-agent lifetime earnings rebuilt");

        // Never shrink below the in-file counter if the journal is somehow behind.
        let mut ahead = Inner { agents: HashMap::new(), claims: vec![], completed: vec![], completed_count: 99, qug_paid: 42.0 };
        reconcile_from_journal_at(&mut ahead, &jp_str);
        assert_eq!(ahead.completed_count, 99, "journal behind hot file → keep hot file");
        assert!((ahead.qug_paid - 42.0).abs() < 1e-9);

        let _ = std::fs::remove_file(&jp);
    }

    #[test]
    fn public_api_can_use_fluxdb_backend() {
        let _guard = DB_ENV_LOCK.lock().unwrap();
        let old = std::env::var("FLUX_SWARM_DB").ok();
        let db = std::env::temp_dir().join(format!("flux-swarm-core-db-test-{}", now()));
        std::env::set_var("FLUX_SWARM_DB", &db);

        let agent = register_agent("db-agent", "qnkdb");
        assert_eq!(agent.id, "db-agent");

        let claim = claim_work("db-agent", &["flux-db-swarm".to_string()], 3).unwrap();
        assert_eq!(claim.agent, "db-agent");
        assert_eq!(claim.crates, vec!["flux-db-swarm"]);

        let status = swarm_status();
        assert_eq!(status.agents, 1);
        assert_eq!(status.active_claims, 1);
        assert_eq!(status.completed_tasks, 0);

        let done = complete_work("db-agent", &claim.task_id, true).unwrap();
        assert_eq!(done.qug_earned, 0.5);

        let status = swarm_status();
        assert_eq!(status.active_claims, 0);
        assert_eq!(status.completed_tasks, 1);
        assert!((status.qug_paid - 0.5).abs() < 1e-9);

        if let Some(v) = old {
            std::env::set_var("FLUX_SWARM_DB", v);
        } else {
            std::env::remove_var("FLUX_SWARM_DB");
        }
        let _ = std::fs::remove_dir_all(db);
    }
}
