//! Sessions: how a perfected (or not) car turns into a lap time, a grid slot,
//! and a finishing position. Deterministic (seeded) so tests are stable and the
//! browser game can replay a weekend identically from a seed.

use crate::calendar::GrandPrix;
use crate::garage::Car;
use crate::regs::Regulations;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionKind {
    Practice,
    Qualifying,
    Sprint,
    Race,
}

impl SessionKind {
    pub fn label(&self) -> &'static str {
        match self {
            SessionKind::Practice => "Practice",
            SessionKind::Qualifying => "Qualifying",
            SessionKind::Sprint => "Sprint",
            SessionKind::Race => "Race",
        }
    }
}

/// Tiny deterministic PRNG (xorshift) — no external crate, reproducible from a seed.
fn xorshift(state: &mut u64) -> f64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    // map to 0..1
    (x >> 11) as f64 / (1u64 << 53) as f64
}

/// A representative best lap, in seconds, for a car at the given pace on a circuit.
/// Perfect legal car (perf 1.0) ≈ the circuit's base lap; a junk car is ~30% slower.
pub fn lap_time(perf: f64, base_laptime: f64) -> f64 {
    base_laptime * (1.30 - 0.30 * perf.clamp(0.0, 1.10))
}

/// The result of one session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionResult {
    pub kind: SessionKind,
    pub gp: String,
    pub best_lap: f64,
    /// 1 = pole / win. Field size is 20.
    pub position: u8,
    pub legal: bool,
    pub points: u32,
    /// True if the driver set the fastest lap of the race.
    pub fastest_lap: bool,
}

const FIELD: usize = 20;

fn race_points(pos: u8) -> u32 {
    match pos {
        1 => 25, 2 => 18, 3 => 15, 4 => 12, 5 => 10,
        6 => 8, 7 => 6, 8 => 4, 9 => 2, 10 => 1,
        _ => 0,
    }
}

fn sprint_points(pos: u8) -> u32 {
    match pos {
        1 => 8, 2 => 7, 3 => 6, 4 => 5, 5 => 4, 6 => 3, 7 => 2, 8 => 1, _ => 0,
    }
}

/// Run a session for the player's car. `seed` makes rival pace + the weekend
/// reproducible. An illegal car is disqualified (last place, zero points).
pub fn run_session(
    kind: SessionKind,
    gp: &GrandPrix,
    car: &Car,
    regs: &Regulations,
    seed: u64,
) -> SessionResult {
    let legal = car.is_legal(regs);
    let mut rng = seed ^ (gp.round as u64).wrapping_mul(0x9E3779B97F4A7C15);

    let player_perf = car.performance(regs);
    let player_lap = lap_time(player_perf, gp.base_laptime);

    // Build a 19-strong rival field clustered around mid-grid pace (perf ~0.82–0.99).
    let mut rival_laps: Vec<f64> = (0..FIELD - 1)
        .map(|_| {
            let r = xorshift(&mut rng);
            let rival_perf = 0.82 + r * 0.17; // 0.82..=0.99
            lap_time(rival_perf, gp.base_laptime) * (1.0 + (xorshift(&mut rng) - 0.5) * 0.004)
        })
        .collect();

    // Player position = where their lap slots into the field (illegal => back of grid).
    let position = if legal {
        let faster = rival_laps.iter().filter(|&&l| l < player_lap).count();
        (faster + 1) as u8
    } else {
        FIELD as u8
    };

    // Fastest lap: player has the single quickest lap of everyone and finished top 10.
    rival_laps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let fastest_lap = legal
        && matches!(kind, SessionKind::Race | SessionKind::Sprint)
        && player_lap < rival_laps[0]
        && position <= 10;

    let mut points = match kind {
        SessionKind::Race => race_points(position),
        SessionKind::Sprint => sprint_points(position),
        _ => 0,
    };
    if fastest_lap && kind == SessionKind::Race {
        points += 1;
    }
    if !legal {
        points = 0;
    }

    SessionResult {
        kind,
        gp: gp.name.to_string(),
        best_lap: player_lap,
        position,
        legal,
        points,
        fastest_lap,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::season_2026;
    use crate::garage::Car;

    fn monaco() -> GrandPrix {
        season_2026().into_iter().find(|g| g.name == "Monaco Grand Prix").unwrap()
    }

    #[test]
    fn perfect_car_laps_at_or_below_base() {
        let r = Regulations::y2026();
        let mut car = Car::new_2026(&r);
        car.perfect_all(&r);
        let lap = lap_time(car.performance(&r), monaco().base_laptime);
        assert!(lap <= monaco().base_laptime + 0.01, "perfect car should hit base lap, got {lap}");
    }

    #[test]
    fn perfecting_the_car_improves_finishing_position() {
        let r = Regulations::y2026();
        let gp = monaco();
        let weak = Car::new_2026(&r);
        let weak_res = run_session(SessionKind::Race, &gp, &weak, &r, 42);
        let mut strong = Car::new_2026(&r);
        strong.perfect_all(&r);
        let strong_res = run_session(SessionKind::Race, &gp, &strong, &r, 42);
        assert!(strong_res.position < weak_res.position,
            "perfect car ({}) should out-qualify baseline ({})", strong_res.position, weak_res.position);
        assert!(strong_res.points >= weak_res.points);
    }

    #[test]
    fn illegal_car_is_disqualified_to_last() {
        let r = Regulations::y2026();
        let gp = monaco();
        let mut car = Car::new_2026(&r);
        car.perfect_all(&r);
        car.overtune(crate::garage::PartKind::Mguk, 30.0, &r);
        let res = run_session(SessionKind::Race, &gp, &car, &r, 7);
        assert!(!res.legal);
        assert_eq!(res.position, 20);
        assert_eq!(res.points, 0);
    }

    #[test]
    fn same_seed_is_reproducible() {
        let r = Regulations::y2026();
        let gp = monaco();
        let mut car = Car::new_2026(&r);
        car.perfect_all(&r);
        let a = run_session(SessionKind::Race, &gp, &car, &r, 99);
        let b = run_session(SessionKind::Race, &gp, &car, &r, 99);
        assert_eq!(a, b);
    }
}
