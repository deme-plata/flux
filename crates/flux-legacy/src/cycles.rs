//! cycles.rs (flux-legacy PROTOTYPE 2) — dependency-CYCLE detection over the workspace graph.
//! Algorithm authored by deepseek-v4-flash (via the DeepSeek API); integrated + type-fitted +
//! verified by rocky-vision-skill. Color-DFS back-edge detection, rotation-canonicalized dedup,
//! self-loop = length-1 cycle. Pure, std-only.

use crate::{LegacyCrate, LegacyReport};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub struct DepCycle {
    pub crates: Vec<String>,
}

pub fn find_cycles(report: &LegacyReport) -> Vec<DepCycle> {
    // Build name -> index mapping
    let mut index_of = HashMap::new();
    for (i, krate) in report.crates.iter().enumerate() {
        index_of.insert(krate.name.clone(), i);
    }

    let n = report.crates.len();
    // Build adjacency list from dependency names to indices
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for krate in &report.crates {
        if let Some(&from) = index_of.get(&krate.name) {
            for dep_name in &krate.deps {
                if let Some(&to) = index_of.get(dep_name) {
                    adj[from].push(to);
                }
                // Ignore deps not in workspace (not defined in report)
            }
        }
    }

    // Colors: 0 = unvisited, 1 = visiting (on stack), 2 = done
    let mut color = vec![0u8; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut raw_cycles: Vec<Vec<usize>> = Vec::new();

    fn dfs(
        u: usize,
        adj: &[Vec<usize>],
        color: &mut [u8],
        stack: &mut Vec<usize>,
        raw_cycles: &mut Vec<Vec<usize>>,
    ) {
        color[u] = 1;
        stack.push(u);

        for &v in &adj[u] {
            if color[v] == 0 {
                dfs(v, adj, color, stack, raw_cycles);
            } else if color[v] == 1 {
                // back edge: v is ancestor (or same) in current stack
                // Find position of v in stack
                let pos = stack.iter().position(|&x| x == v).unwrap();
                // Cycle from v to u inclusive, then back to v (implicit)
                let cycle: Vec<usize> = stack[pos..].to_vec();
                raw_cycles.push(cycle);
            }
            // color[v] == 2 (done) -> ignore
        }

        stack.pop();
        color[u] = 2;
    }

    for start in 0..n {
        if color[start] == 0 {
            dfs(start, &adj, &mut color, &mut stack, &mut raw_cycles);
        }
    }

    // Normalize and deduplicate cycles
    let mut canonical_set: HashSet<Vec<String>> = HashSet::new();
    let names: Vec<String> = report.crates.iter().map(|c| c.name.clone()).collect();

    for cycle_idx in &raw_cycles {
        // Convert to names
        let cycle_names: Vec<String> = cycle_idx.iter().map(|&idx| names[idx].clone()).collect();
        let canonical = min_rotation(&cycle_names);
        canonical_set.insert(canonical);
    }

    // Build result, sorted deterministically
    let mut result: Vec<DepCycle> = canonical_set
        .into_iter()
        .map(|c| DepCycle { crates: c })
        .collect();
    result.sort_by(|a, b| {
        // Compare by first element, then length, then rest
        let cmp_first = a.crates[0].cmp(&b.crates[0]);
        if cmp_first != std::cmp::Ordering::Equal {
            return cmp_first;
        }
        let cmp_len = a.crates.len().cmp(&b.crates.len());
        if cmp_len != std::cmp::Ordering::Equal {
            return cmp_len;
        }
        a.crates.cmp(&b.crates)
    });

    result
}

/// Rotate a cycle to start with the lexicographically smallest element.
/// If the smallest element appears multiple times, choose the rotation
/// that starts with the first occurrence.
fn min_rotation(cycle: &[String]) -> Vec<String> {
    if cycle.is_empty() {
        return cycle.to_vec();
    }
    let len = cycle.len();
    // Find indices of minimum element (lexicographically)
    let min_elem = cycle.iter().min().unwrap();
    let candidates: Vec<usize> = cycle
        .iter()
        .enumerate()
        .filter(|(_, e)| *e == min_elem)
        .map(|(i, _)| i)
        .collect();

    // Among all rotations starting at candidate indices, pick the minimal sequence
    let best = candidates
        .into_iter()
        .min_by(|&i, &j| {
            // Compare lexicographically rotations starting at i and j
            for k in 0..len {
                let a = &cycle[(i + k) % len];
                let b = &cycle[(j + k) % len];
                if a < b {
                    return std::cmp::Ordering::Less;
                }
                if a > b {
                    return std::cmp::Ordering::Greater;
                }
            }
            std::cmp::Ordering::Equal
        })
        .unwrap();

    let mut result = Vec::with_capacity(len);
    for k in 0..len {
        result.push(cycle[(best + k) % len].clone());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_cycles_in_dag() {
        let report = LegacyReport {
            crates: vec![
                LegacyCrate {
                    name: "A".into(),
                    deps: vec!["B".into()],
                    dependents: vec![],
                    ..Default::default()
                },
                LegacyCrate {
                    name: "B".into(),
                    deps: vec!["C".into()],
                    dependents: vec![],
                    ..Default::default()
                },
                LegacyCrate {
                    name: "C".into(),
                    deps: vec![],
                    dependents: vec![],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let cycles = find_cycles(&report);
        assert_eq!(cycles.len(), 0);
    }

    #[test]
    fn three_crate_cycle_detected_once() {
        let report = LegacyReport {
            crates: vec![
                LegacyCrate {
                    name: "A".into(),
                    deps: vec!["B".into()],
                    dependents: vec![],
                    ..Default::default()
                },
                LegacyCrate {
                    name: "B".into(),
                    deps: vec!["C".into()],
                    dependents: vec![],
                    ..Default::default()
                },
                LegacyCrate {
                    name: "C".into(),
                    deps: vec!["A".into()],
                    dependents: vec![],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let cycles = find_cycles(&report);
        assert_eq!(cycles.len(), 1);
        let cycle = &cycles[0];
        // Canonical rotation starts with A
        assert_eq!(cycle.crates, vec!["A", "B", "C"]);
    }

    #[test]
    fn self_loop_detected() {
        let report = LegacyReport {
            crates: vec![
                LegacyCrate {
                    name: "X".into(),
                    deps: vec!["X".into()],
                    dependents: vec![],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let cycles = find_cycles(&report);
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].crates, vec!["X"]);
    }
}