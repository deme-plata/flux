// graph.rs — Build dependency DAG, topological sort (Kahn's algorithm),
// cycle detection, and parallel batch grouping.
//
// Takes a Vec<CrateInfo> and produces a Vec<Vec<usize>> where each inner
// Vec is a batch of crate indices that can be compiled in parallel.

use std::collections::VecDeque;
use crate::{CrateInfo, DepKind};

/// Adjacency list representation of the dependency graph.
/// edges[i] = list of crate indices that crate i depends on.
pub struct DepGraph {
    /// edges[i] → crates that i depends on (incoming edges for topological sort)
    pub depends_on: Vec<Vec<usize>>,
    /// reverse_edges[i] → crates that depend on i
    pub depended_by: Vec<Vec<usize>>,
    /// Number of crates
    pub len: usize,
}

/// Build the dependency DAG from parsed crate infos.
/// Returns an error if a cycle is detected.
pub fn build_dag(crates: &[CrateInfo]) -> Result<DepGraph, String> {
    let n = crates.len();
    let mut depends_on: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut depended_by: Vec<Vec<usize>> = vec![Vec::new(); n];

    // For each crate, resolve its path dependencies to crate indices
    for (i, ci) in crates.iter().enumerate() {
        for dep in &ci.dependencies {
            // Only path deps resolve to workspace crates
            if dep.kind != DepKind::Path { continue; }
            // Find which workspace crate this path points to
            if let Some(ref dep_path) = dep.path {
                if let Some(dep_idx) = find_crate_by_path(crates, dep_path) {
                    depends_on[i].push(dep_idx);
                    depended_by[dep_idx].push(i);
                }
            }
        }
    }

    // Cycle detection via DFS
    detect_cycle(n, &depends_on)?;

    Ok(DepGraph { depends_on, depended_by, len: n })
}

/// Find a crate index by its canonical path.
fn find_crate_by_path(crates: &[CrateInfo], dep_path: &std::path::Path) -> Option<usize> {
    for (i, ci) in crates.iter().enumerate() {
        // Compare canonical paths
        let ci_canon = ci.path.canonicalize().unwrap_or_else(|_| ci.path.clone());
        let dep_canon = dep_path.canonicalize().unwrap_or_else(|_| dep_path.to_path_buf());
        if ci_canon == dep_canon {
            return Some(i);
        }
    }
    None
}

/// Kahn's algorithm: topological sort returning parallel batches.
/// Each batch contains crate indices with in-degree 0 at that stage.
pub fn topological_batches(dag: &DepGraph) -> Vec<Vec<usize>> {
    let n = dag.len;
    let mut in_degree: Vec<usize> = vec![0; n];
    for edges in &dag.depends_on {
        for &dep in edges {
            in_degree[dep] += 1; // wait: crate i depends on dep, so dep is a dependency OF i
        }
    }

    // Actually let me re-think. In Kahn's, we want:
    // in_degree[i] = number of unfinished dependencies of crate i
    // A crate is ready when in_degree[i] == 0
    let mut in_deg: Vec<usize> = vec![0; n];
    for (i, edges) in dag.depends_on.iter().enumerate() {
        in_deg[i] = edges.len(); // crate i depends on this many other crates
    }

    let mut queue: VecDeque<usize> = VecDeque::new();
    for i in 0..n {
        if in_deg[i] == 0 {
            queue.push_back(i);
        }
    }

    let mut batches: Vec<Vec<usize>> = Vec::new();
    let mut processed = 0;

    while !queue.is_empty() {
        // Current batch: all crates in queue right now
        let batch: Vec<usize> = queue.drain(..).collect();

        for &crate_idx in &batch {
            processed += 1;
            // For every crate that depends on crate_idx, decrement its in-degree
            for &dependent in &dag.depended_by[crate_idx] {
                in_deg[dependent] -= 1;
                if in_deg[dependent] == 0 {
                    queue.push_back(dependent);
                }
            }
        }

        batches.push(batch);
    }

    if processed != n {
        // Should not happen if cycle detection passed
        eprintln!("flux-graph warning: only processed {}/{} crates in topological sort", processed, n);
    }

    batches
}

