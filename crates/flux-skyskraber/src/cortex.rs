//! The building cortex — sense → score → decide → act.
//!
//! The tower's subsystems (transport, vault, bank, auditorium) are treated as
//! peers in the REAL `flux-p2p` X-algo table: every cortex round, each
//! subsystem reports whether it met its SLO (Service Level Objective — the
//! promise, e.g. "p95 elevator wait ≤ 60 ticks") and a 0–1 quality reading.
//! X-algo's temporal-trust weighting then does what it does on the mesh:
//! recent behavior counts most, old sins fade, but never for free.
//!
//! Decisions are pure functions of the senses (deterministic — the same day
//! replays identically); the score tables are the cortex's memory and the
//! diagnostics we report, and `sap::composite_score` fuses a worker's SAP
//! score with the building's own X-algo health into one number.

use flux_p2p::sap;
use flux_p2p::x_algo::{CrossScoreTable, PeerId as XPeerId};
use crate::workforce::Workforce;
use serde::{Deserialize, Serialize};

pub const SUBSYSTEMS: [&str; 4] = ["transport", "vault", "bank", "auditorium"];

/// SLO thresholds the cortex holds the building to.
pub const SLO_P95_WAIT_TICKS: u64 = 60;

/// One round of readings from the building.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubsystemSenses {
    pub p95_wait_ticks: u64,
    pub pending_calls: u64,
    pub vault_chain_intact: bool,
    pub payroll_failures: u32,
    pub treasury_uqug: u128,
    pub wisdom_index: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CortexDecision {
    /// Transport SLO missed — commission another freight car.
    AddRobotCar,
    /// Custody chain failed verification — freeze the vault, audit now.
    FreezeVaultAndAudit,
    /// Payroll failures — top up the treasury before the next cycle.
    RefillTreasury,
    /// The auditorium has gone quiet — invite a big thinker.
    ScheduleWisdomSession,
    /// Everything within SLO — the best decision is no decision.
    SteadyState,
}

pub struct BuildingCortex {
    systems: CrossScoreTable,
    round: u64,
}

impl BuildingCortex {
    pub fn new(_wf: &Workforce) -> Self {
        Self { systems: CrossScoreTable::new(), round: 0 }
    }

    /// One cortex round: feed each subsystem's SLO verdict + quality into the
    /// X-algo table, return the decisions the senses demand.
    pub fn tick(&mut self, senses: &SubsystemSenses) -> Vec<CortexDecision> {
        self.round += 1;
        let transport_ok = senses.p95_wait_ticks <= SLO_P95_WAIT_TICKS;
        let transport_q = 1.0 - (senses.p95_wait_ticks.min(300) as f64 / 300.0);
        self.systems.record_round(XPeerId::from("transport"), self.round, transport_ok, transport_q);

        let vault_q = if senses.vault_chain_intact { 1.0 } else { 0.0 };
        self.systems.record_round(XPeerId::from("vault"), self.round, senses.vault_chain_intact, vault_q);

        let bank_ok = senses.payroll_failures == 0;
        let bank_q = if bank_ok { 1.0 } else { 0.3 };
        self.systems.record_round(XPeerId::from("bank"), self.round, bank_ok, bank_q);

        let aud_ok = senses.wisdom_index > 0.3;
        self.systems.record_round(XPeerId::from("auditorium"), self.round, aud_ok, senses.wisdom_index.clamp(0.0, 1.0));

        let mut decisions = Vec::new();
        if !transport_ok || senses.pending_calls > 50 {
            decisions.push(CortexDecision::AddRobotCar);
        }
        if !senses.vault_chain_intact {
            decisions.push(CortexDecision::FreezeVaultAndAudit);
        }
        if senses.payroll_failures > 0 || senses.treasury_uqug < 100_000 {
            decisions.push(CortexDecision::RefillTreasury);
        }
        if !aud_ok {
            decisions.push(CortexDecision::ScheduleWisdomSession);
        }
        if decisions.is_empty() {
            decisions.push(CortexDecision::SteadyState);
        }
        decisions
    }

    /// X-algo health of one subsystem (0–1), if it has been sensed at least once.
    pub fn system_health(&self, system: &str) -> Option<f64> {
        self.systems.get(&XPeerId::from(system)).map(|s| s.total)
    }

    /// Mean X-algo health across all sensed subsystems — "how is the building".
    pub fn building_health(&self) -> f64 {
        let vals: Vec<f64> = SUBSYSTEMS.iter().filter_map(|s| self.system_health(s)).collect();
        if vals.is_empty() {
            0.0
        } else {
            vals.iter().sum::<f64>() / vals.len() as f64
        }
    }

    /// Fuse a worker's SAP score with the building's X-algo health via the
    /// real `flux-p2p` composite — a great worker in a failing building (or
    /// the reverse) lands in the middle, which is exactly the point.
    pub fn worker_composite(&self, wf: &Workforce, worker_id: &str) -> Option<f64> {
        let sap_total = wf.score_of(worker_id)?;
        Some(sap::composite_score(sap_total, self.building_health(), worker_id))
    }

    pub fn rounds(&self) -> u64 {
        self.round
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> SubsystemSenses {
        SubsystemSenses {
            p95_wait_ticks: 20,
            pending_calls: 0,
            vault_chain_intact: true,
            payroll_failures: 0,
            treasury_uqug: 5_000_000,
            wisdom_index: 0.8,
        }
    }

    #[test]
    fn healthy_building_reaches_steady_state() {
        let wf = Workforce::quillon_default(2, 1);
        let mut cortex = BuildingCortex::new(&wf);
        let decisions = cortex.tick(&healthy());
        assert_eq!(decisions, vec![CortexDecision::SteadyState]);
        assert!(cortex.building_health() > 0.0);
    }

    #[test]
    fn each_failure_triggers_its_own_decision() {
        let wf = Workforce::quillon_default(2, 1);
        let mut cortex = BuildingCortex::new(&wf);
        let mut senses = healthy();
        senses.p95_wait_ticks = 200;
        senses.vault_chain_intact = false;
        senses.payroll_failures = 3;
        senses.wisdom_index = 0.0;
        let decisions = cortex.tick(&senses);
        assert!(decisions.contains(&CortexDecision::AddRobotCar));
        assert!(decisions.contains(&CortexDecision::FreezeVaultAndAudit));
        assert!(decisions.contains(&CortexDecision::RefillTreasury));
        assert!(decisions.contains(&CortexDecision::ScheduleWisdomSession));
        assert!(!decisions.contains(&CortexDecision::SteadyState));
    }

    #[test]
    fn repeated_slo_misses_lower_system_health() {
        let wf = Workforce::quillon_default(1, 0);
        let mut good = BuildingCortex::new(&wf);
        let mut bad = BuildingCortex::new(&wf);
        let mut miss = healthy();
        miss.p95_wait_ticks = 290;
        for _ in 0..10 {
            good.tick(&healthy());
            bad.tick(&miss);
        }
        assert!(good.system_health("transport").unwrap() > bad.system_health("transport").unwrap());
    }

    #[test]
    fn worker_composite_fuses_sap_and_building_health() {
        let mut wf = Workforce::quillon_default(1, 0);
        for _ in 0..10 {
            wf.record_task_done("robot-000", 30);
        }
        let mut cortex = BuildingCortex::new(&wf);
        cortex.tick(&healthy());
        let c = cortex.worker_composite(&wf, "robot-000").unwrap();
        assert!(c > 0.0 && c <= 1.0);
        assert!(cortex.worker_composite(&wf, "robot-999").is_none());
    }
}
