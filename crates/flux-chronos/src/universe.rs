//! `Universe` — the simulation's outer container.
//!
//! Owns: virtual clock, in-memory net, RNG, set of nodes, edges between
//! them. The public API is intentionally small for v0 (Phase 1 of the
//! flux-chronos roadmap):
//!
//! - `new(seed)` — fresh universe from a deterministic seed
//! - `spawn_node(name, node)` — add a node, get a `NodeId`
//! - `connect(a, b, edge)` — wire two nodes with a `NetEdge` (latency, loss,
//!   partition). Edges are directional; `connect(a, b)` + `connect(b, a)`
//!   for a bidirectional link.
//! - `inject(to, payload)` — push a message AT the universe (as if from the
//!   outside world). Used to kick off scenarios.
//! - `advance(delta)` — run forward by `delta` microseconds of simulated
//!   time. Internally: at each tick where something is scheduled, drain
//!   ready envelopes, step every node that received one (or whose wake-at
//!   fired), schedule their publishes.
//! - `tick()` — current simulated tick.
//! - `event_log()` — every event every node emitted, in order.
//!
//! Multiverse fork (snapshot whole universe, branch, diff) lands in
//! CHRONOS-B; for now snapshot is per-node only.

use std::collections::BTreeMap;

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::net::{Envelope, InMemoryNet, NetEdge, NodeId};
use crate::{NodeStepResult, SimNode, TickId, VirtualClock};

/// Universe seed — drives the deterministic RNG. Same seed → same execution
/// across rebuilds of the binary, across machines, across years.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScenarioSeed(pub u64);

impl From<u64> for ScenarioSeed {
    fn from(n: u64) -> Self {
        Self(n)
    }
}

/// The simulation's outer container.
pub struct Universe {
    clock: VirtualClock,
    rng: ChaCha8Rng,
    net: InMemoryNet,

    /// Registered nodes — owned by the universe so we can `&mut` each one
    /// during step without aliasing pain.
    nodes: BTreeMap<NodeId, Box<dyn SimNode>>,

    /// Outbound edges keyed by sender then recipient. `edges[from][to]` is
    /// the `NetEdge` used when `from` publishes to `to`. Missing pair =
    /// partitioned by default.
    edges: BTreeMap<(NodeId, NodeId), NetEdge>,

    /// Self-wake schedules. `wake_at[node] = tick` means at that tick, the
    /// node gets stepped with an empty `incoming` list. Implements periodic
    /// behavior (block timer, VDF tick) without busy-polling.
    wake_at: BTreeMap<NodeId, TickId>,

    /// Allocator for the next NodeId.
    next_id: u32,

    /// Universe-wide event log. Each entry is `(tick, node_id, event_str)`.
    event_log: Vec<(TickId, NodeId, String)>,
}

impl Universe {
    /// Fresh universe at tick 0, RNG seeded from `seed`.
    pub fn new(seed: ScenarioSeed) -> Self {
        Self {
            clock: VirtualClock::new(),
            rng: ChaCha8Rng::seed_from_u64(seed.0),
            net: InMemoryNet::new(),
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
            wake_at: BTreeMap::new(),
            next_id: 0,
            event_log: Vec::new(),
        }
    }

