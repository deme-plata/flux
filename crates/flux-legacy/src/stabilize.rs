//! stabilize.rs - flux-legacy Beta-1: turn live runtime PAIN into the smallest SAFE fix.
//!
//! `pulse` mines the running node's journald into a [`PulseReport`](crate::pulse::PulseReport) -
//! per-crate panic / timeout / rejection / VDF-contention counts. `stability` (P10) gives the
//! is-it-stable VERDICT. This module is the bridge between them and an ACTION: for each crate on
//! fire it proposes the *minimal* "stop the bleeding" remedy, classified by risk tier, and - the
//! whole point - it is FAIL-CLOSED. It NEVER proposes an automatic fix to a consensus, balance, or
//! crypto crate; it surfaces that pain as `Blocked` for the operator's hands only.
//!
//! Pipeline: `read_journal -> mine -> stabilize::plan -> (only auto-stageable) drive/autopilot`.

use serde::{Deserialize, Serialize};
use crate::pulse::{Category, CratePulse, PulseReport};

/// Risk tier of a proposed remedy (mirrors the master-plan T1..T5, collapsed for stabilization).
/// The stabilizer only ever emits T1/T2 as actionable; anything heavier is `Blocked`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Tier {
    /// T1: cosmetic / observability - a metric, a log downgrade. No behavior change.
    Cosmetic,
    /// T2: local, reversible behavior - a bounded timeout, a graceful unwrap, a backoff.
    Local,
    /// Consensus / balance / crypto-critical, or an inherently risky class (VDF). SURFACE ONLY -
    /// never auto-staged; the operator decides by hand.
    Blocked,
}

/// One concrete, minimal remedy for one crate's dominant pain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Remedy {
    pub crate_name: String,
    pub category: Category,
    pub tier: Tier,
    pub action: String,
    pub rationale: String,
    pub pain: f64,
}

/// The fail-closed gate: is this remedy safe for the autonomous pipeline to STAGE (never land)?
/// Only T1/T2; `Blocked` is always false. One predicate, used everywhere.
pub fn is_auto_stageable(r: &Remedy) -> bool {
    matches!(r.tier, Tier::Cosmetic | Tier::Local)
}

/// Consensus / balance / crypto surfaces the stabilizer must NEVER auto-touch. Fail-closed:
/// matched by exact name or `<name>-` prefix, lowercased. (Grounded in the node audit: q-types
/// holds the consensus verify path; q-storage holds the balance engine.)
pub fn is_protected_crate(crate_name: &str) -> bool {
    const PROTECTED: &[&str] = &[
        "q-types", "q-consensus", "q-dag", "q-vdf", "q-emission", "q-block",
        "q-storage", "q-balance", "q-ledger",
        "q-crypto", "q-crypto-advanced", "q-quantum-mixing", "q-zk-stark", "q-zk-snark", "q-sqisign",
    ];
    let c = crate_name.trim().to_ascii_lowercase();
    PROTECTED.iter().any(|p| c == *p || c.starts_with(&format!("{p}-")))
}

/// The dominant pain category for a crate, from its pulse counters (severity-ordered).
pub fn dominant_category(c: &CratePulse) -> Category {
    if c.panics > 0 {
        Category::Panic
    } else if c.vdf_contention > 0 && c.vdf_contention >= c.timeouts && c.vdf_contention >= c.rejections {
        Category::VdfContention
    } else if c.timeouts > 0 && c.timeouts >= c.rejections {
        Category::Timeout
    } else if c.rejections > 0 {
        Category::Rejection
    } else {
        Category::Other
    }
}

