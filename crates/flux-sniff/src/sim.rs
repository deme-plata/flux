//! flux-sniff in chronos — network diagnostics over the SIMULATED mesh.
//!
//! flux-sniff's job is to be the eyes-and-ears of the P2P mesh: health
//! reports + anomaly detection (floods, partitions, loss spikes). On the real
//! mesh it gets its packets from tshark. In `flux-chronos` there is no tshark
//! — the mesh is an in-memory bus. This module lets the SAME diagnostics run
//! on simulated traffic: a chronos scenario extracts its message events,
//! feeds them here as [`SimPacket`]s, and gets back the same
//! [`NetworkHealthReport`] + [`AnomalyAlert`]s it would on real hardware.
//!
//! Decoupled by design: this takes primitive records, not `flux_chronos`
//! types, so flux-sniff doesn't depend on the simulation crate. The caller
//! (sigil-chronos, a chronos harness) does the trivial `Envelope -> SimPacket`
//! mapping. Same pattern as the SimNode being transport-agnostic: the
//! diagnostics are mesh-agnostic.
//!
//! Why it matters: you can now detect a partition / flood / loss-spike in a
//! deterministic chronos scenario — reproducibly, in virtual time — before it
//! ever happens on real Delta/Epsilon. The anomaly detector is the same code
//! that watches the live mesh.

use crate::{AnomalyAlert, NetworkHealthReport, P2PConnection, PacketSummary};

/// One simulated message, as a chronos scenario observed it. `from`/`to` are
/// node ids; `delivered` is false if the in-memory net dropped it (loss /
/// partition).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SimPacket {
    /// Sending node id.
    pub from: u32,
    /// Destination node id.
    pub to: u32,
    /// Simulated send time, microseconds.
    pub tick_us: u64,
    /// Payload size in bytes.
    pub bytes: u32,
    /// Gossipsub topic (e.g. `/sigil/g0/blocks`).
    pub topic: String,
    /// Did the in-memory net actually deliver it? (false = dropped/partitioned)
    pub delivered: bool,
}

/// Map a node id to a synthetic 10.x.x.x address so the existing
/// `PacketSummary`-shaped tooling renders sim nodes like real peers.
fn node_ip(id: u32) -> String {
    format!("10.{}.{}.{}", (id >> 16) & 0xFF, (id >> 8) & 0xFF, id & 0xFF)
}

/// Convert a sim packet into the canonical [`PacketSummary`] so all the
/// existing flux-sniff analysis (and any JSON/MCP consumer) works unchanged.
pub fn to_summary(p: &SimPacket) -> PacketSummary {
    PacketSummary {
        timestamp: p.tick_us as f64 / 1_000_000.0,
        src_ip: node_ip(p.from),
        dst_ip: node_ip(p.to),
        src_port: 9501, // SIGIL P2P port — cosmetic
        dst_port: 9501,
        protocol: "sim/gossipsub".into(),
        length: p.bytes,
        info: format!("{} {}", p.topic, if p.delivered { "DELIVERED" } else { "DROPPED" }),
    }
}

/// Chronos-native anomaly detection. The real-mesh detector keys off TCP flags
/// (SYN/RST) which don't exist at the gossipsub layer; this keys off the
/// signals a SIMULATED mesh actually exposes: drops, per-node volume, and
/// silent peers.
pub fn detect_sim_anomalies(packets: &[SimPacket]) -> Vec<AnomalyAlert> {
    let mut alerts = Vec::new();
    if packets.is_empty() {
        return alerts;
    }
    let total = packets.len();
    let dropped = packets.iter().filter(|p| !p.delivered).count();
    let loss_pct = dropped as f64 / total as f64 * 100.0;

    // High packet loss → partition or congestion.
    if loss_pct > 20.0 {
        alerts.push(AnomalyAlert {
            alert_type: "HIGH_PACKET_LOSS".into(),
            severity: if loss_pct > 60.0 { "critical" } else { "high" }.into(),
            description: format!("{loss_pct:.1}% of simulated messages dropped — partition or congestion"),
            source_ip: None,
            timestamp: 0,
        });
    }

    // Full partition: an edge where EVERY packet dropped.
    use std::collections::BTreeMap;
    let mut edge: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new(); // (sent, delivered)
    let mut sent_by: BTreeMap<u32, u32> = BTreeMap::new();
    let mut recv_by: BTreeMap<u32, u32> = BTreeMap::new();
    for p in packets {
        let e = edge.entry((p.from, p.to)).or_default();
        e.0 += 1;
        if p.delivered {
            e.1 += 1;
            *recv_by.entry(p.to).or_default() += 1;
        }
        *sent_by.entry(p.from).or_default() += 1;
    }
    for ((from, to), (sent, delivered)) in &edge {
        if *sent >= 3 && *delivered == 0 {
            alerts.push(AnomalyAlert {
                alert_type: "PARTITION".into(),
                severity: "high".into(),
                description: format!("edge {}→{}: {sent} sent, 0 delivered — link is partitioned", node_ip(*from), node_ip(*to)),
                source_ip: Some(node_ip(*from)),
                timestamp: 0,
            });
        }
    }

    // Message storm: one node sending a dominant share of all traffic (flood).
    if let Some((&node, &count)) = sent_by.iter().max_by_key(|(_, c)| **c) {
        if total >= 20 && count as f64 / total as f64 > 0.7 {
            alerts.push(AnomalyAlert {
                alert_type: "MESSAGE_STORM".into(),
                severity: "medium".into(),
                description: format!("{} sent {count}/{total} messages ({:.0}%) — possible flood", node_ip(node), count as f64 / total as f64 * 100.0),
                source_ip: Some(node_ip(node)),
                timestamp: 0,
            });
        }
    }

    // Bandwidth spike — same threshold as the real detector, for parity.
    if total > 1000 {
        alerts.push(AnomalyAlert {
            alert_type: "BANDWIDTH_SPIKE".into(),
            severity: "low".into(),
            description: format!("{total} messages in window — unusual simulated volume"),
            source_ip: None,
            timestamp: 0,
        });
    }

    alerts
}

