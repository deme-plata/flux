//! shadow.rs — flux-legacy **P6: SHADOW VERIFICATION** (the gate for T3 logic changes).
//!
//! Structural refactors (T1/T2) are proven by P4 (build + tests). A *logic* change (libp2p version
//! fix, RPC timeout, storage encoding) can pass build+tests and STILL alter the chain's computed
//! state — a silent fork. The only safe gate is **equivalence under real blocks**: run the patched
//! node in shadow (consume the same live mainnet blocks, produce NOTHING) and compare the committed
//! state to the canonical node, height by height. Quillon commits FOUR roots per block
//! (wallet / dex / events / contract); shadow compares all four and reports the FIRST divergence —
//! the exact height and which root drifted — so a logic bug is caught before it can fork mainnet.
//!
//! Pure comparison core (unit-tested); the live reader polls a node's per-height state-root endpoint.

use serde::{Deserialize, Serialize};

/// The four state roots Quillon commits per block.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateRoots {
    pub wallet: String,
    pub dex: String,
    pub events: String,
    pub contract: String,
}

/// A node's committed state at one height.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub height: u64,
    pub roots: StateRoots,
}

/// The result of a shadow run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShadowVerdict {
    /// Candidate matched canonical for `blocks` aligned heights (up to `last_height`).
    Match { blocks: u64, last_height: u64 },
    /// First divergence: at `at_height`, root `root` differed.
    Diverged { at_height: u64, root: String, baseline: String, candidate: String },
    /// Could not compare (no overlapping heights / empty stream).
    Incomplete { reason: String },
}

impl ShadowVerdict {
    pub fn matched(&self) -> bool { matches!(self, ShadowVerdict::Match { .. }) }
    pub fn label(&self) -> String {
        match self {
            ShadowVerdict::Match { blocks, last_height } => format!("MATCH ({blocks} blocks → h{last_height})"),
            ShadowVerdict::Diverged { at_height, root, .. } => format!("DIVERGED @ h{at_height} · {root} root"),
            ShadowVerdict::Incomplete { reason } => format!("INCOMPLETE: {reason}"),
        }
    }
}

/// Compare a candidate's snapshots against the canonical baseline. Aligns by height (only heights
/// present in BOTH are compared), then for each shared height checks the four roots in commit order
/// (wallet → dex → events → contract). The FIRST mismatch wins (deterministic). All shared heights
/// equal → [`ShadowVerdict::Match`].
pub fn compare(baseline: &[StateSnapshot], candidate: &[StateSnapshot]) -> ShadowVerdict {
    use std::collections::BTreeMap;
    let cand: BTreeMap<u64, &StateRoots> = candidate.iter().map(|s| (s.height, &s.roots)).collect();
    let mut shared = 0u64;
    let mut last = 0u64;
    // baseline drives the order, ascending by height
    let mut base_sorted: Vec<&StateSnapshot> = baseline.iter().collect();
    base_sorted.sort_by_key(|s| s.height);
    for b in base_sorted {
        let Some(c) = cand.get(&b.height) else { continue };
        for (name, bv, cv) in [
            ("wallet", &b.roots.wallet, &c.wallet),
            ("dex", &b.roots.dex, &c.dex),
            ("events", &b.roots.events, &c.events),
            ("contract", &b.roots.contract, &c.contract),
        ] {
            if bv != cv {
                return ShadowVerdict::Diverged {
                    at_height: b.height,
                    root: name.to_string(),
                    baseline: bv.clone(),
                    candidate: cv.clone(),
                };
            }
        }
        shared += 1;
        last = b.height;
    }
    if shared == 0 {
        ShadowVerdict::Incomplete { reason: "no overlapping heights between baseline and candidate".into() }
    } else {
        ShadowVerdict::Match { blocks: shared, last_height: last }
    }
}

/// The T3 logic-change gate: equivalence holds over at least `min_blocks` aligned heights.
/// Fail-closed — anything but a wide-enough Match rejects.
pub fn shadow_gate(verdict: &ShadowVerdict, min_blocks: u64) -> bool {
    matches!(verdict, ShadowVerdict::Match { blocks, .. } if *blocks >= min_blocks)
}