/// Map (crate, dominant category) -> the SMALLEST safe remedy. Fail-closed: a protected crate, or
/// the VDF class, is always `Blocked` (surfaced, never auto-fixed). A panic outside protected code
/// is T2 (graceful the unwrap); a timeout is T2 (bound it); a rejection flood is T1 (meter it).
pub fn remedy_for(crate_name: &str, category: Category, pain: f64) -> Remedy {
    let protected = is_protected_crate(crate_name);
    let (tier, action, rationale): (Tier, &str, &str) = match category {
        // VDF contention is consensus timing - never auto, regardless of crate.
        Category::VdfContention => (
            Tier::Blocked,
            "SURFACE ONLY: VDF contention is consensus timing - operator-only",
            "the verifiable-delay path governs block timing; the stabilizer never auto-tunes it",
        ),
        // A panic crashes the node. Outside protected code, graceful it; inside, surface it.
        Category::Panic if !protected => (
            Tier::Local,
            "convert the panicking unwrap/expect on this path to a graceful Result + error log",
            "an unhandled unwrap takes the whole node down; returning an error keeps it serving",
        ),
        Category::Panic => (
            Tier::Blocked,
            "SURFACE ONLY: panic in a consensus/balance/crypto crate - operator-only",
            "a panic here may be an intentional invariant guard; only the operator may convert it",
        ),
        Category::Timeout if !protected => (
            Tier::Local,
            "add a bounded timeout + exponential backoff on the blocking/dial call",
            "an unbounded wait lets one slow peer or lock stall the node; a deadline contains it",
        ),
        Category::Freeze if !protected => (
            Tier::Local,
            "add a yield/await point (or watchdog) to the hot loop holding the thread",
            "a freeze means a loop monopolizes a runtime thread; a yield keeps the node responsive",
        ),
        Category::Rejection if !protected => (
            Tier::Cosmetic,
            "meter the reject/drop flood with a counter + downgrade the per-event log to debug",
            "a rejection flood is mostly log pressure; metering it cuts noise without changing behavior",
        ),
        // any pain in a protected crate that fell through, or an unclassified Other-class pain
        _ if protected => (
            Tier::Blocked,
            "SURFACE ONLY: pain in a consensus/balance/crypto crate - operator-only",
            "the stabilizer never proposes an automatic change to consensus, balance, or crypto code",
        ),
        _ => (
            Tier::Cosmetic,
            "add a metric + structured log around the pain site (observability first)",
            "an unclassified pain still deserves a measurement before any change is considered",
        ),
    };
    Remedy {
        crate_name: crate_name.to_string(),
        category,
        tier,
        action: action.to_string(),
        rationale: rationale.to_string(),
        pain,
    }
}

/// A ranked, fail-closed stabilization plan derived from a pulse snapshot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StabilizationPlan {
    pub window: String,
    pub remedies: Vec<Remedy>,
    /// remedies the pipeline MAY stage (T1/T2, never protected).
    pub auto_safe: usize,
    /// remedies surfaced for the operator only (Blocked).
    pub blocked: usize,
}

/// Turn a [`PulseReport`] into a [`StabilizationPlan`]: one remedy per painful crate, ranked
/// auto-safe-first then by pain (worst first). Crates with zero pain are skipped.
pub fn plan(report: &PulseReport) -> StabilizationPlan {
    let mut remedies: Vec<Remedy> = report
        .crates
        .iter()
        .filter(|c| c.pain > 0.0)
        .map(|c| remedy_for(&c.crate_name, dominant_category(c), c.pain))
        .collect();
    // auto-safe (stageable) first, then by pain descending
    remedies.sort_by(|a, b| {
        is_auto_stageable(b)
            .cmp(&is_auto_stageable(a))
            .then(b.pain.partial_cmp(&a.pain).unwrap_or(std::cmp::Ordering::Equal))
    });
    let auto_safe = remedies.iter().filter(|r| is_auto_stageable(r)).count();
    let blocked = remedies.len() - auto_safe;
    StabilizationPlan { window: report.window.clone(), remedies, auto_safe, blocked }
}

