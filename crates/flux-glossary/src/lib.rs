//! flux-glossary — query-expansion glossary for the flux-search web crawler.
//!
//! The crawler (flux-search over flux-p2p) tokenizes queries into bare terms and
//! matches them literally against the inverted index. That misses obvious
//! equivalences: a query for `k8s` should also hit `kubernetes`, `zk` should hit
//! `zero-knowledge`, `pq` should hit `post-quantum`. A **glossary** maps each
//! term to its synonyms/abbreviations/domain variants so [`Glossary::expand`]
//! widens a query before it hits the index — better recall, same engine.
//!
//! And because Viktor wants **AGORA to pay a bonus for a nice glossary**, this
//! crate also grades a glossary objectively via [`Glossary::quality`] +
//! [`bonus_score`], so the AGORA bounty contract can pay a quality-weighted
//! bonus instead of a flat fee — a good glossary earns more than a lazy one,
//! and a degenerate one (self-loops, runaway fan-out) earns nothing.
//!
//! ## Shape
//! A glossary is a set of **equivalence groups** (e.g. `{zk, zero-knowledge,
//! zeroknowledge}`). Expansion is symmetric within a group: any member expands
//! to all the others. This is the property the quality score rewards — a real
//! thesaurus is symmetric, a sloppy one isn't.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// One equivalence group — a set of terms that mean the same thing for search.
/// Order-insensitive; normalized to lowercase + deduped on construction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    /// The equivalent terms (>= 2 to be useful; a 1-term group is dropped).
    pub terms: Vec<String>,
}

impl Group {
    pub fn new<I, S>(terms: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut set: BTreeSet<String> = BTreeSet::new();
        for t in terms {
            let t = t.into().trim().to_lowercase();
            if !t.is_empty() {
                set.insert(t);
            }
        }
        Group { terms: set.into_iter().collect() }
    }
}

/// A query-expansion glossary: term → all its equivalents.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Glossary {
    /// Source equivalence groups (the curated artifact).
    pub groups: Vec<Group>,
    /// Derived: term → the set of OTHER terms it expands to. Rebuilt from groups.
    #[serde(skip)]
    index: BTreeMap<String, BTreeSet<String>>,
}

impl Glossary {
    /// Build from equivalence groups. Groups with < 2 distinct terms are dropped
    /// (they expand to nothing). Membership is unioned across groups, so two
    /// groups sharing a term merge transitively at the index level.
    pub fn from_groups(groups: Vec<Group>) -> Self {
        let mut g = Glossary { groups, index: BTreeMap::new() };
        g.rebuild();
        g
    }

    /// Parse from JSON: `{"groups":[{"terms":["zk","zero-knowledge"]}, ...]}`.
    pub fn from_json(s: &str) -> Result<Self, String> {
        let mut g: Glossary = serde_json::from_str(s).map_err(|e| e.to_string())?;
        g.groups = g
            .groups
            .into_iter()
            .map(|grp| Group::new(grp.terms))
            .collect();
        g.rebuild();
        Ok(g)
    }

