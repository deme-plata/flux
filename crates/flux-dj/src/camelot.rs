//! Camelot-wheel harmonic-mixing compatibility graph.
//!
//! Ported unchanged (same predicates, same rules) from
//! `crates/flux-arxiv-latex/src/bin/ai_dj_live.rs`'s "The Beat Budget" paper
//! generator — the paper's Section 4 headline numbers (72 strict-compatible
//! ordered pairs out of 552 possible, 13.0% strict / extended raises that to
//! 120 pairs, 21.7%) are exactly the values `strict_edge_counts()` /
//! `extended_edge_counts()` below compute, and `tests::camelot_matches_paper`
//! pins them so a future edit can't silently drift from the paper's numbers.
//!
//! The Camelot system (rekordbox, Serato, Mixed In Key, engine DJ, ...)
//! relabels the 24 major/minor keys as 1A..12A (minor) and 1B..12B (major)
//! arranged on a wheel so a harmonically "safe" transition is graph
//! adjacency: the SAME code, an ADJACENT number with the same letter (a
//! perfect-fifth move), or the SAME number with the other letter (relative
//! major/minor). An "extended" rule some curators also allow is a same-letter
//! ±2 "energy jump".

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize, Deserialize)]
pub struct Camelot {
    pub num: i16,
    /// 0 = A (minor), 1 = B (major)
    pub letter: u8,
}

impl Camelot {
    /// Parse a Camelot code like `"8A"`, `"12B"`, case-insensitive, optional
    /// surrounding whitespace. Returns `None` for anything outside 1..=12 A/B.
    pub fn parse(s: &str) -> Option<Camelot> {
        let s = s.trim().to_uppercase();
        if s.len() < 2 {
            return None;
        }
        let (num_part, letter_part) = s.split_at(s.len() - 1);
        let num: i16 = num_part.parse().ok()?;
        if !(1..=12).contains(&num) {
            return None;
        }
        let letter = match letter_part {
            "A" => 0u8,
            "B" => 1u8,
            _ => return None,
        };
        Some(Camelot { num, letter })
    }
}

impl std::fmt::Display for Camelot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.num, if self.letter == 0 { "A" } else { "B" })
    }
}

/// All 24 Camelot codes, 1A..12A then 1B..12B (order doesn't matter for the
/// graph predicates, only for deterministic iteration in tests/output).
pub fn all_camelot() -> Vec<Camelot> {
    let mut v = Vec::with_capacity(24);
    for n in 1..=12i16 {
        for l in [0u8, 1u8] {
            v.push(Camelot { num: n, letter: l });
        }
    }
    v
}

/// Strict rule set: same-letter neighbor (±1, i.e. a perfect-fifth move) OR
/// same-number other-letter (relative major/minor). Ported verbatim from
/// `ai_dj_live.rs::compatible_strict`.
pub fn compatible_strict(a: Camelot, b: Camelot) -> bool {
    if a == b {
        return false;
    }
    let d = (a.num - b.num).rem_euclid(12);
    (a.letter == b.letter && (d == 1 || d == 11)) || (a.num == b.num && a.letter != b.letter)
}

/// Extended rule set: strict, plus the same-letter ±2 "energy boost" jump
/// some curators also allow. Ported verbatim from
/// `ai_dj_live.rs::compatible_extended`.
pub fn compatible_extended(a: Camelot, b: Camelot) -> bool {
    if compatible_strict(a, b) {
        return true;
    }
    let d = (a.num - b.num).rem_euclid(12);
    a.letter == b.letter && (d == 2 || d == 10)
}

/// `(compatible_ordered_pairs, total_ordered_pairs)` under the strict rules,
/// counted by brute-force over the 24-node graph (not a closed-form
/// shortcut) — mirrors exactly how `ai_dj_live.rs` computes its headline
/// numbers.
pub fn strict_edge_counts() -> (usize, usize) {
    edge_counts(compatible_strict)
}

/// Same as [`strict_edge_counts`] but under the extended rule set.
pub fn extended_edge_counts() -> (usize, usize) {
    edge_counts(compatible_extended)
}

fn edge_counts(pred: fn(Camelot, Camelot) -> bool) -> (usize, usize) {
    let nodes = all_camelot();
    let mut edges = 0usize;
    let mut total = 0usize;
    for &a in &nodes {
        for &b in &nodes {
            if a != b {
                total += 1;
                if pred(a, b) {
                    edges += 1;
                }
            }
        }
    }
    (edges, total)
}

/// All codes harmonically compatible with `key`, under either rule set.
pub fn compatible_keys(key: Camelot, extended: bool) -> Vec<Camelot> {
    all_camelot()
        .into_iter()
        .filter(|&b| if extended { compatible_extended(key, b) } else { compatible_strict(key, b) })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity check that the port is faithful: these are the exact headline
    /// numbers "The Beat Budget" (ai_dj_live.rs) reports for Section 4 —
    /// 72 of 552 ordered pairs (13.0%) strict, 120 of 552 (21.7%) extended.
    #[test]
    fn camelot_matches_paper() {
        let (strict, total) = strict_edge_counts();
        assert_eq!(total, 552, "24 nodes * 23 others = 552 ordered pairs");
        assert_eq!(strict, 72, "strict Camelot edges must match the paper's reported 72");

        let (ext, total2) = extended_edge_counts();
        assert_eq!(total2, 552);
        assert_eq!(ext, 120, "extended Camelot edges must match the paper's reported 120");
    }

    #[test]
    fn parse_roundtrip() {
        for s in ["1A", "12B", "8a", " 5B "] {
            let c = Camelot::parse(s).unwrap();
            assert!((1..=12).contains(&c.num));
        }
        assert!(Camelot::parse("13A").is_none());
        assert!(Camelot::parse("0B").is_none());
        assert!(Camelot::parse("8C").is_none());
        assert!(Camelot::parse("x").is_none());
    }

    #[test]
    fn display_roundtrip() {
        let c = Camelot::parse("8A").unwrap();
        assert_eq!(c.to_string(), "8A");
        let c = Camelot::parse("12b").unwrap();
        assert_eq!(c.to_string(), "12B");
    }

    #[test]
    fn strict_neighbors_of_8a() {
        let key = Camelot::parse("8A").unwrap();
        let mut compat: Vec<String> = compatible_keys(key, false).iter().map(|c| c.to_string()).collect();
        compat.sort();
        // 7A (perfect fifth down), 9A (perfect fifth up), 8B (relative major)
        assert_eq!(compat, vec!["7A".to_string(), "8B".to_string(), "9A".to_string()]);
    }

    #[test]
    fn extended_adds_energy_jumps() {
        let key = Camelot::parse("8A").unwrap();
        let strict = compatible_keys(key, false);
        let extended = compatible_keys(key, true);
        assert_eq!(strict.len(), 3);
        assert_eq!(extended.len(), 5, "extended adds the ±2 same-letter energy jump");
        let mut compat: Vec<String> = extended.iter().map(|c| c.to_string()).collect();
        compat.sort();
        assert_eq!(compat, vec!["10A".to_string(), "6A".to_string(), "7A".to_string(), "8B".to_string(), "9A".to_string()]);
    }

    #[test]
    fn compatibility_is_never_reflexive() {
        for key in all_camelot() {
            assert!(!compatible_strict(key, key));
            assert!(!compatible_extended(key, key));
        }
    }
}
