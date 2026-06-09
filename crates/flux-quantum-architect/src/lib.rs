// flux_quantum_architect — Quantum Architecture Oracle
//
// A quantum-inspired algorithm that analyzes the codebase, computes the
// "Platonic ideal" architecture, measures the gap between current and ideal,
// and generates a prioritized blueprint to close that gap.
//
// Quantum Superpowers:
//   1. Superposition  — all possible architectures exist simultaneously
//   2. Entanglement   — changes in one crate ripple through dependents
//   3. Collapse       — observe the optimal architecture via scoring
//   4. Tunneling      — skip intermediate states, go straight to ideal
//
// Architecture Dimensions:
//   1. Cohesion Score       — how well each crate's internals belong together
//   2. Coupling Score       — how cleanly crates depend on each other (low = good)
//   3. Test Coverage Gap    — % of code without tests
//   4. Documentation Debt   — public APIs without docs
//   5. Dependency Health    — are deps fresh, minimal, correct?
//   6. Code Entropy          — how far from ideal structure (0 = perfect)

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

// ── Architecture Model ──

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CrateBlueprint {
    pub name: String,
    pub path: String,
    /// Lines of code
    pub loc: usize,
    /// Number of public items
    pub public_items: usize,
    /// Number of dependencies
    pub dependencies: Vec<String>,
    /// Number of dependents (who depends on this crate)
    pub dependents: Vec<String>,
    /// Architecture scores
    pub scores: ArchitectureScores,
    /// Gap to ideal (0.0 = perfect, 1.0 = needs full rewrite)
    pub gap_to_ideal: f64,
    /// Prioritized recommendations
    pub recommendations: Vec<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ArchitectureScores {
    pub cohesion: f64,          // 0–1, higher = better
    pub coupling: f64,          // 0–1, lower = better (inverted in composite)
    pub test_coverage: f64,     // 0–1
    pub documentation: f64,     // 0–1
    pub dependency_health: f64, // 0–1
    pub code_entropy: f64,      // 0–1, lower = better (inverted in composite)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QuantumArchitecture {
    /// The workspace root
    pub root: String,
    /// All crate blueprints
    pub crates: Vec<CrateBlueprint>,
    /// Dependency graph (crate → dependents)
    pub dep_graph: HashMap<String, Vec<String>>,
    /// Overall architecture score (0–1)
    pub architecture_score: f64,
    /// Number of crates in superposition (all analyzed)
    pub superposition_count: usize,
    /// Top 5 prioritized actions
    pub priority_actions: Vec<PriorityAction>,
    /// Time to compute (quantum speed)
    pub compute_ms: u128,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PriorityAction {
    pub rank: usize,
    pub crate_name: String,
    pub action: String,
    pub impact: f64,           // 0–1, how much this improves architecture score
    pub effort: String,        // "low", "medium", "high"
    pub estimated_minutes: u64,
}

// ── Workspace Analysis ──

/// Analyze the Flux workspace and compute quantum architecture blueprint.
pub fn analyze_workspace(workspace_root: &str) -> QuantumArchitecture {
    let start = Instant::now();
    let root = PathBuf::from(workspace_root);
    
    // Step 1: Discover all crates
    let crates_dir = root.join("crates");
    let crate_names = discover_crates(&crates_dir);
    
    // Step 2: Build dependency graph from Cargo.toml files
    let dep_graph = build_dep_graph(&crates_dir, &crate_names);
    
    // Step 3: Quantum analysis — score each crate in superposition
    let mut blueprints = Vec::new();
    for name in &crate_names {
        let crate_path = crates_dir.join(name);
        let bp = analyze_crate(&crate_path, name, &dep_graph);
        blueprints.push(bp);
    }
    
    // Step 4: Collapse superposition → compute composite architecture score
    let total_crates = blueprints.len() as f64;
    let avg_gap: f64 = blueprints.iter().map(|b| b.gap_to_ideal).sum::<f64>() / total_crates;
    let architecture_score = 1.0 - avg_gap;
    
    // Step 5: Generate priority actions (quantum tunneling shortcuts)
    let priority_actions = generate_priority_actions(&blueprints);
    
    let compute_ms = start.elapsed().as_millis();
    
    QuantumArchitecture {
        root: workspace_root.to_string(),
        crates: blueprints,
        dep_graph,
        architecture_score,
        superposition_count: crate_names.len(),
        priority_actions,
        compute_ms,
    }
}

fn discover_crates(crates_dir: &PathBuf) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(entries) = fs::read_dir(crates_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if path.join("Cargo.toml").exists() {
                        names.push(name.to_string());
                    }
                }
            }
        }
    }
    names.sort();
    names
}

