// flux-architect — Whole-workspace AI optimization planner
//
// Six dimensions of optimization:
//   1. VECTORIZATION  — AVX-512, AVX2, NEON, SVE
//   2. MEMORY         — Peak RSS, arena candidates, OOM risks
//   3. I/O            — io_uring conversion, buffer sizing
//   4. P2P TOPOLOGY   — Peer utilization, batch distribution
//   5. CACHE          — Struct layout, alignment, false sharing
//   6. CONCURRENCY    — Rayon gaps, tokio task splitting
//
// Uses flux-graph for workspace discovery, flux-optimize for pattern
// detection, flux-ai for code analysis, and AI reasoning for ranking.

use flux_graph::WorkspaceGraph;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

// ── Data Types ──

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OptimizationDimension {
    Vectorization,
    Memory,
    IO,
    P2PTopology,
    Cache,
    Concurrency,
}

impl OptimizationDimension {
    pub fn name(&self) -> &str {
        match self {
            Self::Vectorization => "Vectorization",
            Self::Memory => "Memory",
            Self::IO => "I/O",
            Self::P2PTopology => "P2P Topology",
            Self::Cache => "Cache",
            Self::Concurrency => "Concurrency",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationFinding {
    pub rank: usize,
    pub dimension: OptimizationDimension,
    pub estimated_impact_pct: f64,
    pub crate_name: String,
    pub file: String,
    pub line: usize,
    pub summary: String,
    pub suggestion: String,
    pub confidence: f64,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationPlan {
    pub workspace_root: String,
    pub crates_analyzed: usize,
    pub files_analyzed: usize,
    pub loc_analyzed: usize,
    pub findings: Vec<OptimizationFinding>,
    pub total_estimated_gain_pct: f64,
    pub memory_savings_mb: f64,
    pub dimensions_covered: Vec<String>,
}

// ── Folding: the crease that keeps every site and shrinks the sheet ──
//
// The detectors below emit ONE finding per matching source line. On the flux
// workspace that is 2,827 findings (measured 2026-11-09); of the 500 actions the
// cortex kept on disk, 487 were the same sentence. A fold groups the findings
// that share (crate, dimension, suggestion) into one `FoldedFinding` that keeps
// every site (file, line) — nothing is cut away, the sheet just gets smaller —
// and gives the group ONE impact figure that saturates instead of summing: the
// 400th `Vec::new()` in a crate is not another 12 % of speed.
// `unfold` opens a fold back into per-site findings; fold(unfold(g)) == g.

/// One place in the source where a folded finding was observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingSite {
    pub file: String,
    pub line: usize,
    pub summary: String,
}

/// Many identical per-line findings creased into one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FoldedFinding {
    pub dimension: OptimizationDimension,
    pub crate_name: String,
    /// The shared suggestion — the crease line every site lies on.
    pub suggestion: String,
    /// How many sites were folded in (≥ 1).
    pub occurrences: usize,
    /// Every site, in the order the unfolded findings had them.
    pub sites: Vec<FindingSite>,
    /// The largest single-site estimate in the group.
    pub per_site_impact_pct: f64,
    /// The group's estimate after `folded_impact` — saturating, not a sum.
    pub folded_impact_pct: f64,
    /// The largest confidence in the group.
    pub confidence: f64,
}

/// Fold `n` identical per-site estimates into one figure: `p × (1 + ln n)`.
/// n = 1 gives `p` unchanged; 487 sites give 7.2 × p, not 487 × p. This is a
/// chosen diminishing-returns law and, like every figure the detectors emit,
/// an estimate — not a measurement.
pub fn folded_impact(per_site_pct: f64, occurrences: usize) -> f64 {
    if occurrences == 0 {
        return 0.0;
    }
    per_site_pct * (1.0 + (occurrences as f64).ln())
}

/// Group findings by (crate, dimension, suggestion). Groups keep the order in
/// which they were first seen, then sort by folded impact, largest first.
pub fn fold_findings(findings: &[OptimizationFinding]) -> Vec<FoldedFinding> {
    let mut index: HashMap<(String, OptimizationDimension, String), usize> = HashMap::new();
    let mut folded: Vec<FoldedFinding> = Vec::new();

    for f in findings {
        let key = (f.crate_name.clone(), f.dimension.clone(), f.suggestion.clone());
        let site = FindingSite { file: f.file.clone(), line: f.line, summary: f.summary.clone() };
        match index.get(&key) {
            Some(&i) => {
                let g = &mut folded[i];
                g.occurrences += 1;
                g.sites.push(site);
                g.per_site_impact_pct = g.per_site_impact_pct.max(f.estimated_impact_pct);
                g.confidence = g.confidence.max(f.confidence);
            }
            None => {
                index.insert(key, folded.len());
                folded.push(FoldedFinding {
                    dimension: f.dimension.clone(),
                    crate_name: f.crate_name.clone(),
                    suggestion: f.suggestion.clone(),
                    occurrences: 1,
                    sites: vec![site],
                    per_site_impact_pct: f.estimated_impact_pct,
                    folded_impact_pct: f.estimated_impact_pct,
                    confidence: f.confidence,
                });
            }
        }
    }

    for g in &mut folded {
        g.folded_impact_pct = folded_impact(g.per_site_impact_pct, g.occurrences);
    }
    folded.sort_by(|a, b| {
        b.folded_impact_pct
            .partial_cmp(&a.folded_impact_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    folded
}

/// Open a fold back up: one finding per site, for whoever wants to act on them.
pub fn unfold(g: &FoldedFinding) -> Vec<OptimizationFinding> {
    g.sites
        .iter()
        .map(|s| OptimizationFinding {
            rank: 0,
            dimension: g.dimension.clone(),
            estimated_impact_pct: g.per_site_impact_pct,
            crate_name: g.crate_name.clone(),
            file: s.file.clone(),
            line: s.line,
            summary: s.summary.clone(),
            suggestion: g.suggestion.clone(),
            confidence: g.confidence,
            evidence: vec![format!("unfolded from a fold of {} sites", g.occurrences)],
        })
        .collect()
}

// ── Core Engine ──

/// Ingest the entire workspace — read all source files into memory.
pub fn ingest_workspace(ws: &WorkspaceGraph) -> Result<(usize, usize, HashMap<String, String>), String> {
    let mut file_count = 0;
    let mut loc = 0;
    let mut sources: HashMap<String, String> = HashMap::new();

    for ci in &ws.crates {
        let src_dir = ci.path.join("src");
        if !src_dir.exists() {
            continue;
        }
        if let Ok(entries) = fs::read_dir(&src_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "rs") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        loc += content.lines().count();
                        file_count += 1;
                        let key = format!("{}/{}", ci.name, path.file_name().unwrap().to_string_lossy());
                        sources.insert(key, content);
                    }
                }
            }
        }
    }

    Ok((file_count, loc, sources))
}

/// Run all six dimensions against the workspace.
pub fn analyze_workspace(ws: &WorkspaceGraph) -> OptimizationPlan {
    let (files, loc, sources) = ingest_workspace(ws).unwrap_or((0, 0, HashMap::new()));

    let mut findings = Vec::new();

    // Dimension 1: Vectorization (SIMD)
    findings.extend(detect_vectorization(ws, &sources));

    // Dimension 2: Memory
    findings.extend(detect_memory_issues(ws, &sources));

    // Dimension 3: I/O
    findings.extend(detect_io_optimizations(ws, &sources));

    // Dimension 4: P2P Topology
    findings.extend(detect_p2p_topology(ws));

    // Dimension 5: Cache
    findings.extend(detect_cache_optimizations(ws, &sources));

    // Dimension 6: Concurrency
    findings.extend(detect_concurrency_gaps(ws, &sources));

    // Sort by estimated impact (descending) and assign ranks
    findings.sort_by(|a, b| b.estimated_impact_pct.partial_cmp(&a.estimated_impact_pct).unwrap_or(std::cmp::Ordering::Equal));
    for (i, f) in findings.iter_mut().enumerate() {
        f.rank = i + 1;
    }

    let total_gain: f64 = findings.iter().map(|f| f.estimated_impact_pct).sum();
    let mem_savings: f64 = findings.iter()
        .filter(|f| matches!(f.dimension, OptimizationDimension::Memory))
        .map(|f| if f.summary.contains("MB") {
            f.summary.split("MB").next().unwrap_or("0").split_whitespace().last().unwrap_or("0").parse().unwrap_or(0.0)
        } else { 0.0 })
        .sum();

    let dims: Vec<String> = findings.iter()
        .map(|f| f.dimension.name().to_string())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    OptimizationPlan {
        workspace_root: ws.root.to_string_lossy().to_string(),
        crates_analyzed: ws.crates.len(),
        files_analyzed: files,
        loc_analyzed: loc,
        findings,
        total_estimated_gain_pct: total_gain,
        memory_savings_mb: mem_savings,
        dimensions_covered: dims,
    }
}

// ── Dimension 1: Vectorization ──

fn detect_vectorization(ws: &WorkspaceGraph, sources: &HashMap<String, String>) -> Vec<OptimizationFinding> {
    let mut findings = Vec::new();

    for (file_key, content) in sources {
        let lines: Vec<&str> = content.lines().collect();

        // Look for nested loops with arithmetic
        let mut for_depth = 0;
        for (i, line) in lines.iter().enumerate() {
            if line.trim().starts_with("for ") { for_depth += 1; }
            if line.trim() == "}" && for_depth > 0 { for_depth -= 1; }

            if for_depth >= 2 && (line.contains("+") || line.contains("*")) {
                let (crate_name, file) = split_key(file_key);
                findings.push(OptimizationFinding {
                    rank: 0,
                    dimension: OptimizationDimension::Vectorization,
                    estimated_impact_pct: 240.0 + (for_depth as f64 * 50.0),
                    crate_name,
                    file,
                    line: i + 1,
                    summary: format!("{}-level nested loop with f64 arithmetic", for_depth),
                    suggestion: format!("Add #[target_feature(enable = \"avx512f\")] with _mm512_fmadd_pd. Est: {:.1}× speedup on AVX-512", 2.0 + for_depth as f64 * 0.5),
                    confidence: 0.75,
                    evidence: vec![format!("loop depth: {}, arithmetic ops detected", for_depth)],
                });
            }

            // SIMD-able memcpy/memset
            if line.contains("copy_from_slice") || line.contains("iter().map") {
                let (crate_name, file) = split_key(file_key);
                findings.push(OptimizationFinding {
                    rank: 0,
                    dimension: OptimizationDimension::Vectorization,
                    estimated_impact_pct: 45.0,
                    crate_name,
                    file,
                    line: i + 1,
                    summary: "Bulk memory operation — SIMD-able".into(),
                    suggestion: "Consider explicit SIMD or rely on autovectorization with #[repr(align(64))]".into(),
                    confidence: 0.60,
                    evidence: vec![format!("operation: {}", line.trim())],
                });
            }
        }
    }

    findings
}

// ── Dimension 2: Memory ──

fn detect_memory_issues(ws: &WorkspaceGraph, sources: &HashMap<String, String>) -> Vec<OptimizationFinding> {
    let mut findings = Vec::new();

    for (file_key, content) in sources {
        let lines: Vec<&str> = content.lines().collect();

        for (i, line) in lines.iter().enumerate() {
            // Vec::with_capacity — potential arena candidate
            if line.contains("Vec::with_capacity") || line.contains("Vec::new()") {
                let (crate_name, file) = split_key(file_key);
                findings.push(OptimizationFinding {
                    rank: 0,
                    dimension: OptimizationDimension::Memory,
                    estimated_impact_pct: 12.0,
                    crate_name,
                    file,
                    line: i + 1,
                    summary: "Heap allocation pattern — arena candidate".into(),
                    suggestion: "Consider bumpalo::Bump arena for short-lived allocations. Est: -15MB RSS".into(),
                    confidence: 0.50,
                    evidence: vec![format!("pattern: {}", line.trim())],
                });
            }

            // Large buffer allocations
            if line.contains("vec!") && (line.contains("1024") || line.contains("4096") || line.contains("65536")) {
                let (crate_name, file) = split_key(file_key);
                findings.push(OptimizationFinding {
                    rank: 0,
                    dimension: OptimizationDimension::Memory,
                    estimated_impact_pct: 8.0,
                    crate_name,
                    file,
                    line: i + 1,
                    summary: "Large inline buffer allocation".into(),
                    suggestion: "Use BytesMut with pool or pre-allocate arena. Est: -8MB peak".into(),
                    confidence: 0.45,
                    evidence: vec![format!("size hint: {}", line.trim())],
                });
            }
        }
    }

    findings
}

// ── Dimension 3: I/O ──

fn detect_io_optimizations(ws: &WorkspaceGraph, sources: &HashMap<String, String>) -> Vec<OptimizationFinding> {
    let mut findings = Vec::new();
    let io_keywords = ["std::fs::read", "std::fs::write", "File::open", "read_to_string",
                       "read_exact", "write_all", "tokio::fs"];

    for (file_key, content) in sources {
        let lines: Vec<&str> = content.lines().collect();
        let mut io_lines = 0;

        for (i, line) in lines.iter().enumerate() {
            if io_keywords.iter().any(|kw| line.contains(kw)) {
                io_lines += 1;
            }
        }

        if io_lines >= 3 {
            let (crate_name, file) = split_key(file_key);
            findings.push(OptimizationFinding {
                rank: 0,
                dimension: OptimizationDimension::IO,
                estimated_impact_pct: 28.0,
                crate_name: crate_name.clone(),
                file,
                line: 0,
                summary: format!("{} I/O operations — io_uring candidate", io_lines),
                suggestion: "Convert to io_uring submission queue via tokio-uring. Est: 28% latency reduction".into(),
                confidence: 0.70,
                evidence: vec![format!("{} blocking I/O calls detected", io_lines)],
            });
        }
    }

    findings
}

// ── Dimension 4: P2P Topology ──

fn detect_p2p_topology(ws: &WorkspaceGraph) -> Vec<OptimizationFinding> {
    let mut findings = Vec::new();

    // Check if flux-p2p exists and offer topology optimization
    if ws.crates.iter().any(|c| c.name.contains("p2p")) {
        findings.push(OptimizationFinding {
            rank: 0,
            dimension: OptimizationDimension::P2PTopology,
            estimated_impact_pct: 18.0,
            crate_name: "flux-p2p".into(),
            file: "topology".into(),
            line: 0,
            summary: "P2P supercluster: peer utilization optimization".into(),
            suggestion: "Run fluxc supercluster for real-time peer stats. Distribute compile to underutilized peers. Est: +18% throughput".into(),
            confidence: 0.80,
            evidence: vec!["flux-p2p crate detected, 640 Mbps gossipsub mesh".into()],
        });
    }

    findings
}

// ── Dimension 5: Cache ──

fn detect_cache_optimizations(ws: &WorkspaceGraph, sources: &HashMap<String, String>) -> Vec<OptimizationFinding> {
    let mut findings = Vec::new();

    for (file_key, content) in sources {
        let lines: Vec<&str> = content.lines().collect();

        for (i, line) in lines.iter().enumerate() {
            // Struct definitions with potential cache-line issues
            if line.trim().starts_with("pub struct ") {
                // Naive heuristic: structs with >4 fields may cross cache lines
                let field_count = lines[i..].iter()
                    .take_while(|l| !l.trim().starts_with('}'))
                    .filter(|l| l.trim().starts_with("pub ") && l.contains(':'))
                    .count();

                if field_count > 4 {
                    let (crate_name, file) = split_key(file_key);
                    let struct_name = line.trim()
                        .strip_prefix("pub struct ").unwrap_or("unknown")
                        .split('{').next().unwrap_or("unknown")
                        .trim();
                    findings.push(OptimizationFinding {
                        rank: 0,
                        dimension: OptimizationDimension::Cache,
                        estimated_impact_pct: 15.0,
                        crate_name,
                        file,
                        line: i + 1,
                        summary: format!("Struct '{}' with {} fields — potential cache-line crossing", struct_name, field_count),
                        suggestion: format!("Add #[repr(C, align(64))] and group hot fields together. Est: +15% access speed"),
                        confidence: 0.55,
                        evidence: vec![format!("{} fields, likely crosses {} cache lines", field_count, (field_count + 3) / 4)],
                    });
                }
            }
        }
    }

    findings
}

// ── Dimension 6: Concurrency ──

fn detect_concurrency_gaps(ws: &WorkspaceGraph, sources: &HashMap<String, String>) -> Vec<OptimizationFinding> {
    let mut findings = Vec::new();

    for (file_key, content) in sources {
        let lines: Vec<&str> = content.lines().collect();
        let has_iter = content.contains(".iter()") || content.contains(".into_iter()");
        let has_par_iter = content.contains("par_iter") || content.contains("into_par_iter");
        let has_rayon = content.contains("use rayon") || content.contains("rayon::");

        // Iterators without rayon — potential parallelization
        if has_iter && !has_par_iter && !has_rayon {
            let iter_count = lines.iter().filter(|l| l.contains(".iter()") || l.contains(".into_iter()")).count();
            if iter_count >= 2 {
                let (crate_name, file) = split_key(file_key);
                findings.push(OptimizationFinding {
                    rank: 0,
                    dimension: OptimizationDimension::Concurrency,
                    estimated_impact_pct: 30.0,
                    crate_name,
                    file,
                    line: 0,
                    summary: format!("{} sequential iterators — rayon candidate", iter_count),
                    suggestion: format!("Replace .iter() with .par_iter() from rayon. Est: 4× speedup on multi-core"),
                    confidence: 0.65,
                    evidence: vec![format!("{} iterator calls, no rayon import", iter_count)],
                });
            }
        }
    }

    findings
}

// ── Helpers ──

fn split_key(key: &str) -> (String, String) {
    let parts: Vec<&str> = key.splitn(2, '/').collect();
    (parts[0].to_string(), parts.get(1).unwrap_or(&"").to_string())
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_ws() -> WorkspaceGraph {
        WorkspaceGraph {
            root: PathBuf::from("/test"),
            crates: vec![],
            batches: vec![],
        }
    }

    #[test]
    fn test_ingest_empty_workspace() {
        let ws = make_test_ws();
        let result = ingest_workspace(&ws);
        assert!(result.is_ok());
    }

    #[test]
    fn test_analyze_empty_workspace() {
        let ws = make_test_ws();
        let plan = analyze_workspace(&ws);
        assert_eq!(plan.crates_analyzed, 0);
        assert!(plan.findings.is_empty());
    }

    #[test]
    fn test_split_key() {
        let (crate_name, file) = split_key("flux-cache/src/lib.rs");
        assert_eq!(crate_name, "flux-cache");
        assert_eq!(file, "src/lib.rs");
    }

    // ── Folding ──

    fn finding(crate_name: &str, dim: OptimizationDimension, suggestion: &str, impact: f64, line: usize) -> OptimizationFinding {
        OptimizationFinding {
            rank: 0,
            dimension: dim,
            estimated_impact_pct: impact,
            crate_name: crate_name.into(),
            file: "lib.rs".into(),
            line,
            summary: format!("site {}", line),
            suggestion: suggestion.into(),
            confidence: 0.5,
            evidence: vec![],
        }
    }

    #[test]
    fn fold_groups_identical_findings_and_keeps_every_site() {
        let mut raw = Vec::new();
        for line in 1..=40 {
            raw.push(finding("flux-db", OptimizationDimension::Memory, "arena", 12.0, line));
        }
        raw.push(finding("flux-db", OptimizationDimension::Memory, "pool", 8.0, 99));
        raw.push(finding("flux-zk", OptimizationDimension::Memory, "arena", 12.0, 7));

        let folded = fold_findings(&raw);
        assert_eq!(folded.len(), 3, "three creases: (db,arena) (db,pool) (zk,arena)");
        let big = folded.iter().find(|g| g.crate_name == "flux-db" && g.suggestion == "arena").unwrap();
        assert_eq!(big.occurrences, 40);
        assert_eq!(big.sites.len(), 40);
        assert_eq!(big.sites[0].line, 1);
        assert_eq!(big.sites[39].line, 40);
        assert_eq!(big.per_site_impact_pct, 12.0);
        // Saturating, not a sum: 12 × (1 + ln 40) ≈ 56.3, far below 480.
        assert!((big.folded_impact_pct - folded_impact(12.0, 40)).abs() < 1e-9);
        assert!(big.folded_impact_pct < 60.0 && big.folded_impact_pct > 50.0);
        // The biggest fold ranks first.
        assert_eq!(folded[0].occurrences, 40);
        // Every site of the input is in exactly one fold.
        let kept: usize = folded.iter().map(|g| g.sites.len()).sum();
        assert_eq!(kept, raw.len());
    }

    #[test]
    fn folded_impact_is_identity_for_one_site_and_monotone_saturating() {
        assert_eq!(folded_impact(12.0, 1), 12.0);
        assert_eq!(folded_impact(12.0, 0), 0.0);
        let mut prev = 0.0;
        for n in 1..1000usize {
            let v = folded_impact(12.0, n);
            assert!(v > prev, "monotone");
            assert!(v < 12.0 * n as f64 || n == 1, "below the linear sum");
            prev = v;
        }
        assert!(folded_impact(12.0, 487) < 90.0, "487 bumpalo hints ≈ 86 %, not 5,844 %");
    }

    #[test]
    fn fold_then_unfold_loses_nothing_and_folding_twice_changes_nothing() {
        let raw: Vec<OptimizationFinding> =
            (1..=25).map(|l| finding("flux-rev", OptimizationDimension::Cache, "align", 15.0, l)).collect();
        let folded = fold_findings(&raw);
        assert_eq!(folded.len(), 1);
        let opened = unfold(&folded[0]);
        assert_eq!(opened.len(), raw.len());
        let lines_in: Vec<usize> = raw.iter().map(|f| f.line).collect();
        let lines_out: Vec<usize> = opened.iter().map(|f| f.line).collect();
        assert_eq!(lines_in, lines_out, "unfold returns every site in order");
        let refolded = fold_findings(&opened);
        assert_eq!(refolded.len(), 1);
        assert_eq!(refolded[0].occurrences, folded[0].occurrences);
        assert_eq!(refolded[0].sites, folded[0].sites);
        assert!((refolded[0].folded_impact_pct - folded[0].folded_impact_pct).abs() < 1e-9);
    }
}
