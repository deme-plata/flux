// fluxc-core cortex bridge — wraps flux-cortex for CLI dispatch.
// Provides `fluxc cortex` and `fluxc architect` CLI entry points.

use flux_cortex::Cortex;

use flux_cortex::ai_cortex::{AiCortex, AiTaskKind};

/// Run the AI Cortex — AI-native development platform.
/// Routes development tasks through the agent registry, learning which models
/// produce the best results for each task type.
pub fn run_ai_cortex(target: &str, mode: &str, iterations: usize, json: bool) {
    let root = fluxc_util::version::workspace_root();

    let modes: Vec<AiTaskKind> = match mode {
        "heal" => vec![AiTaskKind::Heal],
        "suggest" => vec![AiTaskKind::Suggest],
        "test" => vec![AiTaskKind::Test],
        "webhook" => vec![AiTaskKind::WebhookGen],
        "review" => vec![AiTaskKind::Review],
        "optimize" => vec![AiTaskKind::Optimize],
        "full" => vec![
            AiTaskKind::Review,
            AiTaskKind::Suggest,
            AiTaskKind::Heal,
            AiTaskKind::Test,
        ],
        _ => {
            eprintln!("❌ Unknown AI cortex mode: {}", mode);
            eprintln!("  Modes: heal, suggest, test, webhook, review, optimize, full");
            return;
        }
    };

    println!("⚡ Flux AI Cortex — {}", mode);
    println!("  Target: {}", target);
    println!("  Agents: {}", flux_cortex::ai_cortex::default_agent_registry().len());

    let mut cortex = AiCortex::new(root);

    if iterations > 1 {
        let results = cortex.run_continuous(iterations, target, &modes);
        if json {
            println!("{}", serde_json::to_string_pretty(&results).unwrap_or_default());
        } else {
            for r in &results {
                println!(
                    "  Loop {}: {} tasks, {} succeeded, {:.0}% learning",
                    r.iteration, r.tasks_dispatched, r.tasks_succeeded,
                    r.learning_improvement * 100.0
                );
            }
        }
    } else {
        let result = cortex.run_iteration(target, &modes);
        if json {
            println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
        } else {
            println!("  Iteration {} — {}ms", result.iteration, result.duration_ms);
            println!("  Tasks: {} dispatched, {} succeeded, {} failed",
                result.tasks_dispatched, result.tasks_succeeded, result.tasks_failed);
            if let Some(ref best) = result.best_agent {
                println!("  Best agent: {}", best);
            }
            println!("  Agent scores:");
            for (id, score) in &result.agent_scores {
                println!("    {}: {:.2}", id, score);
            }
        }
    }

    // Print summary
    let summary = cortex.summary();
    println!("\n  Total: {} tasks, {:.0}% success rate, {} tokens",
        summary.total_tasks, summary.success_rate * 100.0, summary.total_tokens);
    if let Some(ref best) = summary.best_agent {
        println!("  Top agent: {}", best);
    }
    if summary.learning_plateau {
        println!("  ⚠ Learning plateaued — consider adding new agents or tasks");
    }
}

