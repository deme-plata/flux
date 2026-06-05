// handler_extract.rs — Auto-extract match arms into handler modules.
//
// Phase 2 (v0.2.0): Parse a Rust match statement and split arms into
// functional groups based on name prefixes. Generates handler module files
// and ToolRegistry wiring.
//
// Stub for v0.1.0 — returns a placeholder.

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExtractedModule {
    pub name: String,
    pub tools: Vec<String>,
    pub content: String,
}

/// Analyze tool names for functional grouping.
pub fn group_tools(tool_names: &[String]) -> Vec<Vec<String>> {
    // Group by prefix pattern
    let groups = vec![
        ("build", vec!["flux_compile", "flux_iterate", "flux_format", "flux_batch_compile", "flux_self_build"]),
        ("test", vec!["flux_test", "flux_combo", "flux_quickcast", "flux_ult"]),
        ("stats", vec!["flux_stats", "flux_version", "flux_bench", "flux_benchmark", "flux_health_report", "flux_heatmap", "flux_benchmark_history"]),
        ("predict", vec!["flux_predict", "flux_predict_batch", "flux_feedback", "flux_qspec"]),
        ("webhook", vec!["flux_webhook_register", "flux_webhook_list", "flux_webhook_trigger", "flux_webhook_test"]),
        ("session", vec!["flux_quickstart", "flux_bootstrap", "flux_fullcheck", "flux_v4_optimize", "flux_diagnose", "flux_quantum_architect", "flux_swot", "flux_optimize"]),
        ("ops", vec!["flux_gpu", "flux_sign", "flux_hot_swap", "flux_zk_batch", "flux_zk_compose", "flux_search", "flux_search_index", "flux_cache_clear", "flux_peer_list", "flux_tune", "flux_tune_status", "flux_deploy", "flux_sap_status", "flux_sniff"]),
    ];

    let mut result = Vec::new();
    for (name, patterns) in groups {
        let matching: Vec<String> = tool_names.iter()
            .filter(|n| patterns.contains(&n.as_str()))
            .cloned()
            .collect();
        if !matching.is_empty() {
            result.push(matching);
        }
    }
    result
}

/// Suggested module structure for a set of tool names.
pub fn suggest_modules(tool_names: &[String]) -> Vec<ExtractedModule> {
    let groups = group_tools(tool_names);
    groups.into_iter().enumerate().map(|(i, tools)| {
        ExtractedModule {
            name: format!("handler_{}", i),
            tools: tools.clone(),
            content: format!("// {} tools: {:?}", i, tools),
        }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_tools() {
        let tools: Vec<String> = vec!["flux_compile", "flux_test", "flux_predict"]
            .into_iter().map(String::from).collect();
        let groups = group_tools(&tools);
        assert!(groups.len() >= 2, "expected at least 2 groups");
    }
}
