//! box_registry — a shared registry of rented compute boxes (Vast.ai etc.).
//!
//! Qwen3's **BoxUsageRegistry** (swarm-flow optimization #2), implemented the
//! FLUXFOOD way: one atomic shared JSON file, no round-trips, the same
//! [`with_locked`](crate::atomic_lock::with_locked) idiom as file-claims.
//!
//! **Why it exists:** today an agent (rocky) destroyed a Vast box that a sibling
//! (rocky-lite) was actively using — twice — because there was no shared record of
//! who rented what. The rule this enforces: **before any teardown, call
//! [`may_destroy`]; if another agent owns the box, don't.** Register on rent,
//! release on (own) teardown.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::atomic_lock::{with_locked, LockError};

/// The registry shared file.
pub const BOX_REGISTRY: &str = "/tmp/flux-swarm-boxes.json";
/// Reuse the swarm-wide lock so box + file claims serialize together.
pub const BOX_LOCK: &str = "/tmp/flux-swarm.lock";

/// One rented box, owned by an agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoxClaim {
    /// Provider instance id (e.g. Vast instance id).
    pub instance_id: String,
    pub agent: String,
    /// Reachable endpoint (e.g. `http://ip:port`), for siblings to share it.
    pub endpoint: String,
    pub label: String,
    pub ts_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BoxRegistry {
    pub boxes: Vec<BoxClaim>,
}

impl BoxRegistry {
    fn load(bytes: &[u8]) -> Self {
        if bytes.is_empty() {
            return Self::default();
        }
        serde_json::from_slice(bytes).unwrap_or_default()
    }
    fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).unwrap_or_default()
    }
}

#[derive(Debug)]
pub enum BoxRegistryError {
    Lock(LockError),
}
impl From<LockError> for BoxRegistryError {
    fn from(e: LockError) -> Self {
        Self::Lock(e)
    }
}
impl std::fmt::Display for BoxRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lock(e) => write!(f, "box-registry lock error: {e:?}"),
        }
    }
}
impl std::error::Error for BoxRegistryError {}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

// ── internal path-parameterized core (so tests don't race the real files) ──

fn register_at(lock: &str, path: &str, claim: BoxClaim) -> Result<(), BoxRegistryError> {
    with_locked(lock, path, |cur| {
        let mut reg = BoxRegistry::load(cur);
        reg.boxes.retain(|b| b.instance_id != claim.instance_id); // idempotent on id
        reg.boxes.push(claim.clone());
        reg.to_bytes()
    })?;
    Ok(())
}

fn owner_at(lock: &str, path: &str, instance_id: &str) -> Result<Option<String>, BoxRegistryError> {
    let mut owner = None;
    with_locked(lock, path, |cur| {
        owner = BoxRegistry::load(cur)
            .boxes
            .iter()
            .find(|b| b.instance_id == instance_id)
            .map(|b| b.agent.clone());
        cur.to_vec() // unchanged
    })?;
    Ok(owner)
}

fn release_at(lock: &str, path: &str, agent: &str, instance_id: &str) -> Result<bool, BoxRegistryError> {
    let mut removed = false;
    with_locked(lock, path, |cur| {
        let mut reg = BoxRegistry::load(cur);
        let before = reg.boxes.len();
        // owner may release; `force` overrides (e.g. operator stop-all).
        reg.boxes.retain(|b| !(b.instance_id == instance_id && (b.agent == agent || agent == "force")));
        removed = reg.boxes.len() != before;
        reg.to_bytes()
    })?;
    Ok(removed)
}

fn list_at(lock: &str, path: &str) -> Result<Vec<BoxClaim>, BoxRegistryError> {
    let mut out = Vec::new();
    with_locked(lock, path, |cur| {
        out = BoxRegistry::load(cur).boxes;
        cur.to_vec()
    })?;
    Ok(out)
}

// ── public API (real shared files) ──

/// Register a rented box (call on rent). Idempotent per `instance_id`.
pub fn register_box(agent: &str, instance_id: &str, endpoint: &str, label: &str) -> Result<BoxClaim, BoxRegistryError> {
    let claim = BoxClaim {
        instance_id: instance_id.to_string(),
        agent: agent.to_string(),
        endpoint: endpoint.to_string(),
        label: label.to_string(),
        ts_ms: now_ms(),
    };
    register_at(BOX_LOCK, BOX_REGISTRY, claim.clone())?;
    Ok(claim)
}

