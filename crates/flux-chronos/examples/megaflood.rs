//! megaflood — flux-chronos at LARGE N: the uncapped millions-node scale test.
//!
//! The `flux_chronos_run` MCP combo clamps `nodes` to 16 (to stay snappy). This
//! example drives the same star-flood through the flux-chronos *core* with no
//! cap, so we can find the real scale ceiling + memory floor.
//!
//! The speed win at scale: a **lean sink** — it dedups by a u64 bitmask (zero
//! allocation, messages < 64) and emits **no event strings**, so the universe's
//! event log never grows. (The MCP sink pushes a `format!("sink N recv-unique")`
//! per delivery → at 1M sinks that's 1M Strings/message = the memory wall this
//! avoids.) Unique deliveries are tallied through a shared atomic.
//!
//! Run:  megaflood <nodes> [messages=1] [redundancy=1] [drop=0.0] [latency_ms=50] [seed=42]
//! Deterministic: same seed → same result.

use flux_chronos::{millis, Envelope, NetEdge, NodeId, NodeStepResult, ScenarioSeed, SimNode, TickId, Universe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

struct Producer {
    id: NodeId,
    messages: u64,
    redundancy: u64,
    peers: Vec<NodeId>,
    fired: bool,
}
impl SimNode for Producer {
    fn step(&mut self, now: TickId, _incoming: &[Envelope]) -> NodeStepResult {
        let mut out = NodeStepResult::default();
        if !self.fired {
            self.fired = true;
            let r = self.redundancy.max(1) as usize;
            out.publish.reserve(self.messages as usize * self.peers.len() * r);
            for m in 0..self.messages {
                let payload = m.to_le_bytes().to_vec();
                for &p in &self.peers {
                    for _ in 0..r {
                        out.publish.push(Envelope { from: self.id, to: p, sent_at: now, payload: payload.clone() });
                    }
                }
            }
        }
        out // no events: the producer logs nothing either, for scale
    }
    fn snapshot(&self) -> Vec<u8> { vec![self.fired as u8] }
    fn restore(&mut self, b: &[u8]) -> Result<(), String> { self.fired = b.first() == Some(&1); Ok(()) }
    fn name(&self) -> &str { "producer" }
}

struct Sink {
    seen: u64, // bitmask dedup for message ids < 64 (scale test uses few messages)
    unique_total: Arc<AtomicU64>,
}
impl SimNode for Sink {
    fn step(&mut self, _now: TickId, incoming: &[Envelope]) -> NodeStepResult {
        for env in incoming {
            let mut idb = [0u8; 8];
            let n = env.payload.len().min(8);
            idb[..n].copy_from_slice(&env.payload[..n]);
            let id = u64::from_le_bytes(idb);
            if id < 64 {
                let bit = 1u64 << id;
                if self.seen & bit == 0 {
                    self.seen |= bit;
                    self.unique_total.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        NodeStepResult::default() // NO event strings → universe log stays flat = the scale win
    }
    fn snapshot(&self) -> Vec<u8> { self.seen.to_le_bytes().to_vec() }
    fn restore(&mut self, b: &[u8]) -> Result<(), String> {
        let mut a = [0u8; 8]; let n = b.len().min(8); a[..n].copy_from_slice(&b[..n]);
        self.seen = u64::from_le_bytes(a); Ok(())
    }
    fn name(&self) -> &str { "sink" }
}

fn peak_rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
        })
        .map(|kb| kb / 1024)
        .unwrap_or(0)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let nodes: u64 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(100_000);
    let messages: u64 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);
    let redundancy: u64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
    let drop_prob: f64 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let latency_ms: u64 = a.get(5).and_then(|s| s.parse().ok()).unwrap_or(50);
    let seed: u64 = a.get(6).and_then(|s| s.parse().ok()).unwrap_or(42);

    let sinks = nodes.saturating_sub(1);
    let unique = Arc::new(AtomicU64::new(0));

    let t_build = Instant::now();
    let mut u = Universe::new(ScenarioSeed(seed));
    // sinks first (ids 0..sinks-1), then the producer (id == sinks)
    let mut peers = Vec::with_capacity(sinks as usize);
    for _ in 0..sinks {
        peers.push(u.spawn_node(Box::new(Sink { seen: 0, unique_total: unique.clone() })));
    }
    let prod_id = u.spawn_node(Box::new(Producer {
        id: NodeId(sinks as u32),
        messages,
        redundancy,
        peers: peers.clone(),
        fired: false,
    }));
    let edge = NetEdge { latency_micros: latency_ms * 1_000, drop_prob, partitioned: false };
    for &p in &peers {
        u.connect(prod_id, p, edge.clone());
    }
    // kickoff: advance() only steps nodes with incoming/wake — inject a trigger
    // so the producer steps once and fires its flood.
    u.inject(prod_id, b"go".to_vec());
    let build_ms = t_build.elapsed().as_millis();

    let t_run = Instant::now();
    // drain: producer fires at tick 0, deliveries land at +latency. Advance well past.
    u.advance(millis(latency_ms * 4 + 1000));
    let run_ms = t_run.elapsed().as_millis();

    let delivered = unique.load(Ordering::Relaxed);
    let expected = messages.saturating_mul(sinks);
    let rate = if expected > 0 { 100.0 * delivered as f64 / expected as f64 } else { 0.0 };
    println!(
        "megaflood: nodes={nodes} sinks={sinks} messages={messages} redundancy={redundancy} drop={drop_prob} latency_ms={latency_ms}\n\
         unique_delivered={delivered} / expected={expected}  rate={rate:.2}%\n\
         build_ms={build_ms}  run_ms={run_ms}  peak_rss_mb={}  deterministic=true(seed {seed})",
        peak_rss_mb()
    );
}
