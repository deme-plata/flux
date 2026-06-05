//! Scheduled events. The universe owns a min-heap of `(virtual_micros,
//! node, payload)`. Advancing time pops everything with `t <= deadline` in
//! deterministic order (tie-break by an insertion-order sequence number)
//! and dispatches each to the target node.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::clock::VirtualMicros;
use crate::node::{NodeId, SimEvent};

/// One scheduled event in the universe's queue. `seq` breaks ties between
/// two events at the same virtual time — earlier-scheduled wins. That
/// gives us strict determinism without making node code think about
/// ordering.
#[derive(Debug, Clone)]
pub(crate) struct ScheduledEvent {
    pub at: VirtualMicros,
    pub seq: u64,
    pub target: NodeId,
    pub event: SimEvent,
}

// BinaryHeap is a MAX-heap; we want a min-heap so the earliest event pops
// first. Invert Ord.
impl Ord for ScheduledEvent {
    fn cmp(&self, other: &Self) -> Ordering {
        // Earlier time = "larger" so it bubbles to the top.
        other.at.cmp(&self.at).then(other.seq.cmp(&self.seq))
    }
}
impl PartialOrd for ScheduledEvent { fn partial_cmp(&self, other: &Self) -> Option<Ordering> { Some(self.cmp(other)) } }
impl PartialEq  for ScheduledEvent { fn eq(&self, other: &Self) -> bool { self.at == other.at && self.seq == other.seq } }
impl Eq for ScheduledEvent {}

/// The universe's pending-event queue. Pure data — `Universe` owns the
/// scheduling policy.
#[derive(Debug, Default)]
pub(crate) struct EventQueue {
    heap: BinaryHeap<ScheduledEvent>,
    next_seq: u64,
}

impl EventQueue {
    pub fn new() -> Self { Self::default() }

    /// Schedule an event. Returns the assigned sequence number — useful
    /// for tests that want to assert ordering.
    pub fn push(&mut self, at: VirtualMicros, target: NodeId, event: SimEvent) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.heap.push(ScheduledEvent { at, seq, target, event });
        seq
    }

    /// Pop the earliest event if it's at or before `deadline`. Returns
    /// `None` once the queue is empty or the next event is past `deadline`
    /// — caller advances the clock and stops.
    pub fn pop_if_due(&mut self, deadline: VirtualMicros) -> Option<ScheduledEvent> {
        match self.heap.peek() {
            Some(top) if top.at <= deadline => self.heap.pop(),
            _ => None,
        }
    }

    /// Peek at the next event's virtual time, if any. Used to compute "how
    /// far should I advance the clock before the next dispatch?"
    pub fn next_at(&self) -> Option<VirtualMicros> {
        self.heap.peek().map(|e| e.at)
    }

    /// Outstanding event count. Useful for tests that assert quiescence.
    pub fn len(&self) -> usize { self.heap.len() }
    pub fn is_empty(&self) -> bool { self.heap.is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::SimEvent;

    fn evt(s: &str) -> SimEvent { SimEvent::Message(s.as_bytes().to_vec()) }

    #[test]
    fn min_heap_pops_in_time_order() {
        let mut q = EventQueue::new();
        q.push(50, 0, evt("c"));
        q.push(10, 0, evt("a"));
        q.push(30, 0, evt("b"));
        // Pop everything by t=∞
        let mut times: Vec<u64> = Vec::new();
        while let Some(e) = q.pop_if_due(u64::MAX) { times.push(e.at); }
        assert_eq!(times, vec![10, 30, 50]);
    }

    #[test]
    fn tie_break_uses_insertion_order() {
        let mut q = EventQueue::new();
        let a = q.push(10, 0, evt("a"));
        let b = q.push(10, 0, evt("b"));
        let c = q.push(10, 0, evt("c"));
        let mut order: Vec<u64> = Vec::new();
        while let Some(e) = q.pop_if_due(u64::MAX) { order.push(e.seq); }
        assert_eq!(order, vec![a, b, c]);
    }

    #[test]
    fn pop_if_due_respects_deadline() {
        let mut q = EventQueue::new();
        q.push(100, 0, evt("late"));
        q.push(50, 0, evt("early"));
        // deadline=60 only releases the t=50 event
        let first = q.pop_if_due(60).unwrap();
        assert_eq!(first.at, 50);
        // Next event is still in the queue.
        assert_eq!(q.next_at(), Some(100));
        assert!(q.pop_if_due(60).is_none());
    }
}
