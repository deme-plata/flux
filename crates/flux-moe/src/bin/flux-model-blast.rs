use flux_moe::blast::{blast, review, BlastConfig};
fn main() {
    // Blast the next Qwen3.6-27B (Q4 ≈ 16 GB) across a Vast swarm via libp2p gossip.
    for (nodes, fanout, gbps, tag) in [(100u64,8u64,10.0,"100 nodes @ 10Gbps fanout-8"),
                                        (100,8,20.0,"100 nodes @ 20Gbps fanout-8"),
                                        (1000,8,20.0,"1000 nodes @ 20Gbps fanout-8")] {
        let cfg = BlastConfig { nodes, fanout, latency_ms: 40.0, model_gb: 16.0, link_gbps: gbps };
        let r = blast(&cfg);
        println!("💣 BLAST — {tag}");
        println!("   coverage by round: {:?}", r.coverage);
        println!("   blast radius {}/{} · {} rounds · {:.1}s to full-coverage\n",
            r.blast_radius, nodes, r.rounds, r.time_to_full_ms/1000.0);
    }
    let cfg = BlastConfig { nodes: 100, fanout: 8, latency_ms: 40.0, model_gb: 16.0, link_gbps: 10.0 };
    println!("{}", review(&cfg, &blast(&cfg)));
}
