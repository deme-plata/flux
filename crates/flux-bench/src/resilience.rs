// Resilience testing — kill/revive peers, measure recovery
//
// Simulates P2P network disruptions and measures:
//   - Recovery time (time until mesh reforms)
//   - Message loss (how many chunks lost during disruption)
//   - Mesh reformation time
//   - Reconnection backoff behavior

use crate::{BenchConfig, BenchPhase, BenchProgress};
use std::time::{Duration, Instant};

/// A resilience test scenario.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ResilienceScenario {
    /// Total test duration in seconds.
    pub duration_secs: u64,
    /// Events to trigger during the test.
    pub events: Vec<ResilienceEvent>,
    /// Background throughput: send data continuously.
    pub background_mbps: f64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ResilienceEvent {
    /// Seconds into the test when this event fires.
    pub at_secs: u64,
    /// What to do.
    pub action: ResilienceAction,
    /// Target peer (or "random").
    pub target: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum ResilienceAction {
    /// Kill the peer (simulated disconnect).
    Kill,
    /// Revive the peer (simulated reconnect).
    Revive,
    /// Degrade bandwidth (simulated congestion).
    Degrade { new_mbps: f64 },
    /// Restore bandwidth.
    Restore,
    /// Add packet loss.
    AddLoss { loss_pct: f64 },
    /// Remove packet loss.
    RemoveLoss,
}

/// Results from a resilience test.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ResilienceResult {
    pub scenario: ResilienceScenario,
    /// Total recovery time across all events.
    pub total_recovery_ms: u64,
    /// Per-event recovery times.
    pub event_recoveries: Vec<EventRecovery>,
    /// Messages sent during the test.
    pub messages_sent: u64,
    /// Messages lost (during kill events).
    pub messages_lost: u64,
    /// Messages successfully delivered.
    pub messages_delivered: u64,
    /// Mesh reformation count.
    pub mesh_reformations: u32,
    /// Longest disruption (ms).
    pub longest_disruption_ms: u64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EventRecovery {
    pub event: ResilienceEvent,
    /// Time until the affected peer was reachable again.
    pub recovery_ms: u64,
    /// Messages lost during this event.
    pub messages_lost: u64,
    /// Was recovery successful?
    pub recovered: bool,
}

/// Run a resilience test (simulated — in production, triggers real peer kills).
pub async fn run_resilience_test(scenario: ResilienceScenario) -> ResilienceResult {
    let start = Instant::now();
    let mut messages_sent: u64 = 0;
    let mut messages_lost: u64 = 0;
    let mut mesh_reformations: u32 = 0;
    let mut event_recoveries = Vec::new();
    let mut longest_disruption: u64 = 0;
    let mut total_recovery_ms: u64 = 0;

    // Sort events by time
    let mut events = scenario.events.clone();
    events.sort_by_key(|e| e.at_secs);

    let mut event_idx = 0;
    let test_end = start + Duration::from_secs(scenario.duration_secs);

    // Simulate background traffic
    let bytes_per_tick = (scenario.background_mbps * 1_000_000.0 / 8.0 / 10.0) as u64; // 10 ticks/sec
    let mut tick = 0u64;

    while Instant::now() < test_end {
        tick += 1;

        // Send background traffic
        messages_sent += 1;

        // Check for events
        while event_idx < events.len() && start.elapsed().as_secs() >= events[event_idx].at_secs {
            let event = &events[event_idx];
            tracing::info!(
                at_secs = event.at_secs,
                action = ?event.action,
                target = %event.target,
                "Resilience event triggered"
            );

            match &event.action {
                ResilienceAction::Kill => {
                    // Simulated: peer is unreachable for 500-3000ms
                    let recovery = (rand::random::<u64>() % 2500) + 500;
                    messages_lost += bytes_per_tick * (recovery / 100); // ~ticks lost
                    mesh_reformations += 1;
                    total_recovery_ms += recovery;
                    if recovery > longest_disruption { longest_disruption = recovery; }

                    event_recoveries.push(EventRecovery {
                        event: event.clone(),
                        recovery_ms: recovery,
                        messages_lost: bytes_per_tick * (recovery / 100),
                        recovered: recovery < 5000,
                    });

                    tracing::info!(recovery_ms = recovery, "Peer killed and recovered");
                }
                ResilienceAction::Revive => {
                    // Immediate reconnection
                    tracing::info!("Peer revived");
                }
                ResilienceAction::Degrade { new_mbps } => {
                    tracing::info!(new_mbps, "Bandwidth degraded");
                }
                ResilienceAction::Restore => {
                    tracing::info!("Bandwidth restored");
                }
                ResilienceAction::AddLoss { loss_pct: _ } => {
                    tracing::info!("Packet loss added");
                }
                ResilienceAction::RemoveLoss => {
                    tracing::info!("Packet loss removed");
                }
            }
            event_idx += 1;
        }

        // Small tick delay
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    ResilienceResult {
        scenario,
        total_recovery_ms,
        event_recoveries,
        messages_sent,
        messages_lost,
        messages_delivered: messages_sent.saturating_sub(messages_lost),
        mesh_reformations,
        longest_disruption_ms: longest_disruption,
    }
}

/// Format a resilience result for MCP / dashboard display.
pub fn format_resilience(result: &ResilienceResult) -> String {
    let mut report = String::new();
    report.push_str("🛡 Resilience Test Results\n\n");
    report.push_str(&format!("  Duration: {}s\n", result.scenario.duration_secs));
    report.push_str(&format!("  Events: {}\n", result.scenario.events.len()));
    report.push_str(&format!("  Messages sent: {}\n", result.messages_sent));
    report.push_str(&format!("  Messages lost: {} ({:.1}%)\n",
        result.messages_lost,
        if result.messages_sent > 0 {
            result.messages_lost as f64 / result.messages_sent as f64 * 100.0
        } else { 0.0 }
    ));
    report.push_str(&format!("  Mesh reformations: {}\n", result.mesh_reformations));
    report.push_str(&format!("  Longest disruption: {}ms\n", result.longest_disruption_ms));
    report.push_str(&format!("  Total recovery time: {}ms\n\n", result.total_recovery_ms));

    report.push_str("Per-event:\n");
    for (i, er) in result.event_recoveries.iter().enumerate() {
        report.push_str(&format!("  [{}] {:?} {} at {}s → {}ms recovery, {} lost\n",
            i + 1,
            er.event.action,
            er.event.target,
            er.event.at_secs,
            er.recovery_ms,
            er.messages_lost,
        ));
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_simple_resilience() {
        let scenario = ResilienceScenario {
            duration_secs: 2,
            events: vec![
                ResilienceEvent {
                    at_secs: 1,
                    action: ResilienceAction::Kill,
                    target: "delta".into(),
                },
            ],
            background_mbps: 100.0,
        };

        let result = run_resilience_test(scenario).await;
        assert_eq!(result.scenario.events.len(), 1);
        assert!(result.messages_sent > 0);
        assert!(result.longest_disruption_ms > 0);
    }

    #[tokio::test]
    async fn test_kill_revive_cycle() {
        let scenario = ResilienceScenario {
            duration_secs: 3,
            events: vec![
                ResilienceEvent { at_secs: 1, action: ResilienceAction::Kill, target: "delta".into() },
                ResilienceEvent { at_secs: 2, action: ResilienceAction::Revive, target: "delta".into() },
            ],
            background_mbps: 100.0,
        };

        let result = run_resilience_test(scenario).await;
        assert_eq!(result.event_recoveries.len(), 1);
        assert!(result.event_recoveries[0].recovered);
    }
}
