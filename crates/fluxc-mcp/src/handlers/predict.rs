use serde_json::{json, Value};
use crate::handlers::{ToolDef, ToolRegistry};
use fluxc_webhooks::webhook;
use fluxc_analytics::predict;

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_predict",
            description: "Predict build performance using X-Algo 5-dimension scoring (source delta, cache affinity, dep graph, historical accuracy, peer consensus). Returns predicted build time, cache rate, test pass probability, and confidence.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "package": {"type": "string", "description": "Package name to predict for"},
                    "is_cold": {"type": "boolean", "description": "Is this a cold build?"},
                    "changed_files": {"type": "array", "items": {"type": "string"}, "description": "List of changed source files"}
                }
            }),
        },
        flux_predict,
    );
    registry.register(
        ToolDef {
            name: "flux_predict_batch",
            description: "Predict build performance for all 15 workspace crates in a single call. Saves 14 round-trips vs individual flux_predict calls. Returns text or JSON.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "format": {"type": "string", "description": "Output format: text (default) or json"}
                }
            }),
        },
        flux_predict_batch,
    );
    registry.register(
        ToolDef {
            name: "flux_feedback",
            description: "Provide feedback on a prediction to improve future predictions. Calibrates the X-Algo model.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "package": {"type": "string", "description": "Package that was built"},
                    "actual_ms": {"type": "integer", "description": "Actual build time in milliseconds"},
                    "actual_cache_rate": {"type": "number", "description": "Actual cache hit rate (0.0-1.0)"},
                    "was_successful": {"type": "boolean", "description": "Did the build succeed?"}
                },
                "required": ["package", "actual_ms"]
            }),
        },
        flux_feedback,
    );
    registry.register(
        ToolDef {
            name: "flux_qspec",
            description: "Quantum Speculation: speculative fix engine. Proposes fixes for build/test failures without running them.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "error": {"type": "string", "description": "Build or test error message to analyze"},
                    "file": {"type": "string", "description": "Source file containing the error"},
                    "package": {"type": "string", "description": "Package to fix"}
                },
                "required": ["error"]
            }),
        },
        flux_qspec,
    );
}

fn flux_predict(args: &Value) -> String {
    let package = args.get("package").and_then(|v| v.as_str()).unwrap_or("fluxc");
    let is_cold = args.get("is_cold").and_then(|v| v.as_bool()).unwrap_or(false);
    let changed_files: Vec<String> = args.get("changed_files")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let prediction = predict::predict_build(package, is_cold, &changed_files);
    webhook::auto_dispatch("prediction_made", predict::prediction_webhook_data(&prediction));
    predict::format_prediction(&prediction)
}

fn flux_predict_batch(args: &Value) -> String {
    let crates: &[&str] = &[
        "flux-cache", "flux-db", "flux-driver", "flux-gpu", "flux-gui",
        "flux-hotswap", "flux-mempool", "flux-p2p", "flux-science",
        "flux-search", "flux-sniff", "flux-zk", "fluxc-core", "fluxc-mcp", "fluxc",
    ];
    let format = args.get("format").and_then(|v| v.as_str()).unwrap_or("text");
    let mut predictions: Vec<predict::BuildPrediction> = Vec::with_capacity(15);
    let mut total_ms: u64 = 0;
    let mut total_cache_rate: f64 = 0.0;
    let mut total_confidence: f64 = 0.0;

    for crate_name in crates {
        let p = predict::predict_build(crate_name, false, &[]);
        total_ms += p.predicted_ms;
        total_cache_rate += p.predicted_cache_rate;
        total_confidence += p.confidence;
        predictions.push(p);
    }

    let n = predictions.len() as f64;
    if format == "json" {
        let items: Vec<Value> = predictions.iter().map(|p| json!({
            "crate": p.package,
            "predicted_ms": p.predicted_ms,
            "cache_rate_pct": (p.predicted_cache_rate * 100.0).round(),
            "test_pass_pct": (p.predicted_test_pass * 100.0).round(),
            "confidence_pct": (p.confidence * 100.0).round(),
        })).collect();
        let report = json!({
            "workspace": "flux-15",
            "crates": n as u64,
            "total_predicted_ms": total_ms,
            "avg_cache_rate_pct": (total_cache_rate / n * 100.0).round(),
            "avg_confidence_pct": (total_confidence / n * 100.0).round(),
            "predictions": items,
        });
        serde_json::to_string_pretty(&report).unwrap_or_else(|e| format!("json error: {}", e))
    } else {
        let mut lines = vec![format!(
            "🔮 Flux Predict Batch — {} crates\n  Total: {}ms · Avg cache: {:.0}% · Avg conf: {:.0}%",
            predictions.len(),
            total_ms,
            total_cache_rate / n * 100.0,
            total_confidence / n * 100.0,
        )];
        for p in &predictions {
            lines.push(format!(
                "  {} — {}ms · {:.0}% cache · {:.0}% conf",
                p.package,
                p.predicted_ms,
                p.predicted_cache_rate * 100.0,
                p.confidence * 100.0,
            ));
        }
        lines.join("\n")
    }
}

fn flux_feedback(args: &Value) -> String {
    let package = args.get("package").and_then(|v| v.as_str()).unwrap_or("fluxc");
    let actual_ms = args.get("actual_ms").and_then(|v| v.as_u64()).unwrap_or(0);
    let actual_cache_rate = args.get("actual_cache_rate").and_then(|v| v.as_f64()).unwrap_or(0.85);
    let was_successful = args.get("was_successful").and_then(|v| v.as_bool()).unwrap_or(true);

    // Get last prediction for this package
    let pred = predict::predict_build(package, false, &[]);
    let fb = predict::record_feedback(&pred, actual_ms, actual_cache_rate, was_successful);

    let event = if fb.was_accurate { "prediction_accurate" } else { "prediction_deviation" };
    webhook::auto_dispatch(event, predict::feedback_webhook_data(&fb));

    if fb.was_accurate {
        format!(
            "✓ Feedback recorded: {} — predicted {}ms, actual {}ms ({}% error)\n  Model calibrated: {} total predictions",
            package,
            fb.prediction.predicted_ms,
            fb.actual_ms,
            (if fb.prediction.predicted_ms > 0 { (fb.error_ms as f64 / fb.prediction.predicted_ms as f64).abs() } else { 0.0 } * 100.0).round(),
            1u64,
        )
    } else {
        format!(
            "⚠ Feedback deviation: {} — predicted {}ms, actual {}ms ({}% off)\n  Model adjusted. {} total predictions.",
            package,
            fb.prediction.predicted_ms,
            fb.actual_ms,
            (if fb.prediction.predicted_ms > 0 { (fb.error_ms as f64 / fb.prediction.predicted_ms as f64).abs() } else { 0.0 } * 100.0).round(),
            1u64,
        )
    }
}

use fluxc_analytics::qspec;

fn flux_qspec(args: &Value) -> String {
    let error = args.get("error").and_then(|v| v.as_str()).unwrap_or("");
    let file = args.get("file").and_then(|v| v.as_str());
    let package = args.get("package").and_then(|v| v.as_str());

    if error.is_empty() {
        return "Error: 'error' parameter is required (the build/test error message)".into();
    }

    let file_path = file.unwrap_or("");
    let spec = qspec::speculate_fixes(file_path, 0, error, "", package.unwrap_or(""));
    qspec::format_qspec_result(&spec)
}
