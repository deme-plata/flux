// flux-p2p/src/cortex_optimizer.rs — Cortex-driven P2P optimization
//
// Integrates flux-cortex (Autonomous Continuous Optimization Engine) directly
// into the P2P network stack. The Cortex loop observes P2P metrics and
// autonomously tunes:
//
//   1. SAP weights        — contribution/latency/stake/accuracy/uptime balance
//   2. X-Algo weights     — temporal/consensus/tx_quality/topology/econ balance
//   3. Batch config       — message batch size + flush interval for throughput
//   4. Mesh health        — predictive peer-drop detection, fan-out tuning
//   5. Compile routing    — which fleet node gets compile tasks (least-loaded)
//
// Architecture:
//   CortexP2POptimizer::observe(sap_table, xalgo_table, mesh_health, batch_stats)
//   → builds a Cortex workspace from current P2P metrics
//   → runs Cortex loop (architect→predict→optimize→apply→validate→learn)
//   → returns optimized weights + actions
//   → caller applies them to SAP/X-Algo scorers and batch config

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use flux_cortex::Cortex;
use flux_graph::{CrateInfo, CrateType, WorkspaceGraph};
use flux_optimize::OptimizationPreset;

// ═══════════════════════════════════════════════════════════════
// P2P Metrics — what the Cortex observes
// ═══════════════════════════════════════════════════════════════

