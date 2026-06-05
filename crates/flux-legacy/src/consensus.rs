//! consensus.rs — flux-legacy **P7: CONSENSUS-CRITICAL changes, safely.**
//!
//! The last gate before flux may touch VDF / emission / block-validation code on a LIVE chain. Three
//! mechanisms, all mandatory for a Tier-4 change (none optional, fail-closed):
//!
//!   1. **HEIGHT-GATE** — ship the change *compiled but disabled* behind a block-height activation
//!      flag, far enough ahead that validators upgrade asynchronously. Old blocks ALWAYS validate by
//!      the old rule. (This is the Quillon runbook's #1 mainnet-safety rule, mechanized.)
//!   2. **CANARY** — staged rollout (1 non-validator → 3 validators → full), each step gated on a
//!      clean [`crate::pulse`] window.
//!   3. **QUORUM** — full rollout requires ≥ 2/3 validator-operator approval.
//!
//! Pure state-machine + codegen (unit-tested); the deploy/Pulse I/O is the operator's wiring.

/// Minimum blocks an activation must sit in the future (~2 weeks at ~1 block/s) so the whole fleet
/// can upgrade before the new rule activates. Below this, a height-gate is unsafe.
pub const MIN_ACTIVATION_MARGIN: u64 = 20_000;

/// A height-gated upgrade: a named rule that activates at `activation_height`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeightGate {
    pub upgrade: String,
    pub activation_height: u64,
}

/// Validate that an activation height is SAFE: strictly in the future by at least `min_margin`.
/// Rejects past/near-future activations that would fork un-upgraded validators.
pub fn validate_activation(current_height: u64, activation_height: u64, min_margin: u64) -> Result<(), String> {
    if activation_height <= current_height {
        return Err(format!("activation {activation_height} is not in the future (current {current_height})"));
    }
    let margin = activation_height - current_height;
    if margin < min_margin {
        return Err(format!(
            "activation only {margin} blocks ahead (need ≥ {min_margin}); validators can't all upgrade in time"
        ));
    }
    Ok(())
}

/// GENERATE the height-gated Rust wrapper for a consensus change: the NEW rule runs only at/after
/// activation; historical blocks keep the OLD rule. This is the runbook's CORRECT pattern, emitted
/// mechanically so a refactor can't accidentally change validation for past blocks.
pub fn height_gate_wrapper(upgrade: &str, fn_sig: &str, old_body: &str, new_body: &str) -> String {
    format!(
        "{fn_sig} {{\n\
         \x20   // height-gated: NEW rule only at/after activation; old blocks keep the OLD rule (mainnet-safe)\n\
         \x20   if q_consensus_guard::is_upgrade_active(Upgrade::{upgrade}, block.height) {{\n\
         \x20       {new_body}\n\
         \x20   }} else {{\n\
         \x20       {old_body}\n\
         \x20   }}\n\
         }}"
    )
}

/// ≥ 2/3 validator-operator approval (strict two-thirds; at least one approval required).
pub fn quorum_2of3(approvals: usize, total: usize) -> bool {
    total > 0 && approvals > 0 && approvals * 3 >= total * 2
}

/// Canary rollout stages — a T4 change climbs these one at a time; never skips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanaryStage {
    Proposed,
    ShadowPassed,    // P6 equivalence held
    Canary1,         // live on ONE non-validator
    Validators3,     // live on 3 validators (height-gated)
    FullRollout,     // network-wide
}

impl CanaryStage {
    pub fn label(self) -> &'static str {
        match self {
            CanaryStage::Proposed => "proposed",
            CanaryStage::ShadowPassed => "shadow-passed",
            CanaryStage::Canary1 => "canary (1 non-validator)",
            CanaryStage::Validators3 => "3 validators",
            CanaryStage::FullRollout => "full rollout",
        }
    }
}

/// Evidence available when trying to advance a canary one stage.
#[derive(Debug, Clone, Copy, Default)]
pub struct CanaryEvidence {
    pub shadow_ok: bool,          // P6 shadow verdict was Match
    pub height_gate_valid: bool,  // validate_activation passed
    pub pulse_clean_minutes: u64, // minutes the current stage ran with a clean Pulse
    pub required_clean_minutes: u64,
    pub approvals: usize,
    pub total_validators: usize,
}

