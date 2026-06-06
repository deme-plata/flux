//! FluxDbStore — swarm state on flux-db. One column family per record kind; keys
//! are chosen so the LSM tree's sorted order does the work:
//!
//!   agents     <agent_id>                    → Agent
//!   claims     <task_id>                      → Claim          (deleted on take)
//!   completed  <seq: u64 BE>                  → Completed       (durable, no TTL)
//!   messages   <ts_ms: u64 BE><id: u64 BE>    → Message         (TTL; since = range scan)
//!   activity   <at: u64 BE><seq: u64 BE>      → Activity        (TTL; seq breaks ties)
//!   files      <path>                         → FileClaim
//!   meta       counter keys                   → u64 BE
//!
//! Big-endian timestamps make lexicographic key order == chronological order, so
//! `messages_since(ts)` is `iter_from(ts_be)` — O(result), not O(whole log).

use std::path::Path;

use flux_db::Database;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::types::{Activity, Agent, Claim, Completed, FileClaim, Message};
use crate::{StoreError, SwarmStore, LOG_TTL_SECONDS};

pub struct FluxDbStore {
    agents: Database,
    claims: Database,
    completed: Database,
    messages: Database,
    activity: Database,
    files: Database,
    meta: Database,
}

fn to_json<T: Serialize>(v: &T) -> Result<Vec<u8>, StoreError> {
    Ok(serde_json::to_vec(v)?)
}
fn from_json<T: DeserializeOwned>(b: &[u8]) -> Result<T, StoreError> {
    Ok(serde_json::from_slice(b)?)
}

/// Strip the TTL header (`[expiry: u64 LE][value]`) from a raw iterated value.
/// `get()` does this automatically, but raw `iter*()` returns the wrapped bytes,
/// so the TTL'd column families (messages, activity) must unwrap on read. Returns
/// None for an expired entry (which iteration then skips).
fn live(stored: &[u8]) -> Option<Vec<u8>> {
    flux_db::ttl::unwrap(stored, flux_db::ttl::now_unix())
}

impl FluxDbStore {
    /// Open (or create) the swarm DB at `path`. Idempotent — re-opening attaches
    /// to the existing column families.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let base = Database::open(path.as_ref().to_path_buf()).map_err(StoreError::Db)?;
        Ok(Self {
            agents: base.create_cf("agents").map_err(StoreError::Db)?,
            claims: base.create_cf("claims").map_err(StoreError::Db)?,
            completed: base.create_cf("completed").map_err(StoreError::Db)?,
            messages: base.create_cf("messages").map_err(StoreError::Db)?,
            activity: base.create_cf("activity").map_err(StoreError::Db)?,
            files: base.create_cf("files").map_err(StoreError::Db)?,
            meta: base.create_cf("meta").map_err(StoreError::Db)?,
        })
    }

    fn read_counter(&self, key: &[u8]) -> Result<u64, StoreError> {
        match self.meta.get(key).map_err(StoreError::Db)? {
            Some(v) if v.len() == 8 => Ok(u64::from_be_bytes(v.try_into().unwrap())),
            _ => Ok(0),
        }
    }
    fn write_counter(&self, key: &[u8], v: u64) -> Result<(), StoreError> {
        self.meta.put(key, &v.to_be_bytes()).map_err(StoreError::Db)
    }
}

impl SwarmStore for FluxDbStore {
    // ── agents ──
    fn put_agent(&self, a: &Agent) -> Result<(), StoreError> {
        self.agents.put(a.id.as_bytes(), &to_json(a)?).map_err(StoreError::Db)
    }
    fn get_agent(&self, id: &str) -> Result<Option<Agent>, StoreError> {
        match self.agents.get(id.as_bytes()).map_err(StoreError::Db)? {
            Some(v) => Ok(Some(from_json(&v)?)),
            None => Ok(None),
        }
    }
    fn list_agents(&self) -> Result<Vec<Agent>, StoreError> {
        let mut out = Vec::new();
        for (_k, v) in self.agents.iter() {
            out.push(from_json::<Agent>(&v)?);
        }
        Ok(out)
    }

    // ── claims ──
    fn put_claim(&self, c: &Claim) -> Result<(), StoreError> {
        self.claims.put(c.task_id.as_bytes(), &to_json(c)?).map_err(StoreError::Db)
    }
    fn take_claim(&self, task_id: &str) -> Result<Option<Claim>, StoreError> {
        match self.claims.get(task_id.as_bytes()).map_err(StoreError::Db)? {
            Some(v) => {
                self.claims.delete(task_id.as_bytes()).map_err(StoreError::Db)?;
                Ok(Some(from_json(&v)?))
            }
            None => Ok(None),
        }
    }
    fn list_claims(&self) -> Result<Vec<Claim>, StoreError> {
        let mut out = Vec::new();
        for (_k, v) in self.claims.iter() {
            out.push(from_json::<Claim>(&v)?);
        }
        Ok(out)
    }

