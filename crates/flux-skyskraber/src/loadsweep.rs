//! Load sweep — where the tower's transport artery actually breaks.
//!
//! OK, so here's the deal. [`crate::sim`] runs one scripted day and proves it is
//! deterministic. That is worth having, but notice what it can never tell you:
//! a single day at a single arrival rate cannot find a *limit*. It reports that
//! the building coped. It cannot report how much more it would have taken.
//!
//! And there is a stability condition already written down. [`crate::elevator`]
//! states it in prose:
//!
//! ```text
//! rho = lambda * E[T_route] / (N * E[b])   < 1
//! ```
//!
//! — arrivals per second, times how long a route takes, divided by how many cars
//! there are times how many jobs each route carries. If that number is below one
//! the queue drains; above one it grows without bound. Until now nothing in this
//! crate computed it and nothing tested it. It was a claim in a comment.
//!
//! ## What the sweep actually found (measured, seed 9, 4 robot cars, capacity 8)
//!
//! ```text
//! lambda/h  offered  served  pending    E[b]   E[T]s   p95 s  rho_meas  rho_name  clear
//!       30      128     128        0   1.000    73.3      53     0.153     0.019   true
//!       60      263     263        0   1.000    77.8      54     0.324     0.041   true
//!      120      434     434        0   1.009    76.3      98     0.630     0.079   true
//!      240      992     992        0   1.645    93.1     151     0.943     0.194   true
//!      480     1973    1973        0   5.752   170.2     237     0.986     0.709   true
//!      720     2880    2880        0   7.742   198.9    3937     1.284     1.243   true
//!      960     3760    3760        0   7.866   200.4    9086     1.699     1.670   true
//!     1440     5737    4490     1247   7.933   202.5   16728     2.553     2.532  false
//!     2160     8570    4511     4059   7.942   202.0   20195     3.814     3.787  false
//! ```
//!
//! Three things fall out of that table, and the third one is the one worth having.
//!
//! **1. `E[b]` is not a constant, and it moves in the helpful direction.** It runs
//! from 1.000 at a trickle to 7.94 at saturation — essentially the nameplate 8.
//! A car assembles its batch from whatever is already waiting when it goes idle,
//! so at low load it almost always leaves with one job and under pressure it
//! leaves full. The artery gets more efficient exactly when it is being pushed.
//!
//! **2. But `E[T_route]` grows too, and nearly cancels it.** 73 s to 202 s over the
//! same range, because a fuller batch is a longer multi-stop route. This is the
//! part that is easy to miss: batching does not buy throughput for free, it
//! trades trip count against trip length. Net of the two, `rho` still climbs with
//! `lambda` — just more slowly than the naive linear reading suggests.
//!
//! So the nameplate figure is optimistic at every single rung, and worst where
//! people are most tempted to trust it: at `lambda` = 30 it reads 0.019 against a
//! true 0.153, **eight times** too rosy. The two converge only at saturation,
//! where `E[b]` has finally reached the nameplate — i.e. the nameplate number is
//! only accurate in exactly the regime where you no longer need it.
//!
//! **3. `rho` = 1 is NOT where the queue stops draining. It is where the building
//! stops being usable.** This is the result that surprised me and it is the reason
//! the module exists. `rho_measured` crosses 1 at `lambda` = 720, yet the queue
//! still drains at 720 AND at 960; it first fails to clear at 1440. By the
//! textbook reading of the stability condition that looks like the condition
//! being wrong by two rungs.
//!
//! Look at the wait column instead. The p95 goes 237 s at `lambda` = 480
//! (`rho` = 0.986) to **3,937 s at 720** (`rho` = 1.284) — a sixteen-fold jump
//! across the `rho` = 1 boundary, from four minutes to over an hour. The queue at
//! 720 does technically empty, given four idle hours to do it in. Nobody riding
//! it would call that working.
//!
//! What nature is telling us here is that the two questions are simply different.
//! "Does the backlog eventually clear?" is a statement about an infinite horizon
//! and it is generous. "Is the wait tolerable?" is the one an occupant asks, and
//! `rho` < 1 answers *that* one, sharply. Hence [`SweepReport::lambda_service_knee`]
//! beside [`SweepReport::lambda_crit_measured`]: the sweep reports both, and they
//! are not the same number.
//!
//! ## What is proven vs. what is still pretend
//!
//! Proven: the sweep is deterministic (same seed, same ladder, same hash), the
//! two `rho` figures are computed from measurement, and the stability condition
//! is checked against the observed drain rather than asserted.
//!
//! Pretend, on purpose and worth stating: arrivals are Bernoulli-per-second
//! (memoryless, so Poisson in the limit) and homogeneous over the offer window.
//! A real morning peak is neither — it ramps. That makes `lambda_crit` here a
//! *steady-state* capacity, which is the right quantity for sizing shafts and
//! the wrong one for surviving a fire drill. The evacuation envelope in
//! [`crate::physics`] is the model for that, and it deliberately ignores lifts.

