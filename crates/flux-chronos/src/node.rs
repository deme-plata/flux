//! `SimNode` — the trait a chain crate implements to plug into chronos.
//!
//! The contract is intentionally narrow:
//!
//! 1. **`step`** receives every envelope addressed to this node since the
//!    last step plus the current tick. It returns the envelopes it wants to
//!    publish + any local state events worth recording (for replay /
//!    multiverse diff).
//! 2. **`snapshot`** serializes the node's full state into bytes. Used by
//!    CHRONOS-B fork/branch. v0 chains can ship this as `bincode::serialize`.
//! 3. **`restore`** is the inverse — install state from bytes. Required for
//!    multiverse fork.
//!
//! Crucially: the node has NO access to wall-clock time, NO access to a
//! non-seeded RNG, NO access to the filesystem. Every external dependency
//! comes through the trait or the envelopes. This is what makes the
//! simulation deterministic.

use crate::{Envelope, TickId};

/// What a node's `step` returns to the universe.
#[derive(Debug, Default, Clone)]
pub struct NodeStepResult {
    /// Envelopes to publish — the universe routes each via the configured
    /// edge to its recipient.
    pub publish: Vec<Envelope>,
    /// Free-form event log entries — a future CHRONOS-G renders these as
    /// per-node swimlane annotations. Phase 0 stores them verbatim.
    pub events: Vec<String>,
    /// If `Some`, schedule a self-tick at this future tick. Lets a node
    /// implement periodic behavior (block timer, VDF tick, heartbeat)
    /// without polling. Universe drains scheduled self-ticks like envelopes.
    pub wake_at: Option<TickId>,
}

/// Implement this for any chain-side node you want to simulate. SIGIL's
/// `sigil-node` will get an adapter in CHRONOS-E that wires
/// `Block`/`Transaction` plumbing into this trait.
pub trait SimNode {
    /// Single deterministic step. `now` is the current universe tick;
    /// `incoming` is every envelope addressed to this node that became
    /// visible during this tick window.
    fn step(&mut self, now: TickId, incoming: &[Envelope]) -> NodeStepResult;

    /// Serialize the node's full state. Used for multiverse fork — a
    /// universe with N nodes at tick T snapshots into a `Vec<Vec<u8>>` that
    /// any number of forked universes can restore from.
    fn snapshot(&self) -> Vec<u8>;

    /// Inverse of snapshot. Restore the node's state from bytes.
    fn restore(&mut self, bytes: &[u8]) -> Result<(), String>;

    /// Free-form name for logs / viz. Doesn't need to be unique (universe
    /// uses [`NodeId`](crate::NodeId) for identity) — humans use it.
    fn name(&self) -> &str;

    /// Type tag for snapshot serde. The default returns `std::any::type_name`
    /// but implementors should override with a stable short string so that
    /// snapshots remain loadable across refactors. Used by
    /// [`crate::snapshot::SnapshotArchive`] to know which concrete type to
    /// reconstruct on deserialization.
    fn type_tag(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal SimNode for tests: just echoes every incoming envelope
    /// back to its sender. Useful as a unit-test smoke node — proves the
    /// trait shape composes without dragging in chain dependencies.
    struct Echo {
        name: String,
        my_id: crate::NodeId,
    }

    impl SimNode for Echo {
        fn step(&mut self, now: TickId, incoming: &[Envelope]) -> NodeStepResult {
            let publish = incoming
                .iter()
                .map(|e| Envelope {
                    from: self.my_id,
                    to: e.from,
                    sent_at: now,
                    payload: e.payload.clone(),
                })
                .collect();
            NodeStepResult { publish, events: vec![], wake_at: None }
        }

        fn snapshot(&self) -> Vec<u8> {
            self.name.as_bytes().to_vec()
        }

        fn restore(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.name = String::from_utf8(bytes.to_vec()).map_err(|e| e.to_string())?;
            Ok(())
        }

        fn name(&self) -> &str {
            &self.name
        }
    }

    #[test]
    fn echo_node_replies_to_every_incoming() {
        let mut node = Echo { name: "echo".into(), my_id: crate::NodeId(1) };
        let incoming = vec![
            Envelope { from: crate::NodeId(2), to: crate::NodeId(1), sent_at: 0, payload: b"a".to_vec() },
            Envelope { from: crate::NodeId(3), to: crate::NodeId(1), sent_at: 0, payload: b"b".to_vec() },
        ];
        let result = node.step(100, &incoming);
        assert_eq!(result.publish.len(), 2);
        assert_eq!(result.publish[0].to, crate::NodeId(2));
        assert_eq!(result.publish[0].sent_at, 100);
        assert_eq!(result.publish[0].payload, b"a");
    }

    #[test]
    fn snapshot_restore_roundtrips() {
        let mut a = Echo { name: "alpha".into(), my_id: crate::NodeId(1) };
        let bytes = a.snapshot();
        let mut b = Echo { name: "PLACEHOLDER".into(), my_id: crate::NodeId(1) };
        b.restore(&bytes).unwrap();
        assert_eq!(b.name(), "alpha");
    }
}
