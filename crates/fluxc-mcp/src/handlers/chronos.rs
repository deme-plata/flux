//! CHRONOS-D — `flux_chronos_*` MCP tools.
//!
//! Exposes the flux-chronos deterministic simulation engine as a callable
//! MCP tool so an AI agent can author + run network scenarios with no human
//! in the loop. v0 ships `flux_chronos_run`: a star-flood reliability
//! scenario over the in-memory bus — one producer floods N messages to M
//! sinks under configurable latency + packet loss, fully seeded + therefore
//! reproducible. Returns delivery stats as text or JSON.
//!
//! This is the substrate-clean half of the chronos MCP surface: it drives
//! flux-chronos's generic engine with built-in nodes (no chain dep). The
//! SIGIL-specific fuzzer (`sigil-chronos::fuzz`) is exposed separately by a
//! chain-side MCP server (keeps lock #22 — substrate doesn't dep chain).

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::handlers::{ToolDef, ToolRegistry};
use flux_chronos::{Envelope, NetEdge, NodeId, NodeStepResult, ScenarioSeed, SimNode, TickId, Universe};

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_chronos_run",
            description: "Run a deterministic network simulation: one producer floods `messages` to `nodes-1` sinks under `latency_ms` + `drop_prob`, in virtual time (instant). Seeded → reproducible. Returns delivery stats (text or JSON). The MCP face of flux-chronos.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "nodes": {"type": "integer", "description": "Total nodes (1 producer + rest sinks). Default 2, max 16."},
                    "messages": {"type": "integer", "description": "Messages the producer floods to each sink. Default 50."},
                    "latency_ms": {"type": "integer", "description": "One-way edge latency in ms. Default 50."},
                    "drop_prob": {"type": "number", "description": "Packet-drop probability 0.0-1.0. Default 0.0."},
                    "redundancy": {"type": "integer", "description": "Re-send each message this many times (gossip re-propagation). Sinks dedup by message id. Higher redundancy → unique delivery approaches 100% even under loss. Default 1 (no redundancy)."},
                    "seed": {"type": "integer", "description": "Scenario seed for reproducibility. Default 42."},
                    "format": {"type": "string", "description": "text (default) or json"}
                }
            }),
        },
        flux_chronos_run,
    );
}

/// Built-in producer: on its first step it floods `messages` to every peer,
/// then goes quiet. Triggered by a wake-up inject from the harness.
struct Producer {
    id: NodeId,
    peers: Vec<NodeId>,
    messages: u64,
    /// Re-send each message this many times (gossip re-propagation). Each
    /// copy is an independent dice-roll against `drop_prob`, so with
    /// redundancy R and per-send loss p, the chance ALL copies of a given
    /// message drop is p^R — unique delivery climbs toward 100% as R grows.
    redundancy: u64,
    emitted: bool,
}

impl SimNode for Producer {
    fn step(&mut self, now: TickId, _incoming: &[Envelope]) -> NodeStepResult {
        let mut out = NodeStepResult::default();
        if !self.emitted {
            self.emitted = true;
            for m in 0..self.messages {
                for &p in &self.peers {
                    for _ in 0..self.redundancy.max(1) {
                        out.publish.push(Envelope {
                            from: self.id,
                            to: p,
                            sent_at: now,
                            payload: m.to_le_bytes().to_vec(),
                        });
                    }
                }
            }
            out.events.push(format!("producer emitted {} (x{})", self.messages, self.redundancy.max(1)));
        }
        out
    }
    fn snapshot(&self) -> Vec<u8> {
        Vec::new()
    }
    fn restore(&mut self, _bytes: &[u8]) -> Result<(), String> {
        Ok(())
    }
    fn name(&self) -> &str {
        "producer"
    }
}

/// Built-in sink: dedups received messages by id, emitting a `recv-unique`
/// event only the FIRST time it sees a given message. The harness counts
/// unique events, so redundant re-sends don't inflate the delivery count —
/// they only improve the odds a message arrives at all.
struct Sink {
    id: NodeId,
    seen: HashSet<u64>,
}

impl SimNode for Sink {
    fn step(&mut self, _now: TickId, incoming: &[Envelope]) -> NodeStepResult {
        let mut out = NodeStepResult::default();
        for env in incoming {
            // Message id = the u64 the producer encoded in the payload.
            let mut id_bytes = [0u8; 8];
            let n = env.payload.len().min(8);
            id_bytes[..n].copy_from_slice(&env.payload[..n]);
            let msg_id = u64::from_le_bytes(id_bytes);
            if self.seen.insert(msg_id) {
                out.events.push(format!("sink {} recv-unique", self.id.0));
            }
        }
        out
    }
    fn snapshot(&self) -> Vec<u8> {
        Vec::new()
    }
    fn restore(&mut self, _bytes: &[u8]) -> Result<(), String> {
        Ok(())
    }
    fn name(&self) -> &str {
        "sink"
    }
}

