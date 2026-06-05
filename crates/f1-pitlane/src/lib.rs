//! # F1-Pitlane — become the driver
//!
//! A Flux-native F1 career game. The loop:
//!
//! 1. **Garage** — fix the tyres and perfect every car part ([`garage`]) …
//! 2. **…within the real 2026 regulations** ([`regs`]) — push to the ceiling,
//!    never past it, or the scrutineers disqualify you.
//! 3. **Race the real 2026 calendar** ([`calendar`], [`session`]) — qualify,
//!    sprint, race; the faster (and legal) the car, the better the result.
//! 4. **Build a career** ([`career`]) — bank championship points, climb from
//!    Rookie to World Champion.
//!
//! Every rule here is real, tested Rust. The browser game (`f1-pitlane.html`)
//! mirrors this model and can be fed a [`Snapshot`] JSON to stay in sync.

pub mod calendar;
pub mod circuits;
pub mod career;
pub mod drivers;
pub mod garage;
pub mod incident;
pub mod multiplayer;
pub mod physics;
pub mod powertrain;
pub mod race;
pub mod regs;
pub mod session;
pub mod track;
pub mod tyres;
pub mod util;
pub mod weather;

pub use calendar::{next_gp, season_2026, GrandPrix};
pub use career::{Career, Driver, WeekendReport};
pub use drivers::{
    ai_siblings, random_sigil_humans, simulate_field, sigil_human, starting_grid, DriverKind,
    FieldResult, GridDriver,
};
pub use multiplayer::{Lobby, RaceMsg, RACE_TOPIC};
pub use garage::{Car, Compound, Part, PartKind};
pub use incident::{
    default_reaction, resolve_reaction, roll_corner, CornerEvent, Flag, Incident, Reaction,
};
pub use physics::{quali_lap, simulate_lap, LapResult};
pub use powertrain::{AeroMode, EnergyState, LapEnergy, PowerUnit};
pub use race::{simulate_race, simulate_race_auto, RaceOutcome, Temperament};
pub use regs::{Regulations, Violation};
pub use session::{lap_time, run_session, SessionKind, SessionResult};
pub use track::{catalog, flux_ring, generate_custom, monaco, silverstone, Corner, CornerKind, RunOff, Track};
pub use tyres::TyreState;
pub use weather::Weather;

use serde::{Deserialize, Serialize};

/// A complete, serialisable snapshot of a game session — the bridge the browser
/// game reads so the on-screen garage exactly matches this engine's truth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub regs: Regulations,
    pub car: Car,
    pub career: Career,
    pub next_gp: Option<GrandPrix>,
    pub performance: f64,
    pub readiness: f64,
    pub legal: bool,
    pub violations: Vec<Violation>,
}

impl Snapshot {
    pub fn capture(car: &Car, career: &Career, regs: &Regulations) -> Self {
        Snapshot {
            regs: *regs,
            car: car.clone(),
            career: career.clone(),
            next_gp: career.upcoming(),
            performance: car.performance(regs),
            readiness: car.readiness(regs),
            legal: car.is_legal(regs),
            violations: car.scrutineer(regs),
            // (next_gp recomputed above for clarity; upcoming() is cheap)
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("snapshot serialises")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_roundtrips_through_json() {
        let r = Regulations::y2026();
        let mut car = Car::new_2026(&r);
        car.perfect_all(&r);
        let career = Career::starting_at_round("Rocky", 8);
        let snap = Snapshot::capture(&car, &career, &r);
        let json = snap.to_json();
        let back: Snapshot = serde_json::from_str(&json).expect("parse back");
        assert_eq!(snap, back);
        assert!(snap.legal);
        assert_eq!(snap.next_gp.unwrap().name, "Monaco Grand Prix");
    }

    #[test]
    fn end_to_end_perfect_then_win_at_monaco() {
        let r = Regulations::y2026();
        let mut car = Car::new_2026(&r);
        // fix tyres + perfect everything within the rules
        car.perfect_all(&r);
        assert!(car.is_legal(&r));
        let mut career = Career::starting_at_round("Rocky", 8);
        let report = career.race_weekend(&car, &r, 5).unwrap();
        assert_eq!(report.gp, "Monaco Grand Prix");
        // a flawless legal car should be scoring points
        assert!(report.race.points > 0);
    }
}