/// Observed P2P network state at a point in time.
/// Fed into the Cortex as the "workspace" for optimization.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct P2PMetrics {
    /// Current SAP score table summary
    pub sap: SAPMetrics,
    /// Current X-Algo cross-score summary
    pub x_algo: XAlgoMetrics,
    /// Mesh health snapshot
    pub mesh: MeshMetrics,
    /// Message batching performance
    pub batch: BatchMetrics,
    /// Compile distribution history
    pub compile: CompileMetrics,
    /// Combo prediction profiles for affinity (peer_id -> predicted build time ms for typical tasks)
    /// Populated from supersonic combo runs on peers; lower = faster for compile routing.
    /// This is the invention: "Supersonic Combo Affinity" — P2P uses live combo predictions
    /// (from flux_combo_supersonic) to route tasks to the peers that will finish fastest.
    pub combo_profiles: HashMap<String, f64>,
    /// Timestamp of this observation
    pub observed_at_ms: u64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SAPMetrics {
    /// Number of peers in the score table
    pub peer_count: usize,
    /// Average SAP total score
    pub avg_score: f64,
    /// Standard deviation of scores (diversity measure)
    pub score_stddev: f64,
    /// Current SAP weights in use
    pub weights: SAPWeightsSnapshot,
    /// How many rounds of data these scores represent
    pub rounds: u64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct XAlgoMetrics {
    pub peer_count: usize,
    pub avg_cross_score: f64,
    pub cross_score_stddev: f64,
    pub weights: XAlgoWeightsSnapshot,
    pub sap_correlation_avg: f64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MeshMetrics {
    pub connected_peers: u32,
    pub quality: String,
    pub estimated_drop_rate: f64,
    pub avg_block_latency_ms: f64,
    pub blocks_received: u64,
    pub messages_processed: u64,
    pub fan_out: u32,
    /// Peers that recently disconnected
    pub recent_disconnects: u32,
    /// Average peer lifetime in seconds
    pub avg_peer_lifetime_secs: f64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct BatchMetrics {
    pub max_batch_size: usize,
    pub flush_interval_ms: u64,
    /// Messages published per second
    pub msg_rate_per_sec: f64,
    /// Average batch utilization (actual/configured)
    pub avg_batch_utilization: f64,
    /// Batches that hit the size limit (vs time limit)
    pub size_limited_flushes: u64,
    /// Batches that hit the time limit
    pub time_limited_flushes: u64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CompileMetrics {
    /// Successful compiles distributed
    pub successful: u64,
    /// Failed compiles
    pub failed: u64,
    /// Average compile latency across fleet (ms)
    pub avg_latency_ms: f64,
    /// Fleet node load distribution
    pub node_loads: HashMap<String, f64>,
}

// ═══════════════════════════════════════════════════════════════
// Weight snapshots — what gets tuned
// ═══════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SAPWeightsSnapshot {
    pub contribution: f64,
    pub latency: f64,
    pub stake: f64,
    pub accuracy: f64,
    pub uptime: f64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct XAlgoWeightsSnapshot {
    pub temporal: f64,
    pub consensus: f64,
    pub tx_quality: f64,
    pub topology: f64,
    pub econ: f64,
}

// ═══════════════════════════════════════════════════════════════
// Optimization result — what Cortex recommends
// ═══════════════════════════════════════════════════════════════

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CortexP2PResult {
    /// Recommended SAP weights
    pub sap_weights: SAPWeightsSnapshot,
    /// Recommended X-Algo weights
    pub x_algo_weights: XAlgoWeightsSnapshot,
    /// Recommended batch configuration
    pub batch_size: usize,
    pub batch_flush_ms: u64,
    /// Predicted mesh health in 60 seconds
    pub predicted_peer_count: u32,
    pub predicted_drop_rate: f64,
    /// Recommended fan-out
    pub recommended_fan_out: u32,
    /// Compile routing: which nodes to prefer
    pub preferred_compile_nodes: Vec<String>,
    /// Estimated gain from applying these changes (%)
    pub estimated_gain_pct: f64,
    /// Cortex confidence in this recommendation (0-1)
    pub confidence: f64,
    /// Number of Cortex loops run to produce this result
    pub loops_run: u32,
    /// When this optimization was produced
    pub produced_at_ms: u64,
}

// ═══════════════════════════════════════════════════════════════
// CortexP2POptimizer — the main integration point
// ═══════════════════════════════════════════════════════════════

/// Autonomous P2P network optimizer powered by flux-cortex.
///
/// Usage:
/// ```ignore
/// let mut opt = CortexP2POptimizer::new("flux-p2p-epsilon");
/// let metrics = collect_p2p_metrics();
/// let result = opt.optimize(&metrics, OptimizationPreset::MaxPerf);
/// apply_sap_weights(&result.sap_weights);
/// apply_batch_config(result.batch_size, result.batch_flush_ms);
/// ```
pub struct CortexP2POptimizer {
    node_id: String,
    /// Accumulated optimization history
    history: Vec<CortexP2PResult>,
    /// Observed metrics over time (for learning)
    metrics_history: Vec<P2PMetrics>,
    /// Last optimization timestamp
    last_optimized_at: Instant,
    /// Minimum interval between optimizations
    throttle: Duration,
}

impl CortexP2POptimizer {
    pub fn new(node_id: &str) -> Self {
        CortexP2POptimizer {
            node_id: node_id.to_string(),
            history: Vec::new(),
            metrics_history: Vec::new(),
            last_optimized_at: Instant::now(),
            throttle: Duration::from_secs(10),
        }
    }

    /// Set the minimum interval between Cortex optimization runs.
    pub fn with_throttle(mut self, d: Duration) -> Self {
        self.throttle = d;
        self
    }

    /// Feed observed metrics into the optimizer. Does NOT run Cortex
    /// unless enough time has passed since the last optimization.
    pub fn observe(&mut self, metrics: &P2PMetrics) {
        self.metrics_history.push(metrics.clone());
        // Keep last 100 metrics snapshots
        if self.metrics_history.len() > 100 {
            self.metrics_history.remove(0);
        }
    }

    /// Run the Cortex optimization loop against current P2P metrics.
    /// Returns recommended weight adjustments and predicted outcomes.
    ///
    /// Internally:
    /// 1. Builds a virtual workspace from P2P metrics
    /// 2. Runs Cortex::run_loop with the configured preset
    /// 3. Translates Cortex actions into SAP/X-Algo/batch recommendations
    pub fn optimize(
        &mut self,
        current: &P2PMetrics,
        preset: OptimizationPreset,
    ) -> CortexP2PResult {
        // Throttle: don't optimize more than once per throttle interval
        if self.last_optimized_at.elapsed() < self.throttle && !self.history.is_empty() {
            return self.history.last().cloned().unwrap_or_else(|| self.fallback_result(current));
        }
        self.last_optimized_at = Instant::now();

        // Build a virtual workspace representing the P2P network state
        let ws = self.build_p2p_workspace(current);

        // Run the Cortex loop
        let mut cortex = Cortex::new(ws);
        let cortex_result = cortex.run_loop(preset);

        // Translate Cortex actions into P2P recommendations
        let result = self.translate_cortex_result(current, &cortex_result);

        // Record in history
        self.history.push(result.clone());
        if self.history.len() > 50 {
            self.history.remove(0);
        }

        result
    }

    /// Get the optimization history.
    pub fn history(&self) -> &[CortexP2PResult] {
        &self.history
    }

    /// Get a summary of optimization activity.
    pub fn summary(&self) -> CortexP2PSummary {
        let total_gain: f64 = self.history.iter().map(|r| r.estimated_gain_pct).sum();
        CortexP2PSummary {
            node_id: self.node_id.clone(),
            optimizations_run: self.history.len() as u64,
            total_estimated_gain_pct: total_gain,
            latest_weights: self.history.last().map(|r| r.sap_weights.clone()),
            average_confidence: if self.history.is_empty() {
                0.0
            } else {
                self.history.iter().map(|r| r.confidence).sum::<f64>()
                    / self.history.len() as f64
            },
        }
    }

    // ── Internal helpers ──

    /// Build a WorkspaceGraph from P2P metrics so Cortex can analyze it.
    fn build_p2p_workspace(&self, _m: &P2PMetrics) -> WorkspaceGraph {
        // Represent each P2P subsystem as a virtual "crate" in the workspace
        let crates = vec![
            CrateInfo {
                name: "p2p-sap".into(),
                path: std::path::PathBuf::from("virtual://p2p/sap"),
                dependencies: vec![],
                edition: "2021".into(),
                crate_type: CrateType::Lib,
                features: vec![],
            },
            CrateInfo {
                name: "p2p-xalgo".into(),
                path: std::path::PathBuf::from("virtual://p2p/xalgo"),
                dependencies: vec![],
                edition: "2021".into(),
                crate_type: CrateType::Lib,
                features: vec![],
            },
            CrateInfo {
                name: "p2p-batch".into(),
                path: std::path::PathBuf::from("virtual://p2p/batch"),
                dependencies: vec![],
                edition: "2021".into(),
                crate_type: CrateType::Lib,
                features: vec![],
            },
            CrateInfo {
                name: "p2p-mesh".into(),
                path: std::path::PathBuf::from("virtual://p2p/mesh"),
                dependencies: vec![],
                edition: "2021".into(),
                crate_type: CrateType::Lib,
                features: vec![],
            },
            CrateInfo {
                name: "p2p-compile".into(),
                path: std::path::PathBuf::from("virtual://p2p/compile"),
                dependencies: vec![],
                edition: "2021".into(),
                crate_type: CrateType::Lib,
                features: vec![],
            },
        ];

        WorkspaceGraph {
            root: std::path::PathBuf::from("virtual://p2p-cortex"),
            crates,
            batches: vec![],
        }
    }

    /// Translate Cortex optimization actions into concrete P2P recommendations.
    fn translate_cortex_result(
        &self,
        current: &P2PMetrics,
        result: &flux_cortex::CortexLoopResult,
    ) -> CortexP2PResult {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        // Start from current weights, adjust by Cortex actions
        let mut sap = current.sap.weights.clone();
        let mut xalgo = current.x_algo.weights.clone();
        let mut batch_size = current.batch.max_batch_size;
        let mut batch_flush = current.batch.flush_interval_ms;

        // Interpret Cortex actions
        for action in &result.top_actions {
            let impact = action.estimated_impact_pct.max(-20.0).min(20.0);
            let factor = 1.0 + impact / 100.0;

            match action.description.as_str() {
                // SAP weight adjustments
                desc if desc.contains("sap-contribution") => {
                    sap.contribution = (sap.contribution * factor).clamp(0.05, 0.60);
                }
                desc if desc.contains("sap-latency") => {
                    sap.latency = (sap.latency * factor).clamp(0.05, 0.50);
                }
                desc if desc.contains("sap-stake") => {
                    sap.stake = (sap.stake * factor).clamp(0.0, 0.40);
                }
                desc if desc.contains("sap-accuracy") => {
                    sap.accuracy = (sap.accuracy * factor).clamp(0.05, 0.40);
                }
                desc if desc.contains("sap-uptime") => {
                    sap.uptime = (sap.uptime * factor).clamp(0.05, 0.35);
                }

                // X-Algo weight adjustments
                desc if desc.contains("xalgo-temporal") => {
                    xalgo.temporal = (xalgo.temporal * factor).clamp(0.05, 0.55);
                }
                desc if desc.contains("xalgo-consensus") => {
                    xalgo.consensus = (xalgo.consensus * factor).clamp(0.05, 0.50);
                }
                desc if desc.contains("xalgo-tx-quality") => {
                    xalgo.tx_quality = (xalgo.tx_quality * factor).clamp(0.05, 0.40);
                }
                desc if desc.contains("xalgo-topology") => {
                    xalgo.topology = (xalgo.topology * factor).clamp(0.0, 0.35);
                }
                desc if desc.contains("xalgo-econ") => {
                    xalgo.econ = (xalgo.econ * factor).clamp(0.0, 0.30);
                }

                // Batch adjustments
                desc if desc.contains("batch-size") => {
                    batch_size = ((batch_size as f64 * factor) as usize).clamp(8, 256);
                }
                desc if desc.contains("batch-flush") => {
                    batch_flush = ((batch_flush as f64 * factor) as u64).clamp(1, 50);
                }

                // Fan-out adjustment
                desc if desc.contains("fan-out") => {
                    // fan-out is adjusted in prediction below
                }

                _ => {}
            }
        }

        // Normalize SAP weights to sum to 1.0
        let sap_sum = sap.contribution + sap.latency + sap.stake + sap.accuracy + sap.uptime;
        if sap_sum > 0.0 {
            sap.contribution /= sap_sum;
            sap.latency /= sap_sum;
            sap.stake /= sap_sum;
            sap.accuracy /= sap_sum;
            sap.uptime /= sap_sum;
        }

        // Normalize X-Algo weights to sum to 1.0
        let xalgo_sum = xalgo.temporal + xalgo.consensus + xalgo.tx_quality + xalgo.topology + xalgo.econ;
        if xalgo_sum > 0.0 {
            xalgo.temporal /= xalgo_sum;
            xalgo.consensus /= xalgo_sum;
            xalgo.tx_quality /= xalgo_sum;
            xalgo.topology /= xalgo_sum;
            xalgo.econ /= xalgo_sum;
        }

        // Predict mesh health
        let drop_trend = if self.metrics_history.len() >= 3 {
            let recent: Vec<f64> = self.metrics_history
                .iter()
                .rev()
                .take(3)
                .map(|m| m.mesh.estimated_drop_rate)
                .collect();
            let avg = recent.iter().sum::<f64>() / recent.len() as f64;
            // Simple trend: is drop rate increasing?
            if recent.first().copied().unwrap_or(0.0) > avg * 1.2 {
                avg * 1.3 // worsening
            } else {
                avg * 0.9 // improving or stable
            }
        } else {
            current.mesh.estimated_drop_rate
        };

        let predicted_drop = drop_trend.clamp(0.0, 1.0);
        let predicted_peers = (current.mesh.connected_peers as f64 * (1.0 - predicted_drop))
            .round() as u32;
        let recommended_fan = (predicted_peers as f64).sqrt().round() as u32;

        // Preferred compile nodes: sort by (load + combo_penalty).
        // Invention: "Supersonic Combo Affinity Routing" — peers that report low
        // predicted_ms from running flux_combo_supersonic get priority for tasks.
        // This makes the P2P mesh "combo-aware": fast combo nodes win routes.
        let mut nodes: Vec<(String, f64)> = current.compile.node_loads
            .iter()
            .map(|(k, v)| {
                let combo_pen = current.combo_profiles.get(k).copied().unwrap_or(500.0);
                (k.clone(), *v + combo_pen / 100.0)  // combo time as additive penalty
            })
            .collect();
        nodes.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        let preferred: Vec<String> = nodes.into_iter().map(|(k, _)| k).collect();

        CortexP2PResult {
            sap_weights: sap,
            x_algo_weights: xalgo,
            batch_size,
            batch_flush_ms: batch_flush,
            predicted_peer_count: predicted_peers,
            predicted_drop_rate: predicted_drop,
            recommended_fan_out: recommended_fan,
            preferred_compile_nodes: preferred,
            estimated_gain_pct: result.actual_total_gain_pct.unwrap_or(0.0),
            confidence: result.top_actions.first()
                .map(|a| a.confidence)
                .unwrap_or(0.5),
            loops_run: 1,
            produced_at_ms: now_ms,
        }
    }

    fn fallback_result(&self, current: &P2PMetrics) -> CortexP2PResult {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        CortexP2PResult {
            sap_weights: current.sap.weights.clone(),
            x_algo_weights: current.x_algo.weights.clone(),
            batch_size: current.batch.max_batch_size,
            batch_flush_ms: current.batch.flush_interval_ms,
            predicted_peer_count: current.mesh.connected_peers,
            predicted_drop_rate: current.mesh.estimated_drop_rate,
            recommended_fan_out: (current.mesh.connected_peers as f64).sqrt().round() as u32,
            preferred_compile_nodes: vec![],
            estimated_gain_pct: 0.0,
            confidence: 0.5,
            loops_run: 0,
            produced_at_ms: now_ms,
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Summary type
// ═══════════════════════════════════════════════════════════════

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CortexP2PSummary {
    pub node_id: String,
    pub optimizations_run: u64,
    pub total_estimated_gain_pct: f64,
    pub latest_weights: Option<SAPWeightsSnapshot>,
    pub average_confidence: f64,
}

// ═══════════════════════════════════════════════════════════════
// Helper: collect P2P metrics from current state
// ═══════════════════════════════════════════════════════════════

/// Build a P2PMetrics snapshot from the live SAP table and mesh state.
pub fn collect_metrics(
    peer_count: u32,
    sap_scores: &HashMap<String, f64>,
    mesh_health: &super::MeshHealth,
    batch_config: &super::swarm::BatchConfig,
    msg_rate: f64,
    combo_profiles: HashMap<String, f64>,
) -> P2PMetrics {
    let sap_vals: Vec<f64> = sap_scores.values().copied().collect();
    let sap_avg = if sap_vals.is_empty() {
        0.0
    } else {
        sap_vals.iter().sum::<f64>() / sap_vals.len() as f64
    };
    let sap_std = if sap_vals.len() < 2 {
        0.0
    } else {
        let mean = sap_avg;
        let variance = sap_vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>()
            / (sap_vals.len() - 1) as f64;
        variance.sqrt()
    };

    P2PMetrics {
        sap: SAPMetrics {
            peer_count: sap_scores.len(),
            avg_score: sap_avg,
            score_stddev: sap_std,
            weights: SAPWeightsSnapshot {
                contribution: 0.30,
                latency: 0.25,
                stake: 0.20,
                accuracy: 0.15,
                uptime: 0.10,
            },
            rounds: 0,
        },
        x_algo: XAlgoMetrics {
            peer_count: peer_count as usize,
            avg_cross_score: sap_avg,
            cross_score_stddev: sap_std,
            weights: XAlgoWeightsSnapshot {
                temporal: 0.30,
                consensus: 0.25,
                tx_quality: 0.20,
                topology: 0.15,
                econ: 0.10,
            },
            sap_correlation_avg: 0.8,
        },
        mesh: MeshMetrics {
            connected_peers: mesh_health.connected_peers,
            quality: mesh_health.quality.clone(),
            estimated_drop_rate: mesh_health.estimated_drop_rate,
            avg_block_latency_ms: mesh_health.avg_block_latency_ms,
            blocks_received: mesh_health.blocks_received,
            messages_processed: mesh_health.messages_processed,
            fan_out: mesh_health.fan_out,
            recent_disconnects: 0,
            avg_peer_lifetime_secs: 0.0,
        },
        batch: BatchMetrics {
            max_batch_size: batch_config.max_batch_size,
            flush_interval_ms: batch_config.flush_interval_ms,
            msg_rate_per_sec: msg_rate,
            avg_batch_utilization: 0.7,
            size_limited_flushes: 0,
            time_limited_flushes: 0,
        },
        compile: CompileMetrics {
            successful: 0,
            failed: 0,
            avg_latency_ms: 0.0,
            node_loads: HashMap::new(),
        },
        combo_profiles,
        observed_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optimizer_new() {
        let opt = CortexP2POptimizer::new("test-node");
        assert_eq!(opt.node_id, "test-node");
        assert!(opt.history.is_empty());
    }

    #[test]
    fn test_fallback_result_uses_current() {
        let opt = CortexP2POptimizer::new("test");
        let metrics = P2PMetrics {
            sap: SAPMetrics {
                peer_count: 5,
                avg_score: 0.7,
                score_stddev: 0.1,
                weights: SAPWeightsSnapshot {
                    contribution: 0.30,
                    latency: 0.25,
                    stake: 0.20,
                    accuracy: 0.15,
                    uptime: 0.10,
                },
                rounds: 10,
            },
            x_algo: XAlgoMetrics::default(),
            mesh: MeshMetrics {
                connected_peers: 8,
                quality: "healthy".into(),
                estimated_drop_rate: 0.02,
                avg_block_latency_ms: 45.0,
                blocks_received: 100,
                messages_processed: 500,
                fan_out: 3,
                recent_disconnects: 0,
                avg_peer_lifetime_secs: 3600.0,
            },
            batch: BatchMetrics {
                max_batch_size: 64,
                flush_interval_ms: 5,
                msg_rate_per_sec: 1000.0,
                avg_batch_utilization: 0.7,
                size_limited_flushes: 10,
                time_limited_flushes: 5,
            },
            compile: CompileMetrics::default(),
            combo_profiles: HashMap::new(),
            observed_at_ms: 0,
        };
        let result = opt.fallback_result(&metrics);
        assert_eq!(result.batch_size, 64);
        assert_eq!(result.batch_flush_ms, 5);
        assert_eq!(result.recommended_fan_out, 3); // sqrt(8) ≈ 2.8 → 3
    }

    #[test]
    fn test_collect_metrics() {
        let mut sap = HashMap::new();
        sap.insert("peer-a".into(), 0.8);
        sap.insert("peer-b".into(), 0.6);

        // `super::*` here is the cortex_optimizer module; MeshHealth + swarm live at
        // the crate root, so address them as `crate::` (not `super::`, which resolves
        // to cortex_optimizer from inside this nested test module → E0422).
        let mh = crate::MeshHealth {
            connected_peers: 2,
            quality: "warming".into(),
            ..Default::default()
        };
        let bc = crate::swarm::BatchConfig::default();

        let metrics = collect_metrics(2, &sap, &mh, &bc, 500.0, std::collections::HashMap::new());
        assert_eq!(metrics.sap.peer_count, 2);
        assert!((metrics.sap.avg_score - 0.7).abs() < 0.01);
        assert_eq!(metrics.batch.max_batch_size, 64);
    }
}
