//! aether-sov-demo — prove dynamic sovereign convergence across a 4-node mesh.
//!
//! Scenario (the verification Viktor asked for):
//!   1. genesis artifact gossiped to all 4 nodes → converged (divergence 0)
//!   2. PARTITION: drop `delta`; `epsilon` bumps `fluxc` + adds `flux-sov`;
//!      gossip only among {epsilon, beta, gamma} → mesh diverges
//!   3. REJOIN `delta` → gossip → delta catches up → divergence back to 0
//!
//! Writes the live mesh sync-status JSON to `FLUX_AETHER_STATUS_PATH`
//! (default `/tmp/flux-aether-status.json`) so `fluxc serve` shows it on its
//! SSE dashboard at `/api/aether`. Touches no consensus/balance/crypto.

use flux_aether::sov::{
    divergence, gossip_until_converged, mesh_status_json, Manifest, NodeIdentity, Ver,
};

fn content(tag: &str) -> [u8; 32] {
    *blake3::hash(tag.as_bytes()).as_bytes()
}

fn write_status(nodes: &[(String, Manifest)]) {
    let path = std::env::var("FLUX_AETHER_STATUS_PATH")
        .unwrap_or_else(|_| "/tmp/flux-aether-status.json".to_string());
    let _ = std::fs::write(&path, mesh_status_json(nodes));
}

fn roots(nodes: &[(String, Manifest)]) -> String {
    nodes
        .iter()
        .map(|(n, m)| format!("{}={}", n, m.root()[..3].iter().map(|b| format!("{:02x}", b)).collect::<String>()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn main() {
    let names = ["epsilon", "delta", "beta", "gamma"];
    let eps = NodeIdentity::from_seed(b"epsilon");
    let mut nodes: Vec<(String, Manifest)> =
        names.iter().map(|n| (n.to_string(), Manifest::new())).collect();

    println!("⚡ AETHER-SOV — sovereign version mesh ({} nodes)\n", names.len());

    // ── 1. genesis ───────────────────────────────────────────────────────────
    let g = eps.author("fluxc", Ver::new(0, 18, 0), content("fluxc-0.18.0"), 1);
    for (_, m) in nodes.iter_mut() {
        m.put(g.clone());
    }
    gossip_until_converged(&mut nodes, &[]);
    println!("1. genesis gossiped     → divergence={} | {}", divergence(&nodes), roots(&nodes));
    assert_eq!(divergence(&nodes), 0);

    // ── 2. partition (drop delta), mutate epsilon ────────────────────────────
    nodes[0].1.put(eps.author("fluxc", Ver::new(0, 18, 1), content("fluxc-0.18.1"), 200));
    nodes[0].1.put(eps.author("flux-sov", Ver::new(0, 1, 0), content("flux-sov-0.1.0"), 201));
    let rounds_p = gossip_until_converged(&mut nodes, &["delta"]);
    let div_p = divergence(&nodes);
    println!(
        "2. partition (delta down) → divergence={} after {} rounds | {}",
        div_p, rounds_p, roots(&nodes)
    );
    println!(
        "   delta still at fluxc {} (epsilon is at {})",
        nodes[1].1.entries["fluxc"].ver.display(),
        nodes[0].1.entries["fluxc"].ver.display()
    );
    assert!(div_p > 0, "expected divergence while delta is partitioned");
    write_status(&nodes);

    // ── 3. rejoin delta → re-converge ────────────────────────────────────────
    let before = nodes[1].1.len();
    let rounds_r = gossip_until_converged(&mut nodes, &[]);
    let pulled = nodes[1].1.len() - before;
    let div_r = divergence(&nodes);
    println!(
        "3. delta rejoins          → divergence={} after {} rounds | {}",
        div_r, rounds_r, roots(&nodes)
    );
    println!(
        "   delta caught up: fluxc {} + pulled {} new artifact(s) (flux-sov present: {})",
        nodes[1].1.entries["fluxc"].ver.display(),
        pulled,
        nodes[1].1.entries.contains_key("flux-sov")
    );
    write_status(&nodes);

    let r0 = nodes[0].1.root();
    let all_equal = nodes.iter().all(|(_, m)| m.root() == r0);
    println!(
        "\nVERDICT: converged={} divergence={} all_roots_identical={} artifacts={}",
        div_r == 0,
        div_r,
        all_equal,
        nodes[0].1.len()
    );
    assert_eq!(div_r, 0);
    assert!(all_equal);
    println!("status written → {}", std::env::var("FLUX_AETHER_STATUS_PATH").unwrap_or_else(|_| "/tmp/flux-aether-status.json".into()));
}
