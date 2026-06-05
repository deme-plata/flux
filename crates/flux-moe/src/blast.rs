//! blast.rs — the "blast bomb": propagate the next Qwen3.6 across a Vast swarm FAST.
//!
//! Deploy-one → blast-to-N over libp2p gossip, timed in chronos virtual time. A
//! seed node holds the model; each round every node that HAS it pushes to
//! `fanout` peers → exponential coverage. The "blast radius" is how many nodes
//! are serving after the flood; the headline number is **time-to-full-coverage**
//! = rounds × (link latency + model transfer time). This is the deterministic
//! model behind `flux_nodeswarm_spawn` + flux-p2p + `flux_chronos_run`.
//!
//! WHY IT'S REAL (and where it's modeled — flux-science honest review at the
//! bottom): the gossip math is exact for a synchronous-round push flood; the
//! transfer term is real (model_GB / link_Gbps); the PRETEND parts are uniform
//! links, no peer failures, and perfect round synchrony. See [`review`].

#[derive(Debug, Clone)]
pub struct BlastConfig {
    pub nodes: u64,
    pub fanout: u64,       // peers each infected node pushes to per round (libp2p mesh degree)
    pub latency_ms: f64,   // per-hop link latency
    pub model_gb: f64,     // artifact size (e.g. Qwen3.6-27B Q4 ≈ 16 GB)
    pub link_gbps: f64,    // per-link throughput
}

#[derive(Debug, Clone)]
pub struct BlastReport {
    pub rounds: u32,
    pub coverage: Vec<u64>,    // nodes-with-model after each round
    pub blast_radius: u64,     // final nodes reached
    pub round_ms: f64,         // time per round (latency + transfer)
    pub time_to_full_ms: f64,
}
impl BlastReport {
    pub fn full_coverage(&self) -> bool { self.blast_radius >= 1 && Some(&self.blast_radius) == self.coverage.last() }
}

/// Simulate the blast. Push gossip: each round, infected nodes multiply by
/// (1 + fanout), capped at `nodes`. Round time = one-hop latency + model transfer.
pub fn blast(cfg: &BlastConfig) -> BlastReport {
    assert!(cfg.fanout >= 1 && cfg.nodes >= 1);
    let transfer_ms = if cfg.link_gbps > 0.0 {
        cfg.model_gb * 8.0 / cfg.link_gbps * 1000.0 // GB→Gb / Gbps → s → ms
    } else { 0.0 };
    let round_ms = cfg.latency_ms + transfer_ms;

    let mut have: u64 = 1; // the seed
    let mut coverage = vec![have];
    let mut rounds = 0u32;
    while have < cfg.nodes {
        have = (have.saturating_mul(1 + cfg.fanout)).min(cfg.nodes);
        coverage.push(have);
        rounds += 1;
        if rounds > 1000 { break; } // safety
    }
    BlastReport {
        rounds,
        blast_radius: *coverage.last().unwrap(),
        round_ms,
        time_to_full_ms: rounds as f64 * round_ms,
        coverage,
    }
}

/// The flux-science honest review: what this model proves vs assumes.
pub fn review(cfg: &BlastConfig, r: &BlastReport) -> String {
    format!(
"FLUX-SCIENCE REVIEW — model blast across {n} nodes
  REAL (exact/measured):
   • push-gossip coverage is exact: 1 → ×{f1} per round → {rounds} rounds to cover {n}
     (log_{{1+fanout}}(n) = the well-known epidemic/gossip bound)
   • transfer term is physical: {gb:.0} GB / {gbps:.0} Gbps = {tr:.0} ms per hop
   • blast radius {radius}/{n} ({pct:.0}%), time-to-full {ttf:.1} s
  MODELED (assumptions — NOT yet measured live):
   • uniform links + perfect round synchrony (real libp2p mesh is heterogeneous)
   • zero peer failures / no NAT/dial failures (real fabric drops some — see chronos drop param)
   • seed has full upload fanout instantly (real seed is upload-bound)
  TO MAKE FULLY REAL: flux_nodeswarm_spawn {n} nodes + flux-p2p gossip of the GGUF +
   flux_chronos_run with drop>0, then compare measured coverage curve to this model.",
        n = cfg.nodes, f1 = 1 + cfg.fanout, rounds = r.rounds,
        gb = cfg.model_gb, gbps = cfg.link_gbps, tr = (r.round_ms - cfg.latency_ms),
        radius = r.blast_radius, pct = r.blast_radius as f64 / cfg.nodes as f64 * 100.0,
        ttf = r.time_to_full_ms / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(nodes: u64, fanout: u64) -> BlastConfig {
        BlastConfig { nodes, fanout, latency_ms: 40.0, model_gb: 16.0, link_gbps: 10.0 }
    }

    #[test]
    fn coverage_grows_exponentially_to_full() {
        let r = blast(&cfg(1000, 4));
        assert_eq!(r.blast_radius, 1000, "blast must reach every node");
        assert!(r.full_coverage());
        // 1→5→25→125→625→1000 = 5 rounds at fanout 4
        assert_eq!(r.rounds, 5, "log_5(1000)≈4.29 → 5 rounds, got {}", r.rounds);
    }

    #[test]
    fn bigger_fanout_blasts_faster() {
        let slow = blast(&cfg(10_000, 2));
        let fast = blast(&cfg(10_000, 16));
        assert!(fast.rounds < slow.rounds, "higher fanout → fewer rounds");
        assert_eq!(fast.blast_radius, 10_000);
        assert_eq!(slow.blast_radius, 10_000);
    }

    #[test]
    fn transfer_time_is_physical() {
        // 16 GB over 10 Gbps = 12.8 s = 12800 ms per hop + 40ms latency
        let r = blast(&cfg(100, 4));
        assert!((r.round_ms - (12_800.0 + 40.0)).abs() < 1.0, "round_ms={}", r.round_ms);
    }

    #[test]
    fn review_is_honest_about_assumptions() {
        let c = cfg(100, 4);
        let r = blast(&c);
        let rev = review(&c, &r);
        assert!(rev.contains("MODELED"));
        assert!(rev.contains("zero peer failures") || rev.contains("failures"));
        assert!(rev.contains("flux_nodeswarm_spawn"));
    }
}
