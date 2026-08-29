//! A deterministic day in the tower.
//!
//! One tick = one minute; a day = 1,440 ticks. Same seed → the identical day,
//! down to the report's BLAKE3 hash — which is the whole trick: an
//! entrepreneur (or a regression test) can replay any scenario bit-for-bit.
//! Randomness comes from a seeded ChaCha8 stream, the same discipline
//! `flux-chronos` uses for its network universes.
//!
//! The day's script: robots surge up from the bays at 06:00, humans arrive
//! 09:00, gold moves mid-morning, a big thinker speaks at 13:00, payroll runs
//! 17:00, and the cortex takes a reading every hour.

use crate::bank::TREASURY;
use crate::cortex::{CortexDecision, SubsystemSenses};
use crate::culture::{self, CultureInputs, CultureReport};
use crate::elevator::{CarKind, HallCall, TransportMetrics};
use crate::auditorium::Talk;
use crate::tower::Zone;
use crate::vault::{Clearance, GoldBar};
use crate::Building;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

pub const TICKS_PER_DAY: u64 = 1_440;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayReport {
    pub tower: String,
    pub seed: u64,
    pub ticks: u64,
    pub transport: TransportMetrics,
    pub vault_bars: usize,
    pub vault_head: String,
    pub vault_chain_intact: bool,
    pub treasury_uqug: u128,
    pub payroll_paid: u32,
    pub payroll_failed: u32,
    pub talks_held: usize,
    pub culture: CultureReport,
    pub cortex_rounds: u64,
    pub decisions: Vec<CortexDecision>,
    /// BLAKE3 over the deterministic fields above — the day's fingerprint.
    pub report_hash: String,
}

fn fingerprint(r: &DayReport) -> String {
    let mut h = blake3::Hasher::new();
    h.update(r.tower.as_bytes());
    h.update(&r.seed.to_le_bytes());
    h.update(&r.ticks.to_le_bytes());
    h.update(&r.transport.served.to_le_bytes());
    h.update(&r.transport.p95_wait_ticks.to_le_bytes());
    h.update(&(r.transport.avg_wait_ticks * 1e6).round().to_bits().to_le_bytes());
    h.update(&(r.vault_bars as u64).to_le_bytes());
    h.update(r.vault_head.as_bytes());
    h.update(&r.treasury_uqug.to_le_bytes());
    h.update(&r.payroll_paid.to_le_bytes());
    h.update(&(r.talks_held as u64).to_le_bytes());
    h.update(&(r.culture.total * 1e6).round().to_bits().to_le_bytes());
    h.update(serde_json::to_string(&r.decisions).unwrap().as_bytes());
    hex::encode(&h.finalize().as_bytes()[..16])
}