/// Human-readable plan.
pub fn render(p: &StabilizationPlan) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "STABILIZATION PLAN ({}) - {} auto-safe (T1/T2), {} operator-only (blocked)\n",
        p.window, p.auto_safe, p.blocked
    ));
    for r in &p.remedies {
        let tag = match r.tier {
            Tier::Cosmetic => "T1",
            Tier::Local => "T2",
            Tier::Blocked => "!!",
        };
        let gate = if is_auto_stageable(r) { "stageable" } else { "OPERATOR " };
        s.push_str(&format!(
            "  [{tag}] {:<22} pain={:>10.1}  {}  {:?}: {}\n",
            r.crate_name, r.pain, gate, r.category, r.action
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cp(name: &str, panics: u64, timeouts: u64, rejections: u64, vdf: u64) -> CratePulse {
        let mut c = CratePulse { crate_name: name.to_string(), ..Default::default() };
        c.panics = panics;
        c.timeouts = timeouts;
        c.rejections = rejections;
        c.vdf_contention = vdf;
        c.pain = (panics as f64) * 1000.0 + (timeouts as f64) * 3.0 + (rejections as f64) * 1.0 + (vdf as f64) * 0.5;
        c
    }

    #[test]
    fn protected_crates_are_recognized() {
        assert!(is_protected_crate("q-types"));
        assert!(is_protected_crate("q-storage"));
        assert!(is_protected_crate("q-crypto-advanced"));
        assert!(is_protected_crate("q-vdf"));
        assert!(!is_protected_crate("q-network"));
        assert!(!is_protected_crate("q-api-server"));
    }

    #[test]
    fn fail_closed_on_consensus_and_vdf() {
        // a panic in a protected crate is surfaced, never auto-staged
        let r = remedy_for("q-types", Category::Panic, 1000.0);
        assert_eq!(r.tier, Tier::Blocked);
        assert!(!is_auto_stageable(&r));
        // VDF contention is blocked even outside a protected crate
        let r = remedy_for("q-api-server", Category::VdfContention, 50.0);
        assert_eq!(r.tier, Tier::Blocked);
        assert!(!is_auto_stageable(&r));
        // a timeout inside a protected crate also falls closed
        let r = remedy_for("q-storage", Category::Timeout, 9.0);
        assert_eq!(r.tier, Tier::Blocked);
    }

    #[test]
    fn network_pain_is_safely_stageable() {
        // a dial timeout in q-network (the 0-peer stall class) -> T2, auto-stageable
        let r = remedy_for("q-network", Category::Timeout, 30.0);
        assert_eq!(r.tier, Tier::Local);
        assert!(is_auto_stageable(&r));
        // a reject/drop flood -> T1 cosmetic, stageable
        let r = remedy_for("q-api-server", Category::Rejection, 5.0);
        assert_eq!(r.tier, Tier::Cosmetic);
        assert!(is_auto_stageable(&r));
        // a non-protected panic -> T2 graceful, stageable
        let r = remedy_for("q-network", Category::Panic, 1000.0);
        assert_eq!(r.tier, Tier::Local);
        assert!(is_auto_stageable(&r));
    }

    #[test]
    fn plan_ranks_safe_first_then_pain_and_counts() {
        let report = PulseReport {
            window: "last 30 min".to_string(),
            total_lines: 100,
            parsed: 50,
            crates: vec![
                cp("q-types", 1, 0, 0, 0),      // panic in protected -> Blocked, pain 1000
                cp("q-network", 0, 5, 0, 0),    // timeouts -> T2 stageable, pain 15
                cp("q-api-server", 0, 0, 8, 0), // rejections -> T1 stageable, pain 8
            ],
        };
        let p = plan(&report);
        assert_eq!(p.auto_safe, 2);
        assert_eq!(p.blocked, 1);
        // auto-safe come first despite q-types having the highest pain
        assert!(is_auto_stageable(&p.remedies[0]));
        assert!(is_auto_stageable(&p.remedies[1]));
        assert_eq!(p.remedies[2].crate_name, "q-types"); // blocked last
        // among the safe ones, higher pain first (q-network 15 > q-api-server 8)
        assert_eq!(p.remedies[0].crate_name, "q-network");
    }

    #[test]
    fn dominant_category_prioritizes_severity() {
        assert_eq!(dominant_category(&cp("x", 1, 5, 5, 5)), Category::Panic);
        assert_eq!(dominant_category(&cp("x", 0, 0, 0, 9)), Category::VdfContention);
        assert_eq!(dominant_category(&cp("x", 0, 4, 2, 0)), Category::Timeout);
        assert_eq!(dominant_category(&cp("x", 0, 0, 3, 0)), Category::Rejection);
        assert_eq!(dominant_category(&cp("x", 0, 0, 0, 0)), Category::Other);
    }
}
