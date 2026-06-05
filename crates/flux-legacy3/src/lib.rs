//! flux-legacy PROTOTYPE 3 — the CONVERGENCE / RISK lens.
//!
//! P1 (`flux-legacy`) is the file lens; P2 (`flux-legacy2`) is the architecture lens. Alone, each
//! gives a partial picture. P3 MERGES them into ONE per-crate `RiskScore` and a single ranked
//! "modernize-me" backlog — the thing a maintainer of a brownfield 100-crate node actually wants:
//! *what should I touch first, and how dangerous is it?*
//!
//! `RiskScore` (0–100, transparent + weighted):
//!   40 · blast-radius (fan-in, normalized)  — changing it ripples far
//!   25 · god-fileness (biggest file LOC)     — hard to change safely
//!   20 · NO tests                            — no safety net
//!   15 · classical crypto (needs PQ)         — a security debt on a PQ chain
//!
//! High score = high RISK to leave as-is AND high care needed to change. The backlog ranks by risk;
//! the executor (P4) still sequences low-blast LEAVES first — risk ≠ order, both are surfaced.
//!
//! Pure: composes two already-computed reports, no IO of its own.

use std::collections::HashSet;

use flux_legacy::LegacyReport;
use flux_legacy2::ArchReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskBand {
    Critical,
    High,
    Medium,
    Low,
}

impl RiskBand {
    pub fn of(score: f64) -> Self {
        if score >= 70.0 { RiskBand::Critical }
        else if score >= 45.0 { RiskBand::High }
        else if score >= 20.0 { RiskBand::Medium }
        else { RiskBand::Low }
    }
    pub fn label(&self) -> &'static str {
        match self { RiskBand::Critical => "CRITICAL", RiskBand::High => "HIGH", RiskBand::Medium => "MEDIUM", RiskBand::Low => "LOW" }
    }
}

/// Per-crate converged risk, with the contributing factors kept for auditability.
#[derive(Debug, Clone)]
pub struct CrateRisk {
    pub name: String,
    pub score: f64,
    pub blast: usize,            // fan-in (crates depending on this one)
    pub biggest_file_loc: usize, // god-file signal
    pub untested: bool,
    pub classical: bool,         // needs a PQ migration
    pub is_leaf: bool,           // 0 dependents — safe to start here
    pub band: RiskBand,
}

/// The merged, ranked backlog.
#[derive(Debug, Clone, Default)]
pub struct ConvergedReport {
    pub root: String,
    pub crates: Vec<CrateRisk>, // sorted by score, worst-first
}

impl ConvergedReport {
    pub fn critical(&self) -> usize { self.crates.iter().filter(|c| c.band == RiskBand::Critical).count() }
    pub fn high(&self) -> usize { self.crates.iter().filter(|c| c.band == RiskBand::High).count() }
}

/// Merge the file lens (P1) and the architecture lens (P2) into one ranked risk backlog.
pub fn converge(p1: &LegacyReport, p2: &ArchReport) -> ConvergedReport {
    let classical: HashSet<&str> = p2.migrations.iter().map(|(n, _)| n.as_str()).collect();
    let leaves: HashSet<&str> = p2.leaves.iter().map(|s| s.as_str()).collect();
    let max_blast = p1.crates.iter().map(|c| c.dependents.len()).max().unwrap_or(1).max(1);

    let mut risks: Vec<CrateRisk> = p1
        .crates
        .iter()
        .map(|c| {
            let blast = c.dependents.len();
            let blast_n = blast as f64 / max_blast as f64;
            let god_n = (c.biggest_file_loc as f64 / 2000.0).min(1.0);
            let untested = !c.has_tests;
            let classical = classical.contains(c.name.as_str());
            let score = 40.0 * blast_n
                + 25.0 * god_n
                + 20.0 * if untested { 1.0 } else { 0.0 }
                + 15.0 * if classical { 1.0 } else { 0.0 };
            CrateRisk {
                name: c.name.clone(),
                score,
                blast,
                biggest_file_loc: c.biggest_file_loc,
                untested,
                classical,
                is_leaf: leaves.contains(c.name.as_str()),
                band: RiskBand::of(score),
            }
        })
        .collect();
    // worst-first; stable tiebreak by name so the backlog is deterministic.
    risks.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.name.cmp(&b.name)));
    ConvergedReport { root: p1.root.clone(), crates: risks }
}

/// Human-readable backlog (worst-first), top `n`.
pub fn render_backlog(r: &ConvergedReport, n: usize) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "📊 flux-legacy3 — converged risk backlog\n   {}\n   {} crates · {} CRITICAL · {} HIGH\n\n",
        r.root, r.crates.len(), r.critical(), r.high()
    ));
    s.push_str("   score  band      blast  file  flags                 crate\n");
    for c in r.crates.iter().take(n) {
        let mut flags = String::new();
        if c.untested { flags.push_str("no-tests "); }
        if c.classical { flags.push_str("classical-pq "); }
        if c.is_leaf { flags.push_str("leaf "); }
        s.push_str(&format!(
            "   {:>5.1}  {:<9} {:>4}  {:>5} {:<21} {}\n",
            c.score, c.band.label(), c.blast, c.biggest_file_loc, flags.trim(), c.name
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_legacy::LegacyCrate;

    fn p1_with(crates: Vec<LegacyCrate>) -> LegacyReport {
        LegacyReport { root: "/ws".into(), crate_count: crates.len(), crates, ..Default::default() }
    }
    fn crate_named(name: &str, deps_in: usize, biggest: usize, has_tests: bool) -> LegacyCrate {
        LegacyCrate {
            name: name.into(),
            biggest_file_loc: biggest,
            has_tests,
            dependents: (0..deps_in).map(|i| format!("dep{i}")).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn dangerous_crate_outranks_a_safe_leaf() {
        // q-core: high fan-in (20), god-file (3000 LOC), no tests → should top the backlog.
        // util-leaf: 0 dependents, small, tested → bottom.
        let p1 = p1_with(vec![
            crate_named("q-core", 20, 3000, false),
            crate_named("util-leaf", 0, 120, true),
        ]);
        let p2 = ArchReport {
            migrations: vec![("q-core".into(), "dilithium5".into())], // also classical
            leaves: vec!["util-leaf".into()],
            ..Default::default()
        };
        let r = converge(&p1, &p2);
        assert_eq!(r.crates[0].name, "q-core", "high-blast/god/untested/classical ranks first");
        assert_eq!(r.crates[0].band, RiskBand::Critical);
        assert!(r.crates[0].score > r.crates[1].score);
        let leaf = r.crates.last().unwrap();
        assert_eq!(leaf.name, "util-leaf");
        assert!(leaf.is_leaf && !leaf.untested && !leaf.classical);
    }

    #[test]
    fn score_components_are_bounded_and_banded() {
        // worst possible crate: max blast + huge god-file + untested + classical → near 100, Critical.
        let p1 = p1_with(vec![crate_named("worst", 5, 9999, false)]);
        let p2 = ArchReport { migrations: vec![("worst".into(), "x".into())], ..Default::default() };
        let r = converge(&p1, &p2);
        assert!(r.crates[0].score <= 100.0 && r.crates[0].score >= 70.0);
        assert_eq!(r.crates[0].band, RiskBand::Critical);
        // a clean small tested crate with no dependents → Low.
        let p1b = p1_with(vec![crate_named("clean", 0, 50, true)]);
        let rb = converge(&p1b, &ArchReport::default());
        assert_eq!(rb.crates[0].band, RiskBand::Low);
    }
}
