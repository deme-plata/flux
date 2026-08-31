//! A deterministic day in the tower — v0.2, in real seconds.
//!
//! One tick = one second; a day = 86,400 ticks. Same seed → the identical
//! day, down to the report's BLAKE3 fingerprint. Randomness comes from a
//! seeded ChaCha8 stream, the same discipline `flux-chronos` uses.
//!
//! The day's script: robots surge up from the bays at 06:00, humans arrive
//! 09:00, gold moves mid-morning, the beacon fires an ANCHOR PULSE at noon
//! (custody history externally frozen), a big thinker speaks at 13:00,
//! payroll runs 17:00, a second anchor pulse at 20:00, and the cortex takes
//! a reading every hour. The day closes by folding the whole organism into
//! its Merkle state commitment.

use crate::bank::TREASURY;
use crate::cortex::{CortexDecision, SubsystemSenses};
use crate::culture::{self, OperatingInputs, OperatingReport};
use crate::elevator::{CarKind, HallCall, TransportMetrics};
use crate::auditorium::Talk;
use crate::state_root;
use crate::tower::Zone;
use crate::vault::{Clearance, GoldBar};
use crate::Building;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

pub const TICKS_PER_DAY: u64 = 86_400;

const H: u64 = 3_600;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayReport {
    pub tower: String,
    pub seed: u64,
    pub ticks: u64,
    pub transport: TransportMetrics,
    pub vault_bars: usize,
    pub vault_head: String,
    pub vault_anchors: usize,
    /// Chain verifies AND every external anchor recomputes.
    pub vault_witnessed: bool,
    pub treasury_uqug: u128,
    pub payroll_paid: u32,
    pub payroll_failed: u32,
    pub talks_held: usize,
    pub operating: OperatingReport,
    /// Emanation-security posture as the tower stands today (EMSEC Doctrine v0).
    pub emsec: crate::emsec::EmsecPosture,
    /// The posture the same blueprint reaches once hardened to doctrine.
    pub emsec_hardened_total: f64,
    pub cortex_rounds: u64,
    pub decisions: Vec<CortexDecision>,
    /// The tower's Merkle state commitment at end of day (hex).
    pub state_commitment: String,
    /// BLAKE3 over the deterministic fields above — the day's fingerprint.
    pub report_hash: String,
}

fn fingerprint(r: &DayReport) -> String {
    let mut h = blake3::Hasher::new();
    h.update(r.tower.as_bytes());
    h.update(&r.seed.to_le_bytes());
    h.update(&r.ticks.to_le_bytes());
    h.update(&r.transport.served.to_le_bytes());
    h.update(&r.transport.p95_wait_s.to_le_bytes());
    h.update(&(r.transport.avg_wait_s * 1e6).round().to_bits().to_le_bytes());
    h.update(&(r.transport.mean_batch * 1e6).round().to_bits().to_le_bytes());
    h.update(&(r.vault_bars as u64).to_le_bytes());
    h.update(r.vault_head.as_bytes());
    h.update(&(r.vault_anchors as u64).to_le_bytes());
    h.update(&r.treasury_uqug.to_le_bytes());
    h.update(&r.payroll_paid.to_le_bytes());
    h.update(&(r.talks_held as u64).to_le_bytes());
    h.update(&(r.operating.total * 1e6).round().to_bits().to_le_bytes());
    h.update(&(r.emsec.total * 1e6).round().to_bits().to_le_bytes());
    h.update(r.state_commitment.as_bytes());
    h.update(serde_json::to_string(&r.decisions).unwrap().as_bytes());
    hex::encode(&h.finalize().as_bytes()[..16])
}

/// Run one full day on the canonical (as-it-stands) tower.
pub fn run_day(seed: u64) -> DayReport {
    run_day_inner(seed, false)
}

/// Run one full day on the guardian-ring tower, born to EMSEC Doctrine v0 — the
/// building AND its emanations config both hardened, so the posture reads 1.000.
pub fn run_day_hardened(seed: u64) -> DayReport {
    run_day_inner(seed, true)
}