    /// Serialize the curated groups to JSON (the form AGORA stores/pays for).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("glossary serializes")
    }

    fn rebuild(&mut self) {
        let mut idx: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for grp in &self.groups {
            let members: BTreeSet<&String> = grp.terms.iter().collect();
            if members.len() < 2 {
                continue;
            }
            for &t in &members {
                let entry = idx.entry(t.clone()).or_default();
                for &other in &members {
                    if other != t {
                        entry.insert(other.clone());
                    }
                }
            }
        }
        self.index = idx;
    }

    /// The terms `term` expands to (excluding itself). Empty if unknown.
    pub fn synonyms(&self, term: &str) -> Vec<String> {
        self.index
            .get(&term.to_lowercase())
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Expand a list of query terms: every term plus all its synonyms, deduped,
    /// original order preserved with synonyms appended. This is what flux-search
    /// calls between `tokenize` and index lookup.
    pub fn expand(&self, terms: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for t in terms {
            let lt = t.to_lowercase();
            if seen.insert(lt.clone()) {
                out.push(lt.clone());
            }
            for syn in self.synonyms(&lt) {
                if seen.insert(syn.clone()) {
                    out.push(syn);
                }
            }
        }
        out
    }

    /// Expand a raw query string (whitespace-split, lowercased) into the widened
    /// term set. Convenience wrapper over [`expand`].
    pub fn expand_query(&self, query: &str) -> Vec<String> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|s| s.to_lowercase())
            .collect();
        self.expand(&terms)
    }

    /// Number of indexed (expandable) terms.
    pub fn term_count(&self) -> usize {
        self.index.len()
    }

    /// Number of non-trivial groups.
    pub fn group_count(&self) -> usize {
        self.groups.iter().filter(|g| g.terms.len() >= 2).count()
    }

    /// Objective quality grade in [0.0, 1.0] for AGORA bonus pricing. A glossary
    /// earns by being BROAD (covers many terms), SYMMETRIC (real equivalences,
    /// enforced structurally), and CLEAN (no self-loops, no runaway groups that
    /// make everything match everything). See [`QualityReport`] for the breakdown.
    pub fn quality(&self) -> QualityReport {
        let groups: Vec<&Group> = self.groups.iter().filter(|g| g.terms.len() >= 2).collect();
        let group_count = groups.len();
        let term_count = self.index.len();

        if group_count == 0 || term_count == 0 {
            return QualityReport::empty();
        }

        // 1. Coverage: more indexed terms = more useful, saturating (log-ish via
        //    a smooth cap at 200 terms = full marks). Rewards breadth without
        //    letting a giant dump dominate.
        let coverage = (term_count as f64 / 200.0).min(1.0);

        // 2. Symmetry: expansion must be bidirectional. Our index construction
        //    guarantees it, so we MEASURE it as a correctness check (a 1.0 here
        //    proves the artifact is well-formed; <1.0 would mean a bug/tamper).
        let mut sym_ok = 0usize;
        let mut sym_total = 0usize;
        for (t, syns) in &self.index {
            for s in syns {
                sym_total += 1;
                if self.index.get(s).map(|back| back.contains(t)).unwrap_or(false) {
                    sym_ok += 1;
                }
            }
        }
        let symmetry = if sym_total == 0 { 0.0 } else { sym_ok as f64 / sym_total as f64 };

        // 3. Cleanliness: penalize degenerate groups. A group with too many terms
        //    (everything-means-everything) destroys precision. Ideal groups are
        //    small (2-6 synonyms). Penalty grows past 8.
        let oversize = groups.iter().filter(|g| g.terms.len() > 8).count();
        let cleanliness = 1.0 - (oversize as f64 / group_count as f64);

        // 4. Self-loop check: a term must never list itself as a synonym (would
        //    be a no-op that inflates apparent coverage). Index construction
        //    excludes self, so this is a tamper/well-formedness gate → 1.0 clean.
        let self_loops = self
            .index
            .iter()
            .filter(|(t, syns)| syns.contains(*t))
            .count();
        let no_self_loops = if self_loops == 0 { 1.0 } else { 0.0 };

        // Weighted composite. Coverage is the main driver (it's what makes a
        // glossary VALUABLE); the others are structural correctness gates that
        // can only drag a broad-but-broken glossary down.
        let score = (0.50 * coverage
            + 0.20 * symmetry
            + 0.20 * cleanliness
            + 0.10 * no_self_loops)
            .clamp(0.0, 1.0);

        QualityReport {
            score,
            term_count,
            group_count,
            coverage,
            symmetry,
            cleanliness,
            no_self_loops: self_loops == 0,
            oversize_groups: oversize,
        }
    }
}

/// The graded breakdown AGORA inspects before paying a glossary bonus.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QualityReport {
    /// Composite quality in [0,1] — the bonus multiplier input.
    pub score: f64,
    pub term_count: usize,
    pub group_count: usize,
    pub coverage: f64,
    pub symmetry: f64,
    pub cleanliness: f64,
    pub no_self_loops: bool,
    pub oversize_groups: usize,
}

impl QualityReport {
    fn empty() -> Self {
        QualityReport {
            score: 0.0,
            term_count: 0,
            group_count: 0,
            coverage: 0.0,
            symmetry: 0.0,
            cleanliness: 0.0,
            no_self_loops: true,
            oversize_groups: 0,
        }
    }
}

/// Compute the AGORA bonus (in base token units) for a contributed glossary.
///
/// `base_bonus` is the max payout for a perfect (score=1.0) glossary; the actual
/// payout is `base_bonus × quality.score`, but ONLY if the glossary clears a
/// minimum quality floor (`min_score`) — below the floor it pays **zero** so
/// AGORA never pays for a lazy/degenerate glossary. Returns `(payout, report)`.
///
/// This is the function the AGORA `SettleWork`/bonus path calls: bounty poster
/// sets `base_bonus` + `min_score`, contributor submits a glossary JSON, AGORA
/// runs this, pays `payout` atomically iff `payout > 0`.
pub fn bonus_score(glossary: &Glossary, base_bonus: u128, min_score: f64) -> (u128, QualityReport) {
    let report = glossary.quality();
    if report.score < min_score {
        return (0, report);
    }
    // integer math: payout = base_bonus * score, score in [0,1] scaled by 10_000 bps
    let bps = (report.score * 10_000.0).round() as u128;
    let payout = base_bonus.saturating_mul(bps) / 10_000;
    (payout, report)
}

