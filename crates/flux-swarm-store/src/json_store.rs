//! JsonStore — in-memory swarm state. Two jobs:
//!   1. The import SOURCE: [`JsonStore::load_dir`] parses the existing
//!      `/tmp/flux-swarm*.json[l]` files.
//!   2. A deterministic in-memory backing for tests (build a corpus via the
//!      `SwarmStore` write methods, no filesystem).

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::RwLock;

use serde::Deserialize;

use crate::types::{Activity, Agent, Claim, Completed, FileClaim, Message};
use crate::{StoreError, SwarmStore};

#[derive(Default)]
struct Inner {
    agents: Vec<Agent>,
    claims: Vec<Claim>,
    completed: Vec<Completed>,
    messages: Vec<Message>,
    activity: Vec<Activity>,
    files: Vec<FileClaim>,
    parse_errors: usize,
}

/// In-memory swarm state (RwLock for interior mutability so it satisfies the
/// `&self` `SwarmStore` interface).
#[derive(Default)]
pub struct JsonStore {
    inner: RwLock<Inner>,
}

// ── the on-disk wrappers (hot state + files registry) ──
#[derive(Deserialize, Default)]
struct HotState {
    #[serde(default)]
    agents: HashMap<String, Agent>,
    #[serde(default)]
    claims: Vec<Claim>,
}
#[derive(Deserialize, Default)]
struct FilesWrap {
    #[serde(default)]
    claims: HashMap<String, FileClaim>,
}

impl JsonStore {
    pub fn new_empty() -> Self {
        Self::default()
    }

    /// Count of JSONL lines that failed to parse during `load_dir` (should be 0
    /// for a clean migration; surfaced so the importer can report it).
    pub fn parse_errors(&self) -> usize {
        self.inner.read().unwrap().parse_errors
    }

    /// Load the existing swarm files from `dir` (the live `/tmp`). Missing files
    /// are treated as empty — only the present ones are imported.
    pub fn load_dir(dir: &Path) -> Result<Self, StoreError> {
        let s = Self::new_empty();

        // hot state: agents map + active claims
        if let Ok(txt) = fs::read_to_string(dir.join("flux-swarm.json")) {
            let hot: HotState = serde_json::from_str(&txt)?;
            for a in hot.agents.into_values() {
                s.put_agent(&a)?;
            }
            for c in hot.claims {
                s.put_claim(&c)?;
            }
        }

        // append-only logs (one JSON object per line)
        s.load_jsonl::<Completed>(&dir.join("flux-swarm-completed.jsonl"), |s, c| {
            s.inner.write().unwrap().completed.push(c)
        })?;
        s.load_jsonl::<Message>(&dir.join("flux-swarm-messages.jsonl"), |s, m| {
            s.inner.write().unwrap().messages.push(m)
        })?;
        s.load_jsonl::<Activity>(&dir.join("flux-swarm-activity.jsonl"), |s, a| {
            s.inner.write().unwrap().activity.push(a)
        })?;

        // file leases
        if let Ok(txt) = fs::read_to_string(dir.join("flux-swarm-files.json")) {
            let fw: FilesWrap = serde_json::from_str(&txt)?;
            for f in fw.claims.into_values() {
                s.put_file_claim(&f)?;
            }
        }

        Ok(s)
    }

    fn load_jsonl<T: for<'de> Deserialize<'de>>(
        &self,
        path: &Path,
        push: impl Fn(&Self, T),
    ) -> Result<(), StoreError> {
        let txt = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => return Ok(()), // absent file = empty
        };
        for line in txt.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<T>(line) {
                Ok(v) => push(self, v),
                Err(_) => self.inner.write().unwrap().parse_errors += 1,
            }
        }
        Ok(())
    }
}

impl SwarmStore for JsonStore {
    fn put_agent(&self, a: &Agent) -> Result<(), StoreError> {
        let mut g = self.inner.write().unwrap();
        if let Some(slot) = g.agents.iter_mut().find(|x| x.id == a.id) {
            *slot = a.clone();
        } else {
            g.agents.push(a.clone());
        }
        Ok(())
    }
    fn get_agent(&self, id: &str) -> Result<Option<Agent>, StoreError> {
        Ok(self.inner.read().unwrap().agents.iter().find(|x| x.id == id).cloned())
    }
    fn list_agents(&self) -> Result<Vec<Agent>, StoreError> {
        Ok(self.inner.read().unwrap().agents.clone())
    }

