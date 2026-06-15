// fluxc predict — X-Algo-Inspired Build Prediction Engine
//
// Uses 5-dimension scoring (inspired by flux-p2p x_algo.rs) to predict
// build outcomes before compilation. Compares predictions against reality
// to continuously improve scoring accuracy via feedback loops.
//
// Dimensions:
//   1. Source Delta Score    — how much source changed since last build
//   2. Cache Affinity Score  — likelihood of cache hits based on change patterns
//   3. Dependency Graph Score — topological sensitivity of changed crates
//   4. Historical Accuracy   — how well past predictions matched reality
//   5. Peer Consensus Score  — agreement with supercluster peers on build times
//
// After each build, compare prediction vs actual and fire webhook events.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use blake3::Hasher;

// ── Prediction Model ──

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BuildPrediction {
    /// Predicted build time in milliseconds
    pub predicted_ms: u64,
    /// Predicted cache hit rate (0.0–1.0)
    pub predicted_cache_rate: f64,
    /// Predicted test pass probability (0.0–1.0)
    pub predicted_test_pass: f64,
    /// Overall confidence in this prediction (0.0–1.0)
    pub confidence: f64,
    /// Individual dimension scores
    pub dimensions: PredictionDimensions,
    /// Whether this is a cold build or incremental
    pub is_cold: bool,
    /// Timestamp of prediction
    pub timestamp_secs: u64,
    /// Package being predicted for
    pub package: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PredictionDimensions {
    /// Source delta: 0 = no changes, 1 = full rewrite
    pub source_delta: f64,
    /// Cache affinity: 1 = high cache hit probability
    pub cache_affinity: f64,
    /// Dependency graph: 0 = leaf crate, 1 = root (affects everything)
    pub dep_graph_sensitivity: f64,
    /// Historical accuracy of past predictions (0–1)
    pub historical_accuracy: f64,
    /// Peer consensus agreement (0–1)
    pub peer_consensus: f64,
    /// System stability from heatmap (0–1, 6th dimension)
    pub system_stability: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BuildFeedback {
    pub prediction: BuildPrediction,
    pub actual_ms: u64,
    pub actual_cache_rate: f64,
    pub actual_test_pass: bool,
    /// Absolute error in ms
    pub error_ms: i64,
    /// Was the prediction within ±20%?
    pub was_accurate: bool,
    pub timestamp_secs: u64,
}

// ── Persistent History ──

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PredictionHistory {
    pub predictions: Vec<BuildPrediction>,
    pub feedback: Vec<BuildFeedback>,
    /// Rolling accuracy (last N predictions)
    pub accuracy_window: Vec<bool>,
    /// Average error over lifetime
    pub avg_error_ms: f64,
    /// Total predictions made
    pub total_predictions: u64,
}

fn history_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".flux").join("predictions.json")
}

pub fn load_history() -> PredictionHistory {
    let path = history_path();
    if path.exists() {
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        PredictionHistory::default()
    }
}

fn save_history(history: &PredictionHistory) {
    let path = history_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(history) {
        let _ = fs::write(&path, json);
    }
}

// ── Prediction Engine ──

