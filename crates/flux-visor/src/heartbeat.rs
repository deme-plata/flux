//! Worker capacity-heartbeat daemon (FluxVisor board T2).
//!
//! A worker host periodically announces two things to the control plane:
//!
//! - **liveness** on `/fluxvisor/1/host-heartbeat` ([`HostHeartbeat`]),
//! - **capacity** on `/fluxvisor/1/capacity` ([`CapacityHeartbeat`]) — its
//!   sellable / used / remaining [`ResourceSet`]s straight from the
//!   [`CapacityLedger`], so the seed can route provisioning to a host with room.
//!
//! This module ships the **pure** half: [`HeartbeatDaemon::tick`] reads the
//! ledger and returns the messages to publish. It advances a monotonic `seq` and
//! takes `uptime_secs` as an argument — no wall clock, no `Instant`, so a tick is
//! deterministic and testable offline.
//!
//! Publishing is a separate, injected boundary: a [`HeartbeatSink`]. The only
//! sink shipped here is [`RecordingSink`], which touches no network. A real sink
//! wrapping a `flux-p2p` gossipsub handle is intentionally **not** in this crate
//! — bringing the transport up (`NetworkManager::start()`) is the operator-gated
//! step T2 must not take. So this daemon can render exactly what it would gossip
//! without starting anything.
//!
//! The topics [`HeartbeatDaemon`] publishes on are a subset of
//! [`crate::FLUXVISOR_P2P_TOPICS`], and a worker built from
//! [`crate::FluxP2pCluster::network_config_for`] is subscribed to them — proven
//! by `heartbeat_topics_present_in_generated_network_config`.

use crate::{CapacityLedger, FluxVisorError, HostRole, P2pHostNode, ResourceSet};
use serde::{Deserialize, Serialize};

/// The gossipsub topic a heartbeat is published on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeartbeatTopic {
    /// Liveness — `/fluxvisor/1/host-heartbeat`.
    Host,
    /// Capacity — `/fluxvisor/1/capacity`.
    Capacity,
}

impl HeartbeatTopic {
    /// The exact gossipsub topic string. These are members of
    /// [`crate::FLUXVISOR_P2P_TOPICS`].
    pub fn topic_str(&self) -> &'static str {
        match self {
            HeartbeatTopic::Host => "/fluxvisor/1/host-heartbeat",
            HeartbeatTopic::Capacity => "/fluxvisor/1/capacity",
        }
    }
}

/// Liveness announcement for a worker host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostHeartbeat {
    /// Stable host id.
    pub host_id: String,
    /// Host role in the fleet.
    pub role: HostRole,
    /// Monotonic sequence number (increments once per daemon tick).
    pub seq: u64,
    /// Seconds the worker has been up, supplied by the caller.
    pub uptime_secs: u64,
    /// Whether the worker considers itself healthy enough to accept work.
    pub healthy: bool,
}

/// Capacity announcement for a worker host, derived from its [`CapacityLedger`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapacityHeartbeat {
    /// Stable host id.
    pub host_id: String,
    /// Monotonic sequence number, shared with the tick's [`HostHeartbeat`].
    pub seq: u64,
    /// Sellable capacity after the overcommit policy.
    pub total: ResourceSet,
    /// Currently reserved resources.
    pub used: ResourceSet,
    /// Remaining sellable resources.
    pub remaining: ResourceSet,
    /// Number of active reservations.
    pub reservations: u32,
}

/// A typed heartbeat payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeartbeatPayload {
    /// Liveness payload.
    Host(HostHeartbeat),
    /// Capacity payload.
    Capacity(CapacityHeartbeat),
}

/// A heartbeat ready to publish: the topic plus its typed payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeartbeatMessage {
    /// Topic to publish on.
    pub topic: HeartbeatTopic,
    /// Typed payload.
    pub payload: HeartbeatPayload,
}

impl HeartbeatMessage {
    /// The gossipsub topic string this message publishes on.
    pub fn topic_str(&self) -> &'static str {
        self.topic.topic_str()
    }

    /// Serialize the payload to the JSON bytes a worker would gossip.
    pub fn to_json(&self) -> Result<String, FluxVisorError> {
        serde_json::to_string(&self.payload)
            .map_err(|e| FluxVisorError::Serialization(e.to_string()))
    }
}

/// The transport a daemon publishes heartbeats through.
///
/// A live implementation wraps a `flux-p2p` gossipsub handle; the only one
/// shipped here is [`RecordingSink`], which records messages and touches no
/// network.
pub trait HeartbeatSink {
    /// Publish a serialized payload on a topic.
    fn publish(&mut self, topic: &str, payload_json: &str) -> Result<(), FluxVisorError>;
}

