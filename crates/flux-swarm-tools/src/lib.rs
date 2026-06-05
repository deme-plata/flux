//! Swarm coordination extensions for Flux.
//!
//! Sits next to `fluxc-core::swarm` rather than inside it (rocky's claim
//! covers `fluxc-core` at the time this crate was added). Adds three
//! features the existing swarm doesn't have:
//!
//! 1. **Cross-process atomic save** (`atomic_lock` module). A `LockedFile`
//!    guard holds a POSIX-style sentinel-file mutex across processes; the
//!    `with_locked` helper does read-modify-write inside the guard so
//!    parallel MCP processes can't lose each other's writes the way
//!    `fluxc-core::swarm::save` does today. Cleans up on Drop, with
//!    stale-lock detection if a holder crashes.
//!
//! 2. **File-level claims** (`file_claims` module). The base swarm is
//!    crate-grained — every agent editing flux-db gets blocked even if
//!    they're touching disjoint files. `FileClaimStore` lives in its own
//!    JSON file (`/tmp/flux-swarm-files.json`) and grants exclusive
//!    leases on individual paths. Composes with crate-level claims:
//!    callers typically take both.
//!
//! 3. **Append-only activity log** (`activity` module). Every swarm
//!    transition appended as one JSON object per line to
//!    `/tmp/flux-swarm-activity.jsonl`. Append is single-writer per
//!    line, so the log survives concurrent writers and gives a real
//!    audit trail for "what did each agent do, when, and which task
//!    settled how much QUG?"

pub mod activity;
pub mod atomic_lock;
pub mod box_registry;
pub mod file_claims;
pub mod messages;

pub use activity::{Activity, ActivityKind, ActivityLog};
pub use atomic_lock::{with_locked, LockedFile, LockError};
pub use box_registry::{register_box, release_box, owner_of, may_destroy, list_boxes, BoxClaim, BoxRegistry, BoxRegistryError};
pub use file_claims::{FileClaim, FileClaimError, FileClaimStore};
pub use messages::{inbox, list_filtered, send, MessageError, SwarmMessage, MESSAGES_LOG};

/// Default on-disk locations for swarm-tools state. Match `fluxc-core::swarm`'s
/// `/tmp/flux-swarm.json` convention so it's all in one place.
pub mod paths {
    pub const FILE_CLAIMS: &str = "/tmp/flux-swarm-files.json";
    pub const ACTIVITY_LOG: &str = "/tmp/flux-swarm-activity.jsonl";
    pub const SWARM_LOCK: &str = "/tmp/flux-swarm.lock";
    pub const BOX_REGISTRY: &str = "/tmp/flux-swarm-boxes.json";
}
