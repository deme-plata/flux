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

/// Bars the `RelieveVault` actuator ships off-site each day it fires. One day of
/// intake is +3 and one settlement is −1 (net +2), so a single relief buys ~256
/// days of headroom — enough that the vault never lives permanently at the cap.
const RELIEVE_BARS: usize = 512;

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

/// What one played day produced, before it is folded into a report.
#[derive(Debug, Clone, Default)]
pub struct DayOutcome {
    pub payroll_paid: u32,
    pub payroll_failed: u32,
    pub decisions: Vec<CortexDecision>,
    /// Gold intake the vault REFUSED (full). Impossible on a fresh building;
    /// the whole point of a lifetime run is to find the day it stops being.
    pub vault_refused: u32,
    /// Settlement withdrawals that found no bar (the intake it belonged to
    /// was refused).
    pub withdraw_refused: u32,
    /// Custody revenue the RefillTreasury ACTUATOR accrued into the treasury today
    /// (uQUG). The cortex has always DECIDED `RefillTreasury`; until this field
    /// existed nothing acted on it — the drain to insolvency was a decision with no
    /// hand attached.
    pub treasury_refilled_uqug: u128,
    /// Times the RefillTreasury actuator fired today (≤1 — billed once per day).
    pub refill_actions: u32,
    /// Bars the RelieveVault actuator shipped off-site today to reclaim capacity.
    pub bars_offloaded: u32,
    /// Times the RelieveVault actuator fired today (≤1).
    pub relieve_actions: u32,
}

/// Per-tick observer. `run_day` passes [`NoSink`]; a lifetime run streams
/// telemetry through it. Every hook has a no-op default.
pub trait TickSink {
    fn on_tick(&mut self, _b: &Building, _tick: u64) {}
    fn on_hour(&mut self, _b: &Building, _tick: u64, _senses: &SubsystemSenses, _decisions: &[CortexDecision]) {}
}
pub struct NoSink;
impl TickSink for NoSink {}

/// How the hourly cortex sense `vault_witnessed` is obtained. A fresh building
/// re-verifies its whole (tiny) log and every anchor each hour — [`FullCheck`].
/// A building carried across 256 years cannot (that is O(n²)); `crate::life`
/// supplies an incremental check instead. The policy is explicit so the
/// deviation is visible, not buried.
pub trait VaultCheck {
    fn witnessed(&mut self, b: &Building) -> bool;
}
pub struct FullCheck;
impl VaultCheck for FullCheck {
    fn witnessed(&mut self, b: &Building) -> bool {
        b.vault.verify_chain().is_ok() && b.vault.verify_witnessed().is_ok()
    }
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
    let mut tasks_done: std::collections::BTreeMap<String, u64> = Default::default();
    let out = play_day(&mut b, seed, 0, &mut tasks_done, &mut NoSink, &mut FullCheck);
    assert_eq!(out.vault_refused, 0, "intake fits");
    assert_eq!(out.withdraw_refused, 0, "bar exists");
    let witnessed = b.vault.verify_chain().is_ok() && b.vault.verify_witnessed().is_ok();
    assemble_report(&b, seed, &out, hardened, witnessed)
}