/// An inert sink: records every `(topic, payload_json)` it is handed and never
/// touches the network. Used for tests and dry-run review.
#[derive(Clone, Debug, Default)]
pub struct RecordingSink {
    /// Published `(topic, payload_json)` pairs, in order.
    pub published: Vec<(String, String)>,
}

impl RecordingSink {
    /// Build an empty recording sink.
    pub fn new() -> Self {
        Self::default()
    }
}

impl HeartbeatSink for RecordingSink {
    fn publish(&mut self, topic: &str, payload_json: &str) -> Result<(), FluxVisorError> {
        self.published.push((topic.to_string(), payload_json.to_string()));
        Ok(())
    }
}

/// Produces a worker's capacity + liveness heartbeats. Stateful only in its
/// monotonic `seq`; all the data comes from the ledger passed to [`Self::tick`].
#[derive(Clone, Debug)]
pub struct HeartbeatDaemon {
    host: P2pHostNode,
    seq: u64,
}

impl HeartbeatDaemon {
    /// Build a daemon for a worker's P2P identity.
    pub fn new(host: P2pHostNode) -> Self {
        Self { host, seq: 0 }
    }

    /// Current sequence number (number of ticks emitted so far).
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Produce one tick's heartbeats from the current ledger. Advances `seq`.
    /// Pure — performs no I/O.
    pub fn tick(
        &mut self,
        ledger: &CapacityLedger,
        uptime_secs: u64,
        healthy: bool,
    ) -> Vec<HeartbeatMessage> {
        self.seq += 1;
        let host_id = self.host.host_id.clone();

        let host_hb = HostHeartbeat {
            host_id: host_id.clone(),
            role: self.host.role,
            seq: self.seq,
            uptime_secs,
            healthy,
        };

        let cap_hb = CapacityHeartbeat {
            host_id,
            seq: self.seq,
            total: sellable_as_resource_set(ledger),
            used: ledger.used(),
            remaining: ledger.remaining(),
            reservations: ledger.reservations.len() as u32,
        };

        vec![
            HeartbeatMessage {
                topic: HeartbeatTopic::Host,
                payload: HeartbeatPayload::Host(host_hb),
            },
            HeartbeatMessage {
                topic: HeartbeatTopic::Capacity,
                payload: HeartbeatPayload::Capacity(cap_hb),
            },
        ]
    }

    /// Tick, then publish each message through `sink`. Returns the messages.
    /// The daemon itself starts nothing — the side effect lives entirely in the
    /// injected [`HeartbeatSink`].
    pub fn publish_tick(
        &mut self,
        ledger: &CapacityLedger,
        uptime_secs: u64,
        healthy: bool,
        sink: &mut dyn HeartbeatSink,
    ) -> Result<Vec<HeartbeatMessage>, FluxVisorError> {
        let messages = self.tick(ledger, uptime_secs, healthy);
        for message in &messages {
            sink.publish(message.topic_str(), &message.to_json()?)?;
        }
        Ok(messages)
    }
}