/// Live reader: poll a node's per-height state-roots endpoint over `[from_h, from_h+n)`.
/// Expects a JSON array of `{height, wallet, dex, events, contract}`. The exact route is wired by
/// the deployment (Quillon's ledger surface exposes the four roots); kept generic here.
pub fn read_snapshots(base_url: &str, route: &str, from_h: u64, n: u64) -> Result<Vec<StateSnapshot>, String> {
    let url = format!("{base_url}{route}?from={from_h}&n={n}");
    let body = std::process::Command::new("curl")
        .args(["-s", "--max-time", "20", &url])
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !body.status.success() {
        return Err(format!("fetch {url} failed"));
    }
    let txt = String::from_utf8_lossy(&body.stdout);
    serde_json::from_str::<Vec<StateSnapshot>>(&txt)
        .map_err(|e| format!("decode {url}: {e} (body starts: {})", txt.chars().take(80).collect::<String>()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(h: u64, w: &str, d: &str, e: &str, c: &str) -> StateSnapshot {
        StateSnapshot { height: h, roots: StateRoots { wallet: w.into(), dex: d.into(), events: e.into(), contract: c.into() } }
    }

    #[test]
    fn identical_streams_match() {
        let a = vec![snap(100, "w1", "d1", "e1", "c1"), snap(101, "w2", "d2", "e2", "c2")];
        let v = compare(&a, &a.clone());
        assert!(v.matched());
        assert_eq!(v, ShadowVerdict::Match { blocks: 2, last_height: 101 });
        assert!(shadow_gate(&v, 2));
    }

    #[test]
    fn detects_first_diverging_root_and_height() {
        let base = vec![snap(100, "w1", "d1", "e1", "c1"), snap(101, "w2", "d2", "e2", "c2")];
        // candidate drifts on the DEX root at height 101 (a logic bug touching the AMM)
        let cand = vec![snap(100, "w1", "d1", "e1", "c1"), snap(101, "w2", "dX", "e2", "c2")];
        match compare(&base, &cand) {
            ShadowVerdict::Diverged { at_height, root, baseline, candidate } => {
                assert_eq!(at_height, 101);
                assert_eq!(root, "dex");
                assert_eq!((baseline.as_str(), candidate.as_str()), ("d2", "dX"));
            }
            o => panic!("expected Diverged, got {o:?}"),
        }
    }

    #[test]
    fn wallet_root_checked_before_others() {
        let base = vec![snap(5, "w", "d", "e", "c")];
        let cand = vec![snap(5, "WRONG", "d", "e", "DIFF")]; // both wallet+contract differ
        match compare(&base, &cand) {
            ShadowVerdict::Diverged { root, .. } => assert_eq!(root, "wallet", "commit-order: wallet first"),
            o => panic!("got {o:?}"),
        }
    }

    #[test]
    fn no_overlap_is_incomplete() {
        let base = vec![snap(1, "w", "d", "e", "c")];
        let cand = vec![snap(9, "w", "d", "e", "c")];
        assert!(matches!(compare(&base, &cand), ShadowVerdict::Incomplete { .. }));
        assert!(!shadow_gate(&compare(&base, &cand), 1));
    }

    #[test]
    fn gate_requires_min_blocks() {
        let a = vec![snap(1, "w", "d", "e", "c")];
        let v = compare(&a, &a.clone());
        assert!(shadow_gate(&v, 1));
        assert!(!shadow_gate(&v, 100), "1 matched block must not pass a 100-block gate");
    }

    #[test]
    fn compares_only_shared_heights() {
        // candidate has extra heights; only the overlap (100,101) is compared
        let base = vec![snap(100, "w", "d", "e", "c"), snap(101, "w2", "d2", "e2", "c2")];
        let cand = vec![snap(99, "x", "x", "x", "x"), snap(100, "w", "d", "e", "c"), snap(101, "w2", "d2", "e2", "c2")];
        assert_eq!(compare(&base, &cand), ShadowVerdict::Match { blocks: 2, last_height: 101 });
    }
}