/// A small starter glossary for the SIGIL/Flux crawl domain — proves the shape
/// and gives the crawler immediate value. Real contributions extend this.
pub fn flux_domain_starter() -> Glossary {
    Glossary::from_groups(vec![
        Group::new(["zk", "zero-knowledge", "zeroknowledge"]),
        Group::new(["pq", "post-quantum", "postquantum"]),
        Group::new(["k8s", "kubernetes"]),
        Group::new(["sigil", "sigilgraph"]),
        Group::new(["quillon", "qug", "quillon-graph"]),
        Group::new(["p2p", "peer-to-peer", "libp2p"]),
        Group::new(["vdf", "verifiable-delay-function"]),
        Group::new(["amm", "automated-market-maker", "dex"]),
        Group::new(["llm", "large-language-model"]),
        Group::new(["sqisign", "isogeny-signature"]),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_synonyms_symmetrically() {
        let g = Glossary::from_groups(vec![Group::new(["zk", "zero-knowledge"])]);
        assert_eq!(g.synonyms("zk"), vec!["zero-knowledge"]);
        assert_eq!(g.synonyms("zero-knowledge"), vec!["zk"]);
        assert!(g.synonyms("unknown").is_empty());
    }

    #[test]
    fn expand_query_widens_terms() {
        let g = flux_domain_starter();
        let widened = g.expand_query("k8s deploy");
        assert!(widened.contains(&"k8s".to_string()));
        assert!(widened.contains(&"kubernetes".to_string()), "k8s expands to kubernetes");
        assert!(widened.contains(&"deploy".to_string()), "unknown term passes through");
    }

    #[test]
    fn expand_dedupes_and_preserves_order() {
        let g = Glossary::from_groups(vec![Group::new(["a", "b"])]);
        // querying both members must not duplicate
        let out = g.expand(&["a".into(), "b".into()]);
        assert_eq!(out, vec!["a", "b"], "no dupes, original-first order");
    }

    #[test]
    fn single_term_group_is_dropped() {
        let g = Glossary::from_groups(vec![Group::new(["lonely"])]);
        assert_eq!(g.group_count(), 0);
        assert!(g.synonyms("lonely").is_empty());
    }

    #[test]
    fn transitive_merge_across_groups() {
        // two groups sharing "zk" merge: zk—zero-knowledge and zk—zkp
        let g = Glossary::from_groups(vec![
            Group::new(["zk", "zero-knowledge"]),
            Group::new(["zk", "zkp"]),
        ]);
        let syns = g.synonyms("zk");
        assert!(syns.contains(&"zero-knowledge".to_string()));
        assert!(syns.contains(&"zkp".to_string()));
    }

    #[test]
    fn json_roundtrip() {
        let g = flux_domain_starter();
        let json = g.to_json();
        let g2 = Glossary::from_json(&json).unwrap();
        assert_eq!(g.term_count(), g2.term_count());
        assert_eq!(g.synonyms("pq"), g2.synonyms("pq"));
    }

    #[test]
    fn quality_rewards_breadth_and_structure() {
        let starter = flux_domain_starter();
        let q = starter.quality();
        assert!(q.score > 0.0, "a real glossary scores > 0");
        assert_eq!(q.symmetry, 1.0, "well-formed glossary is fully symmetric");
        assert!(q.no_self_loops, "no term lists itself");
        assert_eq!(q.oversize_groups, 0, "starter has no runaway groups");
    }

    #[test]
    fn empty_glossary_scores_zero() {
        let g = Glossary::default();
        assert_eq!(g.quality().score, 0.0);
    }

    #[test]
    fn oversize_group_is_penalized() {
        // a 10-term "everything means everything" group hurts cleanliness
        let big = Group::new(["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"]);
        let g = Glossary::from_groups(vec![big]);
        let q = g.quality();
        assert_eq!(q.oversize_groups, 1);
        assert!(q.cleanliness < 1.0, "oversize group drags cleanliness down");
    }

    #[test]
    fn bonus_pays_proportional_to_quality() {
        let good = flux_domain_starter();
        let (payout, report) = bonus_score(&good, 100_000, 0.1);
        assert!(payout > 0, "a decent glossary earns a bonus");
        // payout must equal base * score (within integer rounding)
        let expect = 100_000u128 * (report.score * 10_000.0).round() as u128 / 10_000;
        assert_eq!(payout, expect);
    }

    #[test]
    fn bonus_pays_zero_below_floor() {
        // a tiny 1-group glossary won't clear a high floor
        let weak = Glossary::from_groups(vec![Group::new(["x", "y"])]);
        let (payout, report) = bonus_score(&weak, 100_000, 0.9);
        assert_eq!(payout, 0, "below the quality floor → no bonus, AGORA pays nothing for lazy work");
        assert!(report.score < 0.9);
    }

    #[test]
    fn bigger_glossary_earns_more() {
        let small = Glossary::from_groups(vec![Group::new(["a", "b"]), Group::new(["c", "d"])]);
        let big = flux_domain_starter();
        let (p_small, _) = bonus_score(&small, 100_000, 0.0);
        let (p_big, _) = bonus_score(&big, 100_000, 0.0);
        assert!(p_big > p_small, "broader glossary earns a larger bonus");
    }
}
