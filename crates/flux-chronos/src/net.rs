//! In-memory message bus — the gossipsub replacement at simulation time.
//!
//! Real `flux-p2p` uses libp2p over TCP. flux-chronos plugs an `InMemoryNet`
//! into the same conceptual slot: nodes publish envelopes onto it, the bus
//! resolves recipients + applies per-edge latency + loss + partition, and
//! schedules delivery at the appropriate future tick. The scheduled-delivery
//! queue is what `Universe::advance()` drains.
//!
//! Why an in-memory bus + virtual clock beats real TCP for testing:
//!
//! 1. **Deterministic**: same scenario seed → same delivery order.
//! 2. **Fast**: no syscall overhead, no kernel TCP retransmits.
//! 3. **Injectable**: a scenario can partition / drop / reorder messages by
//!    flipping flags on `NetEdge`. Real TCP would need `tc netem` and Docker.
//! 4. **Observable**: every envelope is logged with sender + recipient +
//!    tick-sent + tick-delivered + delivery-status. A future CHRONOS-G viz
//!    renders this as a swimlane diagram.

use std::collections::BinaryHeap;

use serde::{Deserialize, Serialize};

use crate::TickId;

/// Stable node identifier within a universe. Allocated by
/// [`Universe::spawn_node`](crate::Universe::spawn_node). Stays valid for
/// the lifetime of the universe.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct NodeId(pub u32);

/// A message in flight or just delivered. `payload` is opaque bytes — the
/// chain's wire format owns the encoding. flux-chronos doesn't inspect it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// Who sent it.
    pub from: NodeId,
    /// Who it's destined for. `to == from` is allowed (loopback) but unusual.
    pub to: NodeId,
    /// Tick at which the sender published it.
    pub sent_at: TickId,
    /// Opaque payload — chain decides the wire format.
    pub payload: Vec<u8>,
}

/// Per-edge network properties. Defaults: 1 ms latency, 0% loss, not
/// partitioned. Scenarios mutate these to inject conditions.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NetEdge {
    /// One-way latency in microseconds.
    pub latency_micros: u64,
    /// Drop probability in `[0.0, 1.0)`. 0 = never drop, 0.5 = drop half.
    /// Sampled from the universe's deterministic RNG so reproducible.
    pub drop_prob: f64,
    /// Hard partition flag. If true, ALL messages on this edge are dropped
    /// (independent of `drop_prob`). Toggle to simulate split-brain.
    pub partitioned: bool,
}

impl Default for NetEdge {
    fn default() -> Self {
        Self {
            latency_micros: 1_000, // 1 ms
            drop_prob: 0.0,
            partitioned: false,
        }
    }
}

/// An envelope scheduled to arrive at a future tick. Heap-ordered by
/// `deliver_at` so the bus drains in chronological order regardless of
/// publish order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledDelivery {
    /// Tick at which this envelope becomes visible to the recipient.
    pub deliver_at: TickId,
    /// The envelope itself.
    pub envelope: Envelope,
}

// Heap ordering: earlier `deliver_at` = higher priority. The std `BinaryHeap`
// is a max-heap so we reverse the comparison.
impl Ord for ScheduledDelivery {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.deliver_at.cmp(&self.deliver_at).then_with(|| {
            // Stable tie-break — envelope-from-id then envelope-to-id.
            other.envelope.from.cmp(&self.envelope.from).then_with(|| {
                other.envelope.to.cmp(&self.envelope.to)
            })
        })
    }
}

impl PartialOrd for ScheduledDelivery {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// In-memory bus. Owned by the universe.
pub(crate) struct InMemoryNet {
    /// Pending deliveries keyed by their `deliver_at` tick.
    pub(crate) pending: BinaryHeap<ScheduledDelivery>,
}

impl InMemoryNet {
    pub(crate) fn new() -> Self {
        Self { pending: BinaryHeap::new() }
    }

    /// Schedule `envelope` for delivery according to `edge`'s rules. If the
    /// edge is partitioned or the RNG drops the packet, returns `None` (the
    /// scenario can still observe the drop via the recorder, but it never
    /// reaches the recipient).
    pub(crate) fn schedule(
        &mut self,
        envelope: Envelope,
        edge: &NetEdge,
        rng_roll: f64,
    ) -> Option<TickId> {
        if edge.partitioned {
            return None;
        }
        if edge.drop_prob > 0.0 && rng_roll < edge.drop_prob {
            return None;
        }
        let deliver_at = envelope.sent_at.saturating_add(edge.latency_micros);
        let item = ScheduledDelivery { deliver_at, envelope };
        self.pending.push(item);
        Some(deliver_at)
    }