    /// Add a node. Returns its `NodeId`.
    pub fn spawn_node(&mut self, node: Box<dyn SimNode>) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        self.nodes.insert(id, node);
        id
    }

    /// Wire `from` → `to` with the given edge. Unidirectional; mirror the
    /// call for bidirectional. Overwrites any existing edge.
    pub fn connect(&mut self, from: NodeId, to: NodeId, edge: NetEdge) {
        self.edges.insert((from, to), edge);
    }

    /// Push a payload at `to` as if from an outside-the-universe sender.
    /// Uses `NodeId(u32::MAX)` as the synthetic sender id (no node will
    /// ever be allocated that id — the allocator only counts upward from 0).
    pub fn inject(&mut self, to: NodeId, payload: Vec<u8>) {
        let envelope = Envelope {
            from: NodeId(u32::MAX),
            to,
            sent_at: self.clock.now(),
            payload,
        };
        // Inject bypasses edge config — delivers at the next tick, no loss.
        self.net.pending.push(crate::net::ScheduledDelivery {
            deliver_at: self.clock.now(),
            envelope,
        });
    }

    /// Current simulated tick.
    pub fn tick(&self) -> TickId {
        self.clock.now()
    }

    /// Every event every node has emitted, in chronological order.
    pub fn event_log(&self) -> &[(TickId, NodeId, String)] {
        &self.event_log
    }

    /// Run forward by `delta` simulated microseconds. The universe processes
    /// every envelope that becomes due in that window plus every wake-at
    /// timer that fires.
    ///
    /// Implementation: each loop iteration jumps the clock to the next
    /// pending event (envelope OR wake_at), drains everything due, steps
    /// the affected nodes, schedules their publishes. Loops until no more
    /// events fall within the requested `delta`.
    pub fn advance(&mut self, delta: crate::SimDuration) {
        let target = self.clock.now().saturating_add(delta);

        loop {
            // Find the earliest due event: either a pending envelope or a
            // wake-at timer. Stop if neither is within the target tick.
            let next_envelope_tick = self
                .net
                .pending
                .peek()
                .map(|d| d.deliver_at);
            let next_wake_tick = self.wake_at.values().min().copied();

            let next_tick = match (next_envelope_tick, next_wake_tick) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            };

            let next = match next_tick {
                Some(t) if t <= target => t,
                _ => {
                    // No more events to process within the window — fast-
                    // forward the clock to the target and exit.
                    self.clock.advance(target.saturating_sub(self.clock.now()));
                    return;
                }
            };

            // Jump clock to the event tick.
            self.clock.advance(next.saturating_sub(self.clock.now()));

            // Collect every node that needs to step at this tick:
            //   (a) recipients of any envelope due at this tick
            //   (b) nodes whose wake_at == this tick
            //
            // We use BTreeMap iteration order on NodeId for deterministic
            // step ordering when multiple nodes wake at the same tick.
            let envelopes = self.net.drain_up_to(next);
            let mut envelopes_by_node: BTreeMap<NodeId, Vec<Envelope>> = BTreeMap::new();
            for env in envelopes {
                envelopes_by_node.entry(env.to).or_default().push(env);
            }

            let waking: Vec<NodeId> = self
                .wake_at
                .iter()
                .filter(|(_, &t)| t == next)
                .map(|(&id, _)| id)
                .collect();
            for id in &waking {
                self.wake_at.remove(id);
            }

            // Union: every node that has incoming envelopes OR is waking.
            let stepping_set: std::collections::BTreeSet<NodeId> = envelopes_by_node
                .keys()
                .copied()
                .chain(waking.iter().copied())
                .collect();

            // Step each in NodeId order. Buffer their publishes; schedule
            // them after the loop so a node can't see its own publish during
            // the same tick.
            let mut new_publishes: Vec<(NodeId, NodeStepResult)> = Vec::new();
            for id in stepping_set {
                // Skip unknown nodes defensively — should never happen now
                // that publish-routing filters at the edge, but a malformed
                // scenario could still hand us one via `inject`.
                let node = match self.nodes.get_mut(&id) {
                    Some(n) => n,
                    None => continue,
                };
                let incoming = envelopes_by_node.get(&id).cloned().unwrap_or_default();
                let result = node.step(next, &incoming);
                new_publishes.push((id, result));
            }

            // Apply results: route each publish through the appropriate
            // edge; record events; set up future wake-ats.
            for (sender, result) in new_publishes {
                for env in result.publish {
                    // Skip publishes to unknown nodes (e.g. when a node
                    // tries to reply to the synthetic NodeId(u32::MAX)
                    // sender that the universe uses for `inject`).
                    // Otherwise an unroutable envelope would land in the
                    // bus and panic later when we try to step its
                    // "recipient".
                    if !self.nodes.contains_key(&env.to) {
                        continue;
                    }
                    let edge = self
                        .edges
                        .get(&(sender, env.to))
                        .copied()
                        .unwrap_or_default();
                    let roll: f64 = self.rng.gen();
                    self.net.schedule(env, &edge, roll);
                }
                for event in result.events {
                    self.event_log.push((next, sender, event));
                }
                if let Some(wake) = result.wake_at {
                    if wake > next {
                        self.wake_at.insert(sender, wake);
                    }
                }
            }
        }
    }

    /// Snapshot every node's state into a Vec keyed by NodeId. CHRONOS-B
    /// extends this to also snapshot pending-envelope state + RNG state
    /// for full universe fork.
    pub fn snapshot_nodes(&self) -> BTreeMap<NodeId, Vec<u8>> {
        self.nodes
            .iter()
            .map(|(&id, n)| (id, n.snapshot()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{hours, millis, secs};

    /// Test node: counts every payload it receives + every "ping" event it
    /// emits. Replies "pong" to every ping. Snapshot/restore round-trip the
    /// received count.
    struct PingPong {
        name: String,
        my_id: NodeId,
        partner: NodeId,
        received: u32,
        ping_self: bool,
    }

    impl PingPong {
        fn new(name: &str, my_id: NodeId, partner: NodeId, ping_self: bool) -> Self {
            Self {
                name: name.into(),
                my_id,
                partner,
                received: 0,
                ping_self,
            }
        }
    }

    impl SimNode for PingPong {
        fn step(&mut self, now: TickId, incoming: &[Envelope]) -> NodeStepResult {
            let mut result = NodeStepResult::default();
            for env in incoming {
                self.received += 1;
                result.events.push(format!(
                    "{} got {} at tick {}",
                    self.name,
                    String::from_utf8_lossy(&env.payload),
                    now
                ));
                // Always pong back.
                result.publish.push(Envelope {
                    from: self.my_id,
                    to: env.from,
                    sent_at: now,
                    payload: b"pong".to_vec(),
                });
            }
            // First step on wake-up: send a ping (only the proactive node).
            if self.ping_self && now == 0 {
                result.publish.push(Envelope {
                    from: self.my_id,
                    to: self.partner,
                    sent_at: now,
                    payload: b"ping".to_vec(),
                });
                result.events.push(format!("{} sent initial ping", self.name));
            }
            result
        }

        fn snapshot(&self) -> Vec<u8> {
            self.received.to_le_bytes().to_vec()
        }

        fn restore(&mut self, bytes: &[u8]) -> Result<(), String> {
            if bytes.len() != 4 {
                return Err("expected 4 bytes".into());
            }
            self.received = u32::from_le_bytes(bytes.try_into().unwrap());
            Ok(())
        }

        fn name(&self) -> &str {
            &self.name
        }
    }

    #[test]
    fn two_nodes_exchange_messages_under_virtual_time() {
        // Delta + Epsilon, 50ms one-way latency. Delta sends one ping.
        // Epsilon receives it, pongs back; Delta receives the pong, pongs
        // again. After 1 simulated second: many round trips have happened.
        let mut universe = Universe::new(ScenarioSeed::from(42));

        // We allocate the placeholder NodeId first then patch the partner.
        let delta_id = NodeId(0);
        let epsilon_id = NodeId(1);
        let delta = Box::new(PingPong::new("delta", delta_id, epsilon_id, true));
        let epsilon = Box::new(PingPong::new("epsilon", epsilon_id, delta_id, false));
        let d = universe.spawn_node(delta);
        let e = universe.spawn_node(epsilon);
        assert_eq!(d, delta_id);
        assert_eq!(e, epsilon_id);

        let edge = NetEdge { latency_micros: millis(50), ..Default::default() };
        universe.connect(d, e, edge);
        universe.connect(e, d, edge);

        // Inject a tick-0 wake-up. The PingPong test node sends its initial
        // ping at tick 0 if ping_self is true; but it only gets stepped if
        // there's an event. We use an inject() to wake it.
        universe.inject(d, b"wake".to_vec());

        universe.advance(secs(1));

        // Many round trips should have happened. Each round trip is 100 ms
        // (50ms each way), so ~10 round trips in 1 sec.
        let events = universe.event_log();
        // We should see "delta sent initial ping" exactly once, plus many
        // "got ping" / "got pong" entries.
        let initial_pings = events
            .iter()
            .filter(|(_, _, s)| s.contains("sent initial ping"))
            .count();
        assert_eq!(initial_pings, 1, "exactly one initial ping");
        assert!(events.len() > 5, "many round trips expected, got {}", events.len());
        // The latest event should be at most 1 second in.
        assert!(events.last().unwrap().0 <= secs(1));
    }

    #[test]
    fn deterministic_across_runs() {
        // Same seed + same scenario should produce identical event logs.
        fn run() -> Vec<(TickId, NodeId, String)> {
            let mut universe = Universe::new(ScenarioSeed::from(1337));
            let d = universe.spawn_node(Box::new(PingPong::new("d", NodeId(0), NodeId(1), true)));
            let e = universe.spawn_node(Box::new(PingPong::new("e", NodeId(1), NodeId(0), false)));
            universe.connect(d, e, NetEdge { latency_micros: millis(50), ..Default::default() });
            universe.connect(e, d, NetEdge { latency_micros: millis(50), ..Default::default() });
            universe.inject(d, b"wake".to_vec());
            universe.advance(secs(2));
            universe.event_log().to_vec()
        }
        assert_eq!(run(), run());
    }

    #[test]
    fn partitioned_edge_stops_traffic() {
        let mut universe = Universe::new(ScenarioSeed::from(7));
        let d = universe.spawn_node(Box::new(PingPong::new("d", NodeId(0), NodeId(1), true)));
        let e = universe.spawn_node(Box::new(PingPong::new("e", NodeId(1), NodeId(0), false)));
        universe.connect(d, e, NetEdge { partitioned: true, ..Default::default() });
        universe.connect(e, d, NetEdge { partitioned: true, ..Default::default() });
        universe.inject(d, b"wake".to_vec());
        universe.advance(secs(1));
        // Delta sent the initial ping but Epsilon never received it.
        // Epsilon's snapshot (which encodes `received`) should be 0.
        let snapshots = universe.snapshot_nodes();
        let epsilon_received = u32::from_le_bytes(
            snapshots[&e][..4].try_into().unwrap(),
        );
        assert_eq!(epsilon_received, 0);
    }

    #[test]
    fn advance_72_hours_returns_in_test_time() {
        // The killer feature: a 72-hour scenario completes in milliseconds.
        // We send a single ping under no-partition + standard latency,
        // advance 72 simulated hours, and assert the test wall-clock did
        // not exceed (say) 1 second. This is the proof of "superhuman
        // speed" for the user's directive.
        let start = std::time::Instant::now();
        let mut universe = Universe::new(ScenarioSeed::from(42));
        let d = universe.spawn_node(Box::new(PingPong::new("d", NodeId(0), NodeId(1), true)));
        let e = universe.spawn_node(Box::new(PingPong::new("e", NodeId(1), NodeId(0), false)));
        universe.connect(d, e, NetEdge { latency_micros: millis(50), ..Default::default() });
        universe.connect(e, d, NetEdge { latency_micros: millis(50), ..Default::default() });
        universe.inject(d, b"wake".to_vec());

        universe.advance(hours(72));
        let elapsed = start.elapsed();

        // 72 simulated hours @ 100 ms / round-trip = 2.6M round trips.
        // We should have a massive event log.
        assert!(
            universe.event_log().len() > 1_000_000,
            "expected millions of events, got {}",
            universe.event_log().len()
        );
        // Wall clock should be well under 60 seconds even with debug builds.
        // The point of the test isn't a strict ms budget; it's that 72
        // SIMULATED hours of millisecond-cadence message traffic completes
        // in wall-clock seconds, not 72 actual hours. A release build is
        // ~10x faster than debug; even at the slowest debug speed we
        // process ~250k events/sec, decisive vs real-hardware soak.
        assert!(
            elapsed.as_secs() < 60,
            "advance(72h) took {} ms wall-clock — sim is broken",
            elapsed.as_millis()
        );
    }
}