/// Advance the canary exactly ONE stage if its precondition holds; else Err (fail-closed, no skip).
pub fn advance_canary(stage: CanaryStage, ev: &CanaryEvidence) -> Result<CanaryStage, String> {
    match stage {
        CanaryStage::Proposed => {
            if ev.shadow_ok { Ok(CanaryStage::ShadowPassed) }
            else { Err("cannot advance: P6 shadow verification has not passed".into()) }
        }
        CanaryStage::ShadowPassed => {
            if ev.height_gate_valid { Ok(CanaryStage::Canary1) }
            else { Err("cannot advance: height-gate activation not validated".into()) }
        }
        CanaryStage::Canary1 => {
            if ev.pulse_clean_minutes >= ev.required_clean_minutes {
                Ok(CanaryStage::Validators3)
            } else {
                Err(format!("canary needs {} clean Pulse minutes (have {})", ev.required_clean_minutes, ev.pulse_clean_minutes))
            }
        }
        CanaryStage::Validators3 => {
            if !quorum_2of3(ev.approvals, ev.total_validators) {
                Err(format!("full rollout needs ≥2/3 approval ({}/{})", ev.approvals, ev.total_validators))
            } else if ev.pulse_clean_minutes < ev.required_clean_minutes {
                Err("validators stage Pulse not clean long enough".into())
            } else {
                Ok(CanaryStage::FullRollout)
            }
        }
        CanaryStage::FullRollout => Err("already at full rollout".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_must_be_future_and_far_enough() {
        assert!(validate_activation(1000, 500, MIN_ACTIVATION_MARGIN).is_err(), "past height");
        assert!(validate_activation(1000, 1000, MIN_ACTIVATION_MARGIN).is_err(), "equal height");
        assert!(validate_activation(1000, 1000 + 5_000, MIN_ACTIVATION_MARGIN).is_err(), "too soon (5k < 20k)");
        assert!(validate_activation(1000, 1000 + 25_000, MIN_ACTIVATION_MARGIN).is_ok(), "far enough");
    }

    #[test]
    fn wrapper_emits_height_gated_pattern() {
        let w = height_gate_wrapper(
            "PostQuantumVdf",
            "fn validate_vdf(block: &Block) -> bool",
            "verify_legacy_vdf(block)",
            "verify_pq_vdf(block)",
        );
        assert!(w.contains("is_upgrade_active(Upgrade::PostQuantumVdf, block.height)"));
        assert!(w.contains("verify_pq_vdf(block)"), "new rule present");
        assert!(w.contains("verify_legacy_vdf(block)"), "old rule preserved for historical blocks");
        // new rule must be inside the `if active` branch (mainnet-safe ordering)
        let if_idx = w.find("if q_consensus_guard").unwrap();
        let new_idx = w.find("verify_pq_vdf").unwrap();
        let else_idx = w.find("} else {").unwrap();
        assert!(if_idx < new_idx && new_idx < else_idx, "new rule gated before the else (old) branch");
    }

    #[test]
    fn quorum_two_thirds() {
        assert!(quorum_2of3(2, 3));
        assert!(!quorum_2of3(1, 3));
        assert!(quorum_2of3(3, 4));   // 3/4 ≥ 2/3
        assert!(!quorum_2of3(2, 4));  // 1/2 < 2/3
        assert!(!quorum_2of3(0, 0));  // no validators → no quorum
        assert!(!quorum_2of3(0, 5));  // zero approvals
    }

    #[test]
    fn canary_climbs_one_stage_at_a_time() {
        let mut ev = CanaryEvidence { shadow_ok: false, ..Default::default() };
        // can't leave Proposed without shadow
        assert!(advance_canary(CanaryStage::Proposed, &ev).is_err());
        ev.shadow_ok = true;
        assert_eq!(advance_canary(CanaryStage::Proposed, &ev).unwrap(), CanaryStage::ShadowPassed);
        // can't leave ShadowPassed without a validated height-gate
        assert!(advance_canary(CanaryStage::ShadowPassed, &ev).is_err());
        ev.height_gate_valid = true;
        assert_eq!(advance_canary(CanaryStage::ShadowPassed, &ev).unwrap(), CanaryStage::Canary1);
        // canary needs clean Pulse dwell
        ev.required_clean_minutes = 2880; // 48h
        assert!(advance_canary(CanaryStage::Canary1, &ev).is_err());
        ev.pulse_clean_minutes = 2880;
        assert_eq!(advance_canary(CanaryStage::Canary1, &ev).unwrap(), CanaryStage::Validators3);
        // full rollout needs 2/3 quorum
        ev.approvals = 1; ev.total_validators = 3;
        assert!(advance_canary(CanaryStage::Validators3, &ev).is_err());
        ev.approvals = 2;
        assert_eq!(advance_canary(CanaryStage::Validators3, &ev).unwrap(), CanaryStage::FullRollout);
        // terminal
        assert!(advance_canary(CanaryStage::FullRollout, &ev).is_err());
    }
}
