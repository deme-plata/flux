//! Tourbillon — rotating-permutation runner for ordering-sensitive bugs.
//!
//! Borrowed from horology: a *tourbillon* is a watch complication where the
//! escapement rides inside a slowly rotating cage so gravity-induced rate
//! errors average out. flux-chronos's tourbillon does the same for
//! **event-ordering bias**: it rotates the order of simultaneously-injected
//! events through every permutation, runs the universe to a deadline, and
//! diffs the resulting node snapshots. Bugs that only surface when, e.g.,
//! tx A arrives one microsecond before tx B vanish under this lens.
//!
//! Use it for:
//! - **Consensus invariants** — does block-validation order matter? (it shouldn't)
//! - **Mempool determinism** — same tx set, different arrival orders → same root?
//! - **DEX trade ordering** — does swap A before swap B differ from B before A
//!   in ways the spec didn't intend?
//! - **Catching races at the Universe layer** before they reach a real node
//!
//! Companion to [`Universe::advance`] in CHRONOS-A. Multiverse fork/diff
//! (CHRONOS-B) generalizes this to *any* divergent fault, not just
//! ordering. Tourbillon is the cheap version: deterministic, exhaustive
//! over a small permutation budget, no fault injection beyond reorder.

use std::collections::BTreeMap;
use std::time::Instant;

use crate::net::NodeId;
use crate::universe::{ScenarioSeed, Universe};

/// One injection into the universe — `(target_node, payload_bytes)`. The
/// tourbillon runs every permutation of a `Vec<Injection>` through the
/// scenario builder.
#[derive(Debug, Clone)]
pub struct Injection {
    /// Target node for this injection.
    pub target: NodeId,
    /// Payload bytes (whatever the node's protocol expects).
    pub payload: Vec<u8>,
}

/// Per-permutation outcome — what the tourbillon recorded for one run.
#[derive(Debug, Clone)]
pub struct PermutationOutcome {
    /// Which permutation index this was (0..n!).
    pub index: usize,
    /// The injection order that produced this outcome.
    pub order: Vec<usize>,
    /// Final per-node snapshot bytes (via `SimNode::snapshot`).
    pub snapshots: BTreeMap<NodeId, Vec<u8>>,
    /// Wall-clock cost of this run — useful for "is the chain getting slower?"
    pub wall_micros: u64,
}

/// What the tourbillon ultimately reports.
#[derive(Debug, Clone)]
pub struct TourbillonReport {
    /// Every permutation that was run.
    pub outcomes: Vec<PermutationOutcome>,
    /// `true` if every permutation produced byte-identical node snapshots.
    /// This is THE invariant a deterministic chain must satisfy: the order
    /// in which transactions reach nodes must not change the final state.
    pub converged: bool,
    /// If `!converged`, the (a, b) permutation indices that disagreed.
    /// Empty when `converged` is true.
    pub divergence_pairs: Vec<(usize, usize)>,
}

/// Run every permutation of `injections` through a freshly-built universe
/// (built via `builder` so each permutation starts from the same canonical
/// initial state), advance by `advance_micros`, snapshot every node, and
/// report whether all permutations converged to the same final state.
///
/// Permutation count grows as `injections.len()!`. The runner caps at
/// `max_permutations` to keep wall time bounded — pass `None` to run all
/// (sane up to ~7 inputs; 7! = 5040).
///
/// `builder` is called once per permutation. It MUST produce a universe in
/// the same canonical pre-injection state every time — same `ScenarioSeed`,
/// same nodes, same edges, no leftover events. Tourbillon checks
/// determinism *given* a deterministic builder.
pub fn run<B>(
    seed: ScenarioSeed,
    injections: &[Injection],
    advance_micros: crate::SimDuration,
    max_permutations: Option<usize>,
    mut builder: B,
) -> TourbillonReport
where
    B: FnMut(ScenarioSeed) -> Universe,
{
    let perms = permutations(injections.len());
    let cap = max_permutations.unwrap_or(perms.len()).min(perms.len());
    let mut outcomes = Vec::with_capacity(cap);

    for (idx, order) in perms.into_iter().take(cap).enumerate() {
        let wall_start = Instant::now();
        let mut u = builder(seed);
        for &i in &order {
            let inj = &injections[i];
            u.inject(inj.target, inj.payload.clone());
        }
        u.advance(advance_micros);
        let snapshots = u.snapshot_nodes();
        let wall_micros = wall_start.elapsed().as_micros() as u64;
        outcomes.push(PermutationOutcome { index: idx, order, snapshots, wall_micros });
    }

    // Compare every pair. Optimization for a strict-equality check: hash each
    // snapshot map first and compare hashes. For POC we just diff bytes — N²
    // pairs but N is small (≤ 5040 by default).
    let mut divergence_pairs = Vec::new();
    for i in 0..outcomes.len() {
        for j in (i + 1)..outcomes.len() {
            if outcomes[i].snapshots != outcomes[j].snapshots {
                divergence_pairs.push((i, j));
            }
        }
    }
    let converged = divergence_pairs.is_empty();
    TourbillonReport { outcomes, converged, divergence_pairs }
}

/// Generate every permutation of `[0..n)` in lexicographic order. Caller
/// is responsible for capping `n` — tourbillon::run already does.
fn permutations(n: usize) -> Vec<Vec<usize>> {
    if n == 0 {
        return vec![vec![]];
    }
    let mut out = Vec::new();
    let mut cur: Vec<usize> = (0..n).collect();
    heap_permute(&mut cur, n, &mut out);
    out
}

/// Heap's algorithm — generates n! permutations in O(n!) time, no extra
/// allocation per swap.
fn heap_permute(arr: &mut Vec<usize>, k: usize, out: &mut Vec<Vec<usize>>) {
    if k == 1 {
        out.push(arr.clone());
        return;
    }
    for i in 0..k {
        heap_permute(arr, k - 1, out);
        let swap_idx = if k % 2 == 0 { i } else { 0 };
        arr.swap(swap_idx, k - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permutations_count_factorial() {
        assert_eq!(permutations(0).len(), 1);
        assert_eq!(permutations(1).len(), 1);
        assert_eq!(permutations(2).len(), 2);
        assert_eq!(permutations(3).len(), 6);
        assert_eq!(permutations(4).len(), 24);
    }

    #[test]
    fn permutations_3_are_distinct() {
        let p = permutations(3);
        let unique: std::collections::HashSet<_> = p.iter().cloned().collect();
        assert_eq!(unique.len(), 6, "all 3! permutations should be distinct");
    }

    #[test]
    fn permutations_of_2_are_swaps() {
        let p = permutations(2);
        assert!(p.contains(&vec![0, 1]));
        assert!(p.contains(&vec![1, 0]));
    }

    #[test]
    fn run_with_no_injections_converges_trivially() {
        let report = run(
            ScenarioSeed(42),
            &[],
            crate::secs(1),
            None,
            |seed| Universe::new(seed),
        );
        assert_eq!(report.outcomes.len(), 1);
        assert!(report.converged);
        assert!(report.divergence_pairs.is_empty());
    }
}
