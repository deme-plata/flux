//! The work-culture index — the meter for "almost exceeding the best
//! workplaces today".
//!
//! Here's the thing that should bother you about culture metrics: most are
//! surveys, which measure *mood*, not *conditions*. This index refuses to ask
//! anyone anything. Every component is derived from the building's own
//! measured behavior — you cannot flatter it, only run a better building.
//!
//! Five components, each 0–1, equally weighted:
//!
//! * **flow** — people and robots don't wait: 1.0 at p95 elevator wait ≤ 30
//!   ticks, decaying linearly to 0 at 300.
//! * **fairness** — payroll actually cleared: paid / (paid + failed).
//! * **wisdom** — the auditorium's fill-weighted session index.
//! * **autonomy** — robot share of transport in its designed band (0.6–0.95);
//!   full marks inside the band, decaying outside (a tower with no robot
//!   traffic isn't this tower; one with no humans at all has stopped hosting
//!   the thinkers).
//! * **safety** — the vault chain verifies: 1.0 intact, 0.0 broken. Binary on
//!   purpose — custody has no partial credit.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CultureComponents {
    pub flow: f64,
    pub fairness: f64,
    pub wisdom: f64,
    pub autonomy: f64,
    pub safety: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CultureReport {
    pub total: f64,
    pub components: CultureComponents,
    pub verdict: String,
}

pub struct CultureInputs {
    pub p95_wait_ticks: u64,
    pub payroll_paid: u32,
    pub payroll_failed: u32,
    pub wisdom_index: f64,
    pub robot_share: f64,
    pub vault_chain_intact: bool,
}

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

pub fn score(inputs: &CultureInputs) -> CultureReport {
    let flow = if inputs.p95_wait_ticks <= 30 {
        1.0
    } else {
        clamp01(1.0 - (inputs.p95_wait_ticks as f64 - 30.0) / 270.0)
    };
    let total_pay = inputs.payroll_paid + inputs.payroll_failed;
    let fairness = if total_pay == 0 { 0.0 } else { inputs.payroll_paid as f64 / total_pay as f64 };
    let wisdom = clamp01(inputs.wisdom_index);
    let autonomy = {
        let s = inputs.robot_share;
        if (0.6..=0.95).contains(&s) {
            1.0
        } else if s < 0.6 {
            clamp01(s / 0.6)
        } else {
            clamp01((1.0 - s) / 0.05)
        }
    };
    let safety = if inputs.vault_chain_intact { 1.0 } else { 0.0 };

    let components = CultureComponents { flow, fairness, wisdom, autonomy, safety };
    let total = (flow + fairness + wisdom + autonomy + safety) / 5.0;
    let verdict = match total {
        t if t >= 0.9 => "exceeds the best workplaces today",
        t if t >= 0.75 => "world-class, with named gaps",
        t if t >= 0.5 => "functional, culture debt accruing",
        _ => "the tower is running, the culture is not",
    }
    .to_string();
    CultureReport { total, components, verdict }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good_inputs() -> CultureInputs {
        CultureInputs {
            p95_wait_ticks: 20,
            payroll_paid: 72,
            payroll_failed: 0,
            wisdom_index: 0.9,
            robot_share: 0.85,
            vault_chain_intact: true,
        }
    }

    #[test]
    fn great_building_earns_the_top_verdict() {
        let r = score(&good_inputs());
        assert!(r.total >= 0.9, "total={}", r.total);
        assert_eq!(r.verdict, "exceeds the best workplaces today");
    }

    #[test]
    fn broken_vault_chain_costs_exactly_one_fifth() {
        let mut inputs = good_inputs();
        let clean = score(&inputs).total;
        inputs.vault_chain_intact = false;
        let broken = score(&inputs).total;
        assert!((clean - broken - 0.2).abs() < 1e-9);
    }

    #[test]
    fn long_waits_drag_flow_to_zero() {
        let mut inputs = good_inputs();
        inputs.p95_wait_ticks = 300;
        assert!(score(&inputs).components.flow.abs() < 1e-9);
    }

    #[test]
    fn autonomy_band_is_rewarded_extremes_are_not() {
        let mut inputs = good_inputs();
        inputs.robot_share = 0.75;
        assert!((score(&inputs).components.autonomy - 1.0).abs() < 1e-9);
        inputs.robot_share = 0.0;
        assert!(score(&inputs).components.autonomy.abs() < 1e-9);
        inputs.robot_share = 1.0;
        assert!(score(&inputs).components.autonomy.abs() < 1e-9);
    }
}
