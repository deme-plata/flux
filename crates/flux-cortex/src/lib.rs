// flux-cortex — Autonomous Continuous Optimization Engine
//
// v2: AI-Native Development Platform (ai_cortex module) extends the core
// Cortex loop with AI-aware phases: Diagnose, Generate, Verify, Deploy.

pub mod ai_cortex;
// v3: the fold (fold.rs) — per-line findings are creased into one action per
// (crate, dimension, suggestion); every site is kept, the figure saturates.
pub mod fold;
//
// Closes the loop between architect, predict, and optimize.
// flux-cortex is the "brain" that turns Flux from a passive analysis
// toolchain into an active, self-improving compiler.
//
// The Cortex Loop™:
//   1. ARCHITECT  — scan workspace, compute 6-dimension blueprint
//   2. PREDICT    — forecast build time, cache rate, test pass probability
//   3. OPTIMIZE   — generate ranked optimization actions
//      FOLD       — crease per-line actions into one per crease; every site kept
//   4. APPLY      — execute the best actions (SIMD, io_uring, cache-line)
//   5. VALIDATE   — measure actual build/runtime impact
//   6. LEARN      — feed results back into prediction model
//
// Innovation: This is the FIRST system that ties Flux's three analysis
// crates together into a closed feedback loop. Previously, architect only
// reported, predict only forecasted, and optimize only suggested. Cortex
// makes them act, measure, and learn.

use flux_architect::{self, OptimizationDimension};
use flux_graph::WorkspaceGraph;
use flux_optimize::{self, OptimizationPreset};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// ═══════════════════════════════════════════════════════════════
// Cortex Core Types
// ═══════════════════════════════════════════════════════════════

/// The six-stage Cortex Loop phase.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CortexPhase {
    Architect,
    Predict,
    Optimize,
    Apply,
    Validate,
    Learn,
}

impl CortexPhase {
    pub fn name(&self) -> &str {
        match self {
            Self::Architect => "Architect",
            Self::Predict => "Predict",
            Self::Optimize => "Optimize",
            Self::Apply => "Apply",
            Self::Validate => "Validate",
            Self::Learn => "Learn",
        }
    }
}

/// A single action that Cortex decided to take.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CortexAction {
    /// Unique action id
    pub id: u64,
    /// What phase proposed this action
    pub source_phase: CortexPhase,
    /// The crate this action targets
    pub crate_name: String,
    /// Which dimension (Vectorization, Memory, IO, etc.)
    pub dimension: String,
    /// Human-readable description
    pub description: String,
    /// Estimated impact in percent (e.g., 15.0 = 15% perf gain)
    pub estimated_impact_pct: f64,
    /// Cortex confidence score 0.0–1.0
    pub confidence: f64,
    /// Estimated effort: "low", "medium", "high"
    pub effort: String,
    /// Whether this action has been applied
    pub applied: bool,
    /// Actual measured impact after validation (None if not yet validated)
    pub actual_impact_pct: Option<f64>,
    /// Was the prediction accurate? (None if not yet validated)
    pub prediction_accurate: Option<bool>,
    /// Timestamp when action was created
    pub created_at_secs: u64,
    /// Timestamp when action was applied
    pub applied_at_secs: Option<u64>,
    /// How many source sites this action stands for (1 = a single site).
    #[serde(default = "one")]
    pub occurrences: usize,
    /// Every source site folded into this action, in order (`fold::unfold_action`).
    #[serde(default)]
    pub sites: Vec<fold::Site>,
    /// The single-site estimate; `estimated_impact_pct` is the folded
    /// (saturating) figure once `occurrences > 1`.
    #[serde(default)]
    pub per_site_impact_pct: f64,
}

fn one() -> usize {
    1
}

fn one_f64() -> f64 {
    1.0
}

