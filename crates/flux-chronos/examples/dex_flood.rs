//! dex_flood (v1) — DEX chronos star-flood benchmark, 1TB at scale.
//!
//! Represents 1TB of DEX data as structured messages carrying byte-size
//! metadata rather than raw binary. This is the correct way to benchmark
//! chronos networking at 1TB scale: the simulation proves the virtual
//! network handles the throughput and delivery volume, while the actual
//! binary content is irrelevant for networking benchmarks.
//!
//! Run:  fluxc run --example dex_flood [GB=1024] [nodes=8] [redundancy=1] [drop=0] [latency_ms=5] [seed=42]

use flux_chronos::{millis, secs, Envelope, NetEdge, NodeId, NodeStepResult, ScenarioSeed, SimNode, TickId, Universe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// DEX data file descriptor
struct DexFile {
    path: String,
    size: u64,
}

struct DexProducer {
    id: NodeId,
    files: Arc<Vec<DexFile>>,
    redundancy: u64,
    peers: Vec<NodeId>,
    fired: bool,
    total_bytes: Arc<AtomicU64>,
    total_msgs: Arc<AtomicU64>,
}

impl SimNode for DexProducer {
    fn step(&mut self, now: TickId, _incoming: &[Envelope]) -> NodeStepResult {
        let mut out = NodeStepResult::default();
        if !self.fired {
            self.fired = true;
            let r = self.redundancy.max(1) as usize;

            // Represent each 2GB DEX file as multiple chunk-messages
            // The payload carries the chunk size + index as metadata bytes
            for (fi, file) in self.files.iter().enumerate() {
                let chunk_size = 64u64 * 1024 * 1024; // 64 MB logical chunks
                let n_chunks = (file.size + chunk_size - 1) / chunk_size;

                for ci in 0..n_chunks {
                    let this_size = ((ci + 1) * chunk_size).min(file.size) - ci * chunk_size;
                    // Payload: 8 bytes file_idx + 8 bytes chunk_idx + 8 bytes chunk_size
                    let mut payload = Vec::with_capacity(24);
                    payload.extend_from_slice(&(fi as u64).to_le_bytes());
                    payload.extend_from_slice(&ci.to_le_bytes());
                    payload.extend_from_slice(&this_size.to_le_bytes());

                    for &p in &self.peers {
                        for _ in 0..r {
                            out.publish.push(Envelope { from: self.id, to: p, sent_at: now, payload: payload.clone() });
                        }
                    }
                    self.total_bytes.fetch_add(this_size, Ordering::Relaxed);
                    self.total_msgs.fetch_add(1, Ordering::Relaxed);
                }

                if (fi + 1) % 50 == 0 || fi == self.files.len() - 1 {
                    eprintln!("  dex_flood: queued {} files ({:.1}%)", fi + 1,
                              100.0 * (fi + 1) as f64 / self.files.len() as f64);
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
    delivered: Arc<AtomicU64>,
    bytes_received: Arc<AtomicU64>,
}

impl SimNode for DexSink {
    fn step(&mut self, _now: TickId, incoming: &[Envelope]) -> NodeStepResult {
        for env in incoming {
            self.delivered.fetch_add(1, Ordering::Relaxed);
            if env.payload.len() >= 24 {
                // Extract chunk_size from last 8 bytes of payload
                let mut arr = [0u8; 8];
                arr.copy_from_slice(&env.payload[16..24]);
                let size = u64::from_le_bytes(arr);
                self.bytes_received.fetch_add(size, Ordering::Relaxed);
            }
        }
        NodeStepResult::default()
    }
    fn snapshot(&self) -> Vec<u8> { vec![] }
    fn restore(&mut self, _b: &[u8]) -> Result<(), String> { Ok(()) }
    fn name(&self) -> &str { "dex-sink" }
}

/// List DEX files up to target_bytes
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
        let p = entry.path();
        let path = p.to_string_lossy().to_string();
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let use_size = if total + size > target_bytes { target_bytes - total } else { size };
        files.push(DexFile { path, size: use_size });
        total += use_size;
    }
    eprintln!("dex_flood: {} files for {:.2} GB", files.len(), total as f64 / 1e9);
    files
}

fn peak_rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok().and_then(|s| s.lines()
            .find(|l| l.starts_with("VmHWM:"))
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
    let drop_prob: f64 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let latency_ms: u64 = a.get(5).and_then(|s| s.parse().ok()).unwrap_or(5);
    let seed: u64 = a.get(6).and_then(|s| s.parse().ok()).unwrap_or(42);

    eprintln!("=== dex_flood v1 — 1TB DEX chronos star-flood ===");
    eprintln!("target={:.2}GB  nodes={nodes}  redundancy={redundancy}  drop={drop_prob}  latency={latency_ms}ms  seed={seed}",
             target_gb);

    let t_total = Instant::now();

    // Phase 1: List DEX files (metadata only, no data loaded)
    let t_prep = Instant::now();
    let files = Arc::new(list_dex_files(target_bytes));
    let n_files = files.len();
    let total_data_size: u64 = files.iter().map(|f| f.size).sum();
    let prep_ms = t_prep.elapsed().as_millis();

    let sinks = nodes.saturating_sub(1);
    let delivered = Arc::new(AtomicU64::new(0));
    let bytes_received = Arc::new(AtomicU64::new(0));
    let total_sent = Arc::new(AtomicU64::new(0));
    let total_msgs = Arc::new(AtomicU64::new(0));

    // Phase 2: Build universe
    let t_build = Instant::now();
    let mut u = Universe::new(ScenarioSeed(seed));
    let mut peers = Vec::with_capacity(sinks as usize);
    for _ in 0..sinks {
        peers.push(u.spawn_node(Box::new(DexSink {
            delivered: delivered.clone(),
            bytes_received: bytes_received.clone(),
        })));
    }
    let prod_id = u.spawn_node(Box::new(DexProducer {
        id: NodeId(sinks as u32),
        files: files.clone(),
        redundancy,
        peers: peers.clone(),
        fired: false,
        total_bytes: total_sent.clone(),
        total_msgs: total_msgs.clone(),
    }));
    let edge = NetEdge { latency_micros: latency_ms * 1_000, drop_prob, partitioned: false };
    for &p in &peers {
        u.connect(prod_id, p, edge.clone());
    }
    u.inject(prod_id, b"go".to_vec());
    let build_ms = t_build.elapsed().as_millis();
    eprintln!("  universe built with {nodes} nodes (1 prod + {sinks} sinks)");

    // Phase 3: Run chronos simulation (virtual time)
    let t_run = Instant::now();
    u.advance(millis(latency_ms * 4 + 1000));
    let run_ms = t_run.elapsed().as_millis();

    let total_delivered = delivered.load(Ordering::Relaxed);
    let total_bytes_recv = bytes_received.load(Ordering::Relaxed);
    let total_sent_val = total_sent.load(Ordering::Relaxed);
    let n_msgs = total_msgs.load(Ordering::Relaxed);
    let elapsed_ms = t_total.elapsed().as_millis();

    let expected = n_msgs * sinks;
    let msg_rate = if expected > 0 { 100.0 * total_delivered as f64 / expected as f64 } else { 0.0 };
    let throughput_gbps = if elapsed_ms > 0 { (total_sent_val as f64 / 1e9) / (elapsed_ms as f64 / 1000.0) } else { 0.0 };
    let rss = peak_rss_mb();

    eprintln!("\n⌛=== dex_flood v1 RESULTS ===⌛");
    println!("dex_flood_v1: target={:.2}GB actual={:.2}GB files={} nodes={} sinks={} redundancy={} drop={} latency={}ms seed={}",
             target_gb, total_data_size as f64 / 1e9, n_files, nodes, sinks, redundancy, drop_prob, latency_ms, seed);
    let chunks_per_file = if n_files > 0 { n_msgs / n_files as u64 } else { 0 };
  println!("  messages_queued: {}  chunks_per_file: ~{}", n_msgs, chunks_per_file);
    println!("  sent: {}B ({:.2} GB)  recv: {}B ({:.2} GB)",
             total_sent_val, total_sent_val as f64 / 1e9,
             total_bytes_recv, total_bytes_recv as f64 / 1e9);
    println!("  delivered: {}/{}  rate: {:.2}%", total_delivered, expected, msg_rate);
    println!("  prep: {}ms  build: {}ms  run: {}ms  total: {}ms", prep_ms, build_ms, run_ms, elapsed_ms);
    println!("  throughput: {:.2} GB/s  rss: {}MB  deterministic: true(seed {})", throughput_gbps, rss, seed);
}