use crate::elevator::{CarKind, ElevatorBank, HallCall};
use crate::tower::{TowerSpec, Zone};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

/// How long calls are offered at the target rate, in ticks (= seconds).
pub const OFFER_TICKS: u64 = 4 * 3_600;
/// How long the bank is then left alone to drain before we judge it.
pub const DRAIN_TICKS: u64 = 4 * 3_600;

/// The wait a hall call is allowed to suffer at the 95th percentile before the
/// artery counts as failed for the people using it. Same 600 s band
/// `sim::tests::batching_physics_beat_the_v01_queue` holds the scripted day to.
pub const SERVICE_SLO_S: u64 = 600;

/// The default ladder of arrival rates, in hall calls per hour.
pub const DEFAULT_RATES: &[f64] = &[30.0, 60.0, 120.0, 240.0, 480.0, 720.0, 960.0, 1_440.0, 2_160.0];

/// One rung of the ladder: an arrival rate, and everything the tower did at it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SweepPoint {
    /// The offered load, hall calls per hour.
    pub arrival_per_hour: f64,
    /// Calls actually generated during the offer window.
    pub offered: u64,
    /// Calls that completed a route by the end of the horizon.
    pub served: u64,
    /// Still queued or in flight when the horizon ended. Non-zero means the
    /// building never caught up.
    pub pending_end: u64,
    /// Measured mean realized batch, E[b]. The term everyone assumes.
    pub mean_batch: f64,
    /// Measured mean route time, E[T_route], seconds.
    pub mean_route_s: f64,
    /// 95th-percentile hall-call wait, seconds.
    pub p95_wait_s: u64,
    /// rho computed from the MEASURED E[b] — the honest one.
    pub rho_measured: f64,
    /// rho computed from the car's nameplate capacity instead of the measured
    /// batch. This is what you get for assuming the cars run full.
    pub rho_nameplate: f64,
    /// Did the queue actually drain to zero inside the horizon?
    pub cleared: bool,
}

/// The whole ladder, plus where the tower gave out.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SweepReport {
    pub tower: String,
    pub seed: u64,
    /// Robot freight cars, the `N` in the stability condition.
    pub cars: usize,
    pub points: Vec<SweepPoint>,
    /// Highest offered rate that still drained. `None` if even the lightest did not.
    pub lambda_crit_measured: Option<f64>,
    /// Lowest offered rate at which `rho_measured` crossed 1.
    pub lambda_first_unstable: Option<f64>,
    /// Lowest offered rate whose p95 wait broke [`SERVICE_SLO_S`] — the point the
    /// building stops being usable, which is NOT the point the queue stops draining.
    pub lambda_service_knee: Option<f64>,
    /// BLAKE3 over the deterministic fields — the sweep's fingerprint.
    pub report_hash: String,
}

/// Run one rung. A fresh bank every time, so nothing leaks between rates.
pub fn sweep_point(seed: u64, spec: &TowerSpec, arrival_per_hour: f64) -> SweepPoint {
    let mut bank = ElevatorBank::for_spec(spec);
    let mut rng = ChaCha8Rng::seed_from_u64(seed ^ (arrival_per_hour.to_bits()));
    let offices: Vec<i32> = spec.levels_of(Zone::Offices);
    let p = arrival_per_hour / 3_600.0; // per-second arrival probability
    let mut offered = 0u64;

    for tick in 0..(OFFER_TICKS + DRAIN_TICKS) {
        if tick < OFFER_TICKS && !offices.is_empty() && rng.gen_bool(p.clamp(0.0, 1.0)) {
            let to = offices[rng.gen_range(0..offices.len())];
            bank.request(HallCall {
                from: -1,
                to,
                kind: CarKind::RobotFreight,
                requested_tick: tick,
            });
            offered += 1;
        }
        bank.step(tick);
    }

    let m = bank.metrics();
    let n = spec.robot_shafts.max(1) as f64;
    let lambda_s = arrival_per_hour / 3_600.0;
    // rho is only meaningful once a route has actually happened; a bank that
    // never moved has no measured E[T_route] to divide by.
    let rho_measured = if m.mean_batch > 0.0 && m.mean_route_s > 0.0 {
        lambda_s * m.mean_route_s / (n * m.mean_batch)
    } else {
        0.0
    };
    let rho_nameplate = if m.mean_route_s > 0.0 {
        lambda_s * m.mean_route_s / (n * CarKind::RobotFreight.capacity() as f64)
    } else {
        0.0
    };

    SweepPoint {
        arrival_per_hour,
        offered,
        served: m.served,
        pending_end: m.pending,
        mean_batch: m.mean_batch,
        mean_route_s: m.mean_route_s,
        p95_wait_s: m.p95_wait_s,
        rho_measured,
        rho_nameplate,
        cleared: m.pending == 0,
    }
}