/// Run the Cortex autonomous optimization loop against the current workspace.
pub fn run_cortex_loop(preset: &str, iterations: usize, json: bool) {
    use flux_graph::workspace::discover_members;
    use flux_graph::{CrateInfo, WorkspaceGraph, CrateType};
    use flux_optimize::OptimizationPreset;

    let root = fluxc_util::version::workspace_root();
    let members = match discover_members(&root) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("❌ cortex: workspace discovery failed — {e}");
            return;
        }
    };

    let crates: Vec<CrateInfo> = members
        .iter()
        .filter_map(|p| {
            let name = p.file_name()?.to_string_lossy().to_string();
            Some(CrateInfo {
                name,
                path: p.clone(),
                dependencies: vec![],
                edition: "2021".to_string(),
                crate_type: CrateType::Lib,
                features: vec![],
            })
        })
        .collect();

    let ws = WorkspaceGraph {
        root,
        crates,
        batches: vec![],
    };

    let preset = match preset.to_lowercase().as_str() {
        "powersaver" | "power_saver" => OptimizationPreset::PowerSaver,
        "balanced" => OptimizationPreset::Balanced,
        _ => OptimizationPreset::MaxPerf,
    };

    let mut cortex = Cortex::with_persistence(ws);

    if iterations > 1 {
        let results = cortex.run_continuous(iterations, preset);
        if json {
            println!("{}", serde_json::to_string_pretty(&results).unwrap_or_default());
        } else {
            println!("⚡ Cortex — {} continuous loops complete", results.len());
            for r in &results {
                println!(
                    "  Loop {}: {} actions generated, {} applied, {:.2}% gain, {:.0}% learning",
                    r.iteration,
                    r.actions_generated,
                    r.actions_applied,
                    r.actual_total_gain_pct.unwrap_or(0.0),
                    r.learning_improvement * 100.0
                );
            }
            let total_gain: f64 = results.iter().filter_map(|r| r.actual_total_gain_pct).sum();
            println!("  Total gain: {:.2}%", total_gain);
        }
    } else {
        let result = cortex.run_loop(preset);
        if json {
            println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
        } else {
            println!("⚡ Cortex Loop #{} — {}ms", result.iteration, result.loop_duration_ms);
            println!("  Architecture score: {:.1}%", result.arch_score_before * 100.0);
            println!("  Findings: {} | Actions generated: {} | Applied: {}",
                result.findings_count, result.actions_generated, result.actions_applied);
            println!("  Est. gain: {:.2}% | Actual gain: {:.2}%",
                result.estimated_total_gain_pct,
                result.actual_total_gain_pct.unwrap_or(0.0));
            println!("  Learning improvement: {:.2}%", result.learning_improvement * 100.0);
            if !result.top_actions.is_empty() {
                println!("  Top actions:");
                for a in &result.top_actions {
                    println!(
                        "    • [{}] {} ({:.1}% impact, {:.0}% conf, {})",
                        a.dimension, a.description, a.estimated_impact_pct, a.confidence * 100.0, a.effort
                    );
                }
            }
        }
    }
}

/// Run the cortex summary report.
pub fn run_cortex_summary(json: bool) {
    use flux_graph::workspace::discover_members;
    use flux_graph::{CrateInfo, WorkspaceGraph, CrateType};

    let root = fluxc_util::version::workspace_root();
    let members = match discover_members(&root) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("❌ cortex: workspace discovery failed — {e}");
            return;
        }
    };

    let crates: Vec<CrateInfo> = members
        .iter()
        .filter_map(|p| {
            let name = p.file_name()?.to_string_lossy().to_string();
            Some(CrateInfo {
                name,
                path: p.clone(),
                dependencies: vec![],
                edition: "2021".to_string(),
                crate_type: CrateType::Lib,
                features: vec![],
            })
        })
        .collect();

    let ws = WorkspaceGraph {
        root,
        crates,
        batches: vec![],
    };

    let cortex = Cortex::with_persistence(ws);
    let summary = cortex.summary();

    if json {
        println!("{}", serde_json::to_string_pretty(&summary).unwrap_or_default());
    } else {
        println!("⚡ Cortex Summary");
        println!("  Iterations: {} | Actions: {} generated / {} applied",
            summary.total_iterations, summary.total_actions_generated, summary.total_actions_applied);
        println!("  Cumulative perf gain: {:.2}%", summary.cumulative_perf_gain_pct);
        println!("  Prediction accuracy: {:.1}% | Accurate predictions: {}",
            summary.prediction_accuracy * 100.0, summary.accurate_predictions);
        if let Some((crate_name, score)) = &summary.top_crate {
            println!("  Top crate: {} ({:.1}%)", crate_name, score * 100.0);
        }
        println!("  Learning plateaued: {}", if summary.learning_plateau { "yes" } else { "no" });
    }
}
