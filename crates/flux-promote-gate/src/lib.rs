//! # flux-promote-gate — the battle-test promote gate
//!
//! Decides whether a candidate build may be promoted from **testnet → mainnet**
//! via the SSH auto-updater (flux-rsync → flux_hot_swap → flux-self-heal). The
//! gate is the keystone: nothing reaches mainnet unless ALL three hold:
//!
//! 1. **Forward-only** — candidate version strictly newer than the published
//!    mainnet version (no downgrade / sideways re-push).
//! 2. **Battle-tested** — every testnet gate passed (green), and the build
//!    actually soaked on testnet.
//! 3. **Governance quorum** — distinct approvals ≥ the scope's quorum:
//!    `MoneyConsensus` ⇒ 2-of-2, `LowRisk` ⇒ 1-of-2 fast-track.
//!
//! The logic is pure and std-only so it's trivially testable; the bin wires it
//! to a real `…-latest.json` manifest (the same one `flux_release_check` reads).

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A semver-ish (major, minor, patch). Pre-release suffix is ignored for
/// ordering (a `-rc` still counts as its base for the forward check).
fn parse_ver(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next().unwrap_or(core);
    let mut it = core.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next().unwrap_or("0").parse().ok()?;
    let c = it.next().unwrap_or("0").parse().ok()?;
    Some((a, b, c))
}

/// Result of the 5-gate testnet battle test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BattleTest {
    pub gates_total: u32,
    pub gates_passed: u32,
    /// Whether the candidate actually ran on testnet (soaked), not just compiled.
    pub soaked_on_testnet: bool,
}

impl BattleTest {
    pub fn green(&self) -> bool {
        self.gates_total > 0 && self.gates_passed == self.gates_total && self.soaked_on_testnet
    }
}

/// Change scope drives the required quorum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    /// Touches money or consensus — 2-of-2 required.
    MoneyConsensus,
    /// Low-risk (UI, docs, tooling) — 1-of-2 fast-track.
    LowRisk,
}

impl Scope {
    pub fn quorum(&self) -> usize {
        match self {
            Scope::MoneyConsensus => 2,
            Scope::LowRisk => 1,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Scope::MoneyConsensus => "money/consensus (2-of-2)",
            Scope::LowRisk => "low-risk (1-of-2 fast-track)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub promote: bool,
    /// Human-readable, ordered reasons (each condition, pass or fail).
    pub reasons: Vec<String>,
}

/// Evaluate the gate. `approvals` is the list of approving agent ids (deduped).
pub fn evaluate(
    candidate: &str,
    published: &str,
    battle: &BattleTest,
    scope: Scope,
    approvals: &[String],
) -> Decision {
    let mut reasons = Vec::new();
    let mut ok = true;

    // 1) forward-only
    match (parse_ver(candidate), parse_ver(published)) {
        (Some(c), Some(p)) if c > p => {
            reasons.push(format!("✓ version forward: {candidate} > {published}"));
        }
        (Some(c), Some(p)) => {
            ok = false;
            reasons.push(format!("✗ version not forward: {candidate} ({c:?}) ≤ published {published} ({p:?})"));
        }
        _ => {
            ok = false;
            reasons.push(format!("✗ unparseable version (candidate='{candidate}', published='{published}')"));
        }
    }

    // 2) battle-tested
    if battle.green() {
        reasons.push(format!("✓ battle-test green: {}/{} gates, soaked on testnet", battle.gates_passed, battle.gates_total));
    } else {
        ok = false;
        let why = if !battle.soaked_on_testnet {
            "not soaked on testnet".to_string()
        } else {
            format!("{}/{} gates passed", battle.gates_passed, battle.gates_total)
        };
        reasons.push(format!("✗ battle-test NOT green: {why}"));
    }

    // 3) governance quorum (distinct approvers)
    let distinct: BTreeSet<&str> = approvals.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
    let need = scope.quorum();
    if distinct.len() >= need {
        reasons.push(format!("✓ {} quorum met: {}/{} ({:?})", scope.label(), distinct.len(), need, distinct));
    } else {
        ok = false;
        reasons.push(format!("✗ {} quorum NOT met: {}/{} approvals", scope.label(), distinct.len(), need));
    }

    Decision { promote: ok, reasons }
}

/// Extract the `version` field from a `…-latest.json` release manifest
/// (the format `flux_release_check` reads). Returns None if absent/invalid.
pub fn published_version_from_manifest(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    v.get("version").and_then(|x| x.as_str()).map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn green() -> BattleTest { BattleTest { gates_total: 5, gates_passed: 5, soaked_on_testnet: true } }
    fn ap(v: &[&str]) -> Vec<String> { v.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn promotes_when_all_three_hold_money() {
        let d = evaluate("0.22.0", "0.17.0", &green(), Scope::MoneyConsensus, &ap(&["rocky", "deepseek"]));
        assert!(d.promote, "{:?}", d.reasons);
    }

    #[test]
    fn money_needs_two_distinct_approvals() {
        let one = evaluate("0.22.0", "0.17.0", &green(), Scope::MoneyConsensus, &ap(&["rocky"]));
        assert!(!one.promote);
        // duplicate approver doesn't count twice
        let dup = evaluate("0.22.0", "0.17.0", &green(), Scope::MoneyConsensus, &ap(&["rocky", "rocky"]));
        assert!(!dup.promote);
    }

    #[test]
    fn low_risk_fast_tracks_with_one() {
        let d = evaluate("0.22.0", "0.17.0", &green(), Scope::LowRisk, &ap(&["rocky"]));
        assert!(d.promote, "{:?}", d.reasons);
    }

    #[test]
    fn holds_when_not_battle_green() {
        let red = BattleTest { gates_total: 5, gates_passed: 4, soaked_on_testnet: true };
        assert!(!evaluate("0.22.0", "0.17.0", &red, Scope::LowRisk, &ap(&["rocky"])).promote);
        let unsoaked = BattleTest { gates_total: 5, gates_passed: 5, soaked_on_testnet: false };
        assert!(!evaluate("0.22.0", "0.17.0", &unsoaked, Scope::LowRisk, &ap(&["rocky"])).promote);
    }

    #[test]
    fn holds_on_downgrade_or_equal() {
        assert!(!evaluate("0.17.0", "0.22.0", &green(), Scope::LowRisk, &ap(&["rocky"])).promote);
        assert!(!evaluate("0.22.0", "0.22.0", &green(), Scope::LowRisk, &ap(&["rocky"])).promote);
    }

    #[test]
    fn version_parsing_tolerates_v_prefix_and_suffix() {
        assert_eq!(parse_ver("v0.22.0"), Some((0, 22, 0)));
        assert_eq!(parse_ver("0.22.0-rc1"), Some((0, 22, 0)));
        assert!(parse_ver("not-a-version").is_none());
    }

    #[test]
    fn manifest_version_extraction() {
        let j = r#"{"version":"0.17.0","url":"x","sha256":"y"}"#;
        assert_eq!(published_version_from_manifest(j).as_deref(), Some("0.17.0"));
        assert_eq!(published_version_from_manifest("{}"), None);
    }

    #[test]
    fn reasons_always_cover_all_three_conditions() {
        let d = evaluate("0.22.0", "0.17.0", &green(), Scope::MoneyConsensus, &ap(&["a", "b"]));
        assert_eq!(d.reasons.len(), 3, "one reason line per condition: {:?}", d.reasons);
    }
}