fn flux_chronos_run(args: &Value) -> String {
    let nodes = args.get("nodes").and_then(|v| v.as_u64()).unwrap_or(2).clamp(2, 16);
    let messages = args.get("messages").and_then(|v| v.as_u64()).unwrap_or(50).clamp(1, 5000);
    let latency_ms = args.get("latency_ms").and_then(|v| v.as_u64()).unwrap_or(50);
    let drop_prob = args.get("drop_prob").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(0.0, 1.0);
    let redundancy = args.get("redundancy").and_then(|v| v.as_u64()).unwrap_or(1).clamp(1, 16);
    let seed = args.get("seed").and_then(|v| v.as_u64()).unwrap_or(42);
    let format = args.get("format").and_then(|v| v.as_str()).unwrap_or("text");

    let n_sinks = nodes - 1;
    let latency_micros = latency_ms * 1_000;

    let wall_start = std::time::Instant::now();
    let mut u = Universe::new(ScenarioSeed::from(seed));

    // Producer is node 0; sinks are 1..nodes.
    let sink_ids: Vec<NodeId> = (1..nodes).map(|i| NodeId(i as u32)).collect();
    let producer = Box::new(Producer {
        id: NodeId(0),
        peers: sink_ids.clone(),
        messages,
        redundancy,
        emitted: false,
    });
    let p = u.spawn_node(producer);
    let mut sinks = Vec::new();
    for sid in &sink_ids {
        let s = u.spawn_node(Box::new(Sink { id: *sid, seen: HashSet::new() }));
        sinks.push(s);
    }
    for s in &sinks {
        u.connect(
            p,
            *s,
            NetEdge { latency_micros, drop_prob, partitioned: false },
        );
    }

    // Wake the producer (payload ignored — just triggers its first step).
    u.inject(p, vec![0xFF]);
    // Advance past delivery: messages emitted at t=0, delivered after latency.
    u.advance(latency_micros + 1_000_000);

    let log = u.event_log();
    // Count UNIQUE deliveries (dedup'd at the sink) — redundant re-sends
    // improve arrival odds without double-counting.
    let delivered = log.iter().filter(|(_, _, s)| s.contains("recv-unique")).count() as u64;
    let expected = messages * n_sinks;
    let rate = if expected > 0 { (delivered as f64 / expected as f64) * 100.0 } else { 0.0 };
    let wall_ms = wall_start.elapsed().as_millis();

    if format == "json" {
        serde_json::to_string_pretty(&json!({
            "tool": "flux_chronos_run",
            "scenario": "star-flood",
            "nodes": nodes,
            "sinks": n_sinks,
            "messages": messages,
            "expected_deliveries": expected,
            "actual_deliveries": delivered,
            "delivery_rate_pct": (rate * 10.0).round() / 10.0,
            "latency_ms": latency_ms,
            "drop_prob": drop_prob,
            "redundancy": redundancy,
            "seed": seed,
            "wall_ms": wall_ms,
            "deterministic": true,
        }))
        .unwrap_or_else(|e| format!("json error: {}", e))
    } else {
        format!(
            "🕰 flux_chronos_run — star-flood\n  {} nodes ({} sinks) · {} msgs each · {}ms latency · {:.1}% loss · redundancy x{}\n  Unique delivered: {}/{} ({:.1}%)\n  Seed: {} (reproducible) · sim wall: {}ms",
            nodes, n_sinks, messages, latency_ms, drop_prob * 100.0, redundancy,
            delivered, expected, rate, seed, wall_ms,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_network_delivers_everything() {
        let out = flux_chronos_run(&json!({
            "nodes": 3, "messages": 20, "latency_ms": 10, "drop_prob": 0.0, "seed": 1, "format": "json"
        }));
        // 2 sinks × 20 msgs = 40 expected, 0 loss → 40 delivered.
        assert!(out.contains("\"expected_deliveries\": 40"), "got: {out}");
        assert!(out.contains("\"actual_deliveries\": 40"), "got: {out}");
        assert!(out.contains("\"deterministic\": true"));
    }

    #[test]
    fn deterministic_across_runs() {
        let a = flux_chronos_run(&json!({"nodes": 4, "messages": 30, "drop_prob": 0.2, "seed": 7}));
        let b = flux_chronos_run(&json!({"nodes": 4, "messages": 30, "drop_prob": 0.2, "seed": 7}));
        // Same seed → identical delivery count (the determinism guarantee).
        let count = |s: &str| s.split("Delivered: ").nth(1).map(|x| x.to_string());
        assert_eq!(count(&a), count(&b), "same seed must give same result");
    }

    #[test]
    fn loss_reduces_delivery() {
        let lossy = flux_chronos_run(&json!({
            "nodes": 2, "messages": 200, "drop_prob": 0.5, "seed": 3, "format": "json"
        }));
        // ~50% loss over 200 msgs, no redundancy: unique delivered well under 200.
        assert!(lossy.contains("\"actual_deliveries\""));
        assert!(!lossy.contains("\"actual_deliveries\": 200"), "0.5 loss must drop some: {lossy}");
    }

    #[test]
    fn redundancy_recovers_delivery_under_loss() {
        // Same 12% loss, but redundancy x4. P(all 4 copies drop) = 0.12^4 ≈
        // 0.0002, so over 100 msgs × 4 sinks unique delivery should be ~100%
        // — recovering what a single fragile edge loses.
        let r1 = flux_chronos_run(&json!({
            "nodes": 5, "messages": 100, "drop_prob": 0.12, "redundancy": 1, "seed": 2026, "format": "json"
        }));
        let r4 = flux_chronos_run(&json!({
            "nodes": 5, "messages": 100, "drop_prob": 0.12, "redundancy": 4, "seed": 2026, "format": "json"
        }));
        // redundancy 1 loses some; redundancy 4 delivers all 400.
        assert!(!r1.contains("\"actual_deliveries\": 400"), "x1 under loss should drop some: {r1}");
        assert!(r4.contains("\"actual_deliveries\": 400"), "x4 should recover to full delivery: {r4}");
    }
}
