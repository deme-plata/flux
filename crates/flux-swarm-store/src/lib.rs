//! flux-swarm-store — the swarm coordination state, moved off the unbounded
//! `/tmp/flux-swarm*.json[l]` files onto **flux-db** (LSM, LZ4-compressed, TTL,
//! sorted-key range scans, durable WAL).
//!
//! Why: the JSONL logs grow without bound (messages 494 KB, activity 306 KB),
//! live in volatile `/tmp` (lost on reboot — including the QUG settlement
//! ledger), and force a read-modify-write race on every mutation. flux-db gives
//! compaction + TTL (space), `iter_from` (inbox-since as a range scan, not a full
//! file read), atomic writes (no race), and a durable path.
//!
//! [`SwarmStore`] is the abstraction the fluxc-mcp handlers will sit on. Two impls:
//!   * [`JsonStore`] — in-memory, loads the existing JSON/JSONL files (the import
//!     source; also a fast deterministic backing for tests).
//!   * [`FluxDbStore`] — flux-db backed, the destination.
//!
//! The migration is non-destructive: [`import::import`] copies JSON→flux-db and
//! [`import::verify`] proves the money ledger (completed count + Σ QUG) is
//! preserved EXACTLY before anything old is touched.

mod fluxdb_store;
mod json_store;
mod types;
pub mod import;

pub use fluxdb_store::FluxDbStore;
pub use json_store::JsonStore;
pub use types::{Activity, Agent, Claim, Completed, FileClaim, Message};

/// TTL applied to messages + activity (the bounded, recreatable logs). Completed
/// and agents/claims/files are never TTL'd. 30 days.
pub const LOG_TTL_SECONDS: u64 = 30 * 24 * 3600;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("db: {0}")]
    Db(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// The storage interface the swarm handlers depend on. Both the legacy JSON files
/// and flux-db implement it, so the handlers swap backends without changing their
/// tool surface (`register`/`claim`/`complete`/`inbox`/…).
pub trait SwarmStore {
    // ── agents ──
    fn put_agent(&self, a: &Agent) -> Result<(), StoreError>;
    fn get_agent(&self, id: &str) -> Result<Option<Agent>, StoreError>;
    fn list_agents(&self) -> Result<Vec<Agent>, StoreError>;

    // ── claims ──
    fn put_claim(&self, c: &Claim) -> Result<(), StoreError>;
    /// Remove and return a claim (on complete/release). None if absent.
    fn take_claim(&self, task_id: &str) -> Result<Option<Claim>, StoreError>;
    fn list_claims(&self) -> Result<Vec<Claim>, StoreError>;

    // ── completed (the durable QUG ledger) ──
    fn append_completed(&self, c: &Completed) -> Result<(), StoreError>;
    fn completed_count(&self) -> Result<u64, StoreError>;
    /// Σ of `qug_earned` over all completed records — the money invariant.
    fn sum_qug_earned(&self) -> Result<f64, StoreError>;
    fn list_completed(&self) -> Result<Vec<Completed>, StoreError>;

    // ── messages ──
    fn append_message(&self, m: &Message) -> Result<(), StoreError>;
    /// Range scan: every message with `ts_ms >= since_ts_ms`, chronological.
    fn messages_since(&self, since_ts_ms: u64) -> Result<Vec<Message>, StoreError>;
    fn list_messages(&self) -> Result<Vec<Message>, StoreError>;
    fn message_count(&self) -> Result<u64, StoreError>;

    // ── activity ──
    fn append_activity(&self, a: &Activity) -> Result<(), StoreError>;
    /// Last `n` entries, chronological.
    fn activity_tail(&self, n: usize) -> Result<Vec<Activity>, StoreError>;
    fn list_activity(&self) -> Result<Vec<Activity>, StoreError>;
    fn activity_count(&self) -> Result<u64, StoreError>;

    // ── file claims ──
    fn put_file_claim(&self, f: &FileClaim) -> Result<(), StoreError>;
    fn get_file_claim(&self, path: &str) -> Result<Option<FileClaim>, StoreError>;
    fn list_file_claims(&self) -> Result<Vec<FileClaim>, StoreError>;
}