fn build_dep_graph(crates_dir: &PathBuf, crate_names: &[String]) -> HashMap<String, Vec<String>> {
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    
    for name in crate_names {
        let cargo_toml = crates_dir.join(name).join("Cargo.toml");
        if let Ok(content) = fs::read_to_string(&cargo_toml) {
            let deps: Vec<String> = crate_names
                .iter()
                .filter(|dep| *dep != name && content.contains(&format!("../{}", dep)))
                .cloned()
                .collect();
            graph.insert(name.clone(), deps);
        }
    }
    
    // Compute dependents (reverse edges)
    let mut full_graph: HashMap<String, Vec<String>> = HashMap::new();
    for name in crate_names {
        let dependents: Vec<String> = graph
            .iter()
            .filter(|(_, deps)| deps.contains(name))
            .map(|(n, _)| n.clone())
            .collect();
        full_graph.insert(name.clone(), dependents);
    }
    
    full_graph
}

fn analyze_crate(crate_path: &PathBuf, name: &str, dep_graph: &HashMap<String, Vec<String>>) -> CrateBlueprint {
    let src_dir = crate_path.join("src");
    
    // Count lines of code
    let (loc, public_items) = count_code_metrics(&src_dir);
    
    // Count dependencies and dependents
    let dependencies: Vec<String> = dep_graph.iter()
        .filter(|(_n, deps)| deps.contains(&name.to_string()))
        .map(|(n, _)| n.clone())
        .collect();
    let dependents = dep_graph.get(name).cloned().unwrap_or_default();
    
    // Score the crate
    let scores = score_crate_architecture(name, loc, &dependencies, &dependents, &src_dir, public_items);
    
    // Compute gap to ideal
    let gap = compute_gap(&scores);
    
    // Generate recommendations
    let recommendations = generate_recommendations(name, &scores, gap);
    
    CrateBlueprint {
        name: name.to_string(),
        path: crate_path.to_string_lossy().to_string(),
        loc,
        public_items,
        dependencies,
        dependents,
        scores,
        gap_to_ideal: gap,
        recommendations,
    }
}

fn count_code_metrics(src_dir: &PathBuf) -> (usize, usize) {
    let mut loc = 0usize;
    let mut public_items = 0usize;
    
    if let Ok(entries) = fs::read_dir(src_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "rs") {
                if let Ok(content) = fs::read_to_string(&path) {
                    loc += content.lines().count();
                    public_items += content.lines()
                        .filter(|l| l.trim_start().starts_with("pub "))
                        .count();
                }
            }
        }
    }
    
    (loc, public_items)
}

fn score_crate_architecture(
    _name: &str,
    loc: usize,
    dependencies: &[String],
    dependents: &[String],
    src_dir: &PathBuf,
    public_items: usize,
) -> ArchitectureScores {
    // Cohesion: well-structured crates have 50-2000 LOC, not too big or small
    let cohesion = if loc == 0 {
        0.5
    } else if loc < 50 {
        0.6 // too small, might need merging
    } else if loc <= 500 {
        0.95 // sweet spot
    } else if loc <= 2000 {
        0.85
    } else {
        0.7 // too large, might need splitting
    };
    
    // Coupling: fewer deps = better (inverted)
    let coupling_raw = dependencies.len() as f64 / 12.0; // 12 = max crates
    let coupling = (1.0 - coupling_raw).max(0.1);
    
    // Test coverage: check for test files
    let test_coverage = estimate_test_coverage(src_dir, loc);
    
    // Documentation: check for doc comments on public items
    let documentation = estimate_documentation(src_dir);
    
    // Dependency health: leaf crates are healthiest
    let dep_health = if dependents.is_empty() {
        1.0 // leaf crate = no one depends on it being wrong
    } else if dependents.len() <= 2 {
        0.8
    } else if dependents.len() <= 5 {
        0.6
    } else {
        0.4 // heavy root crate = high blast radius
    };
    
    // Code entropy: measure structural complexity
    let code_entropy = estimate_entropy(loc, public_items, dependencies);
    
    ArchitectureScores {
        cohesion,
        coupling,
        test_coverage,
        documentation,
        dependency_health: dep_health,
        code_entropy,
    }
}

