//! The workforce — mainly robots, a few people — scored with the REAL
//! `flux-p2p` SAP table.
//!
//! Don't be scared by the acronym: SAP is the peer-scoring system the Flux
//! mesh uses to rank nodes — **S**ervice (contribution), **A**vailability
//! (latency/uptime), **P**articipation (stake + accuracy). Here the insight is
//! that a robot workforce IS a peer swarm: each worker is a `PeerId`, each
//! completed task is a participation round, each task duration is a latency
//! sample, each wallet balance is stake, each fault an equivocation. Nothing
//! is re-derived — we feed the very same `ScoreTable` the mesh uses.

use flux_p2p::sap::{PeerId as SapPeerId, SAPComponents, SAPScore, ScoreTable};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerKind {
    Robot,
    Human,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worker {
    pub id: String,
    pub kind: WorkerKind,
    pub home_floor: i32,
    pub wage_uqug: u128,
    pub wallet: String,
}

pub struct Workforce {
    pub workers: Vec<Worker>,
    scores: ScoreTable,
}

impl Workforce {
    /// The canonical crew: robots live in the bays (B1), humans office at 10+.
    /// Humans earn more per day (there are few of them, and they bring the
    /// judgment); robots earn a maintenance-and-upgrade stipend.
    pub fn quillon_default(robots: u32, humans: u32) -> Self {
        let mut workers = Vec::new();
        for i in 0..robots {
            workers.push(Worker {
                id: format!("robot-{i:03}"),
                kind: WorkerKind::Robot,
                home_floor: -1,
                wage_uqug: 1_000,
                wallet: format!("wallet-robot-{i:03}"),
            });
        }
        for i in 0..humans {
            workers.push(Worker {
                id: format!("human-{i:02}"),
                kind: WorkerKind::Human,
                home_floor: 10 + i as i32,
                wage_uqug: 5_000,
                wallet: format!("wallet-human-{i:02}"),
            });
        }
        let mut scores = ScoreTable::new();
        // SAP mutators only touch ENROLLED peers (on the mesh, peers announce
        // themselves). Hiring is the announcement: every worker starts with a
        // clean baseline — perfect accuracy and uptime, no stake yet.
        for w in &workers {
            scores.update(
                SapPeerId::from(w.id.as_str()),
                SAPComponents {
                    contribution: 0.5,
                    latency: 1.0,
                    stake: 0.0,
                    accuracy: 1.0,
                    uptime: 1.0,
                },
            );
        }
        Self { workers, scores }
    }

    pub fn robot_count(&self) -> usize {
        self.workers.iter().filter(|w| w.kind == WorkerKind::Robot).count()
    }

    /// A finished task = one SAP participation round + one latency sample.
    /// `duration_ticks` maps to milliseconds 1:1 for the latency curve
    /// (SAP scores 1.0 below 100 — so a task under 100 ticks is "fast").
    pub fn record_task_done(&mut self, worker_id: &str, duration_ticks: u64) {
        let peer = SapPeerId::from(worker_id);
        self.scores.record_participation(&peer);
        self.scores.update_latency(&peer, duration_ticks as f64, duration_ticks as f64 * 1.5);
    }

    /// v0.2: the stake component is SERVICE COLLATERAL, never money.
    /// v0.1 fed wallet balances in here, which quietly built the rule
    /// "wealthier worker ⇒ higher structural trust" — wrong for people,
    /// wrong even for robots. Now: robots stake their verified service
    /// history (completed tasks — earned, not owned); humans stake nothing,
    /// and their score rests entirely on contribution, latency, accuracy
    /// and uptime.
    pub fn record_service_collateral(&mut self, worker_id: &str, completed_tasks: u64) {
        let is_robot = self
            .workers
            .iter()
            .any(|w| w.id == worker_id && w.kind == WorkerKind::Robot);
        if is_robot {
            self.scores.update_stake(&SapPeerId::from(worker_id), completed_tasks);
        }
    }

    /// A dropped pallet, a missed shift, a falsified log: SAP calls all of
    /// these an equivocation and zeroes the accuracy component outright —
    /// the mesh does not haggle about trust, and neither does the tower.
    pub fn record_fault(&mut self, worker_id: &str) {
        self.scores.mark_equivocation(&SapPeerId::from(worker_id));
    }

    pub fn score_of(&self, worker_id: &str) -> Option<f64> {
        self.scores.get(&SapPeerId::from(worker_id))
    }

    pub fn top(&self, n: usize) -> Vec<&SAPScore> {
        self.scores.top_peers(n)
    }

    pub fn sap_table(&self) -> &ScoreTable {
        &self.scores
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crew_composition_is_robot_heavy() {
        let wf = Workforce::quillon_default(64, 8);
        assert_eq!(wf.workers.len(), 72);
        assert_eq!(wf.robot_count(), 64);
    }

    #[test]
    fn diligent_worker_outscores_faulty_one() {
        let mut wf = Workforce::quillon_default(2, 0);
        for _ in 0..20 {
            wf.record_task_done("robot-000", 40); // fast, reliable
        }
        for _ in 0..20 {
            wf.record_task_done("robot-001", 40);
        }
        wf.record_fault("robot-001");
        wf.record_fault("robot-001");
        let good = wf.score_of("robot-000").unwrap();
        let bad = wf.score_of("robot-001").unwrap();
        assert!(good > bad, "good={good} bad={bad}");
    }

    #[test]
    fn money_never_raises_a_human_score() {
        let mut wf = Workforce::quillon_default(1, 1);
        wf.record_task_done("robot-000", 40);
        wf.record_task_done("human-00", 40);
        let before = wf.score_of("human-00").unwrap();
        // Even an absurd "collateral" number is ignored for humans…
        wf.record_service_collateral("human-00", u64::MAX);
        assert_eq!(wf.score_of("human-00").unwrap(), before);
        // …while a robot's earned service history does count as stake.
        let robot_before = wf.score_of("robot-000").unwrap();
        wf.record_service_collateral("robot-000", 500);
        assert!(wf.score_of("robot-000").unwrap() >= robot_before);
    }

    #[test]
    fn top_ranking_orders_by_total() {
        let mut wf = Workforce::quillon_default(3, 0);
        wf.record_task_done("robot-000", 30);
        wf.record_task_done("robot-000", 30);
        wf.record_task_done("robot-001", 900); // slow
        wf.record_task_done("robot-002", 30);
        wf.record_fault("robot-002");
        let top = wf.top(3);
        assert_eq!(top[0].peer.0, "robot-000");
    }
}