/// Who owns this box? `None` = unregistered.
pub fn owner_of(instance_id: &str) -> Result<Option<String>, BoxRegistryError> {
    owner_at(BOX_LOCK, BOX_REGISTRY, instance_id)
}

/// **The teardown guard.** `true` iff `agent` may destroy `instance_id` (owns it,
/// or it's unregistered). `false` iff another agent owns it — DON'T destroy.
pub fn may_destroy(agent: &str, instance_id: &str) -> Result<bool, BoxRegistryError> {
    Ok(match owner_of(instance_id)? {
        Some(o) => o == agent,
        None => true,
    })
}

/// Release a box (call after your own teardown). `agent="force"` overrides (operator).
pub fn release_box(agent: &str, instance_id: &str) -> Result<bool, BoxRegistryError> {
    release_at(BOX_LOCK, BOX_REGISTRY, agent, instance_id)
}

/// List all registered boxes (for `flux_swarm_box_list`).
pub fn list_boxes() -> Result<Vec<BoxClaim>, BoxRegistryError> {
    list_at(BOX_LOCK, BOX_REGISTRY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fresh(name: &str) -> (String, String) {
        let d = std::env::temp_dir();
        (
            d.join(format!("boxreg-{name}.lock")).to_string_lossy().into_owned(),
            d.join(format!("boxreg-{name}.json")).to_string_lossy().into_owned(),
        )
    }

    #[test]
    fn register_then_owner_and_list() {
        let (lock, path) = fresh("reg");
        let _ = std::fs::remove_file(&path);
        register_at(&lock, &path, BoxClaim { instance_id: "i1".into(), agent: "rocky".into(), endpoint: "http://x:1".into(), label: "a100".into(), ts_ms: 1 }).unwrap();
        assert_eq!(owner_at(&lock, &path, "i1").unwrap().as_deref(), Some("rocky"));
        assert_eq!(list_at(&lock, &path).unwrap().len(), 1);
        let _ = std::fs::remove_file(&PathBuf::from(&path));
    }

    #[test]
    fn teardown_guard_blocks_other_agents() {
        let (lock, path) = fresh("guard");
        let _ = std::fs::remove_file(&path);
        register_at(&lock, &path, BoxClaim { instance_id: "box9".into(), agent: "rocky-lite".into(), endpoint: "http://x:9".into(), label: "r1".into(), ts_ms: 1 }).unwrap();
        // a DIFFERENT agent must NOT be allowed to destroy rocky-lite's box.
        let owner = owner_at(&lock, &path, "box9").unwrap();
        assert_eq!(owner.as_deref(), Some("rocky-lite"));
        let may = match owner { Some(o) => o == "rocky", None => true };
        assert!(!may, "rocky must NOT be cleared to destroy rocky-lite's box (the bug we hit)");
        // the owner CAN.
        let may_owner = owner_at(&lock, &path, "box9").unwrap().map(|o| o == "rocky-lite").unwrap_or(true);
        assert!(may_owner);
        let _ = std::fs::remove_file(&PathBuf::from(&path));
    }

    #[test]
    fn release_only_by_owner_or_force() {
        let (lock, path) = fresh("rel");
        let _ = std::fs::remove_file(&path);
        register_at(&lock, &path, BoxClaim { instance_id: "i2".into(), agent: "rocky".into(), endpoint: "e".into(), label: "l".into(), ts_ms: 1 }).unwrap();
        assert!(!release_at(&lock, &path, "someone-else", "i2").unwrap(), "non-owner can't release");
        assert!(release_at(&lock, &path, "rocky", "i2").unwrap(), "owner releases");
        assert!(list_at(&lock, &path).unwrap().is_empty());
        let _ = std::fs::remove_file(&PathBuf::from(&path));
    }

    #[test]
    fn register_is_idempotent_on_instance_id() {
        let (lock, path) = fresh("idem");
        let _ = std::fs::remove_file(&path);
        register_at(&lock, &path, BoxClaim { instance_id: "i3".into(), agent: "a".into(), endpoint: "e1".into(), label: "l".into(), ts_ms: 1 }).unwrap();
        register_at(&lock, &path, BoxClaim { instance_id: "i3".into(), agent: "b".into(), endpoint: "e2".into(), label: "l".into(), ts_ms: 2 }).unwrap();
        let boxes = list_at(&lock, &path).unwrap();
        assert_eq!(boxes.len(), 1, "same instance_id updates, not duplicates");
        assert_eq!(boxes[0].agent, "b");
        let _ = std::fs::remove_file(&PathBuf::from(&path));
    }
}