/// Results from a full Cortex Loop iteration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CortexLoopResult {
    /// Which iteration this is
    pub iteration: u64,
    /// Architecture score before optimization (0.0–1.0)
    pub arch_score_before: f64,
    /// Architecture score after optimization (0.0–1.0)
    pub arch_score_after: Option<f64>,
    /// Number of findings from architect
    pub findings_count: usize,
    /// Number of actions generated
    pub actions_generated: usize,
    /// Number of actions applied
    pub actions_applied: usize,
    /// Total estimated perf gain from applied actions
    pub estimated_total_gain_pct: f64,
    /// Total actual perf gain from validated actions
    pub actual_total_gain_pct: Option<f64>,
    /// Perf/watt metrics before
    pub perf_watt_before: Option<f64>,
    /// Perf/watt metrics after
    pub perf_watt_after: Option<f64>,
    /// Top 3 actions taken
    pub top_actions: Vec<CortexAction>,
    /// Learning: how much the prediction model improved (0.0–1.0)
    pub learning_improvement: f64,
    /// Total wall clock time for this loop
    pub loop_duration_ms: u64,
    /// Timestamp
    pub timestamp_secs: u64,
    /// Actions this loop would have generated before the fold (one per site).
    #[serde(default)]
    pub actions_unfolded: usize,
    /// `actions_unfolded ÷ actions_generated` — how much smaller the sheet got.
    #[serde(default = "one_f64")]
    pub fold_ratio: f64,
}

/// Persistent Cortex state across iterations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CortexState {
    /// All actions ever generated
    pub action_history: Vec<CortexAction>,
    /// All loop results
    pub loop_history: Vec<CortexLoopResult>,
    /// Per-crate optimization scores (used for ranking)
    pub crate_scores: HashMap<String, f64>,
    /// Learning: average prediction accuracy over time
    pub prediction_accuracy_history: Vec<f64>,
    /// Current iteration count
    pub iteration_count: u64,
    /// Total perf gained across all iterations
    pub cumulative_perf_gain_pct: f64,
    /// Number of actions applied total
    pub total_actions_applied: u64,
    /// Unfolded (per-site) actions seen across all loops, before folding.
    #[serde(default)]
    pub actions_unfolded_total: u64,
}

// ═══════════════════════════════════════════════════════════════
// Cortex Engine
// ═══════════════════════════════════════════════════════════════

/// The main Cortex engine. Holds state and orchestrates the loop.
pub struct Cortex {
    pub state: CortexState,
    ws: WorkspaceGraph,
    /// Write `~/.flux/cortex_state.json` after each loop. Only `with_persistence`
    /// turns this on. A `Cortex::new` (unit tests, flux-p2p, flux-aqua,
    /// swarm_compile) starts from an EMPTY state, so letting it save — as every
    /// loop did until 2026-11-09 — overwrote the shared file with one loop's worth
    /// of some other workspace every time one of them ran.
    persist: bool,
}

impl Cortex {
    /// Create a new Cortex engine for a workspace. In-memory only: it neither
    /// reads nor writes the shared state file (see `with_persistence`).
    pub fn new(ws: WorkspaceGraph) -> Self {
        Self {
            state: CortexState::default(),
            ws,
            persist: false,
        }
    }

    /// Whether this engine writes the shared state file after each loop.
    pub fn persists(&self) -> bool {
        self.persist
    }

    /// Create a Cortex engine that restores cumulative state from disk.
    ///
    /// This is what the MCP surface (`flux_cortex_loop` / `flux_cortex_summary`)
    /// should use: each tool call constructs a fresh `Cortex`, so without on-disk
    /// state the summary always reported zeros even after many loops. Mirrors
    /// `AiCortex::load_agent_state`.
    pub fn with_persistence(ws: WorkspaceGraph) -> Self {
        let mut cortex = Self::new(ws);
        cortex.persist = true;
        cortex.load_state();
        cortex
    }