    fn put_claim(&self, c: &Claim) -> Result<(), StoreError> {
        let mut g = self.inner.write().unwrap();
        if let Some(slot) = g.claims.iter_mut().find(|x| x.task_id == c.task_id) {
            *slot = c.clone();
        } else {
            g.claims.push(c.clone());
        }
        Ok(())
    }
    fn take_claim(&self, task_id: &str) -> Result<Option<Claim>, StoreError> {
        let mut g = self.inner.write().unwrap();
        if let Some(pos) = g.claims.iter().position(|x| x.task_id == task_id) {
            Ok(Some(g.claims.remove(pos)))
        } else {
            Ok(None)
        }
    }
    fn list_claims(&self) -> Result<Vec<Claim>, StoreError> {
        Ok(self.inner.read().unwrap().claims.clone())
    }

    fn append_completed(&self, c: &Completed) -> Result<(), StoreError> {
        self.inner.write().unwrap().completed.push(c.clone());
        Ok(())
    }
    fn completed_count(&self) -> Result<u64, StoreError> {
        Ok(self.inner.read().unwrap().completed.len() as u64)
    }
    fn sum_qug_earned(&self) -> Result<f64, StoreError> {
        Ok(self.inner.read().unwrap().completed.iter().map(|c| c.qug_earned).sum())
    }
    fn list_completed(&self) -> Result<Vec<Completed>, StoreError> {
        Ok(self.inner.read().unwrap().completed.clone())
    }

    fn append_message(&self, m: &Message) -> Result<(), StoreError> {
        self.inner.write().unwrap().messages.push(m.clone());
        Ok(())
    }
    fn messages_since(&self, since_ts_ms: u64) -> Result<Vec<Message>, StoreError> {
        let mut v: Vec<Message> = self
            .inner
            .read()
            .unwrap()
            .messages
            .iter()
            .filter(|m| m.ts_ms >= since_ts_ms)
            .cloned()
            .collect();
        v.sort_by_key(|m| (m.ts_ms, m.id));
        Ok(v)
    }
    fn list_messages(&self) -> Result<Vec<Message>, StoreError> {
        Ok(self.inner.read().unwrap().messages.clone())
    }
    fn message_count(&self) -> Result<u64, StoreError> {
        Ok(self.inner.read().unwrap().messages.len() as u64)
    }

    fn append_activity(&self, a: &Activity) -> Result<(), StoreError> {
        self.inner.write().unwrap().activity.push(a.clone());
        Ok(())
    }
    fn activity_tail(&self, n: usize) -> Result<Vec<Activity>, StoreError> {
        let g = self.inner.read().unwrap();
        let len = g.activity.len();
        let start = len.saturating_sub(n);
        Ok(g.activity[start..].to_vec())
    }
    fn list_activity(&self) -> Result<Vec<Activity>, StoreError> {
        Ok(self.inner.read().unwrap().activity.clone())
    }
    fn activity_count(&self) -> Result<u64, StoreError> {
        Ok(self.inner.read().unwrap().activity.len() as u64)
    }

    fn put_file_claim(&self, f: &FileClaim) -> Result<(), StoreError> {
        let mut g = self.inner.write().unwrap();
        if let Some(slot) = g.files.iter_mut().find(|x| x.path == f.path) {
            *slot = f.clone();
        } else {
            g.files.push(f.clone());
        }
        Ok(())
    }
    fn get_file_claim(&self, path: &str) -> Result<Option<FileClaim>, StoreError> {
        Ok(self.inner.read().unwrap().files.iter().find(|x| x.path == path).cloned())
    }
    fn list_file_claims(&self) -> Result<Vec<FileClaim>, StoreError> {
        Ok(self.inner.read().unwrap().files.clone())
    }
}
