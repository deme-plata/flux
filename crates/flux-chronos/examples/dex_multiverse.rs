//! dex_multiverse (v3) — DEX multiverse branching benchmark.
//!
//! The most advanced chronos test: runs the 1TB DEX star-flood across 8
//! parallel timelines with different fault-injection profiles (packet loss,
//! latency spikes, network partitions), then diffs node states to prove
//! the DEX converges to the same final state regardless of network chaos.
//!
//! This applies VARFLOW axiom #5: "Multiverse before mainline" — prove
//! convergence under all conditions before deploying to production.
//!
//! Run:  fluxc run --example dex_multiverse [GB=1024] [nodes=8] [redundancy=1] [latency_ms=5] [seed=42]

use flux_chronos::{millis, Envelope, NetEdge, NodeId, NodeStepResult, ScenarioSeed, SimNode, TickId, Universe};
use std::sync::Arc;
use std::time::Instant;

struct DexFile {
    size: u64,
}

struct DexProducer {
    id: NodeId,
    files: Arc<Vec<DexFile>>,
    redundancy: u64,
    peers: Vec<NodeId>,
    fired: bool,
}

impl SimNode for DexProducer {
    fn step(&mut self, now: TickId, _incoming: &[Envelope]) -> NodeStepResult {
        let mut out = NodeStepResult::default();
        if !self.fired {
            self.fired = true;
            let r = self.redundancy.max(1) as usize;
            let chunk_size = 64u64 * 1024 * 1024;
            for (fi, file) in self.files.iter().enumerate() {
                let n_chunks = (file.size + chunk_size - 1) / chunk_size;
                for ci in 0..n_chunks {
                    let this_size = ((ci + 1) * chunk_size).min(file.size) - ci * chunk_size;
                    let mut payload = Vec::with_capacity(24);
                    payload.extend_from_slice(&(fi as u64).to_le_bytes());
                    payload.extend_from_slice(&ci.to_le_bytes());
                    payload.extend_from_slice(&this_size.to_le_bytes());
                    for &p in &self.peers {
                        for _ in 0..r {
                            out.publish.push(Envelope { from: self.id, to: p, sent_at: now, payload: payload.clone() });
                        }
                    }
                }
            }
        }
        out
    }
    fn snapshot(&self) -> Vec<u8> { vec![self.fired as u8] }
    fn restore(&mut self, b: &[u8]) -> Result<(), String> { self.fired = b.first() == Some(&1); Ok(()) }
    fn name(&self) -> &str { "dex-producer" }
}

struct DexSink {
    counter: u64,
    bytes_recv: u64,
}

impl SimNode for DexSink {
    fn step(&mut self, _now: TickId, incoming: &[Envelope]) -> NodeStepResult {
        for env in incoming {
            self.counter += 1;
            if env.payload.len() >= 24 {
                let mut arr = [0u8; 8];
                arr.copy_from_slice(&env.payload[16..24]);
                self.bytes_recv += u64::from_le_bytes(arr);
            }
        }
        NodeStepResult::default()
    }
    fn snapshot(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(16);
        b.extend_from_slice(&self.counter.to_le_bytes());
        b.extend_from_slice(&self.bytes_recv.to_le_bytes());
        b
    }
    fn restore(&mut self, b: &[u8]) -> Result<(), String> {
        if b.len() >= 8 { self.counter = u64::from_le_bytes(b[..8].try_into().unwrap()); }
        if b.len() >= 16 { self.bytes_recv = u64::from_le_bytes(b[8..16].try_into().unwrap()); }
        Ok(())
    }
    fn name(&self) -> &str { "dex-sink" }
}

fn list_dex_files(target_bytes: u64) -> Vec<DexFile> {
    use std::fs;
    use std::path::Path;
    let dir = Path::new("/home/storage/chronos-dex-5tb");
    let mut entries: Vec<_> = fs::read_dir(dir)
        .expect("chronos-dex-5tb")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let n = name.to_string_lossy();
            n.starts_with("dex-t0-") && n.ends_with(".bin")
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());
    let mut files = Vec::new();
    let mut total = 0u64;
    for entry in &entries {
        if total >= target_bytes { break; }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let use_size = if total + size > target_bytes { target_bytes - total } else { size };
        files.push(DexFile { size: use_size });
        total += use_size;
    }
    eprintln!("dex_multiverse: {} files for {:.2} GB", files.len(), total as f64 / 1e9);
    files
}