/// Predict build performance for a package using X-Algo-inspired scoring.
pub fn predict_build(package: &str, is_cold: bool, changed_files: &[String]) -> BuildPrediction {
    let history = load_history();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Dimension 1: Source Delta Score
    let source_delta = compute_source_delta(changed_files, package);
    
    // Dimension 2: Cache Affinity Score
    let cache_affinity = compute_cache_affinity(&source_delta, is_cold, &history);
    
    // Dimension 3: Dependency Graph Sensitivity
    let dep_graph_sensitivity = compute_dep_sensitivity(package);
    
    // Dimension 4: Historical Accuracy
    let historical_accuracy = compute_historical_accuracy(&history);
    
    // Dimension 5: Peer Consensus
    let peer_consensus = compute_peer_consensus(&history);

    // Dimension 6: System Stability (from heatmap)
    let system_stability = crate::heatmap::current_stability_score();

    // Weighted composite prediction (6 dimensions)
    let weights = [0.30, 0.20, 0.12, 0.12, 0.08, 0.18]; // source_delta + stability strongest
    
    let composite = source_delta * weights[0]
        + cache_affinity * weights[1]
        + (1.0 - dep_graph_sensitivity) * weights[2]  // invert: high dep = slower
        + historical_accuracy * weights[3]
        + peer_consensus * weights[4]
        + system_stability * weights[5];  // system health penalizes unstable predictions

    // Map to concrete predictions
    let base_cold_ms: u64 = if is_cold { 3422 } else { 504 };
    let base_incr_ms: u64 = if is_cold { 3422 } else { 504 };
    
    let predicted_ms = if is_cold {
        (base_cold_ms as f64 * (1.0 + source_delta * 2.5)) as u64
    } else {
        (base_incr_ms as f64 * (1.0 + source_delta * 1.8)) as u64
    };

    let predicted_cache_rate = if is_cold {
        0.0
    } else {
        (cache_affinity * 0.95 + 0.05).min(0.99)  // 5-99% range
    };

    let predicted_test_pass = (0.92 + historical_accuracy * 0.07).min(0.99); // 92-99% range

    let confidence = composite.min(0.95);

    let prediction = BuildPrediction {
        predicted_ms,
        predicted_cache_rate,
        predicted_test_pass,
        confidence,
        dimensions: PredictionDimensions {
            source_delta,
            cache_affinity,
            dep_graph_sensitivity,
            historical_accuracy,
            peer_consensus,
            system_stability,
        },
        is_cold,
        timestamp_secs: now,
        package: package.to_string(),
    };

    // Save to history
    let mut hist = history;
    hist.predictions.push(prediction.clone());
    if hist.predictions.len() > 1000 {
        hist.predictions = hist.predictions.split_off(hist.predictions.len() - 500);
    }
    hist.total_predictions += 1;
    save_history(&hist);

    prediction
}

/// After build completes, compare prediction vs reality and record feedback.
pub fn record_feedback(
    prediction: &BuildPrediction,
    actual_ms: u64,
    actual_cache_rate: f64,
    actual_test_pass: bool,
) -> BuildFeedback {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let error_ms = actual_ms as i64 - prediction.predicted_ms as i64;
    let was_accurate = (error_ms.abs() as f64) <= (prediction.predicted_ms as f64 * 0.20);

    let feedback = BuildFeedback {
        prediction: prediction.clone(),
        actual_ms,
        actual_cache_rate,
        actual_test_pass,
        error_ms,
        was_accurate,
        timestamp_secs: now,
    };

    let mut history = load_history();
    history.feedback.push(feedback.clone());
    history.accuracy_window.push(was_accurate);
    if history.accuracy_window.len() > 100 {
        history.accuracy_window = history.accuracy_window.split_off(history.accuracy_window.len() - 50);
    }
    // Rolling average error
    let total_error: i64 = history.feedback.iter().map(|f| f.error_ms.abs()).sum();
    history.avg_error_ms = if history.feedback.is_empty() {
        0.0
    } else {
        total_error as f64 / history.feedback.len() as f64
    };

    save_history(&history);

    feedback
}

// ── Dimension Computers ──

/// Source delta: how much changed based on file count and content hash diff.
fn compute_source_delta(changed_files: &[String], _package: &str) -> f64 {
    if changed_files.is_empty() {
        return 0.0; // no changes → cache hit
    }
    
    let mut total_chars = 0u64;
    let mut hasher = Hasher::new();
    
    for f in changed_files {
        if let Ok(data) = fs::read(f) {
            total_chars += data.len() as u64;
            hasher.update(&data);
        }
    }
    
    // Normalize: 0 lines = 0.0, 10000+ chars = 1.0
    let raw = (total_chars as f64 / 10000.0).min(1.0);
    // Apply sigmoid for smoother curve
    1.0 / (1.0 + (-5.0 * (raw - 0.3)).exp())
}