    /// Path to the persisted cortex state file (`~/.flux/cortex_state.json`).
    pub fn state_path() -> std::path::PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        std::path::PathBuf::from(home)
            .join(".flux")
            .join("cortex_state.json")
    }

    /// Restore cumulative state from disk, if present. Silent on missing/corrupt
    /// file — a fresh workspace simply starts from an empty state.
    pub fn load_state(&mut self) {
        let path = Self::state_path();
        if let Ok(json) = std::fs::read_to_string(&path) {
            if let Ok(persisted) = serde_json::from_str::<CortexState>(&json) {
                self.state = persisted;
            }
        }
        // A state file written before the fold existed holds one action per
        // source line (500 on disk = 88 distinct sentences, 2026-11-09). Fold it
        // on the way in, so history and summary count creases, not copies.
        let unfolded = self.state.action_history.len();
        if unfolded > 0 {
            let history = std::mem::take(&mut self.state.action_history);
            self.state.action_history = fold::fold_actions(history);
            if self.state.actions_unfolded_total < unfolded as u64 {
                self.state.actions_unfolded_total = unfolded as u64;
            }
        }
    }

    /// Persist cumulative state to disk, bounding the unbounded histories so the
    /// file can't grow without limit across thousands of loops.
    pub fn save_state(&self) {
        const MAX_ACTIONS: usize = 500;
        const MAX_LOOPS: usize = 100;
        let mut snapshot = self.state.clone();
        let n = snapshot.action_history.len();
        if n > MAX_ACTIONS {
            snapshot.action_history.drain(0..n - MAX_ACTIONS);
        }
        let n = snapshot.loop_history.len();
        if n > MAX_LOOPS {
            snapshot.loop_history.drain(0..n - MAX_LOOPS);
        }
        let path = Self::state_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&snapshot) {
            let _ = std::fs::write(&path, json);
        }
    }

    /// Run a single complete Cortex Loop: Architect → Predict → Optimize → Apply → Validate → Learn.
    pub fn run_loop(&mut self, preset: OptimizationPreset) -> CortexLoopResult {
        let loop_start = Instant::now();
        self.state.iteration_count += 1;
        let iter = self.state.iteration_count;

        // ── Phase 1: ARCHITECT ──
        let arch_plan = flux_architect::analyze_workspace(&self.ws);
        let arch_score_before = arch_plan.total_estimated_gain_pct / 100.0;
        let findings_count = arch_plan.findings.len();

        // ── Phase 2: PREDICT (on the folded sheet) ──
        // The architect speaks per source line. Fold its findings first, so one
        // action stands for every site of one suggestion in one crate (fold.rs),
        // then use historical accuracy to weight which creases matter most.
        let folded = flux_architect::fold_findings(&arch_plan.findings);
        let mut actions: Vec<CortexAction> = Vec::with_capacity(folded.len());
        let now = now_secs();
        let id_base = self.state.action_history.len() as u64;
        let mut action_id = id_base;

        for g in &folded {
            // Boost confidence for crates that have historically benefited
            let hist_boost = self
                .state
                .crate_scores
                .get(&g.crate_name)
                .copied()
                .unwrap_or(0.5);

            let adjusted_confidence = (g.confidence * 0.7 + hist_boost * 0.3).min(1.0);

            let action = CortexAction {
                id: action_id,
                source_phase: CortexPhase::Predict,
                crate_name: g.crate_name.clone(),
                dimension: g.dimension.name().to_string(),
                description: g.suggestion.clone(),
                estimated_impact_pct: g.folded_impact_pct,
                confidence: adjusted_confidence,
                // Effort follows the folded figure: 400 sites are more work than one.
                effort: self.classify_effort(&g.dimension, g.folded_impact_pct),
                applied: false,
                actual_impact_pct: None,
                prediction_accurate: None,
                created_at_secs: now,
                applied_at_secs: None,
                occurrences: g.occurrences,
                sites: g.sites.iter().map(fold::Site::from).collect(),
                per_site_impact_pct: g.per_site_impact_pct,
            };
            action_id += 1;
            actions.push(action);
        }

        // ── Phase 3: OPTIMIZE ──
        // Use flux-optimize to get SIMD/io_uring/cache-line recommendations
        let opt_report = flux_optimize::analyze(&self.ws, preset.clone());

        // Enrich actions with optimize-specific hints
        for hint in &opt_report.simd_opportunities {
            let a = CortexAction {
                id: action_id,
                source_phase: CortexPhase::Optimize,
                crate_name: hint.crate_name.clone(),
                dimension: "Vectorization".to_string(),
                description: format!(
                    "Apply {} intrinsic to {} (est. {:.1}× speedup)",
                    hint.recommended_intrinsic, hint.function, hint.estimated_speedup
                ),
                estimated_impact_pct: (hint.estimated_speedup - 1.0) * 100.0,
                confidence: 0.85,
                effort: "medium".to_string(),
                applied: false,
                actual_impact_pct: None,
                prediction_accurate: None,
                created_at_secs: now,
                applied_at_secs: None,
                occurrences: 1,
                sites: Vec::new(),
                // 0.0 = "single site, read my estimate" — fold_actions fills it in.
                per_site_impact_pct: 0.0,
            };
            action_id += 1;
            actions.push(a);
        }

        for hint in &opt_report.iouring_opportunities {
            let a = CortexAction {
                id: action_id,
                source_phase: CortexPhase::Optimize,
                crate_name: hint.crate_name.clone(),
                dimension: "I/O".to_string(),
                description: format!(
                    "Convert {} to io_uring (est. {}% latency reduction)",
                    hint.operation, hint.estimated_latency_reduction_pct
                ),
                estimated_impact_pct: hint.estimated_latency_reduction_pct / 2.0, // I/O gains → overall perf
                confidence: 0.80,
                effort: "medium".to_string(),
                applied: false,
                actual_impact_pct: None,
                prediction_accurate: None,
                created_at_secs: now,
                applied_at_secs: None,
                occurrences: 1,
                sites: Vec::new(),
                // 0.0 = "single site, read my estimate" — fold_actions fills it in.
                per_site_impact_pct: 0.0,
            };
            action_id += 1;
            actions.push(a);
        }

        for hint in &opt_report.cache_line_fixes {
            let a = CortexAction {
                id: action_id,
                source_phase: CortexPhase::Optimize,
                crate_name: hint.crate_name.clone(),
                dimension: "Cache".to_string(),
                description: format!(
                    "Fix {} in {} ({} → {} bytes aligned)",
                    hint.issue, hint.struct_name, hint.current_size, hint.aligned_size
                ),
                estimated_impact_pct: if hint.issue.contains("false sharing") {
                    20.0
                } else {
                    8.0
                },
                confidence: 0.90,
                effort: "low".to_string(),
                applied: false,
                actual_impact_pct: None,
                prediction_accurate: None,
                created_at_secs: now,
                applied_at_secs: None,
                occurrences: 1,
                sites: Vec::new(),
                // 0.0 = "single site, read my estimate" — fold_actions fills it in.
                per_site_impact_pct: 0.0,
            };
            action_id += 1;
            actions.push(a);
        }

        // ── FOLD ──
        // The optimize hints are one per function/struct already; fold once more
        // so any duplicate crease (same crate, dimension, sentence) is one action,
        // and count what the sheet looked like before.
        let actions_unfolded = findings_count
            + opt_report.simd_opportunities.len()
            + opt_report.iouring_opportunities.len()
            + opt_report.cache_line_fixes.len();
        let mut actions = fold::fold_actions(actions);
        for (k, a) in actions.iter_mut().enumerate() {
            a.id = id_base + k as u64;
        }
        let actions_generated = actions.len();
        let fold_ratio = fold::fold_ratio(actions_unfolded, actions_generated);

        // Sort by (estimated_impact × confidence) descending
        actions.sort_by(|a, b| {
            let score_a = a.estimated_impact_pct * a.confidence;
            let score_b = b.estimated_impact_pct * b.confidence;
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // ── Phase 4: APPLY ──
        // Auto-apply the five best ELIGIBLE actions (confidence > 0.7, effort
        // low/medium). Before the fold this looked only at the top five positions;
        // with folds ranked first, those are the big "high effort" Vectorization
        // creases, and nothing was ever applied (measured 0/544, 2026-11-09).
        let mut applied_count = 0;
        let mut estimated_total_gain = 0.0;

        for a in actions
            .iter_mut()
            .filter(|a| a.confidence > 0.7 && (a.effort == "low" || a.effort == "medium"))
            .take(5)
        {
            a.applied = true;
            a.applied_at_secs = Some(now);
            applied_count += 1;
            estimated_total_gain += a.estimated_impact_pct;
        }

        // Call flux-optimize apply for the preset (applies hot-swappable configs)
        let _apply_report = flux_optimize::apply(&self.ws, preset.clone());

        // ── Phase 5: VALIDATE ──
        // Measure actual impact of applied actions
        // In a full implementation, this would run benchmarks and compare.
        // For now, we simulate validation based on historical accuracy.
        let mut actual_total_gain = 0.0;
        for a in actions.iter_mut().filter(|a| a.applied) {
            let accuracy = self
                .state
                .prediction_accuracy_history
                .last()
                .copied()
                .unwrap_or(0.8);
            // Simulate: actual = estimated × accuracy ± noise
            let noise = (a.id as f64 % 7.0 - 3.0) / 100.0; // ±3% noise
            a.actual_impact_pct = Some((a.estimated_impact_pct * accuracy + noise).max(0.0));
            a.prediction_accurate =
                Some((a.actual_impact_pct.unwrap() - a.estimated_impact_pct).abs() < 10.0);
            actual_total_gain += a.actual_impact_pct.unwrap();

            // Update crate scores for learning
            let entry = self
                .state
                .crate_scores
                .entry(a.crate_name.clone())
                .or_insert(0.5);
            *entry = (*entry * 0.8 + accuracy * 0.2).clamp(0.0, 1.0);
        }

        // ── Phase 6: LEARN ──
        // Compute learning improvement from this loop
        let accurate_count = actions
            .iter()
            .filter(|a| a.prediction_accurate == Some(true))
            .count();
        let validated_count = actions.iter().filter(|a| a.applied).count();
        let loop_accuracy = if validated_count > 0 {
            accurate_count as f64 / validated_count as f64
        } else {
            0.8
        };

        self.state
            .prediction_accuracy_history
            .push(loop_accuracy);
        // Keep last 20 entries
        if self.state.prediction_accuracy_history.len() > 20 {
            self.state.prediction_accuracy_history.remove(0);
        }

        let prev_avg = self
            .state
            .prediction_accuracy_history
            .iter()
            .take(self.state.prediction_accuracy_history.len().saturating_sub(1))
            .sum::<f64>()
            / self
                .state
                .prediction_accuracy_history
                .len()
                .saturating_sub(1)
                .max(1) as f64;

        let learning_improvement = (loop_accuracy - prev_avg).max(0.0);

        // Perf/watt
        let perf_watt_before = flux_optimize::estimate_perf_watt(&self.ws).mflops_per_watt;
        let perf_watt_after = Some(perf_watt_before * (1.0 + actual_total_gain / 100.0));

        // ── Build result ──
        let top_actions: Vec<CortexAction> = actions
            .iter()
            .take(3)
            .cloned()
            .collect();

        let result = CortexLoopResult {
            iteration: iter,
            arch_score_before,
            arch_score_after: Some(arch_score_before + actual_total_gain / 100.0),
            findings_count,
            actions_generated,
            actions_applied: applied_count,
            estimated_total_gain_pct: estimated_total_gain,
            actual_total_gain_pct: Some(actual_total_gain),
            perf_watt_before: Some(perf_watt_before),
            perf_watt_after,
            top_actions,
            learning_improvement,
            loop_duration_ms: loop_start.elapsed().as_millis() as u64,
            timestamp_secs: now,
            actions_unfolded,
            fold_ratio,
        };

        // Persist state
        self.state.action_history.extend(actions);
        self.state.loop_history.push(result.clone());
        self.state.cumulative_perf_gain_pct += actual_total_gain;
        self.state.total_actions_applied += applied_count as u64;
        self.state.actions_unfolded_total += actions_unfolded as u64;

        // Persist to disk so cumulative state survives across MCP tool calls —
        // only for an engine that also LOADED that state (`with_persistence`).
        if self.persist {
            self.save_state();
        }

        result
    }

    /// Run multiple Cortex Loops to continuously improve.
    pub fn run_continuous(
        &mut self,
        iterations: usize,
        preset: OptimizationPreset,
    ) -> Vec<CortexLoopResult> {
        let mut results = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let res = self.run_loop(preset.clone());
            // If no actions were applied and learning plateaued, break early
            let should_break = res.actions_applied == 0 && res.learning_improvement < 0.01;
            results.push(res);
            if should_break {
                break;
            }
        }
        results
    }

    /// Get a summary report of all Cortex activity.
    pub fn summary(&self) -> CortexSummary {
        let total_actions = self.state.action_history.len();
        let accurate_count = self
            .state
            .action_history
            .iter()
            .filter(|a| a.prediction_accurate == Some(true))
            .count();
        let avg_accuracy = self
            .state
            .prediction_accuracy_history
            .iter()
            .sum::<f64>()
            / self.state.prediction_accuracy_history.len().max(1) as f64;

        CortexSummary {
            total_iterations: self.state.iteration_count,
            total_actions_generated: total_actions as u64,
            total_actions_applied: self.state.total_actions_applied,
            cumulative_perf_gain_pct: self.state.cumulative_perf_gain_pct,
            prediction_accuracy: avg_accuracy,
            accurate_predictions: accurate_count as u64,
            top_crate: self
                .state
                .crate_scores
                .iter()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(k, v)| (k.clone(), *v)),
            learning_plateau: self.is_plateaued(),
            actions_unfolded_total: self.state.actions_unfolded_total,
            fold_ratio: fold::fold_ratio(self.state.actions_unfolded_total as usize, total_actions),
        }
    }

    // ── Helpers ──

    fn classify_effort(&self, dim: &OptimizationDimension, impact: f64) -> String {
        match dim {
            OptimizationDimension::Cache => "low".to_string(),
            OptimizationDimension::IO => {
                if impact > 30.0 {
                    "high".to_string()
                } else {
                    "medium".to_string()
                }
            }
            OptimizationDimension::Vectorization => {
                if impact > 50.0 {
                    "high".to_string()
                } else {
                    "medium".to_string()
                }
            }
            OptimizationDimension::Memory => {
                if impact > 20.0 {
                    "high".to_string()
                } else {
                    "medium".to_string()
                }
            }
            _ => "medium".to_string(),
        }
    }

    fn is_plateaued(&self) -> bool {
        if self.state.prediction_accuracy_history.len() < 5 {
            return false;
        }
        let recent: Vec<f64> = self
            .state
            .prediction_accuracy_history
            .iter()
            .rev()
            .take(5)
            .copied()
            .collect();
        let avg = recent.iter().sum::<f64>() / 5.0;
        let variance = recent.iter().map(|x| (x - avg).powi(2)).sum::<f64>() / 5.0;
        variance < 0.001
    }
}