fn fingerprint(r: &SweepReport) -> String {
    let mut h = blake3::Hasher::new();
    h.update(r.tower.as_bytes());
    h.update(&r.seed.to_le_bytes());
    h.update(&(r.cars as u64).to_le_bytes());
    for p in &r.points {
        h.update(&p.arrival_per_hour.to_bits().to_le_bytes());
        h.update(&p.offered.to_le_bytes());
        h.update(&p.served.to_le_bytes());
        h.update(&p.pending_end.to_le_bytes());
        h.update(&(p.mean_batch * 1e6).round().to_bits().to_le_bytes());
        h.update(&(p.mean_route_s * 1e6).round().to_bits().to_le_bytes());
        h.update(&p.p95_wait_s.to_le_bytes());
        h.update(&[p.cleared as u8]);
    }
    hex::encode(&h.finalize().as_bytes()[..16])
}

/// Sweep the canonical tower across a ladder of arrival rates.
pub fn sweep(seed: u64, spec: &TowerSpec, rates: &[f64]) -> SweepReport {
    let points: Vec<SweepPoint> = rates.iter().map(|&r| sweep_point(seed, spec, r)).collect();
    let lambda_crit_measured = points
        .iter()
        .filter(|p| p.cleared)
        .map(|p| p.arrival_per_hour)
        .fold(None::<f64>, |acc, r| Some(acc.map_or(r, |a: f64| a.max(r))));
    let lambda_first_unstable = points
        .iter()
        .find(|p| p.rho_measured >= 1.0)
        .map(|p| p.arrival_per_hour);
    let lambda_service_knee = points
        .iter()
        .find(|p| p.p95_wait_s > SERVICE_SLO_S)
        .map(|p| p.arrival_per_hour);
    let mut report = SweepReport {
        tower: spec.name.clone(),
        seed,
        cars: spec.robot_shafts as usize,
        points,
        lambda_crit_measured,
        lambda_first_unstable,
        lambda_service_knee,
        report_hash: String::new(),
    };
    report.report_hash = fingerprint(&report);
    report
}

