//! Folding — the cortex's crease.
//!
//! Nature's most economical move is the fold: the same sheet, a smaller shape,
//! nothing cut away, and the fold decides what the thing can do. The cortex
//! needed exactly that. Its detectors speak per source LINE, so one loop on the
//! flux workspace produced 2,442 actions, and of the 500 kept on disk 487 said
//! the same sentence ("Consider bumpalo::Bump arena…", measured 2026-11-09).
//! Ranking 2,442 near-copies ranks nothing.
//!
//! [`fold_actions`] creases actions that share (crate, dimension, description)
//! into one action that keeps every site (file, line) and carries ONE figure for
//! the group — [`flux_architect::folded_impact`], which saturates instead of
//! summing. [`unfold_action`] opens it back up for whoever wants the sites.
//! Folding then unfolding loses nothing; folding twice changes nothing — both
//! are tested below.

use crate::CortexAction;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One source site an action stands for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Site {
    pub file: String,
    pub line: usize,
}

impl From<&flux_architect::FindingSite> for Site {
    fn from(s: &flux_architect::FindingSite) -> Self {
        Site { file: s.file.clone(), line: s.line }
    }
}

/// How much smaller the sheet got: unfolded ÷ folded (1.0 when nothing folded).
pub fn fold_ratio(unfolded: usize, folded: usize) -> f64 {
    if folded == 0 {
        1.0
    } else {
        unfolded as f64 / folded as f64
    }
}

/// Crease actions that share (crate, dimension, description) into one.
///
/// Groups keep first-seen order. Every site is kept; `occurrences` sums; the
/// single-site estimate is the group's largest; the group's
/// `estimated_impact_pct` becomes `folded_impact(per_site, occurrences)`.
/// An already-folded action passes through unchanged (idempotent), and an
/// action from a pre-fold state file (no `occurrences`, no `per_site_impact_pct`)
/// is read as one site whose estimate is its own figure.
pub fn fold_actions(actions: Vec<CortexAction>) -> Vec<CortexAction> {
    let mut index: HashMap<(String, String, String), usize> = HashMap::new();
    let mut folded: Vec<CortexAction> = Vec::with_capacity(actions.len());

    for a in actions {
        let key = (a.crate_name.clone(), a.dimension.clone(), a.description.clone());
        match index.get(&key) {
            None => {
                index.insert(key, folded.len());
                folded.push(normalize(a));
            }
            Some(&i) => merge(&mut folded[i], normalize(a)),
        }
    }

    for a in &mut folded {
        a.estimated_impact_pct = flux_architect::folded_impact(a.per_site_impact_pct, a.occurrences);
    }
    folded
}

/// Open a folded action: every site it stands for, in order.
pub fn unfold_action(a: &CortexAction) -> Vec<Site> {
    a.sites.clone()
}

/// Give a legacy (pre-fold) action the fields the fold reasons with.
fn normalize(mut a: CortexAction) -> CortexAction {
    if a.occurrences == 0 {
        a.occurrences = 1;
    }
    if a.per_site_impact_pct <= 0.0 {
        a.per_site_impact_pct = a.estimated_impact_pct;
    }
    a
}