fn estimate_test_coverage(src_dir: &PathBuf, loc: usize) -> f64 {
    if loc == 0 { return 0.5; }
    
    let mut test_loc = 0usize;
    if let Ok(entries) = fs::read_dir(src_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(content) = fs::read_to_string(&path) {
                test_loc += content.lines()
                    .filter(|l| l.trim_start().starts_with("#[test]") || l.trim_start().starts_with("#[cfg(test)]"))
                    .count();
            }
        }
    }
    
    // Heuristic: each test annotation = ~10 lines of test code
    let estimated_test_lines = test_loc * 10;
    let ratio = estimated_test_lines as f64 / loc.max(1) as f64;
    (ratio.min(1.0) * 0.9 + 0.1).min(1.0) // 10% base coverage
}

fn estimate_documentation(src_dir: &PathBuf) -> f64 {
    let mut doc_lines = 0usize;
    let mut pub_lines = 0usize;
    
    if let Ok(entries) = fs::read_dir(src_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "rs") {
                if let Ok(content) = fs::read_to_string(&path) {
                    let mut _in_doc = false;
                    for line in content.lines() {
                        let trimmed = line.trim();
                        if trimmed.starts_with("///") || trimmed.starts_with("//!") {
                            doc_lines += 1;
                            _in_doc = true;
                        } else if trimmed.starts_with("pub ") && !trimmed.starts_with("pub mod") {
                            pub_lines += 1;
                            _in_doc = false;
                        }
                    }
                }
            }
        }
    }
    
    if pub_lines == 0 { return 0.7; } // no public API = easy to document
    let ratio = doc_lines as f64 / pub_lines.max(1) as f64;
    (ratio.min(3.0) / 3.0).max(0.2) // normalize: 3 doc lines per public item = 1.0
}

fn estimate_entropy(loc: usize, public_items: usize, dependencies: &[String]) -> f64 {
    if loc == 0 { return 0.3; }
    
    // Entropy based on ratio of public to total, and dependency count
    let public_ratio = public_items as f64 / loc.max(1) as f64;
    let dep_factor = (dependencies.len() as f64 / 5.0).min(1.0);
    
    // High public ratio + high deps = high entropy (complex)
    let entropy = (public_ratio * 5.0 + dep_factor * 3.0) / 4.0;
    entropy.min(1.0).max(0.1)
}

fn compute_gap(scores: &ArchitectureScores) -> f64 {
    let ideal = vec![1.0, 1.0, 1.0, 1.0, 1.0, 0.0]; // [cohesion, coupling, tests, docs, dep_health, entropy=0]
    let current = vec![
        scores.cohesion,
        scores.coupling,
        scores.test_coverage,
        scores.documentation,
        scores.dependency_health,
        1.0 - scores.code_entropy, // invert entropy
    ];
    
    let weights = [0.15, 0.20, 0.25, 0.15, 0.15, 0.10];
    let gap: f64 = ideal.iter().zip(current.iter()).zip(weights.iter())
        .map(|((i, c), w)| (i - c).abs() * w)
        .sum();
    
    gap.min(1.0)
}

fn generate_recommendations(name: &str, scores: &ArchitectureScores, _gap: f64) -> Vec<String> {
    let mut recs = Vec::new();
    
    if scores.cohesion < 0.7 {
        recs.push(format!("Split {} into smaller modules (cohesion {:.0}%)", name, scores.cohesion * 100.0));
    }
    if scores.coupling < 0.6 {
        recs.push(format!("Reduce dependencies in {} (coupling {:.0}%)", name, scores.coupling * 100.0));
    }
    if scores.test_coverage < 0.6 {
        recs.push(format!("Add tests to {} (coverage est. {:.0}%)", name, scores.test_coverage * 100.0));
    }
    if scores.documentation < 0.5 {
        recs.push(format!("Document public API in {} (docs {:.0}%)", name, scores.documentation * 100.0));
    }
    if scores.code_entropy > 0.6 {
        recs.push(format!("Reduce complexity in {} (entropy {:.0}%)", name, scores.code_entropy * 100.0));
    }
    
    if recs.is_empty() {
        recs.push(format!("{} looks clean — maintain current quality", name));
    }
    
    recs
}