/// The canonical sweep: the default blueprint over [`DEFAULT_RATES`].
pub fn quillon_sweep(seed: u64) -> SweepReport {
    sweep(seed, &TowerSpec::quillon_default(), DEFAULT_RATES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sweep() {
        assert_eq!(quillon_sweep(9).report_hash, quillon_sweep(9).report_hash);
    }

    #[test]
    fn the_sweep_actually_offered_and_served_traffic() {
        let r = quillon_sweep(9);
        assert_eq!(r.points.len(), DEFAULT_RATES.len());
        for p in &r.points {
            // Bernoulli(lambda/3600) over the offer window: the count must land
            // near lambda * 4 h. Wide band — this is a sanity check on the
            // arrival process, not a test of the RNG.
            let expect = p.arrival_per_hour * (OFFER_TICKS as f64 / 3_600.0);
            assert!(
                (p.offered as f64) > expect * 0.8 && (p.offered as f64) < expect * 1.2,
                "lambda={} offered={} expected~{expect}",
                p.arrival_per_hour,
                p.offered
            );
            assert!(p.served > 0, "nothing moved at lambda={}", p.arrival_per_hour);
        }
    }

    #[test]
    fn batching_is_a_function_of_load_not_a_constant() {
        let r = quillon_sweep(9);
        let first = r.points.first().unwrap();
        let last = r.points.last().unwrap();
        let cap = CarKind::RobotFreight.capacity() as f64;
        // A trickle rides alone…
        assert!(first.mean_batch < 1.05, "E[b] at the lightest load = {}", first.mean_batch);
        // …and saturation rides essentially full.
        assert!(last.mean_batch > cap * 0.95, "E[b] at saturation = {} (cap {cap})", last.mean_batch);
        // Never decreasing along the ladder: more offered load can only give a
        // departing car more to choose from.
        for w in r.points.windows(2) {
            assert!(
                w[1].mean_batch >= w[0].mean_batch - 1e-9,
                "E[b] fell from {} to {} between lambda {} and {}",
                w[0].mean_batch, w[1].mean_batch, w[0].arrival_per_hour, w[1].arrival_per_hour
            );
        }
    }

    #[test]
    fn fuller_batches_cost_longer_routes() {
        // The half of the trade-off that is easy to forget: batching buys trips
        // but pays in stops. Not asserted as monotone — it dips slightly twice —
        // only that the end of the ladder is decisively slower than the start.
        let r = quillon_sweep(9);
        let first = r.points.first().unwrap().mean_route_s;
        let last = r.points.last().unwrap().mean_route_s;
        assert!(last > first * 2.0, "E[T_route] {first} -> {last}: the stop cost never showed up");
    }

    #[test]
    fn the_nameplate_rho_is_optimistic_at_every_single_rate() {
        let r = quillon_sweep(9);
        for p in &r.points {
            assert!(
                p.rho_nameplate < p.rho_measured,
                "nameplate rho {} >= measured {} at lambda={}",
                p.rho_nameplate, p.rho_measured, p.arrival_per_hour
            );
        }
        // Worst exactly where it is most tempting to trust — the quiet end.
        let q = r.points.first().unwrap();
        assert!(
            q.rho_measured / q.rho_nameplate > 5.0,
            "at lambda={} the nameplate was only {}x optimistic",
            q.arrival_per_hour,
            q.rho_measured / q.rho_nameplate
        );
        // …and only honest once E[b] has reached the nameplate, i.e. at saturation.
        let s = r.points.last().unwrap();
        assert!(
            (s.rho_measured - s.rho_nameplate).abs() / s.rho_measured < 0.02,
            "the two rhos should converge at saturation: {} vs {}",
            s.rho_measured, s.rho_nameplate
        );
    }

    #[test]
    fn rho_one_marks_the_service_knee_not_the_drain_limit() {
        // THE result. rho >= 1 does not predict "the backlog never clears" — it
        // predicts "the wait becomes intolerable", and those are two rungs apart.
        let r = quillon_sweep(9);
        let unstable = r.lambda_first_unstable.expect("the ladder must reach rho >= 1");
        let knee = r.lambda_service_knee.expect("the ladder must break the wait SLO");
        let drain = r.lambda_crit_measured.expect("the light end must drain");
        assert_eq!(
            unstable, knee,
            "rho crossed 1 at {unstable}/h but the wait SLO broke at {knee}/h"
        );
        assert!(
            drain > unstable,
            "the queue should still drain past rho=1 (drained to {drain}/h, rho crossed at {unstable}/h)"
        );
        // And the wait does not creep across that boundary, it jumps.
        let below = r.points.iter().rev().find(|p| p.rho_measured < 1.0).unwrap();
        let above = r.points.iter().find(|p| p.rho_measured >= 1.0).unwrap();
        assert!(
            above.p95_wait_s > below.p95_wait_s * 10,
            "p95 {} -> {} across rho=1 — expected a cliff, got a slope",
            below.p95_wait_s, above.p95_wait_s
        );
    }

    /// Not an assertion — the measurement itself, printed. Run with:
    /// `--nocapture` to read the table this module exists to produce.
    #[test]
    fn print_the_sweep_table() {
        let r = quillon_sweep(9);
        println!("\n{} — {} robot cars, capacity {}", r.tower, r.cars, CarKind::RobotFreight.capacity());
        println!("{:>8} {:>7} {:>7} {:>8} {:>8} {:>9} {:>8} {:>8} {:>8} {:>7}",
                 "lambda/h","offered","served","pending","E[b]","E[T]s","p95 s","rho_meas","rho_name","clear");
        for p in &r.points {
            println!("{:>8.0} {:>7} {:>7} {:>8} {:>8.3} {:>9.1} {:>8} {:>8.3} {:>8.3} {:>7}",
                p.arrival_per_hour, p.offered, p.served, p.pending_end,
                p.mean_batch, p.mean_route_s, p.p95_wait_s,
                p.rho_measured, p.rho_nameplate, p.cleared);
        }
        println!("lambda_crit_measured = {:?}   lambda_first_unstable = {:?}",
                 r.lambda_crit_measured, r.lambda_first_unstable);
    }
}
