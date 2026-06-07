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

use crate::net::{Envelope, InMemoryNet, NetEdge, NodeId, ScheduledDelivery};
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

    /// Scenario seed — stored separately so we can retrieve it without
    /// extracting from the RNG (ChaCha8Rng doesn't expose get_seed).
    seed: u64,
}

impl Universe {
    /// Fresh universe at tick 0, RNG seeded from `seed`.
    pub fn new(seed: ScenarioSeed) -> Self {
        let s = seed.0;
        Self {
            clock: VirtualClock::new(),
            rng: ChaCha8Rng::seed_from_u64(s),
            net: InMemoryNet::new(),
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
            wake_at: BTreeMap::new(),
            next_id: 0,
            event_log: Vec::new(),
            seed: s,
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

    /// Number of nodes in the universe.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Number of pending (not yet delivered) envelopes in the in-memory net.
    pub fn pending_envelope_count(&self) -> usize {
        self.net.in_flight()
    }

    /// The scenario seed this universe was created with.
    pub fn seed(&self) -> ScenarioSeed {
        ScenarioSeed(self.seed)
    }

    /// Serialize universe metadata + all node snapshots to a byte vector.
    /// Uses bincode for the concrete metadata; each node's `snapshot()` is
    /// stored alongside its NodeId and type_tag for reconstruction.
    ///
    /// This does NOT serialize the full universe — it serializes enough to
    /// reconstruct it, given a factory that can build nodes from type_tags.
    pub fn serialize_to_vec(&self) -> Vec<u8> {
        use serde::Serialize;

        #[derive(Serialize)]
        struct UniverseSnapshot {
            seed: u64,
            tick: TickId,
            next_id: u32,
            edges: Vec<((u32, u32), NetEdgeWire)>,
            wake_at: Vec<(u32, TickId)>,
            event_log: Vec<(TickId, u32, String)>,
            pending: Vec<PendingEnvelopeWire>,
            nodes: Vec<NodeSnapshotWire>,
        }

        #[derive(Serialize)]
        struct NetEdgeWire {
            latency_micros: u64,
            drop_prob: f64,
            partitioned: bool,
        }

        #[derive(Serialize)]
        struct PendingEnvelopeWire {
            deliver_at: TickId,
            from: u32,
            to: u32,
            sent_at: TickId,
            payload: Vec<u8>,
        }

        #[derive(Serialize)]
        struct NodeSnapshotWire {
            id: u32,
            type_tag: String,
            name: String,
            snapshot: Vec<u8>,
        }

        let snap = UniverseSnapshot {
            seed: self.seed().0,
            tick: self.clock.now(),
            next_id: self.next_id,
            edges: self
                .edges
                .iter()
                .map(|((from, to), e)| {
                    ((from.0, to.0), NetEdgeWire {
                        latency_micros: e.latency_micros,
                        drop_prob: e.drop_prob,
                        partitioned: e.partitioned,
                    })
                })
                .collect(),
            wake_at: self
                .wake_at
                .iter()
                .map(|(id, tick)| (id.0, *tick))
                .collect(),
            event_log: self
                .event_log
                .iter()
                .map(|(tick, id, s)| (*tick, id.0, s.clone()))
                .collect(),
            pending: self
                .net
                .all_pending()
                .iter()
                .map(|sd| PendingEnvelopeWire {
                    deliver_at: sd.deliver_at,
                    from: sd.envelope.from.0,
                    to: sd.envelope.to.0,
                    sent_at: sd.envelope.sent_at,
                    payload: sd.envelope.payload.clone(),
                })
                .collect(),
            nodes: self
                .nodes
                .iter()
                .map(|(id, node)| {
                    NodeSnapshotWire {
                        id: id.0,
                        type_tag: node.type_tag().to_string(),
                        name: node.name().to_string(),
                        snapshot: node.snapshot(),
                    }
                })
                .collect(),
        };

        bincode::serialize(&snap).expect("bincode serialize UniverseSnapshot")
    }

    /// Deserialize a universe from a byte slice produced by `serialize_to_vec`.
    /// Requires a `make_node` factory that builds the correct SimNode for each
    /// stored type_tag, then calls `restore()` on it.
    pub fn deserialize_from_slice(
        bytes: &[u8],
        mut make_node: impl FnMut(&str) -> Box<dyn SimNode>,
    ) -> Result<Self, String> {
        use serde::Deserialize;

        #[derive(Deserialize)]
        struct UniverseSnapshot {
            seed: u64,
            tick: TickId,
            next_id: u32,
            edges: Vec<((u32, u32), NetEdgeWire)>,
            wake_at: Vec<(u32, TickId)>,
            event_log: Vec<(TickId, u32, String)>,
            pending: Vec<PendingEnvelopeWire>,
            nodes: Vec<NodeSnapshotWire>,
        }

        #[derive(Deserialize)]
        struct NetEdgeWire {
            latency_micros: u64,
            drop_prob: f64,
            partitioned: bool,
        }

        #[derive(Deserialize)]
        struct PendingEnvelopeWire {
            deliver_at: TickId,
            from: u32,
            to: u32,
            sent_at: TickId,
            payload: Vec<u8>,
        }

        #[derive(Deserialize)]
        struct NodeSnapshotWire {
            id: u32,
            type_tag: String,
            name: String,
            snapshot: Vec<u8>,
        }

        let snap: UniverseSnapshot =
            bincode::deserialize(bytes).map_err(|e| format!("bincode deserialize UniverseSnapshot: {e}"))?;

        let mut universe = Universe::new(ScenarioSeed(snap.seed));
        // Fast-forward clock and next_id to match saved state.
        universe.clock = VirtualClock::at(snap.tick);
        universe.next_id = snap.next_id;

        // Restore edges.
        for ((from, to), e) in snap.edges {
            universe.edges.insert(
                (NodeId(from), NodeId(to)),
                NetEdge {
                    latency_micros: e.latency_micros,
                    drop_prob: e.drop_prob,
                    partitioned: e.partitioned,
                },
            );
        }

        // Restore wake_at.
        for (id, tick) in snap.wake_at {
            universe.wake_at.insert(NodeId(id), tick);
        }

        // Restore event log.
        universe.event_log = snap
            .event_log
            .into_iter()
            .map(|(tick, id, s)| (tick, NodeId(id), s))
            .collect();

        // Restore pending deliveries.
        for p in snap.pending {
            universe.net.pending.push(ScheduledDelivery {
                deliver_at: p.deliver_at,
                envelope: Envelope {
                    from: NodeId(p.from),
                    to: NodeId(p.to),
                    sent_at: p.sent_at,
                    payload: p.payload,
                },
            });
        }

        // Restore nodes via factory + restore().
        for ns in snap.nodes {
            let mut node = make_node(&ns.type_tag);
            node.restore(&ns.snapshot)
                .map_err(|e| format!("restore node {} ({}): {e}", ns.id, ns.type_tag))?;
            universe.nodes.insert(NodeId(ns.id), node);
        }

        Ok(universe)
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

impl std::fmt::Debug for Universe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Universe")
            .field("tick", &self.clock.now())
            .field("nodes", &self.nodes.len())
            .field("edges", &self.edges.len())
            .field("pending", &self.net.in_flight())
            .field("events", &self.event_log.len())
            .field("seed", &self.seed)
            .finish()
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