fn generate_priority_actions(blueprints: &[CrateBlueprint]) -> Vec<PriorityAction> {
    let mut actions: Vec<PriorityAction> = Vec::new();
    
    for bp in blueprints {
        // Each recommendation becomes a priority action
        for rec in &bp.recommendations {
            if rec.contains("looks clean") { continue; }
            
            let impact = bp.gap_to_ideal;
            let effort = if bp.loc < 100 { "low" } else if bp.loc < 500 { "medium" } else { "high" };
            let est_minutes = match effort {
                "low" => 15,
                "medium" => 45,
                "high" => 120,
                _ => 30,
            };
            
            actions.push(PriorityAction {
                rank: 0, // will be set after sort
                crate_name: bp.name.clone(),
                action: rec.clone(),
                impact,
                effort: effort.to_string(),
                estimated_minutes: est_minutes,
            });
        }
    }
    
    // Sort by impact descending
    actions.sort_by(|a, b| b.impact.partial_cmp(&a.impact).unwrap_or(std::cmp::Ordering::Equal));
    
    // Assign ranks
    for (i, action) in actions.iter_mut().enumerate() {
        action.rank = i + 1;
    }
    
    actions.truncate(10); // top 10
    actions
}

// ── SWOT Analysis ──

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SwotAnalysis {
    pub strengths: Vec<String>,
    pub weaknesses: Vec<String>,
    pub opportunities: Vec<String>,
    pub threats: Vec<String>,
    pub overall_score: f64,
    pub top_priority: String,
}

pub fn generate_swot(architecture: &QuantumArchitecture) -> SwotAnalysis {
    let mut strengths = Vec::new();
    let mut weaknesses = Vec::new();
    let mut opportunities = Vec::new();
    let mut threats = Vec::new();
    
    // Strengths: crates with high scores
    for bp in &architecture.crates {
        if bp.gap_to_ideal < 0.2 {
            strengths.push(format!("{}: near-ideal architecture (gap {:.0}%)", bp.name, bp.gap_to_ideal * 100.0));
        }
        if bp.scores.coupling > 0.8 {
            strengths.push(format!("{}: clean dependency boundaries ({:.0}%)", bp.name, bp.scores.coupling * 100.0));
        }
    }
    
    // Weaknesses: crates with high gaps
    for bp in &architecture.crates {
        if bp.gap_to_ideal > 0.5 {
            weaknesses.push(format!("{}: needs refactor (gap {:.0}%, {} LOC)", bp.name, bp.gap_to_ideal * 100.0, bp.loc));
        }
        if bp.scores.test_coverage < 0.5 {
            weaknesses.push(format!("{}: low test coverage (est. {:.0}%)", bp.name, bp.scores.test_coverage * 100.0));
        }
    }
    
    // Opportunities: features that would improve architecture
    let total_loc: usize = architecture.crates.iter().map(|b| b.loc).sum();
    opportunities.push(format!("Workspace: {} crates, {} LOC total — ripe for supercluster distribution", architecture.crates.len(), total_loc));
    opportunities.push("Webhook-native architecture enables CI/CD integration".into());
    opportunities.push("X-Algo prediction can pre-empt build failures before they happen".into());
    opportunities.push("Q-Spec can eliminate fix→compile→fail loops entirely".into());
    
    // Threats
    let high_entropy: Vec<&str> = architecture.crates.iter()
        .filter(|b| b.scores.code_entropy > 0.6)
        .map(|b| b.name.as_str())
        .collect();
    if !high_entropy.is_empty() {
        threats.push(format!("Code entropy rising in: {}", high_entropy.join(", ")));
    }
    
    let heavy_crates: Vec<&str> = architecture.crates.iter()
        .filter(|b| b.dependents.len() > 5)
        .map(|b| b.name.as_str())
        .collect();
    if !heavy_crates.is_empty() {
        threats.push(format!("High blast-radius crates: {} — changes ripple widely", heavy_crates.join(", ")));
    }
    
    threats.push("Single point of failure: fluxc is the root crate with maximum blast radius".into());
    
    let overall_score = architecture.architecture_score;
    let top_priority = architecture.priority_actions.first()
        .map(|a| a.action.clone())
        .unwrap_or_else(|| "No urgent actions needed".into());
    
    SwotAnalysis {
        strengths,
        weaknesses,
        opportunities,
        threats,
        overall_score,
        top_priority,
    }
}

// ── Formatting ──

