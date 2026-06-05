//! BURST-5 demo — watch VBC reject a backdoor across the build mesh.
//!
//!   burst-mesh-demo [n_workers] [byzantine_id] [quorum]
//!   burst-mesh-demo            # 5 workers, worker 2 byzantine, quorum 3

use flux_burst::mesh::run_burst_mesh;
use flux_burst::vbc::VbcOutcome;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: u32 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(5);
    let byz: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(2);
    let quorum: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(3);

    println!("\n⚡ FLUXBURST · Verifiable Build Consensus over the mesh");
    println!("   {n} workers compile flux-burst · worker {byz} ships a BACKDOOR · quorum {quorum}\n");

    let (report, trace) = run_burst_mesh(n, &[byz], quorum, 42);

    for line in &trace {
        println!("   {line}");
    }

    println!();
    match &report.outcome {
        VbcOutcome::Accepted { artifact_hash, votes, .. } => {
            println!("   ✅ LINKED the honest artifact {:02x}{:02x}… ({votes} workers agreed)", artifact_hash[0], artifact_hash[1]);
        }
        VbcOutcome::Rejected(r) => println!("   ⛔ REJECTED: {r:?}"),
    }
    for b in &report.byzantine {
        println!("   🔪 SLASH worker {} — shipped {:02x}{:02x}…, majority was {:02x}{:02x}…",
            b.worker, b.their_hash[0], b.their_hash[1], b.majority_hash[0], b.majority_hash[1]);
    }
    println!("\n   The backdoor never linked. Build divergence is impossible to hide.\n");
}