/// THE day script, played on `b` starting at absolute tick `day0` (a multiple
/// of [`TICKS_PER_DAY`]). Every schedule test is on the LOCAL tick `t`, so a
/// building that lives many days runs the identical day each day; every call
/// into a subsystem carries the ABSOLUTE tick, so routes, anchors and talks
/// line up on one timeline. With `day0 == 0` on a fresh building this is
/// byte-for-byte the v0.2 `run_day` — the fingerprint tests pin that.
///
/// `tasks_done` is the robots' completed-task ledger (their service collateral);
/// `run_day` hands in an empty one, a lifetime run carries it.
pub fn play_day(
    b: &mut Building,
    seed: u64,
    day0: u64,
    tasks_done: &mut std::collections::BTreeMap<String, u64>,
    sink: &mut impl TickSink,
    check: &mut impl VaultCheck,
) -> DayOutcome {
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
            start_tick: day0 + 13 * H,
            duration_ticks: 90 * 60,
            expected_attendance: 900,
        })
        .expect("the day's one talk cannot conflict");

    let mut out = DayOutcome::default();
    // The two actuators bill / offload at most once per day, however many hours the
    // cortex keeps asking. A fresh day-0 building never trips either (treasury full,
    // vault near-empty), so `run_day` stays byte-for-byte what it was.
    let mut refilled_today = false;
    let mut relieved_today = false;

    for t in 0..TICKS_PER_DAY {
        let tick = day0 + t;
        let hour = t / H;

        // 06:00–08:00 robot surge: bays → office floors, one call / 3 min.
        if (6..8).contains(&hour) && t % 180 == 0 {
            let to = office_floors[rng.gen_range(0..office_floors.len())];
            b.transport.request(HallCall { from: -1, to, kind: CarKind::RobotFreight, requested_tick: tick });
        }
        // 09:00–10:00 humans arrive through the lobby, one call / 10 min.
        if hour == 9 && t % 600 == 0 {
            let to = office_floors[rng.gen_range(0..office_floors.len())];
            b.transport.request(HallCall { from: 0, to, kind: CarKind::HumanCab, requested_tick: tick });
        }
        // All-day trickle of robot logistics, one call / 17 min.
        if (8..18).contains(&hour) && t % 1_020 == 0 {
            let from = office_floors[rng.gen_range(0..office_floors.len())];
            let to = office_floors[rng.gen_range(0..office_floors.len())];
            b.transport.request(HallCall { from, to, kind: CarKind::RobotFreight, requested_tick: tick });
        }

        // 10:00 gold intake; 12:00 ANCHOR PULSE; 14:00 settlement + audit;
        // 20:00 second anchor pulse.
        if t == 10 * H {
            for i in 0..3 {
                let bar = GoldBar {
                    serial: format!("AU-{seed:04x}-{i:03}"),
                    weight_g: 12_400,
                    fineness_ppm: 999_900,
                };
                if b.vault.deposit(bar, "quillon-bank", Clearance::Officer, tick).is_err() {
                    out.vault_refused += 1;
                }
            }
            let _ = b.bank.pay(TREASURY, "gold-client-escrow", 1, "custody intake marker");
        }
        if t == 12 * H || t == 20 * H {
            // The beacon's stronger pulse: custody history frozen externally.
            b.vault.anchor(tick);
        }
        if t == 14 * H {
            let serial = format!("AU-{seed:04x}-000");
            if b.vault.withdraw(&serial, "custodian-quorum", Clearance::Custodian, tick).is_err() {
                out.withdraw_refused += 1;
            }
            b.vault.run_audit("resident-auditor", tick);
        }

        // Talks conclude when their window passes; attendance is seeded-random.
        if t == 15 * H {
            let attendance = rng.gen_range(700..=1_200);
            b.auditorium.hold_due_talks(tick, |_| attendance);
        }

        // 17:00 payroll; afterwards robots stake their EARNED service history.
        if t == 17 * H {
            let report = b.bank.run_payroll(&b.workforce);
            out.payroll_paid = report.paid_workers;
            out.payroll_failed = report.failures;
            for w in b.workforce.workers.clone() {
                let done = tasks_done.get(&w.id).copied().unwrap_or(0);
                b.workforce.record_service_collateral(&w.id, done);
            }
        }

        // Robots finish tasks through the working day; a rare fault occurs.
        if (8..18).contains(&hour) && t % 660 == 0 {
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
        if t % H == H - 1 {
            let m = b.transport.metrics();
            let senses = SubsystemSenses {
                p95_wait_s: m.p95_wait_s,
                pending_calls: m.pending,
                vault_witnessed: check.witnessed(b),
                payroll_failures: out.payroll_failed,
                treasury_uqug: b.bank.balance(TREASURY),
                utilization_index: b.auditorium.wisdom_index(),
                vault_fill_ratio: b.vault.fill_ratio(),
            };
            let decisions = b.cortex.tick(&senses);
            // ── ACTUATORS: the decisions now DO something ──────────────────────
            // Before this, `decisions` was recorded and dropped, so `RefillTreasury`
            // fired every hour from day 95 for 256 years and changed nothing. Each
            // actuator is gated to once per day so 24 hourly repeats bill/offload once.
            for d in &decisions {
                match d {
                    // Only bill when there are bars to bill — an empty vault earns no
                    // custody revenue — so the one daily draw isn't wasted on the low-
                    // treasury hours before the 10:00 intake lands.
                    CortexDecision::RefillTreasury if !refilled_today && b.vault.bar_count() > 0 => {
                        // Bill external custody clients for every bar under storage and
                        // credit the treasury before the next payroll. Early on (few
                        // bars) this cannot yet cover payroll — the building still dips
                        // insolvent ~day 96 — but as bars accumulate the revenue crosses
                        // payroll and the treasury RECOVERS instead of flatlining at 0.
                        let amt = b.bank.accrue_custody_revenue(b.vault.bar_count());
                        out.treasury_refilled_uqug += amt;
                        out.refill_actions += 1;
                        refilled_today = true;
                    }
                    CortexDecision::RelieveVault if !relieved_today => {
                        // Ship a chunk of the oldest bars off-site so intake keeps working
                        // instead of the vault refusing everything for its remaining life.
                        let n = b.vault.offload(RELIEVE_BARS, "custody-offsite", tick);
                        out.bars_offloaded += n as u32;
                        out.relieve_actions += 1;
                        relieved_today = true;
                    }
                    _ => {}
                }
            }
            sink.on_hour(b, tick, &senses, &decisions);
            out.decisions.extend(decisions);
        }

        sink.on_tick(b, tick);
    }
    out
}

/// Fold a played day into its fingerprinted report. `witnessed` is supplied by
/// the caller because how it is obtained is a policy (see [`VaultCheck`]).
pub fn assemble_report(b: &Building, seed: u64, out: &DayOutcome, hardened: bool, witnessed: bool) -> DayReport {
    let transport = b.transport.metrics();
    let operating = culture::score(&OperatingInputs {
        p95_wait_s: transport.p95_wait_s,
        payroll_paid: out.payroll_paid,
        payroll_failed: out.payroll_failed,
        utilization_index: b.auditorium.wisdom_index(),
        robot_share: transport.robot_share,
        custody_witnessed: witnessed,
    });
    let commitment = state_root::tower_state(b);

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
        payroll_paid: out.payroll_paid,
        payroll_failed: out.payroll_failed,
        talks_held: b.auditorium.talks_held(),
        operating,
        emsec,
        emsec_hardened_total,
        cortex_rounds: b.cortex.rounds(),
        decisions: out.decisions.clone(),
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
