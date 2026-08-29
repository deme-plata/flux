//! The Building Operating Index (BOI) — v0.2 rename of the "culture index".
//!
//! v0.1 called this a culture index and its components fairness, wisdom,
//! safety. That overclaimed: payroll clearing measures payment RELIABILITY,
//! not fairness; a full auditorium measures UTILIZATION, not wisdom; a valid
//! custody chain measures CUSTODY INTEGRITY, not the safety of a 392 m
//! tower. The equation was fine — the names lied a little. Now they don't.
//!
//! The index still refuses self-report on principle for what it measures:
//! every component is derived from the building's own behavior, so it cannot
//! be flattered, only run better. But it now claims to measure the OPERATING
//! ORGANISM, not the human experience. Genuine workplace quality needs a
//! separate human component,
//!
//!   Q = w_o · BOI + w_h · C_human,
//!
//! where C_human comes from anonymous, privacy-preserving feedback — because
//! if a thousand workers hate a building, no telemetry is allowed to tell
//! them they're wrong. C_human is SPECIFIED, not yet built.
//!
//! Five components, each 0–1, equally weighted:
//!
//! * **flow** — nobody waits: 1.0 at p95 hall-call wait ≤ 60 s, decaying
//!   linearly to 0 at 600 s.
//! * **payroll_reliability** — wages actually cleared: paid / (paid + failed).
//! * **utilization** — the auditorium's fill-weighted session index.
//! * **autonomy_band** — robot share of transport in its designed band
//!   (0.6–0.95): the tower runs on robots AND hosts humans.
//! * **custody_integrity** — chain verifies against the external witnesses:
//!   binary, because custody has no partial credit.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperatingComponents {
    pub flow: f64,
    pub payroll_reliability: f64,
    pub utilization: f64,
    pub autonomy_band: f64,
    pub custody_integrity: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperatingReport {
    pub total: f64,
    pub components: OperatingComponents,
    pub verdict: String,
}

pub struct OperatingInputs {
    pub p95_wait_s: u64,
    pub payroll_paid: u32,
    pub payroll_failed: u32,
    pub utilization_index: f64,
    pub robot_share: f64,
    pub custody_witnessed: bool,
}

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

pub fn score(inputs: &OperatingInputs) -> OperatingReport {
    let flow = if inputs.p95_wait_s <= 60 {
        1.0
    } else {
        clamp01(1.0 - (inputs.p95_wait_s as f64 - 60.0) / 540.0)
    };
    let total_pay = inputs.payroll_paid + inputs.payroll_failed;
    let payroll_reliability =
        if total_pay == 0 { 0.0 } else { inputs.payroll_paid as f64 / total_pay as f64 };
    let utilization = clamp01(inputs.utilization_index);
    let autonomy_band = {
        let s = inputs.robot_share;
        if (0.6..=0.95).contains(&s) {
            1.0
        } else if s < 0.6 {
            clamp01(s / 0.6)
        } else {
            clamp01((1.0 - s) / 0.05)
        }
    };
    let custody_integrity = if inputs.custody_witnessed { 1.0 } else { 0.0 };

    let components = OperatingComponents {
        flow,
        payroll_reliability,
        utilization,
        autonomy_band,
        custody_integrity,
    };
    let total =
        (flow + payroll_reliability + utilization + autonomy_band + custody_integrity) / 5.0;
    let verdict = match total {
        t if t >= 0.9 => "operating excellently — all systems inside SLO",
        t if t >= 0.75 => "operating well, with named gaps",
        t if t >= 0.5 => "functional, operational debt accruing",
        _ => "the tower is running; the organism is not",
    }
    .to_string();
    OperatingReport { total, components, verdict }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good_inputs() -> OperatingInputs {
        OperatingInputs {
            p95_wait_s: 45,
            payroll_paid: 72,
            payroll_failed: 0,
            utilization_index: 0.9,
            robot_share: 0.85,
            custody_witnessed: true,
        }
    }

    #[test]
    fn great_building_earns_the_top_verdict() {
        let r = score(&good_inputs());
        assert!(r.total >= 0.9, "total={}", r.total);
        assert_eq!(r.verdict, "operating excellently — all systems inside SLO");
    }

    #[test]
    fn broken_custody_witness_costs_exactly_one_fifth() {
        let mut inputs = good_inputs();
        let clean = score(&inputs).total;
        inputs.custody_witnessed = false;
        let broken = score(&inputs).total;
        assert!((clean - broken - 0.2).abs() < 1e-9);
    }

    #[test]
    fn long_waits_drag_flow_to_zero() {
        let mut inputs = good_inputs();
        inputs.p95_wait_s = 600;
        assert!(score(&inputs).components.flow.abs() < 1e-9);
    }

    #[test]
    fn autonomy_band_is_rewarded_extremes_are_not() {
        let mut inputs = good_inputs();
        inputs.robot_share = 0.75;
        assert!((score(&inputs).components.autonomy_band - 1.0).abs() < 1e-9);
        inputs.robot_share = 0.0;
        assert!(score(&inputs).components.autonomy_band.abs() < 1e-9);
        inputs.robot_share = 1.0;
        assert!(score(&inputs).components.autonomy_band.abs() < 1e-9);
    }
}