/// Run one full day and return the fingerprinted report.
pub fn run_day(seed: u64) -> DayReport {
    let mut b = Building::quillon_default().expect("canonical blueprint validates");
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    // Destinations come from the blueprint itself — if the tower changes shape,
    // the day's traffic follows.
    let office_floors: Vec<i32> = b.spec.levels_of(Zone::Offices);

    // 13:00 wisdom session, booked before the day starts.
    b.auditorium
        .schedule(Talk {
            id: "wisdom-1300".into(),
            speaker: "a visiting big thinker".into(),
            title: "Food for Thought".into(),
            start_tick: 13 * 60,
            duration_ticks: 90,
            expected_attendance: 900,
        })
        .expect("the day's one talk cannot conflict");

    let mut payroll_paid = 0u32;
    let mut payroll_failed = 0u32;
    let mut all_decisions: Vec<CortexDecision> = Vec::new();

    for tick in 0..TICKS_PER_DAY {
        let hour = tick / 60;

        // 06:00–08:00 robot surge: bays → office floors.
        if (6..8).contains(&hour) && tick % 3 == 0 {
            let to = office_floors[rng.gen_range(0..office_floors.len())];
            b.transport.request(HallCall { from: -1, to, kind: CarKind::RobotFreight, requested_tick: tick });
        }
        // 09:00–10:00 humans arrive through the lobby.
        if hour == 9 && tick % 10 == 0 {
            let to = office_floors[rng.gen_range(0..office_floors.len())];
            b.transport.request(HallCall { from: 0, to, kind: CarKind::HumanCab, requested_tick: tick });
        }
        // All-day trickle of robot logistics.
        if hour >= 8 && hour < 18 && tick % 17 == 0 {
            let from = office_floors[rng.gen_range(0..office_floors.len())];
            let to = office_floors[rng.gen_range(0..office_floors.len())];
            b.transport.request(HallCall { from, to, kind: CarKind::RobotFreight, requested_tick: tick });
        }

        // 10:00 gold intake; 14:00 an outbound settlement + audit.
        if tick == 10 * 60 {
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
        if tick == 14 * 60 {
            let serial = format!("AU-{seed:04x}-000");
            b.vault.withdraw(&serial, "custodian-quorum", Clearance::Custodian, tick).expect("bar exists");
            b.vault.run_audit("resident-auditor", tick);
        }

        // Talks conclude when their window passes; attendance is seeded-random.
        if tick == 15 * 60 {
            let attendance = rng.gen_range(700..=1_200);
            b.auditorium.hold_due_talks(tick, |_| attendance);
        }

        // 17:00 payroll.
        if tick == 17 * 60 {
            let report = b.bank.run_payroll(&b.workforce);
            payroll_paid = report.paid_workers;
            payroll_failed = report.failures;
            // Wages become SAP stake — skin in the building.
            for w in b.workforce.workers.clone() {
                let bal = b.bank.balance(&w.wallet);
                b.workforce.record_stake(&w.id, bal);
            }
        }

        // Robots finish tasks through the working day; a rare fault occurs.
        if hour >= 8 && hour < 18 && tick % 11 == 0 {
            let w = &b.workforce.workers[rng.gen_range(0..b.workforce.workers.len())];
            let id = w.id.clone();
            let duration = rng.gen_range(20..180);
            b.workforce.record_task_done(&id, duration);
            if rng.gen_ratio(1, 200) {
                b.workforce.record_fault(&id);
            }
        }

        b.transport.step(tick);

        // The cortex takes a reading on the hour.
        if tick % 60 == 59 {
            let m = b.transport.metrics();
            let senses = SubsystemSenses {
                p95_wait_ticks: m.p95_wait_ticks,
                pending_calls: m.pending,
                vault_chain_intact: b.vault.verify_chain().is_ok(),
                payroll_failures: payroll_failed,
                treasury_uqug: b.bank.balance(TREASURY),
                wisdom_index: b.auditorium.wisdom_index(),
            };
            let mut decisions = b.cortex.tick(&senses);
            all_decisions.append(&mut decisions);
        }
    }

    let transport = b.transport.metrics();
    let chain_intact = b.vault.verify_chain().is_ok();
    let culture = culture::score(&CultureInputs {
        p95_wait_ticks: transport.p95_wait_ticks,
        payroll_paid,
        payroll_failed,
        wisdom_index: b.auditorium.wisdom_index(),
        robot_share: transport.robot_share,
        vault_chain_intact: chain_intact,
    });

    let mut report = DayReport {
        tower: b.spec.name.clone(),
        seed,
        ticks: TICKS_PER_DAY,
        transport,
        vault_bars: b.vault.bar_count(),
        vault_head: b.vault.head_hex(),
        vault_chain_intact: chain_intact,
        treasury_uqug: b.bank.balance(TREASURY),
        payroll_paid,
        payroll_failed,
        talks_held: b.auditorium.talks_held(),
        culture,
        cortex_rounds: b.cortex.rounds(),
        decisions: all_decisions,
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
        assert!(r.transport.served > 50, "served={}", r.transport.served);
        assert_eq!(r.vault_bars, 2); // 3 in, 1 out
        assert!(r.vault_chain_intact);
        assert_eq!(r.payroll_paid, 72);
        assert_eq!(r.talks_held, 1);
        assert_eq!(r.cortex_rounds, 24);
        assert!(r.culture.total > 0.5, "culture={}", r.culture.total);
    }
}
