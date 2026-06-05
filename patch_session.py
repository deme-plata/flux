import re

path = "/home/storage/deepseek-codewhale/flux/crates/fluxc-mcp/src/handlers/session.rs"
with open(path) as f:
    content = f.read()

# Add registration for flux_architect_predict
old_reg = '''        ToolDef { name: "flux_optimize", description: "Ranked optimization suggestions with impact/effort estimates for each crate.", input_schema: json!({"type": "object", "properties": {"package": {"type": "string", "description": "Specific crate (optional, default: all)"}}}) },
        flux_optimize,
    );
}'''

new_reg = '''        ToolDef { name: "flux_optimize", description: "Ranked optimization suggestions with impact/effort estimates for each crate.", input_schema: json!({"type": "object", "properties": {"package": {"type": "string", "description": "Specific crate (optional, default: all)"}}}) },
        flux_optimize,
    );
    registry.register(
        ToolDef { name: "flux_architect_predict", description: "Architecture blueprint + batch predict in one call. Combines flux_quantum_architect and flux_predict_batch for a complete workspace snapshot.", input_schema: json!({"type": "object", "properties": {"format": {"type": "string", "description": "text or json"}}}) },
        flux_architect_predict,
    );
}'''

content = content.replace(old_reg, new_reg)

# Add handler function at end of file
handler = '''
// ── flux_architect_predict ──

fn flux_architect_predict(args: &Value) -> String {
    let format = args.get("format").and_then(|v| v.as_str()).unwrap_or("text");
    let start = std::time::Instant::now();
    let arch = quantum_architect::analyze_workspace(".");
    let crates: &[&str] = &[
        "flux-cache", "flux-db", "flux-driver", "flux-gpu", "flux-gui",
        "flux-hotswap", "flux-mempool", "flux-p2p", "flux-science",
        "flux-search", "flux-sniff", "flux-zk", "fluxc-core", "fluxc-mcp", "fluxc",
    ];
    let mut total_ms: u64 = 0;
    let mut total_conf: f64 = 0.0;
    let mut preds: Vec<predict::BuildPrediction> = Vec::with_capacity(15);
    for c in crates {
        let p = predict::predict_build(c, false, &[]);
        total_ms += p.predicted_ms;
        total_conf += p.confidence;
        preds.push(p);
    }
    let n = preds.len() as f64;
    let ms = start.elapsed().as_millis();

    if format == "json" {
        let items: Vec<Value> = preds.iter().map(|p| json!({
            "crate": p.package,
            "predicted_ms": p.predicted_ms,
            "cache_rate_pct": (p.predicted_cache_rate * 100.0).round(),
            "test_pass_pct": (p.predicted_test_pass * 100.0).round(),
            "confidence_pct": (p.confidence * 100.0).round(),
        })).collect();
        let report = json!({
            "arch_score_pct": (arch.architecture_score * 100.0).round(),
            "crate_count": arch.crates.len(),
            "total_loc": arch.crates.iter().map(|b| b.loc).sum::<usize>(),
            "total_pred_ms": total_ms,
            "avg_confidence_pct": (total_conf / n * 100.0).round(),
            "elapsed_ms": ms,
            "predictions": items,
        });
        serde_json::to_string_pretty(&report).unwrap_or_else(|e| format!("json: {}", e))
    } else {
        let mut report = vec![format!(
            "🔮 Flux Architect+Predict — {}ms\\n\\n⚛️  Architecture: {:.0}% · {} crates · {} LOC",
            ms, arch.architecture_score * 100.0, arch.crates.len(),
            arch.crates.iter().map(|b| b.loc).sum::<usize>()
        )];
        for bp in &arch.crates {
            report.push(format!("  {} [{:.0}%] {} LOC, {} dep(s)",
                bp.name, ((1.0 - bp.gap_to_ideal) * 100.0), bp.loc, bp.dependencies.len()));
        }
        report.push(format!("\\n🔮 Batch Predict: {} crates · {}ms total · {:.0}% avg conf",
            preds.len(), total_ms, total_conf / n * 100.0));
        for p in &preds {
            report.push(format!("  {} — {}ms · {:.0}% cache · {:.0}% conf",
                p.package, p.predicted_ms,
                p.predicted_cache_rate * 100.0, p.confidence * 100.0));
        }
        if !arch.priority_actions.is_empty() {
            report.push("\\n🎯 Priority Actions:".to_string());
            for a in arch.priority_actions.iter().take(3) {
                report.push(format!("  #{}. {} — {} ({} effort, ~{}min)",
                    a.rank, a.crate_name, a.action, a.effort, a.estimated_minutes));
            }
        }
        report.join("\\n")
    }
}'''

content = content.rstrip() + handler + "\n"

with open(path, 'w') as f:
    f.write(content)
print("OK: Added flux_architect_predict to session.rs")