/// The host's sellable capacity, expressed as a [`ResourceSet`] for the wire.
fn sellable_as_resource_set(ledger: &CapacityLedger) -> ResourceSet {
    let cap = ledger.host.sellable_capacity();
    ResourceSet {
        vcpu: cap.vcpu_threads,
        ram_mib: cap.ram_mib,
        disk_gib: cap.disk_gib,
        monthly_traffic_tb: cap.monthly_traffic_tb,
        ipv4: cap.ipv4,
        ipv6_prefixes: cap.ipv6_prefixes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        epsilon_host_profile, FluxP2pCluster, Reservation, TenantId, VmName, FLUXVISOR_P2P_TOPICS,
    };

    fn worker_node() -> P2pHostNode {
        P2pHostNode::new(
            "epsilon-host-01",
            HostRole::Worker,
            "/ip4/89.149.241.126/tcp/9003/p2p/12D3KooWEpsilonFluxVisorAlphaWorker11111111111",
            9003,
            true,
        )
        .unwrap()
    }

    fn cold_storage_reservation(vm: &str) -> Reservation {
        let cold = crate::fluxhost_alpha_plans()
            .into_iter()
            .find(|p| p.name == "cold-storage")
            .unwrap();
        Reservation {
            tenant: TenantId::new("viktor").unwrap(),
            vm_name: VmName::new(vm).unwrap(),
            plan: cold.name.clone(),
            resources: cold.resources,
        }
    }

    #[test]
    fn tick_emits_host_then_capacity() {
        let ledger = CapacityLedger::new(epsilon_host_profile());
        let mut daemon = HeartbeatDaemon::new(worker_node());
        let msgs = daemon.tick(&ledger, 3600, true);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].topic, HeartbeatTopic::Host);
        assert_eq!(msgs[1].topic, HeartbeatTopic::Capacity);
    }

    #[test]
    fn heartbeat_topics_are_fluxvisor_topics() {
        for topic in [HeartbeatTopic::Host, HeartbeatTopic::Capacity] {
            assert!(
                FLUXVISOR_P2P_TOPICS.contains(&topic.topic_str()),
                "{} not in FLUXVISOR_P2P_TOPICS",
                topic.topic_str()
            );
        }
    }

    #[test]
    fn heartbeat_topics_present_in_generated_network_config() {
        // A worker built from the cluster config must be subscribed to the exact
        // topics the daemon publishes on — this is the "NetworkConfig fit" gate.
        let cluster = FluxP2pCluster::new(
            "fluxhost-alpha",
            vec![
                P2pHostNode::new(
                    "seed-01",
                    HostRole::Seed,
                    "/ip4/10.0.0.1/tcp/9003/p2p/12D3KooWSeedFluxVisorAlpha111111111111111111111",
                    9003,
                    true,
                )
                .unwrap(),
                worker_node(),
            ],
        )
        .unwrap();
        let cfg = cluster.network_config_for("epsilon-host-01").unwrap();
        assert!(cfg
            .gossipsub_topics
            .contains(&HeartbeatTopic::Host.topic_str().to_string()));
        assert!(cfg
            .gossipsub_topics
            .contains(&HeartbeatTopic::Capacity.topic_str().to_string()));
    }

    #[test]
    fn seq_is_monotonic_across_ticks() {
        let ledger = CapacityLedger::new(epsilon_host_profile());
        let mut daemon = HeartbeatDaemon::new(worker_node());
        let a = daemon.tick(&ledger, 10, true);
        let b = daemon.tick(&ledger, 20, true);
        assert_eq!(daemon.seq(), 2);
        // both messages in a tick share that tick's seq
        let seq_of = |m: &HeartbeatMessage| match &m.payload {
            HeartbeatPayload::Host(h) => h.seq,
            HeartbeatPayload::Capacity(c) => c.seq,
        };
        assert_eq!(seq_of(&a[0]), 1);
        assert_eq!(seq_of(&a[1]), 1);
        assert_eq!(seq_of(&b[0]), 2);
    }

    #[test]
    fn capacity_heartbeat_reflects_the_ledger() {
        let mut ledger = CapacityLedger::new(epsilon_host_profile());
        ledger.reserve(cold_storage_reservation("backup-01")).unwrap();
        let mut daemon = HeartbeatDaemon::new(worker_node());
        let msgs = daemon.tick(&ledger, 60, true);
        match &msgs[1].payload {
            HeartbeatPayload::Capacity(c) => {
                assert_eq!(c.reservations, 1);
                assert_eq!(c.used, ledger.used());
                assert_eq!(c.remaining, ledger.remaining());
                // one cold-storage box used 8192 GiB of disk
                assert_eq!(c.used.disk_gib, 8 * 1024);
            }
            other => panic!("expected capacity payload, got {other:?}"),
        }
    }

    #[test]
    fn payload_json_roundtrips() {
        let ledger = CapacityLedger::new(epsilon_host_profile());
        let mut daemon = HeartbeatDaemon::new(worker_node());
        let msgs = daemon.tick(&ledger, 99, true);
        for m in &msgs {
            let json = m.to_json().unwrap();
            let back: HeartbeatPayload = serde_json::from_str(&json).unwrap();
            assert_eq!(back, m.payload);
        }
    }

    #[test]
    fn recording_sink_captures_both_without_network() {
        let ledger = CapacityLedger::new(epsilon_host_profile());
        let mut daemon = HeartbeatDaemon::new(worker_node());
        let mut sink = RecordingSink::new();
        let msgs = daemon
            .publish_tick(&ledger, 60, true, &mut sink)
            .unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(sink.published.len(), 2);
        assert_eq!(sink.published[0].0, "/fluxvisor/1/host-heartbeat");
        assert_eq!(sink.published[1].0, "/fluxvisor/1/capacity");
        // payloads are non-empty JSON
        assert!(sink.published[0].1.contains("host_id"));
    }
}