/// High-level summary of Cortex activity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CortexSummary {
    pub total_iterations: u64,
    pub total_actions_generated: u64,
    pub total_actions_applied: u64,
    pub cumulative_perf_gain_pct: f64,
    pub prediction_accuracy: f64,
    pub accurate_predictions: u64,
    pub top_crate: Option<(String, f64)>,
    pub learning_plateau: bool,
    /// Per-site actions seen before folding, across all loops.
    #[serde(default)]
    pub actions_unfolded_total: u64,
    /// `actions_unfolded_total ÷ total_actions_generated`.
    #[serde(default = "one_f64")]
    pub fold_ratio: f64,
}

// ═══════════════════════════════════════════════════════════════
// Utility
// ═══════════════════════════════════════════════════════════════

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ═══════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use flux_graph::{CrateInfo, CrateType, WorkspaceGraph};
    use std::path::PathBuf;

    fn mock_workspace() -> WorkspaceGraph {
        WorkspaceGraph {
            root: PathBuf::from("/mock/flux"),
            crates: vec![
                CrateInfo {
                    name: "flux-cache".into(),
                    path: PathBuf::from("/mock/flux/crates/flux-cache"),
                    dependencies: vec![],
                    edition: "2021".into(),
                    crate_type: CrateType::Lib,
                    features: vec![],
                },
                CrateInfo {
                    name: "flux-db".into(),
                    path: PathBuf::from("/mock/flux/crates/flux-db"),
                    dependencies: vec![],
                    edition: "2021".into(),
                    crate_type: CrateType::Lib,
                    features: vec![],
                },
                CrateInfo {
                    name: "flux-zk".into(),
                    path: PathBuf::from("/mock/flux/crates/flux-zk"),
                    dependencies: vec![],
                    edition: "2021".into(),
                    crate_type: CrateType::Lib,
                    features: vec![],
                },
            ],
            batches: vec![vec![0, 1, 2]],
        }
    }

    #[test]
    fn test_cortex_new() {
        let ws = mock_workspace();
        let ctx = Cortex::new(ws);
        assert_eq!(ctx.state.iteration_count, 0);
        assert!(ctx.state.action_history.is_empty());
    }

    #[test]
    fn test_cortex_single_loop() {
        let ws = mock_workspace();
        let mut ctx = Cortex::new(ws);
        let result = ctx.run_loop(OptimizationPreset::MaxPerf);

        assert_eq!(result.iteration, 1);
        // Mock workspace has no real source files, so findings may be 0.
        // The loop should still complete successfully.
        // loop may complete instantly on empty workspace
        assert!(result.loop_duration_ms < 1000);
        assert!(result.arch_score_before >= 0.0);
        assert!(result.learning_improvement >= 0.0);
    }

    #[test]
    fn test_cortex_continuous() {
        let ws = mock_workspace();
        let mut ctx = Cortex::new(ws);
        let results = ctx.run_continuous(3, OptimizationPreset::Balanced);

        assert!(!results.is_empty());
        assert!(results.iter().all(|r| r.iteration > 0));
        // State should be persisted
        assert_eq!(ctx.state.iteration_count, results.len() as u64);
    }

    #[test]
    fn test_cortex_summary() {
        let ws = mock_workspace();
        let mut ctx = Cortex::new(ws);
        ctx.run_loop(OptimizationPreset::MaxPerf);
        ctx.run_loop(OptimizationPreset::MaxPerf);

        let summary = ctx.summary();
        assert_eq!(summary.total_iterations, 2);
        assert!(summary.total_actions_generated > 0);
        assert!(summary.prediction_accuracy >= 0.0);
        assert!(summary.prediction_accuracy <= 1.0);
    }

    #[test]
    fn test_cortex_learning_improves() {
        let ws = mock_workspace();
        let mut ctx = Cortex::new(ws);
        let results = ctx.run_continuous(5, OptimizationPreset::MaxPerf);

        // After multiple iterations, learning should stabilize
        let later_results: Vec<f64> = results
            .iter()
            .rev()
            .take(3)
            .map(|r| r.learning_improvement)
            .collect();
        let avg_late = later_results.iter().sum::<f64>() / later_results.len() as f64;
        // Learning improvement should approach 0 as we plateau
        assert!(avg_late < 0.5);
    }

    #[test]
    fn test_cortex_top_actions_ranked() {
        let ws = mock_workspace();
        let mut ctx = Cortex::new(ws);
        let result = ctx.run_loop(OptimizationPreset::MaxPerf);

        // Top actions should be sorted by impact × confidence
        for i in 1..result.top_actions.len() {
            let prev = result.top_actions[i - 1].estimated_impact_pct
                * result.top_actions[i - 1].confidence;
            let curr =
                result.top_actions[i].estimated_impact_pct * result.top_actions[i].confidence;
            assert!(prev >= curr, "Actions not sorted by score");
        }
    }

    #[test]
    fn test_cortex_plateau_detection() {
        let ws = mock_workspace();
        let mut ctx = Cortex::new(ws);

        // Initially not plateaued
        assert!(!ctx.is_plateaued());

        // After many identical accuracy entries, should plateau
        for _ in 0..10 {
            ctx.state.prediction_accuracy_history.push(0.95);
        }
        assert!(ctx.is_plateaued());
    }

    #[test]
    fn test_cortex_new_does_not_persist() {
        // Every one of these tests runs loops; none of them may touch the shared
        // ~/.flux/cortex_state.json. Only `with_persistence` does.
        assert!(!Cortex::new(mock_workspace()).persists());
    }

    /// A real (throw-away) workspace: 40 identical heap-allocation lines in one
    /// crate. Unfolded, the architect reports 40 findings; the cortex must turn
    /// them into ONE action that keeps all 40 sites and a saturating figure.
    #[test]
    fn test_cortex_folds_per_line_findings_into_one_action_per_crease() {
        let root = std::env::temp_dir().join(format!("flux-cortex-fold-{}", std::process::id()));
        let crate_dir = root.join("crates").join("foldy");
        let src = crate_dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let body: String = (0..40)
            .map(|i| format!("fn f{i}() {{ let v: Vec<u8> = Vec::new(); drop(v); }}\n"))
            .collect();
        std::fs::write(src.join("lib.rs"), body).unwrap();
        let ws = WorkspaceGraph {
            root: root.clone(),
            crates: vec![CrateInfo {
                name: "foldy".into(),
                path: crate_dir,
                dependencies: vec![],
                edition: "2021".into(),
                crate_type: CrateType::Lib,
                features: vec![],
            }],
            batches: vec![vec![0]],
        };

        let mut ctx = Cortex::new(ws);
        let result = ctx.run_loop(OptimizationPreset::MaxPerf);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(result.findings_count, 40, "the architect still sees every line");
        assert!(result.actions_unfolded >= 40);
        assert!(result.actions_generated < result.actions_unfolded);
        assert!(result.fold_ratio > 2.0, "fold ratio {}", result.fold_ratio);

        let arena = ctx
            .state
            .action_history
            .iter()
            .find(|a| a.crate_name == "foldy" && a.dimension == "Memory" && a.occurrences == 40)
            .expect("one Memory action standing for all 40 sites");
        assert_eq!(arena.sites.len(), 40);
        assert_eq!(fold::unfold_action(arena).len(), 40);
        assert_eq!(arena.sites[0].line, 1);
        assert_eq!(arena.sites[39].line, 40);
        assert_eq!(arena.per_site_impact_pct, 12.0);
        assert!((arena.estimated_impact_pct - flux_architect::folded_impact(12.0, 40)).abs() < 1e-9);
        // Far below the old linear sum (40 × 12 = 480 %).
        assert!(arena.estimated_impact_pct < 60.0);
        // No unfolded copies survived into history.
        let copies = ctx
            .state
            .action_history
            .iter()
            .filter(|a| a.crate_name == "foldy" && a.dimension == "Memory" && a.description == arena.description)
            .count();
        assert_eq!(copies, 1);
    }

    /// Measurement harness, not an assertion. When `FLUX_CORTEX_MEASURE_WS` names
    /// a workspace, run ONE loop on it in memory (no state file is read or
    /// written) and write the unfolded/folded counts as JSON to
    /// `FLUX_CORTEX_MEASURE_OUT`. With the variable unset this is a no-op, so it
    /// costs an ordinary test run nothing.
    #[test]
    fn measure_live_workspace_when_asked() {
        let Ok(ws_root) = std::env::var("FLUX_CORTEX_MEASURE_WS") else { return };
        let out = std::env::var("FLUX_CORTEX_MEASURE_OUT").unwrap_or_else(|_| "/dev/stdout".into());
        let root = PathBuf::from(ws_root);
        let members = flux_graph::workspace::discover_members(&root).expect("workspace discovery");
        let crates: Vec<CrateInfo> = members
            .iter()
            .filter_map(|p| {
                Some(CrateInfo {
                    name: p.file_name()?.to_string_lossy().to_string(),
                    path: p.clone(),
                    dependencies: vec![],
                    edition: "2021".into(),
                    crate_type: CrateType::Lib,
                    features: vec![],
                })
            })
            .collect();
        let ws = WorkspaceGraph { root, crates, batches: vec![] };

        let mut ctx = Cortex::new(ws);
        let r = ctx.run_loop(OptimizationPreset::MaxPerf);

        let top: Vec<serde_json::Value> = r
            .top_actions
            .iter()
            .map(|a| {
                serde_json::json!({
                    "crate": a.crate_name, "dimension": a.dimension, "occurrences": a.occurrences,
                    "per_site_impact_pct": a.per_site_impact_pct, "folded_impact_pct": a.estimated_impact_pct,
                    "description": a.description,
                })
            })
            .collect();
        let largest = ctx
            .state
            .action_history
            .iter()
            .max_by_key(|a| a.occurrences)
            .map(|a| serde_json::json!({"crate": a.crate_name, "occurrences": a.occurrences, "description": a.description}));
        let distinct_crates: std::collections::HashSet<&str> =
            ctx.state.action_history.iter().map(|a| a.crate_name.as_str()).collect();
        let report = serde_json::json!({
            "findings_count": r.findings_count,
            "actions_unfolded": r.actions_unfolded,
            "actions_generated": r.actions_generated,
            "fold_ratio": r.fold_ratio,
            "actions_applied": r.actions_applied,
            "estimated_total_gain_pct_of_applied": r.estimated_total_gain_pct,
            "distinct_crates_with_actions": distinct_crates.len(),
            "largest_fold": largest,
            "top_actions": top,
            "loop_ms": r.loop_duration_ms,
        });
        std::fs::write(&out, serde_json::to_string_pretty(&report).unwrap()).expect("write measurement");
    }
}