/// Cache affinity: higher when changes are isolated to leaf crates.
fn compute_cache_affinity(source_delta: &f64, is_cold: bool, history: &PredictionHistory) -> f64 {
    if is_cold {
        return 0.0;
    }
    
    // Low source delta + good history = high cache affinity
    let delta_factor = 1.0 - source_delta;
    
    let hist_factor = if history.predictions.len() > 10 {
        let recent_accurate: f64 = history.accuracy_window.iter()
            .filter(|&&a| a)
            .count() as f64
            / history.accuracy_window.len().max(1) as f64;
        recent_accurate
    } else {
        0.5 // neutral for new projects
    };
    
    (delta_factor * 0.7 + hist_factor * 0.3).clamp(0.0, 1.0)
}

/// Dependency graph sensitivity: leaf crates = low, root crate = high.
fn compute_dep_sensitivity(package: &str) -> f64 {
    compute_dep_sensitivity_inner(package)
}

/// Public re-export for benchmark module.
pub fn compute_dep_sensitivity_for_benchmark(package: &str) -> f64 {
    compute_dep_sensitivity_inner(package)
}

fn compute_dep_sensitivity_inner(package: &str) -> f64 {
    // Heuristic based on known flux crate ordering
    match package {
        "fluxc" => 1.0,          // root: depends on everything
        "flux-gui" => 0.9,       // near-root
        "flux-driver" => 0.8,
        "flux-p2p" => 0.7,
        "flux-mempool" => 0.6,
        "flux-search" => 0.5,
        "flux-hotswap" => 0.5,
        "flux-science" => 0.4,
        "flux-gpu" => 0.4,
        "flux-zk" => 0.3,
        "flux-db" => 0.2,        // leaf: few dependents
        "flux-cache" => 0.1,     // leaf: no dependents
        _ => 0.5,                // unknown
    }
}

/// Historical accuracy of past predictions.
fn compute_historical_accuracy(history: &PredictionHistory) -> f64 {
    if history.accuracy_window.is_empty() {
        return 0.5; // neutral start
    }
    
    let accurate_count = history.accuracy_window.iter().filter(|&&a| a).count();
    accurate_count as f64 / history.accuracy_window.len() as f64
}

/// Peer consensus: agreement with other nodes (placeholder for supercluster).
fn compute_peer_consensus(_history: &PredictionHistory) -> f64 {
    // Phase 1: no supercluster yet, return moderate consensus
    // Phase 2: query supercluster peers for their build times and compare
    0.75
}

// ── Report Generation ──

/// Generate a human-readable prediction report.
pub fn format_prediction(p: &BuildPrediction) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "🔮 Flux Prediction: {}\n  Build: {}ms predicted ({}% confidence)\n  Cache: {}% hit rate\n  Tests: {}% pass probability",
        p.package,
        p.predicted_ms,
        (p.confidence * 100.0) as u32,
        (p.predicted_cache_rate * 100.0) as u32,
        (p.predicted_test_pass * 100.0) as u32,
    ));
    lines.push(format!(
        "  Dimensions: Δsrc={:.2} cache={:.2} dep={:.2} hist={:.2} peer={:.2}",
        p.dimensions.source_delta,
        p.dimensions.cache_affinity,
        p.dimensions.dep_graph_sensitivity,
        p.dimensions.historical_accuracy,
        p.dimensions.peer_consensus,
    ));
    if p.is_cold {
        lines.push("  ⚠ Cold build — expect full recompile".into());
    }
    lines.join("\n")
}

/// Generate a feedback comparison report.
pub fn format_feedback(f: &BuildFeedback) -> String {
    let accuracy = if f.was_accurate { "✓ ACCURATE" } else { "✗ OFF" };
    let direction = if f.error_ms > 0 { "slower" } else if f.error_ms < 0 { "faster" } else { "exact" };
    format!(
        "📊 Feedback: {} ({} than predicted)\n  Predicted: {}ms → Actual: {}ms (Δ{}ms)\n  Cache: {:.0}% predicted vs {:.0}% actual\n  Tests: {:.0}% predicted vs {}",
        accuracy,
        direction,
        f.prediction.predicted_ms,
        f.actual_ms,
        f.error_ms,
        f.prediction.predicted_cache_rate * 100.0,
        f.actual_cache_rate * 100.0,
        f.prediction.predicted_test_pass * 100.0,
        if f.actual_test_pass { "✓ pass" } else { "✗ fail" },
    )
}

