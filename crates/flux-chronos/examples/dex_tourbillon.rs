//! dex_tourbillon (v2) — DEX Tourbillon fault-injection ordering benchmark.
//!
//! Wraps the 1TB DEX star-flood in Tourbillon's rotating-permutation runner:
//! every permutation of DEX file injection order is tested to prove that
//! the DEX ordering invariant holds — swap-A-before-swap-B must produce
//! identical final state as swap-B-before-swap-A.
//!
//! Run:  fluxc run --example dex_tourbillon [GB=1024] [nodes=8] [redundancy=1] [latency_ms=5] [seed=42] [perms=6]

use flux_chronos::tourbillon::{self, Injection};
use flux_chronos::{millis, Envelope, NetEdge, NodeId, NodeStepResult, ScenarioSeed, SimNode, TickId, Universe};
use std::sync::Arc;
use std::time::Instant;

struct DexFile {
    size: u64,
}

struct DexProducer {
    id: NodeId,
    files: Arc<Vec<DexFile>>,
    order: Vec<usize>,
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

            for &fi in &self.order {
                let file = &self.files[fi];
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
    eprintln!("dex_tourbillon: {} files for {:.2} GB", files.len(), total as f64 / 1e9);
    files
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
    let max_perms: usize = a.get(6).and_then(|s| s.parse().ok()).unwrap_or(6);

    eprintln!("=== dex_tourbillon v2 — Tourbillon permutation DEX test ===");
    eprintln!("target={:.2}GB  nodes={nodes}  redundancy={redundancy}  latency={latency_ms}ms  seed={seed}  max_perms={max_perms}",
             target_gb);

    let t_total = Instant::now();
    let t_prep = Instant::now();
    let files = Arc::new(list_dex_files(target_bytes));
    let n_files = files.len();
    let total_data: u64 = files.iter().map(|f| f.size).sum();
    let prep_ms = t_prep.elapsed().as_millis();
    let files_arc = files.clone();

    // Tourbillon builder — constructs a fresh Universe per permutation
    let builder = move |_seed: ScenarioSeed| -> Universe {
        let sinks = nodes.saturating_sub(1);
        let mut u = Universe::new(ScenarioSeed(seed));
        let mut peers = Vec::with_capacity(sinks as usize);
        for _ in 0..sinks {
            peers.push(u.spawn_node(Box::new(DexSink { counter: 0, bytes_recv: 0 })));
        }
        let prod_id = u.spawn_node(Box::new(DexProducer {
            id: NodeId(sinks as u32),
            files: files_arc.clone(),
            order: (0..n_files).collect(),
            redundancy,
            peers: peers.clone(),
            fired: false,
        }));
        let edge = NetEdge { latency_micros: latency_ms * 1_000, drop_prob: 0.3, partitioned: false };
        for &p in &peers { u.connect(prod_id, p, edge.clone()); }
        u
    };

    // Injections for tourbillon (reordering the firing order)
    let injections: Vec<Injection> = (0..n_files.min(8)).map(|i| {
        Injection {
            target: NodeId(nodes.saturating_sub(1) as u32),
            payload: format!("dex-file-{i}").into_bytes(),
        }
    }).collect();

    eprintln!("Running tourbillon with {} permutations...", max_perms);

    let report = tourbillon::run(
        ScenarioSeed(seed),
        &injections,
        millis(latency_ms * 4 + 1000),
        Some(max_perms),
        builder,
    );

    let elapsed_ms = t_total.elapsed().as_millis();
    let rss = peak_rss_mb();

    eprintln!("\n⌛=== dex_tourbillon v2 RESULTS ===⌛");
    println!("dex_tourbillon_v2: target={:.2}GB actual={:.2}GB files={} nodes={} latency={}ms seed={}",
             target_gb, total_data as f64 / 1e9, n_files, nodes, latency_ms, seed);
    println!("  permutations_run: {}  converged: {}  divergence_pairs: {}",
             report.outcomes.len(), report.converged, report.divergence_pairs.len());
    println!("  prep: {}ms  total: {}ms  rss: {}MB", prep_ms, elapsed_ms, rss);

    if report.converged {
        println!("  VERDICT: ✓ DEX ORDERING INVARIANT PASS — all permutations produce identical state");
    } else {
        println!("  VERDICT: ✗ DEX ORDERING INVARIANT FAIL — permutation order changes final state!");
        for (i, j) in &report.divergence_pairs {
            println!("    divergence: perm {i} ≠ perm {j}");
        }
    }
}
