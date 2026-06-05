//! BURST-5 — the build mesh, on the chronos `SimNode` trait.
//!
//! Workers each "compile" a unit and gossip a [`BuildClaim`] to a coordinator;
//! the coordinator runs [`verify_consensus`](crate::verify_consensus) and
//! decides. One worker can be **byzantine** — it ships a tampered artifact.
//! The demo watches VBC reject the backdoor across the messaging layer.
//!
//! Why `SimNode`: it's transport-agnostic. This exact worker/coordinator code
//! runs (a) in-memory under [`flux_chronos::Universe`] — deterministic, instant,
//! $0 — and (b) over **real flux-p2p** via the CHRONOS-T transport adapter
//! (proven Delta↔Epsilon). The sim is the rigorous proof; the wire run is the
//! same bytes through libp2p.

use flux_chronos::{Envelope, NetEdge, NodeId, NodeStepResult, ScenarioSeed, SimNode, TickId, Universe};

use crate::vbc::{verify_consensus, BuildClaim, Hash, VbcOutcome, VbcReject, VbcReport};

fn h(label: &str) -> Hash {
    *blake3::hash(label.as_bytes()).as_bytes()
}

/// A build worker: on its first step it emits its claim, then goes quiet.
struct Worker {
    name: String,
    worker_id: u32,
    coord: NodeId,
    unit: String,
    src: Hash,
    toolchain: Hash,
    artifact: Hash,
    emitted: bool,
}

impl SimNode for Worker {
    fn step(&mut self, now: TickId, _incoming: &[Envelope]) -> NodeStepResult {
        let mut r = NodeStepResult::default();
        if !self.emitted {
            self.emitted = true;
            let claim = BuildClaim::new(self.worker_id, &self.unit, self.src, self.toolchain, self.artifact);
            let payload = serde_json::to_vec(&claim).unwrap_or_default();
            r.publish.push(Envelope { from: NodeId(self.worker_id + 1), to: self.coord, sent_at: now, payload });
            r.events.push(format!("worker {} → coordinator: artifact {:02x}{:02x}…", self.worker_id, self.artifact[0], self.artifact[1]));
        }
        r
    }
    fn snapshot(&self) -> Vec<u8> { vec![] }
    fn restore(&mut self, _b: &[u8]) -> Result<(), String> { Ok(()) }
    fn name(&self) -> &str { &self.name }
}

/// The coordinator: collects claims, runs VBC once it has them all.
struct Coordinator {
    name: String,
    expected: usize,
    quorum: usize,
    claims: Vec<BuildClaim>,
    report: Option<VbcReport>,
}

impl SimNode for Coordinator {
    fn step(&mut self, _now: TickId, incoming: &[Envelope]) -> NodeStepResult {
        let mut r = NodeStepResult::default();
        for env in incoming {
            if let Ok(c) = serde_json::from_slice::<BuildClaim>(&env.payload) {
                self.claims.push(c);
            }
        }
        if self.report.is_none() && self.claims.len() >= self.expected {
            let rep = verify_consensus(&self.claims, self.quorum);
            match &rep.outcome {
                VbcOutcome::Accepted { artifact_hash, votes, quorum, .. } => r.events.push(format!(
                    "VBC ACCEPT: artifact {:02x}{:02x}… ({}/{} agree)",
                    artifact_hash[0], artifact_hash[1], votes, quorum
                )),
                VbcOutcome::Rejected(reason) => r.events.push(format!("VBC REJECT: {:?}", reason)),
            }
            for b in &rep.byzantine {
                r.events.push(format!(
                    "⚠ BYZANTINE worker {} caught: {:02x}{:02x}… ≠ majority {:02x}{:02x}…",
                    b.worker, b.their_hash[0], b.their_hash[1], b.majority_hash[0], b.majority_hash[1]
                ));
            }
            self.report = Some(rep);
        }
        r
    }
    fn snapshot(&self) -> Vec<u8> { serde_json::to_vec(&self.report).unwrap_or_default() }
    fn restore(&mut self, _b: &[u8]) -> Result<(), String> { Ok(()) }
    fn name(&self) -> &str { &self.name }
}

/// Run the mesh: `n_workers` build the same unit, those whose id is in
/// `byzantine` ship a tampered artifact, the coordinator needs `quorum` to
/// agree. Returns the VBC report + the full event trace (the "across the wire"
/// story). Deterministic in `seed`.
pub fn run_burst_mesh(n_workers: u32, byzantine: &[u32], quorum: usize, seed: u64) -> (VbcReport, Vec<String>) {
    let mut u = Universe::new(ScenarioSeed::from(seed));
    let coord = u.spawn_node(Box::new(Coordinator {
        name: "coordinator".into(),
        expected: n_workers as usize,
        quorum,
        claims: vec![],
        report: None,
    }));

    let (src, toolchain) = (h("source@v1"), h("rustc-1.84+musl"));
    let honest = h("artifact-honest");

    for i in 0..n_workers {
        let artifact = if byzantine.contains(&i) { h(&format!("artifact-backdoored-{i}")) } else { honest };
        let w = u.spawn_node(Box::new(Worker {
            name: format!("worker-{i}"),
            worker_id: i,
            coord,
            unit: "flux-burst".into(),
            src,
            toolchain,
            artifact,
            emitted: false,
        }));
        // 50ms edge worker → coordinator; wake the worker to emit.
        u.connect(w, coord, NetEdge { latency_micros: 50_000, drop_prob: 0.0, partitioned: false });
        u.inject(w, vec![1]);
    }

    u.advance(2_000_000); // 2s of virtual time — plenty for one round-trip

    let trace: Vec<String> = u.event_log().iter().map(|(t, n, e)| format!("[t={t}us n{}] {e}", n.0)).collect();
    let report = u
        .snapshot_nodes()
        .get(&coord)
        .and_then(|b| serde_json::from_slice::<Option<VbcReport>>(b).ok().flatten())
        .unwrap_or(VbcReport { outcome: VbcOutcome::Rejected(VbcReject::Empty), byzantine: vec![] });

    (report, trace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn honest_mesh_accepts_over_the_wire() {
        let (rep, _) = run_burst_mesh(3, &[], 2, 42);
        assert!(rep.accepted().is_some(), "all-honest mesh should accept");
        assert!(rep.byzantine.is_empty());
    }

    #[test]
    fn byzantine_worker_rejected_across_the_wire() {
        // 4 honest + worker 2 byzantine, quorum 3.
        let (rep, trace) = run_burst_mesh(5, &[2], 3, 42);
        // The honest artifact is linked; the backdoor never wins.
        assert_eq!(rep.accepted(), Some(h("artifact-honest")));
        // Worker 2 is exposed across the wire.
        assert_eq!(rep.byzantine.len(), 1);
        assert_eq!(rep.byzantine[0].worker, 2);
        // The trace actually carried the messages + the verdict.
        assert!(trace.iter().any(|l| l.contains("BYZANTINE worker 2")));
    }

    #[test]
    fn deterministic_replay() {
        let a = run_burst_mesh(5, &[1], 3, 7).0;
        let b = run_burst_mesh(5, &[1], 3, 7).0;
        assert_eq!(a, b, "same seed → identical verdict");
    }
}