/// Run one full day and return the fingerprinted report.
fn run_day_inner(seed: u64, hardened: bool) -> DayReport {
    let mut b = if hardened {
        Building::quillon_hardened().expect("guardian-ring blueprint validates")
    } else {
        Building::quillon_default().expect("canonical blueprint validates")
    };
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    // Destinations come from the blueprint itself — if the tower changes
    // shape, the day's traffic follows.
    let office_floors: Vec<i32> = b.spec.levels_of(Zone::Offices);

    // 13:00 wisdom session, booked before the day starts.
    b.auditorium
        .schedule(Talk {
            id: "wisdom-1300".into(),
            speaker: "a visiting big thinker".into(),
            title: "Food for Thought".into(),
            start_tick: 13 * H,
            duration_ticks: 90 * 60,
            expected_attendance: 900,
        })
        .expect("the day's one talk cannot conflict");

    let mut payroll_paid = 0u32;
    let mut payroll_failed = 0u32;
    let mut all_decisions: Vec<CortexDecision> = Vec::new();
    let mut tasks_done: std::collections::BTreeMap<String, u64> = Default::default();

    for tick in 0..TICKS_PER_DAY {
        let hour = tick / H;

        // 06:00–08:00 robot surge: bays → office floors, one call / 3 min.
        if (6..8).contains(&hour) && tick % 180 == 0 {
            let to = office_floors[rng.gen_range(0..office_floors.len())];
            b.transport.request(HallCall { from: -1, to, kind: CarKind::RobotFreight, requested_tick: tick });
        }
        // 09:00–10:00 humans arrive through the lobby, one call / 10 min.
        if hour == 9 && tick % 600 == 0 {
            let to = office_floors[rng.gen_range(0..office_floors.len())];
            b.transport.request(HallCall { from: 0, to, kind: CarKind::HumanCab, requested_tick: tick });
        }
        // All-day trickle of robot logistics, one call / 17 min.
        if (8..18).contains(&hour) && tick % 1_020 == 0 {
            let from = office_floors[rng.gen_range(0..office_floors.len())];
            let to = office_floors[rng.gen_range(0..office_floors.len())];
            b.transport.request(HallCall { from, to, kind: CarKind::RobotFreight, requested_tick: tick });
        }

        // 10:00 gold intake; 12:00 ANCHOR PULSE; 14:00 settlement + audit;
        // 20:00 second anchor pulse.
        if tick == 10 * H {
            for i in 0..3 {
                let bar = GoldBar {
                    serial: format!("AU-{seed:04x}-{i:03}"),
                    weight_g: 12_400,
                    fineness_ppm: 999_900,
                };
                b.vault.deposit(bar, "quillon-bank", Clearance::Officer, tick).expect("intake fits");
            }
            let _ = b.bank.pay(TREASURY, "gold-client-escrow", 1, "custody intake marker");
        }
        if tick == 12 * H || tick == 20 * H {
            // The beacon's stronger pulse: custody history frozen externally.
            b.vault.anchor(tick);
        }
        if tick == 14 * H {
            let serial = format!("AU-{seed:04x}-000");
            b.vault.withdraw(&serial, "custodian-quorum", Clearance::Custodian, tick).expect("bar exists");
            b.vault.run_audit("resident-auditor", tick);
        }

        // Talks conclude when their window passes; attendance is seeded-random.
        if tick == 15 * H {
            let attendance = rng.gen_range(700..=1_200);
            b.auditorium.hold_due_talks(tick, |_| attendance);
        }

        // 17:00 payroll; afterwards robots stake their EARNED service history.
        if tick == 17 * H {
            let report = b.bank.run_payroll(&b.workforce);
            payroll_paid = report.paid_workers;
            payroll_failed = report.failures;
            for w in b.workforce.workers.clone() {
                let done = tasks_done.get(&w.id).copied().unwrap_or(0);
                b.workforce.record_service_collateral(&w.id, done);
            }
        }

        // Robots finish tasks through the working day; a rare fault occurs.
        if (8..18).contains(&hour) && tick % 660 == 0 {
            let w = &b.workforce.workers[rng.gen_range(0..b.workforce.workers.len())];
            let id = w.id.clone();
            let duration = rng.gen_range(20..180);
            b.workforce.record_task_done(&id, duration);
            *tasks_done.entry(id.clone()).or_insert(0) += 1;
            if rng.gen_ratio(1, 200) {
                b.workforce.record_fault(&id);
            }
        }

        b.transport.step(tick);

        // The cortex takes a reading on the hour.
        if tick % H == H - 1 {
            let m = b.transport.metrics();
            let senses = SubsystemSenses {
                p95_wait_s: m.p95_wait_s,
                pending_calls: m.pending,
                vault_witnessed: b.vault.verify_chain().is_ok() && b.vault.verify_witnessed().is_ok(),
                payroll_failures: payroll_failed,
                treasury_uqug: b.bank.balance(TREASURY),
                utilization_index: b.auditorium.wisdom_index(),
            };
            let mut decisions = b.cortex.tick(&senses);
            all_decisions.append(&mut decisions);
        }
    }

    let transport = b.transport.metrics();
    let witnessed = b.vault.verify_chain().is_ok() && b.vault.verify_witnessed().is_ok();
    let operating = culture::score(&OperatingInputs {
        p95_wait_s: transport.p95_wait_s,
        payroll_paid,
        payroll_failed,
        utilization_index: b.auditorium.wisdom_index(),
        robot_share: transport.robot_share,
        custody_witnessed: witnessed,
    });
    let commitment = state_root::tower_state(&b);

    // The tower's emanation-security posture is blueprint-derived, so it does
    // not vary with the day's seed — but it belongs in the day's record. A
    // hardened building is scored under the hardened config it was built for;
    // the canonical building is scored honestly as it stands. Either way the
    // hardened target shows what the doctrine buys.
    let emsec_cfg = if hardened {
        crate::emsec::EmsecConfig::doctrine_v0_hardened()
    } else {
        crate::emsec::EmsecConfig::doctrine_v0()
    };
    let emsec = crate::emsec::assess(&b.spec, &emsec_cfg);
    let emsec_hardened_total =
        crate::emsec::assess(&b.spec, &crate::emsec::EmsecConfig::doctrine_v0_hardened()).total;

    let mut report = DayReport {
        tower: b.spec.name.clone(),
        seed,
        ticks: TICKS_PER_DAY,
        transport,
        vault_bars: b.vault.bar_count(),
        vault_head: b.vault.head_hex(),
        vault_anchors: b.vault.anchors().len(),
        vault_witnessed: witnessed,
        treasury_uqug: b.bank.balance(TREASURY),
        payroll_paid,
        payroll_failed,
        talks_held: b.auditorium.talks_held(),
        operating,
        emsec,
        emsec_hardened_total,
        cortex_rounds: b.cortex.rounds(),
        decisions: all_decisions,
        state_commitment: hex::encode(commitment.commitment),
        report_hash: String::new(),
    };
    report.report_hash = fingerprint(&report);
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_day_same_hash() {
        let a = run_day(42);
        let b = run_day(42);
        assert_eq!(a.report_hash, b.report_hash);
        assert_eq!(a.transport, b.transport);
        assert_eq!(a.vault_head, b.vault_head);
        assert_eq!(a.state_commitment, b.state_commitment);
    }

    #[test]
    fn different_seed_different_day() {
        let a = run_day(1);
        let b = run_day(2);
        assert_ne!(a.report_hash, b.report_hash);
    }

    #[test]
    fn the_day_actually_happened() {
        let r = run_day(7);
        assert!(r.transport.served >= 75, "served={}", r.transport.served);
        assert_eq!(r.vault_bars, 2); // 3 in, 1 out
        assert!(r.vault_witnessed);
        assert_eq!(r.vault_anchors, 2);
        assert_eq!(r.payroll_paid, 72);
        assert_eq!(r.talks_held, 1);
        assert_eq!(r.cortex_rounds, 24);
        assert!(r.operating.total > 0.85, "operating={}", r.operating.total);
        assert_eq!(r.state_commitment.len(), 64);
    }

    #[test]
    fn the_day_records_an_honest_emsec_posture() {
        let r = run_day(7);
        // Present, bounded, and the unmasked beacon keeps it porous today…
        assert!(r.emsec.total > 0.0 && r.emsec.total < 1.0);
        assert!(!r.emsec.findings.is_empty());
        // …while the hardened target is meaningfully higher.
        assert!(r.emsec_hardened_total > r.emsec.total + 0.3);
        assert!(r.emsec_hardened_total >= 0.9);
    }

    #[test]
    fn the_guardian_ring_day_scores_a_perfect_posture() {
        let r = run_day_hardened(7);
        assert!((r.emsec.total - 1.0).abs() < 1e-9, "total={}", r.emsec.total);
        assert!(r.emsec.findings.is_empty(), "findings: {:?}", r.emsec.findings);
        assert!((r.emsec.components.red_black_separation - 1.0).abs() < 1e-9);
        assert!((r.emsec.components.inspectable_space - 1.0).abs() < 1e-9);
        assert!((r.emsec.components.averaging_resistance - 1.0).abs() < 1e-9);
        assert!((r.emsec.components.fail_closed - 1.0).abs() < 1e-9);
        // The building still runs the same excellent day.
        assert_eq!(r.payroll_paid, 72);
        assert!(r.vault_witnessed);
    }

    #[test]
    fn batching_physics_beat_the_v01_queue() {
        // v0.1 (toy physics, no batching) measured p95 = 276 minutes-of-tick.
        // With kinematic cars and real batching the surge must clear inside
        // the flow SLO band.
        let r = run_day(42);
        assert!(r.transport.mean_batch >= 1.0);
        assert!(
            r.transport.p95_wait_s < 600,
            "p95 {}s — the artery is choking again",
            r.transport.p95_wait_s
        );
    }
}