/// DFS-based cycle detection. Returns Err if a cycle is found.
fn detect_cycle(n: usize, edges: &[Vec<usize>]) -> Result<(), String> {
    #[derive(Clone, PartialEq)]
    enum Color { White, Gray, Black }
    let mut color = vec![Color::White; n];

    fn dfs(u: usize, edges: &[Vec<usize>], color: &mut [Color]) -> Result<(), String> {
        color[u] = Color::Gray;
        for &v in &edges[u] {
            match color[v] {
                Color::Gray => return Err(format!("Cycle detected: crate involves dependency loop")),
                Color::White => dfs(v, edges, color)?,
                Color::Black => {}
            }
        }
        color[u] = Color::Black;
        Ok(())
    }

    for i in 0..n {
        if color[i] == Color::White {
            dfs(i, edges, &mut color)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CrateInfo, CrateType, Dependency};
    use std::path::PathBuf;

    fn make_crate(name: &str, deps: &[&str]) -> CrateInfo {
        CrateInfo {
            name: name.to_string(),
            path: PathBuf::from(name),
            edition: "2021".into(),
            crate_type: CrateType::Lib,
            dependencies: deps.iter().map(|d| Dependency {
                name: d.to_string(),
                path: Some(PathBuf::from(d)),
                kind: DepKind::Path,
                optional: false,
            }).collect(),
            features: vec![],
        }
    }

    #[test]
    fn test_simple_dag() {
        // A depends on B, B depends on C
        let crates = vec![
            make_crate("a", &["b"]),
            make_crate("b", &["c"]),
            make_crate("c", &[]),
        ];
        // Override paths to match names for test
        let mut crates = crates;
        for ci in &mut crates {
            ci.path = PathBuf::from(format!("/test/{}", ci.name));
        }
        for ci in &mut crates {
            for dep in &mut ci.dependencies {
                dep.path = Some(PathBuf::from(format!("/test/{}", dep.name)));
            }
        }

        let dag = build_dag(&crates).unwrap();
        let batches = topological_batches(&dag);
        // C should be first, then B, then A
        assert_eq!(batches.len(), 3, "expected 3 batches, got {:?}", batches);
    }

    #[test]
    fn test_parallel_batch() {
        // A and B are independent, both depend on C
        let crates = vec![
            make_crate("a", &["c"]),
            make_crate("b", &["c"]),
            make_crate("c", &[]),
        ];
        let mut crates = crates;
        for ci in &mut crates {
            ci.path = PathBuf::from(format!("/test/{}", ci.name));
        }
        for ci in &mut crates {
            for dep in &mut ci.dependencies {
                dep.path = Some(PathBuf::from(format!("/test/{}", dep.name)));
            }
        }

        let dag = build_dag(&crates).unwrap();
        let batches = topological_batches(&dag);
        // C first, then A and B in parallel (same batch)
        assert_eq!(batches[0].len(), 1, "first batch should have 1 crate (C)");
        assert_eq!(batches[1].len(), 2, "second batch should have 2 crates (A, B)");
    }

    #[test]
    fn test_cycle_detection() {
        // A → B → C → A (cycle)
        let crates = vec![
            make_crate("a", &["b"]),
            make_crate("b", &["c"]),
            make_crate("c", &["a"]),
        ];
        let mut crates = crates;
        for ci in &mut crates {
            ci.path = PathBuf::from(format!("/test/{}", ci.name));
        }
        for ci in &mut crates {
            for dep in &mut ci.dependencies {
                dep.path = Some(PathBuf::from(format!("/test/{}", dep.name)));
            }
        }

        let result = build_dag(&crates);
        assert!(result.is_err(), "cycle should be detected");
    }
}
