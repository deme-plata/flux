// fluxc-core self-heal bridge — wires flux-self-heal into CLI and MCP.
// Exposes `fluxc self-heal` command for autonomous swarm recovery.

/// Start the self-healing monitor for the Flux P2P swarm.
pub fn start_self_heal(poll_ms: u64, auto_apply: bool) {
    use flux_self_heal::{SelfHealConfig, SelfHealMonitor};

    let config = SelfHealConfig {
        poll_interval_ms: poll_ms,
        min_peers: 3,
        max_round_stall: 10,
        max_commit_gap_secs: 120,
        sap_suspicious_threshold: 0.3,
        auto_heal: auto_apply,
    };

    println!("⚡ Flux Self-Heal — starting autonomous monitor");
    println!("  Poll interval: {}ms | Auto-heal: {}", config.poll_interval_ms, config.auto_heal);
    println!("  Min peers: {} | Max round stall: {} | Max commit gap: {}s",
        config.min_peers, config.max_round_stall, config.max_commit_gap_secs);
    println!("  SAP threshold: {} | Watch: Swarm + DAG + SAP scoring", config.sap_suspicious_threshold);

    let monitor = SelfHealMonitor::new(config);

    // Start async monitor on the current runtime (or block_on)
    let rt = tokio::runtime::Runtime::new().unwrap_or_else(|e| {
        eprintln!("  ✗ Failed to create tokio runtime: {e}");
        std::process::exit(1);
    });

    rt.block_on(async {
        monitor.start().await;
        println!("  ✓ Self-heal monitor running. Press Ctrl+C to stop.");

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let status = monitor.status();
            println!(
                "  [self-heal] peers:{} | round:{} | checks:{} | anomalies:{} | heals:{} | health:{:.0}%",
                status.peer_count,
                status.dag_round,
                status.checks_run,
                status.anomalies_detected,
                status.heals_applied,
                status.health_score
            );
        }
    });
}
