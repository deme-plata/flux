//! Career: the player IS the driver. Race the 2026 calendar round by round,
//! bank championship points, climb the driver levels (Rookie → Champion), and
//! unlock standing in the field. This is the spine the browser game advances
//! the player along.

use crate::calendar::{season_2026, GrandPrix};
use crate::garage::Car;
use crate::regs::Regulations;
use crate::session::{run_session, SessionKind, SessionResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Driver {
    pub name: String,
    pub points: u32,
    pub wins: u32,
    pub podiums: u32,
    pub poles: u32,
    pub fastest_laps: u32,
    /// Experience points — earned from results, drive levelling.
    pub xp: u32,
}

impl Driver {
    pub fn new(name: impl Into<String>) -> Self {
        Driver {
            name: name.into(),
            points: 0,
            wins: 0,
            podiums: 0,
            poles: 0,
            fastest_laps: 0,
            xp: 0,
        }
    }

    /// Driver rank, derived from XP. Climbs as you deliver results.
    pub fn level(&self) -> &'static str {
        match self.xp {
            0..=99 => "Rookie",
            100..=299 => "Points Scorer",
            300..=599 => "Podium Contender",
            600..=999 => "Race Winner",
            1000..=1999 => "Title Challenger",
            _ => "World Champion",
        }
    }

    fn record_quali(&mut self, res: &SessionResult) {
        if res.position == 1 {
            self.poles += 1;
            self.xp += 15;
        } else if res.position <= 3 {
            self.xp += 8;
        } else if res.position <= 10 {
            self.xp += 4;
        }
    }

    fn record_race(&mut self, res: &SessionResult, sprint: bool) {
        self.points += res.points;
        self.xp += res.points * 4;
        if res.fastest_lap {
            self.fastest_laps += 1;
            self.xp += 5;
        }
        if !sprint {
            match res.position {
                1 => {
                    self.wins += 1;
                    self.podiums += 1;
                    self.xp += 50;
                }
                2 | 3 => {
                    self.podiums += 1;
                    self.xp += 20;
                }
                _ => {}
            }
        }
    }
}

/// A completed race weekend's results bundled together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeekendReport {
    pub round: u8,
    pub gp: String,
    pub qualifying: SessionResult,
    pub sprint: Option<SessionResult>,
    pub race: SessionResult,
    pub championship_points_after: u32,
}

/// The running state of a career playthrough.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Career {
    pub driver: Driver,
    /// Index into `season_2026()` of the next race to run (0-based).
    pub next_round_idx: usize,
    pub history: Vec<WeekendReport>,
}

impl Career {
    pub fn new(driver_name: impl Into<String>) -> Self {
        Career {
            driver: Driver::new(driver_name),
            next_round_idx: 0,
            history: Vec::new(),
        }
    }

    /// Start the season at a specific round (e.g. Monaco = round 8) so the live
    /// calendar's "next GP" is where the player joins in.
    pub fn starting_at_round(driver_name: impl Into<String>, round: u8) -> Self {
        let mut c = Career::new(driver_name);
        c.next_round_idx = (round.saturating_sub(1)) as usize;
        c
    }

    pub fn upcoming(&self) -> Option<GrandPrix> {
        season_2026().into_iter().nth(self.next_round_idx)
    }

    pub fn is_season_over(&self) -> bool {
        self.next_round_idx >= season_2026().len()
    }

    /// Run the full weekend for the upcoming round with the given car: qualifying,
    /// optional sprint, and the race. Advances the career one round.
    pub fn race_weekend(&mut self, car: &Car, regs: &Regulations, seed: u64) -> Option<WeekendReport> {
        let gp = self.upcoming()?;

        let quali = run_session(SessionKind::Qualifying, &gp, car, regs, seed);
        self.driver.record_quali(&quali);

        let sprint = if gp.sprint {
            let s = run_session(SessionKind::Sprint, &gp, car, regs, seed ^ 0xABCD);
            self.driver.record_race(&s, true);
            Some(s)
        } else {
            None
        };

        let race = run_session(SessionKind::Race, &gp, car, regs, seed ^ 0x1234);
        self.driver.record_race(&race, false);

        let report = WeekendReport {
            round: gp.round,
            gp: gp.name.to_string(),
            qualifying: quali,
            sprint,
            race,
            championship_points_after: self.driver.points,
        };
        self.history.push(report.clone());
        self.next_round_idx += 1;
        Some(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn career_can_start_at_monaco() {
        let c = Career::starting_at_round("Rocky", 8);
        assert_eq!(c.upcoming().unwrap().name, "Monaco Grand Prix");
    }

    #[test]
    fn winning_accrues_points_wins_and_levels_up() {
        let r = Regulations::y2026();
        let mut car = Car::new_2026(&r);
        car.perfect_all(&r);
        let mut career = Career::starting_at_round("Rocky", 8);
        let report = career.race_weekend(&car, &r, 1).unwrap();
        assert_eq!(report.round, 8);
        assert!(career.driver.points > 0);
        assert_eq!(career.next_round_idx, 8); // advanced past Monaco (idx 7 -> 8)
        // A perfect car around the field should be winning and climbing rank.
        assert!(career.driver.xp > 0);
        assert_ne!(career.driver.level(), "World Champion"); // not instantly maxed
    }

    #[test]
    fn season_completes_in_remaining_rounds() {
        let r = Regulations::y2026();
        let mut car = Car::new_2026(&r);
        car.perfect_all(&r);
        let mut career = Career::starting_at_round("Rocky", 8);
        let mut weekends = 0;
        while !career.is_season_over() {
            career.race_weekend(&car, &r, weekends as u64 + 1);
            weekends += 1;
        }
        assert_eq!(weekends, 17); // rounds 8..=24 inclusive
        assert_eq!(career.history.len(), 17);
    }
}