/// Generate a compact prediction for webhook payload.
pub fn prediction_webhook_data(p: &BuildPrediction) -> serde_json::Value {
    serde_json::json!({
        "package": p.package,
        "predicted_ms": p.predicted_ms,
        "predicted_cache_rate": p.predicted_cache_rate,
        "predicted_test_pass": p.predicted_test_pass,
        "confidence": p.confidence,
        "is_cold": p.is_cold,
        "source_delta": p.dimensions.source_delta,
        "cache_affinity": p.dimensions.cache_affinity,
        "dep_sensitivity": p.dimensions.dep_graph_sensitivity,
        "historical_accuracy": p.dimensions.historical_accuracy,
        "peer_consensus": p.dimensions.peer_consensus,
    })
}

pub fn feedback_webhook_data(f: &BuildFeedback) -> serde_json::Value {
    serde_json::json!({
        "package": f.prediction.package,
        "predicted_ms": f.prediction.predicted_ms,
        "actual_ms": f.actual_ms,
        "error_ms": f.error_ms,
        "was_accurate": f.was_accurate,
        "predicted_cache": f.prediction.predicted_cache_rate,
        "actual_cache": f.actual_cache_rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    // predict_build / record_feedback / load_history all read+write
    // $HOME/.flux/predictions.json. The shared test_home helper gives each test a
    // fresh, empty HOME under one process-wide lock so the global HOME swap can't
    // interleave with the other modules' persistence tests.
    use crate::test_home::with_temp_home;

    #[test]
    fn test_predict_no_changes() {
        with_temp_home("no_changes", || {
            let p = predict_build("fluxc", false, &[]);
            assert_eq!(p.dimensions.source_delta, 0.0);
            // Fresh history → neutral hist factor (0.5): cache_affinity = 0.7·1 + 0.3·0.5
            // = 0.85 → predicted_cache_rate ≈ 0.857 (rises toward 0.99 as accurate
            // history accumulates). "No changes" still reads as a high cache rate.
            assert!(p.predicted_cache_rate > 0.85, "got {}", p.predicted_cache_rate);
            assert!(!p.is_cold);
        });
    }

    #[test]
    fn test_predict_cold_build() {
        with_temp_home("cold", || {
            let p = predict_build("fluxc", true, &["src/main.rs".into()]);
            assert!(p.is_cold);
            assert_eq!(p.predicted_cache_rate, 0.0);
        });
    }

    #[test]
    fn test_feedback_tracking() {
        with_temp_home("feedback", || {
            let p = predict_build("flux-search", false, &[]);
            // Feed an actual within ±20% of the prediction so it's marked accurate.
            // (The old test fed a value ~50% off yet still asserted accuracy.)
            let actual = (p.predicted_ms as f64 * 0.9) as u64;
            let f = record_feedback(&p, actual, 0.95, true);
            assert!(f.was_accurate, "|{} - {}| should be within 20%", actual, p.predicted_ms);
            assert!(f.error_ms.abs() <= (p.predicted_ms as i64 / 5).max(1));
        });
    }

    #[test]
    fn test_dep_sensitivity_ordering() {
        assert!(compute_dep_sensitivity("fluxc") > compute_dep_sensitivity("flux-cache"));
        assert!(compute_dep_sensitivity("fluxc") > compute_dep_sensitivity("flux-db"));
        assert_eq!(compute_dep_sensitivity("flux-cache"), 0.1);
    }

    #[test]
    fn test_history_persistence_roundtrip() {
        with_temp_home("roundtrip", || {
            let p = predict_build("fluxc", false, &[]);
            let _f = record_feedback(&p, 500, 0.80, true);

            let hist = load_history();
            assert!(hist.total_predictions >= 1);
            assert!(!hist.feedback.is_empty());
        });
    }
}