    // ── completed (durable money ledger) ──
    fn append_completed(&self, c: &Completed) -> Result<(), StoreError> {
        let seq = self.read_counter(b"completed_count")?;
        self.completed.put(&seq.to_be_bytes(), &to_json(c)?).map_err(StoreError::Db)?;
        self.write_counter(b"completed_count", seq + 1)
    }
    fn completed_count(&self) -> Result<u64, StoreError> {
        self.read_counter(b"completed_count")
    }
    fn sum_qug_earned(&self) -> Result<f64, StoreError> {
        let mut s = 0.0;
        for (_k, v) in self.completed.iter() {
            s += from_json::<Completed>(&v)?.qug_earned;
        }
        Ok(s)
    }
    fn list_completed(&self) -> Result<Vec<Completed>, StoreError> {
        let mut out = Vec::new();
        for (_k, v) in self.completed.iter() {
            out.push(from_json::<Completed>(&v)?);
        }
        Ok(out)
    }

    // ── messages (TTL'd; since = range scan) ──
    fn append_message(&self, m: &Message) -> Result<(), StoreError> {
        let mut key = m.ts_ms.to_be_bytes().to_vec();
        key.extend_from_slice(&m.id.to_be_bytes());
        self.messages
            .put_ttl_seconds(&key, &to_json(m)?, LOG_TTL_SECONDS)
            .map_err(StoreError::Db)?;
        let n = self.read_counter(b"message_count")?;
        self.write_counter(b"message_count", n + 1)
    }
    fn messages_since(&self, since_ts_ms: u64) -> Result<Vec<Message>, StoreError> {
        let start = since_ts_ms.to_be_bytes();
        let mut out = Vec::new();
        for (_k, v) in self.messages.iter_from(&start) {
            if let Some(raw) = live(&v) {
                out.push(from_json::<Message>(&raw)?);
            }
        }
        Ok(out)
    }
    fn list_messages(&self) -> Result<Vec<Message>, StoreError> {
        let mut out = Vec::new();
        for (_k, v) in self.messages.iter() {
            if let Some(raw) = live(&v) {
                out.push(from_json::<Message>(&raw)?);
            }
        }
        Ok(out)
    }
    fn message_count(&self) -> Result<u64, StoreError> {
        self.read_counter(b"message_count")
    }

    // ── activity (TTL'd; seq tiebreaks same-second entries) ──
    fn append_activity(&self, a: &Activity) -> Result<(), StoreError> {
        let seq = self.read_counter(b"activity_seq")?;
        let mut key = a.at.to_be_bytes().to_vec();
        key.extend_from_slice(&seq.to_be_bytes());
        self.activity
            .put_ttl_seconds(&key, &to_json(a)?, LOG_TTL_SECONDS)
            .map_err(StoreError::Db)?;
        self.write_counter(b"activity_seq", seq + 1)
    }
    fn activity_tail(&self, n: usize) -> Result<Vec<Activity>, StoreError> {
        // newest-first via reverse iter, take n, then flip back to chronological
        let mut newest: Vec<Activity> = Vec::new();
        for (_k, v) in self.activity.iter_rev() {
            if newest.len() >= n {
                break;
            }
            if let Some(raw) = live(&v) {
                newest.push(from_json::<Activity>(&raw)?);
            }
        }
        newest.reverse();
        Ok(newest)
    }
    fn list_activity(&self) -> Result<Vec<Activity>, StoreError> {
        let mut out = Vec::new();
        for (_k, v) in self.activity.iter() {
            if let Some(raw) = live(&v) {
                out.push(from_json::<Activity>(&raw)?);
            }
        }
        Ok(out)
    }
    fn activity_count(&self) -> Result<u64, StoreError> {
        self.read_counter(b"activity_seq")
    }

    // ── file claims ──
    fn put_file_claim(&self, f: &FileClaim) -> Result<(), StoreError> {
        self.files.put(f.path.as_bytes(), &to_json(f)?).map_err(StoreError::Db)
    }
    fn get_file_claim(&self, path: &str) -> Result<Option<FileClaim>, StoreError> {
        match self.files.get(path.as_bytes()).map_err(StoreError::Db)? {
            Some(v) => Ok(Some(from_json(&v)?)),
            None => Ok(None),
        }
    }
    fn list_file_claims(&self) -> Result<Vec<FileClaim>, StoreError> {
        let mut out = Vec::new();
        for (_k, v) in self.files.iter() {
            out.push(from_json::<FileClaim>(&v)?);
        }
        Ok(out)
    }
}