pub fn format_architecture(arch: &QuantumArchitecture) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "⚛️  Quantum Architecture — {} crates in {}ms\n  Score: {:.1}% ideal",
        arch.superposition_count,
        arch.compute_ms,
        arch.architecture_score * 100.0,
    ));
    lines.push(String::new());
    
    for bp in &arch.crates {
        let bar = "█".repeat((bp.scores.cohesion * 10.0) as usize);
        lines.push(format!(
            "  {} [{:.0}%] {} {} LOC, {} dep(s)",
            bp.name,
            (1.0 - bp.gap_to_ideal) * 100.0,
            bar,
            bp.loc,
            bp.dependents.len(),
        ));
    }
    
    if !arch.priority_actions.is_empty() {
        lines.push(String::new());
        lines.push("  ▶ Priority Actions (quantum tunneling shortcuts):".into());
        for action in &arch.priority_actions {
            lines.push(format!(
                "    #{}. {} — {} (impact {:.0}%, {} effort, ~{}min)",
                action.rank,
                action.crate_name,
                action.action,
                action.impact * 100.0,
                action.effort,
                action.estimated_minutes,
            ));
        }
    }
    
    lines.join("\n")
}

pub fn format_swot(swot: &SwotAnalysis) -> String {
    let mut lines = Vec::new();
    lines.push(format!("📊 SWOT Analysis — Architecture Score: {:.1}%", swot.overall_score * 100.0));
    
    lines.push("\n💪 Strengths:".into());
    for s in &swot.strengths { lines.push(format!("  ✓ {}", s)); }
    if swot.strengths.is_empty() { lines.push("  (none identified)".into()); }
    
    lines.push("\n🔧 Weaknesses:".into());
    for w in &swot.weaknesses { lines.push(format!("  ✗ {}", w)); }
    if swot.weaknesses.is_empty() { lines.push("  (none identified)".into()); }
    
    lines.push("\n🚀 Opportunities:".into());
    for o in &swot.opportunities { lines.push(format!("  → {}", o)); }
    
    lines.push("\n⚠️ Threats:".into());
    for t in &swot.threats { lines.push(format!("  ⚡ {}", t)); }
    
    lines.push(format!("\n🎯 Top Priority: {}", swot.top_priority));
    
    lines.join("\n")
}

pub fn architecture_webhook_data(arch: &QuantumArchitecture) -> serde_json::Value {
    serde_json::json!({
        "crates": arch.crates.len(),
        "architecture_score": arch.architecture_score,
        "compute_ms": arch.compute_ms,
        "priority_count": arch.priority_actions.len(),
        "top_action": arch.priority_actions.first().map(|a| &a.action),
    })
}

pub fn swot_webhook_data(swot: &SwotAnalysis) -> serde_json::Value {
    serde_json::json!({
        "overall_score": swot.overall_score,
        "strengths": swot.strengths.len(),
        "weaknesses": swot.weaknesses.len(),
        "opportunities": swot.opportunities.len(),
        "threats": swot.threats.len(),
        "top_priority": swot.top_priority,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discover_crates() {
        let names = discover_crates(&PathBuf::from("crates"));
        assert!(names.contains(&"fluxc".to_string()));
        assert!(names.contains(&"flux-cache".to_string()));
    }

    #[test]
    fn test_analyze_single_crate() {
        let dep_graph = HashMap::new();
        let bp = analyze_crate(
            &PathBuf::from("crates/flux-cache"),
            "flux-cache",
            &dep_graph,
        );
        assert_eq!(bp.name, "flux-cache");
        assert!(bp.loc > 0);
        assert!(bp.scores.cohesion > 0.0);
    }

    #[test]
    fn test_gap_computation() {
        let scores = ArchitectureScores {
            cohesion: 0.9,
            coupling: 0.8,
            test_coverage: 0.7,
            documentation: 0.6,
            dependency_health: 0.9,
            code_entropy: 0.2,
        };
        let gap = compute_gap(&scores);
        assert!(gap < 0.3);
        assert!(gap > 0.0);
    }

    #[test]
    fn test_swot_generation() {
        let arch = QuantumArchitecture {
            root: ".".into(),
            crates: vec![],
            dep_graph: HashMap::new(),
            architecture_score: 0.75,
            superposition_count: 12,
            priority_actions: vec![PriorityAction {
                rank: 1,
                crate_name: "fluxc".into(),
                action: "Split into smaller modules".into(),
                impact: 0.4,
                effort: "high".into(),
                estimated_minutes: 120,
            }],
            compute_ms: 42,
        };
        let swot = generate_swot(&arch);
        assert!(swot.overall_score > 0.0);
        assert!(!swot.opportunities.is_empty());
    }
}