/// Build a [`NetworkHealthReport`] from simulated traffic — the same report
/// shape `quick_p2p_check()` produces on real hardware, so dashboards / MCP /
/// the chronos viz consume sim + real identically.
pub fn analyze(packets: &[SimPacket]) -> NetworkHealthReport {
    use std::collections::BTreeMap;
    let total = packets.len().max(1);
    let dropped = packets.iter().filter(|p| !p.delivered).count();
    let loss_pct = dropped as f64 / total as f64 * 100.0;

    // Per-edge connection stats.
    let mut conns: BTreeMap<(u32, u32), P2PConnection> = BTreeMap::new();
    let (mut span_lo, mut span_hi) = (u64::MAX, 0u64);
    let mut bytes_delivered = 0u64;
    for p in packets {
        span_lo = span_lo.min(p.tick_us);
        span_hi = span_hi.max(p.tick_us);
        if p.delivered {
            bytes_delivered += p.bytes as u64;
        }
        let c = conns.entry((p.from, p.to)).or_insert_with(|| P2PConnection {
            peer_addr: node_ip(p.to),
            peer_port: 9501,
            latency_ms: 0.0,
            packets_sent: 0,
            packets_recv: 0,
            bytes_sent: 0,
            bytes_recv: 0,
            protocol: "sim/gossipsub".into(),
            established: true,
        });
        c.packets_sent += 1;
        c.bytes_sent += p.bytes as u64;
        if p.delivered {
            c.packets_recv += 1;
            c.bytes_recv += p.bytes as u64;
        }
    }

    let span_us = span_hi.saturating_sub(span_lo).max(1);
    let span_s = span_us as f64 / 1_000_000.0;
    let mbps = (bytes_delivered as f64 * 8.0) / (span_s * 1_000_000.0);

    // Health: start at 100, dock for loss.
    let health_score = (100.0 - loss_pct * 1.5).clamp(0.0, 100.0);

    NetworkHealthReport {
        timestamp: 0,
        interfaces: vec![],
        active_connections: conns.into_values().collect(),
        total_bandwidth_rx_mbps: mbps,
        total_bandwidth_tx_mbps: mbps,
        packet_loss_pct: loss_pct,
        open_sockets: 0,
        tcp_retransmits: 0,
        capture_duration_ms: (span_us / 1000) as u64,
        health_score,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt(from: u32, to: u32, t: u64, delivered: bool) -> SimPacket {
        SimPacket { from, to, tick_us: t, bytes: 4000, topic: "/sigil/g0/blocks".into(), delivered }
    }

    #[test]
    fn clean_traffic_has_no_anomalies_and_full_health() {
        let pkts: Vec<_> = (0..10).map(|i| pkt(0, 1, i * 1000, true)).collect();
        assert!(detect_sim_anomalies(&pkts).is_empty());
        let r = analyze(&pkts);
        assert_eq!(r.packet_loss_pct, 0.0);
        assert_eq!(r.health_score, 100.0);
        assert_eq!(r.active_connections.len(), 1);
    }

    #[test]
    fn partition_is_detected() {
        // 0→1 fully partitioned (all dropped), 0→2 healthy.
        let mut pkts: Vec<_> = (0..5).map(|i| pkt(0, 1, i * 1000, false)).collect();
        pkts.extend((0..5).map(|i| pkt(0, 2, i * 1000, true)));
        let alerts = detect_sim_anomalies(&pkts);
        assert!(alerts.iter().any(|a| a.alert_type == "PARTITION"), "{alerts:?}");
        assert!(alerts.iter().any(|a| a.alert_type == "HIGH_PACKET_LOSS"));
    }

    #[test]
    fn message_storm_is_detected() {
        // node 5 floods 80% of traffic.
        let mut pkts: Vec<_> = (0..80u64).map(|i| pkt(5, 1, i * 100, true)).collect();
        pkts.extend((0..20u64).map(|i| pkt(2, 1, i * 100, true)));
        let alerts = detect_sim_anomalies(&pkts);
        assert!(alerts.iter().any(|a| a.alert_type == "MESSAGE_STORM"), "{alerts:?}");
    }

    #[test]
    fn loss_lowers_health_score() {
        // 40% loss → health docked ~60.
        let mut pkts: Vec<_> = (0..6).map(|i| pkt(0, 1, i * 1000, true)).collect();
        pkts.extend((0..4).map(|i| pkt(0, 1, (i + 6) * 1000, false)));
        let r = analyze(&pkts);
        assert!((r.packet_loss_pct - 40.0).abs() < 0.1);
        assert!(r.health_score < 50.0 && r.health_score > 30.0, "health {}", r.health_score);
    }

    #[test]
    fn summary_roundtrips_through_packetsummary() {
        let s = to_summary(&pkt(1, 2, 5_000_000, true));
        assert_eq!(s.src_ip, "10.0.0.1");
        assert_eq!(s.dst_ip, "10.0.0.2");
        assert_eq!(s.protocol, "sim/gossipsub");
        assert!(s.info.contains("DELIVERED"));
        assert_eq!(s.timestamp, 5.0);
    }
}