fn merge(g: &mut CortexAction, a: CortexAction) {
    // A site is counted once, however many loops report it; occurrences that
    // came without a site (legacy actions) are simply added.
    let siteless = a.occurrences.saturating_sub(a.sites.len());
    let before = g.sites.len();
    for s in a.sites {
        if !g.sites.contains(&s) {
            g.sites.push(s);
        }
    }
    g.occurrences += (g.sites.len() - before) + siteless;
    g.per_site_impact_pct = g.per_site_impact_pct.max(a.per_site_impact_pct);
    g.confidence = g.confidence.max(a.confidence);
    g.id = g.id.min(a.id);
    g.created_at_secs = g.created_at_secs.min(a.created_at_secs);
    if a.applied {
        g.applied = true;
        g.applied_at_secs = match (g.applied_at_secs, a.applied_at_secs) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (x, y) => x.or(y),
        };
    }
    if g.actual_impact_pct.is_none() {
        g.actual_impact_pct = a.actual_impact_pct;
        g.prediction_accurate = a.prediction_accurate;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CortexPhase;

    fn site_action(id: u64, crate_name: &str, dim: &str, desc: &str, impact: f64, line: usize) -> CortexAction {
        CortexAction {
            id,
            source_phase: CortexPhase::Predict,
            crate_name: crate_name.into(),
            dimension: dim.into(),
            description: desc.into(),
            estimated_impact_pct: impact,
            confidence: 0.5,
            effort: "medium".into(),
            applied: false,
            actual_impact_pct: None,
            prediction_accurate: None,
            created_at_secs: 1_000 + id,
            applied_at_secs: None,
            occurrences: 1,
            sites: vec![Site { file: "lib.rs".into(), line }],
            per_site_impact_pct: impact,
        }
    }

    #[test]
    fn fold_merges_identical_actions_and_keeps_sites() {
        let mut raw = Vec::new();
        for l in 1..=30 {
            raw.push(site_action(l as u64, "flux-db", "Memory", "arena", 12.0, l));
        }
        raw.push(site_action(99, "flux-db", "Memory", "pool", 8.0, 99));
        let folded = fold_actions(raw);
        assert_eq!(folded.len(), 2);
        let big = &folded[0];
        assert_eq!(big.occurrences, 30);
        assert_eq!(big.sites.len(), 30);
        assert_eq!(big.id, 1, "the fold keeps the earliest id");
        assert_eq!(big.per_site_impact_pct, 12.0);
        assert!((big.estimated_impact_pct - flux_architect::folded_impact(12.0, 30)).abs() < 1e-9);
        assert_eq!(unfold_action(big).len(), 30);
        let single = &folded[1];
        assert_eq!(single.occurrences, 1);
        assert_eq!(single.estimated_impact_pct, 8.0, "one site: the figure is unchanged");
        assert!((fold_ratio(31, 2) - 15.5).abs() < 1e-9);
        assert_eq!(fold_ratio(0, 0), 1.0);
    }

    #[test]
    fn folding_twice_changes_nothing() {
        let raw: Vec<CortexAction> =
            (1..=12).map(|l| site_action(l as u64, "flux-rev", "Cache", "align", 15.0, l)).collect();
        let once = fold_actions(raw);
        let twice = fold_actions(once.clone());
        assert_eq!(once.len(), twice.len());
        assert_eq!(once[0].occurrences, twice[0].occurrences);
        assert_eq!(once[0].sites, twice[0].sites);
        assert!((once[0].estimated_impact_pct - twice[0].estimated_impact_pct).abs() < 1e-12);
    }

    #[test]
    fn legacy_state_file_actions_fold_too() {
        // The shape every action had in ~/.flux/cortex_state.json before the fold
        // existed: no `occurrences`, no `sites`, no `per_site_impact_pct`.
        let legacy = r#"{"id":7,"source_phase":"Predict","crate_name":"fluxc-core","dimension":"Memory",
            "description":"Consider bumpalo::Bump arena for short-lived allocations. Est: -15MB RSS",
            "estimated_impact_pct":12.0,"confidence":0.5,"effort":"medium","applied":true,
            "actual_impact_pct":9.6,"prediction_accurate":true,"created_at_secs":1757282497,"applied_at_secs":1757282497}"#;
        let a: CortexAction = serde_json::from_str(legacy).expect("old state files must still load");
        assert_eq!(a.occurrences, 1);
        assert!(a.sites.is_empty());
        let mut b = a.clone();
        b.id = 8;
        b.applied = false;
        let folded = fold_actions(vec![a, b]);
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].occurrences, 2);
        assert_eq!(folded[0].id, 7);
        assert!(folded[0].applied, "applied once = applied");
        assert_eq!(folded[0].per_site_impact_pct, 12.0);
        assert!((folded[0].estimated_impact_pct - flux_architect::folded_impact(12.0, 2)).abs() < 1e-9);
    }
}