fn build_universe(
    seed: u64, files: Arc<Vec<DexFile>>, nodes: u64, redundancy: u64,
    latency_ms: u64, drop_prob: f64, partitioned: bool,
) -> Universe {
    let sinks = nodes.saturating_sub(1);
    let mut u = Universe::new(ScenarioSeed(seed));
    let mut peers = Vec::with_capacity(sinks as usize);
    for _ in 0..sinks {
        peers.push(u.spawn_node(Box::new(DexSink { counter: 0, bytes_recv: 0 })));
    }
    let prod_id = u.spawn_node(Box::new(DexProducer {
        id: NodeId(sinks as u32),
        files,
        redundancy,
        peers: peers.clone(),
        fired: false,
    }));
    let edge = NetEdge { latency_micros: latency_ms * 1_000, drop_prob, partitioned };
    for &p in &peers { u.connect(prod_id, p, edge.clone()); }
    u.inject(prod_id, b"go".to_vec());
    u
}

fn peak_rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok().and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|kb| kb.parse::<u64>().ok()))
        .map(|kb| kb / 1024).unwrap_or(0)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let target_gb: f64 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(1024.0);
    let target_bytes = (target_gb * 1_000_000_000.0) as u64;
    let nodes: u64 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(8);
    let redundancy: u64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
    let latency_ms: u64 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(5);
    let seed: u64 = a.get(5).and_then(|s| s.parse().ok()).unwrap_or(42);

    eprintln!("=== dex_multiverse v3 — DEX Multiverse Branching ===");
    eprintln!("target={:.2}GB  nodes={nodes}  redundancy={redundancy}  latency={latency_ms}ms  seed={seed}",
             target_gb);

    let t_total = Instant::now();
    let t_prep = Instant::now();
    let files = Arc::new(list_dex_files(target_bytes));
    let n_files = files.len();
    let total_data: u64 = files.iter().map(|f| f.size).sum();
    let prep_ms = t_prep.elapsed().as_millis();

    // Timeline 0: Baseline (no loss, low latency)
    eprintln!("[multiverse] Baseline timeline 0...");
    let t0 = Instant::now();
    let mut u0 = build_universe(seed, files.clone(), nodes, redundancy, latency_ms, 0.0, false);
    u0.advance(millis(latency_ms * 4 + 1000));
    let base_snap = u0.snapshot_nodes();
    let base_ms = t0.elapsed().as_millis() as u64;
    eprintln!("[multiverse] Baseline done in {base_ms}ms");

    // Forks: 7 alternate timelines with different chaos profiles
    let forks: Vec<(&str, f64, u64, u64, bool)> = vec![
        ("30%-loss",       0.3,   latency_ms,     seed.wrapping_add(1), false),
        ("70%-loss",       0.7,   latency_ms,     seed.wrapping_add(2), false),
        ("10x-latency",    0.0,   latency_ms * 10, seed.wrapping_add(3), false),
        ("loss+latency",   0.3,   latency_ms * 5,  seed.wrapping_add(4), false),
        ("partitioned",    0.0,   latency_ms,     seed.wrapping_add(5), true),
        ("50%-burst-loss", 0.5,   latency_ms * 2,  seed.wrapping_add(6), false),
        ("extreme",        0.5,   latency_ms * 20, seed.wrapping_add(7), false),
    ];

    let t_fork = Instant::now();
    let mut results: Vec<(&str, u64, bool)> = Vec::new();

    for (name, drop, lat, fseed, part) in &forks {
        let t_f = Instant::now();
        let mut u = build_universe(*fseed, files.clone(), nodes, redundancy, *lat, *drop, *part);
        u.advance(millis(lat * 4 + 1000));
        let snap = u.snapshot_nodes();
        let fms = t_f.elapsed().as_millis();
        let identical = snap == base_snap;
        results.push((name, fms as u64, identical));
        eprintln!("[multiverse] Fork '{name}': drop={drop} latency={lat}ms part={part} → identical={identical} ({fms}ms)");
    }

    let fork_ms = t_fork.elapsed().as_millis();
    let elapsed_ms = t_total.elapsed().as_millis();
    let rss = peak_rss_mb();

    eprintln!("\n⌛=== dex_multiverse v3 RESULTS ===⌛");
    println!("dex_multiverse_v3: target={:.2}GB actual={:.2}GB files={} nodes={} base_latency={}ms seed={}",
             target_gb, total_data as f64 / 1e9, n_files, nodes, latency_ms, seed);
    println!("  total_timelines: 1 baseline + {} forks = {}", forks.len(), forks.len() + 1);
    println!("  baseline: {base_ms}ms");

    let mut all_converged = true;
    for (name, fms, identical) in &results {
        let status = if *identical { "✓" } else { "✗" };
        if !*identical { all_converged = false; }
        println!("    {status} '{name}' ({fms}ms)");
    }

    println!("  prep: {prep_ms}ms  fork: {fork_ms}ms  total: {elapsed_ms}ms  rss: {rss}MB");
    if all_converged {
        println!("  MULTIVERSE VERDICT: ✓ ALL CONVERGED — DEX invariant holds across all 8 network conditions");
    } else {
        println!("  MULTIVERSE VERDICT: ✗ DIVERGENCE DETECTED — some conditions alter final state");
    }
}