    /// Drain everything scheduled at or before `current_tick`. Returns the
    /// envelopes in delivery-tick order (then a stable tie-break by sender
    /// then recipient — important so simultaneous deliveries are
    /// deterministic across runs).
    pub(crate) fn drain_up_to(&mut self, current_tick: TickId) -> Vec<Envelope> {
        let mut out = Vec::new();
        while let Some(top) = self.pending.peek() {
            if top.deliver_at > current_tick {
                break;
            }
            let sched = self.pending.pop().expect("peek was Some");
            out.push(sched.envelope);
        }
        out
    }

    /// How many envelopes are still in flight.
    pub(crate) fn in_flight(&self) -> usize {
        self.pending.len()
    }

    /// All pending deliveries (for snapshot serde). Consumed as a Vec;
    /// reconstruct by pushing back into `pending`.
    pub(crate) fn all_pending(&self) -> Vec<ScheduledDelivery> {
        self.pending.clone().into_sorted_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{millis, secs};

    fn env(from: u32, to: u32, sent_at: TickId, payload: &[u8]) -> Envelope {
        Envelope { from: NodeId(from), to: NodeId(to), sent_at, payload: payload.to_vec() }
    }

    #[test]
    fn schedule_default_edge_delivers_after_1ms_latency() {
        let mut net = InMemoryNet::new();
        let when = net.schedule(env(1, 2, 0, b"hi"), &NetEdge::default(), 0.0);
        assert_eq!(when, Some(millis(1))); // 1 ms = 1000 micros
        assert_eq!(net.in_flight(), 1);
    }

    #[test]
    fn partitioned_edge_drops_silently() {
        let mut net = InMemoryNet::new();
        let edge = NetEdge { partitioned: true, ..Default::default() };
        let when = net.schedule(env(1, 2, 0, b"hi"), &edge, 0.0);
        assert_eq!(when, None);
        assert_eq!(net.in_flight(), 0);
    }

    #[test]
    fn drop_prob_zero_never_drops() {
        let mut net = InMemoryNet::new();
        let edge = NetEdge { drop_prob: 0.0, ..Default::default() };
        // Even with the RNG returning 0 (worst-case roll for "should drop"),
        // a zero prob never drops.
        assert!(net.schedule(env(1, 2, 0, b"hi"), &edge, 0.0).is_some());
    }

    #[test]
    fn drop_prob_one_always_drops() {
        let mut net = InMemoryNet::new();
        let edge = NetEdge { drop_prob: 1.0, ..Default::default() };
        // RNG roll of 0.9999 still drops because 0.9999 < 1.0.
        assert!(net.schedule(env(1, 2, 0, b"hi"), &edge, 0.9999).is_none());
    }

    #[test]
    fn drain_returns_envelopes_in_delivery_order_not_publish_order() {
        let mut net = InMemoryNet::new();
        // Publish "late" envelope first with high latency; publish "early"
        // envelope second with low latency. Drain should return early first.
        let high_lat = NetEdge { latency_micros: secs(10), ..Default::default() };
        let low_lat  = NetEdge { latency_micros: millis(5), ..Default::default() };
        net.schedule(env(1, 2, 0, b"late"),  &high_lat, 0.0);
        net.schedule(env(3, 2, 0, b"early"), &low_lat, 0.0);
        let drained = net.drain_up_to(secs(20));
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].payload, b"early");
        assert_eq!(drained[1].payload, b"late");
    }

    #[test]
    fn drain_skips_envelopes_not_yet_due() {
        let mut net = InMemoryNet::new();
        let edge = NetEdge { latency_micros: secs(60), ..Default::default() };
        net.schedule(env(1, 2, 0, b"in_an_hour"), &edge, 0.0);
        let drained = net.drain_up_to(secs(30));
        assert!(drained.is_empty());
        assert_eq!(net.in_flight(), 1);
    }

    #[test]
    fn tie_break_is_deterministic() {
        let mut net = InMemoryNet::new();
        let edge = NetEdge::default(); // 1ms latency
        // Three envelopes scheduled at the same tick from different senders;
        // drain order must depend on sender id, not insertion order.
        net.schedule(env(3, 9, 0, b"c"), &edge, 0.0);
        net.schedule(env(1, 9, 0, b"a"), &edge, 0.0);
        net.schedule(env(2, 9, 0, b"b"), &edge, 0.0);
        let drained = net.drain_up_to(millis(1));
        assert_eq!(drained.iter().map(|e| e.from.0).collect::<Vec<_>>(), vec![1, 2, 3]);
    }
}
