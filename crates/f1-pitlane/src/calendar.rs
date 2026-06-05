//! The real 2026 FIA Formula 1 World Championship calendar — 24 rounds,
//! 6 of them Sprint weekends. Dates are the published 2026 schedule.
//!
//! Each round also carries a `base_laptime` (seconds) used by the session
//! model: a representative fast-lap time for that circuit, so Monaco feels
//! tight-and-slow and Monza feels flat-out.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrandPrix {
    pub round: u8,
    pub name: String,
    pub country: String,
    /// (month, day) of race day in 2026.
    pub race_date: (u8, u8),
    pub sprint: bool,
    /// Representative qualifying lap, seconds. Lower = faster circuit.
    pub base_laptime: f64,
    /// Race distance in laps.
    pub laps: u32,
}

/// The full 2026 season in order.
pub fn season_2026() -> Vec<GrandPrix> {
    fn gp(round: u8, name: &str, country: &str, m: u8, d: u8, sprint: bool, base: f64, laps: u32) -> GrandPrix {
        GrandPrix { round, name: name.to_string(), country: country.to_string(), race_date: (m, d), sprint, base_laptime: base, laps }
    }
    vec![
        gp(1, "Australian Grand Prix", "Australia", 3, 8, false, 77.5, 58),
        gp(2, "Chinese Grand Prix", "China", 3, 15, true, 92.0, 56),
        gp(3, "Japanese Grand Prix", "Japan", 3, 29, false, 89.0, 53),
        gp(4, "Bahrain Grand Prix", "Bahrain", 4, 12, false, 90.5, 57),
        gp(5, "Saudi Arabian Grand Prix", "Saudi Arabia", 4, 19, false, 88.0, 50),
        gp(6, "Miami Grand Prix", "United States", 5, 3, true, 89.5, 57),
        gp(7, "Canadian Grand Prix", "Canada", 5, 24, true, 73.0, 70),
        gp(8, "Monaco Grand Prix", "Monaco", 6, 7, false, 71.5, 78),
        gp(9, "Spanish Grand Prix", "Spain", 6, 14, false, 78.0, 66),
        gp(10, "Austrian Grand Prix", "Austria", 6, 28, false, 64.5, 71),
        gp(11, "British Grand Prix", "Great Britain", 7, 5, true, 87.0, 52),
        gp(12, "Belgian Grand Prix", "Belgium", 7, 12, false, 105.0, 44),
        gp(13, "Hungarian Grand Prix", "Hungary", 7, 26, false, 76.0, 70),
        gp(14, "Dutch Grand Prix", "Netherlands", 8, 23, true, 70.0, 72),
        gp(15, "Italian Grand Prix", "Italy", 9, 6, false, 81.0, 53),
        gp(16, "Madrid Grand Prix", "Spain", 9, 13, false, 82.0, 57),
        gp(17, "Azerbaijan Grand Prix", "Azerbaijan", 9, 26, false, 102.0, 51),
        gp(18, "Singapore Grand Prix", "Singapore", 10, 11, true, 95.0, 62),
        gp(19, "United States Grand Prix", "United States", 10, 25, false, 95.5, 56),
        gp(20, "Mexico City Grand Prix", "Mexico", 11, 1, false, 77.5, 71),
        gp(21, "São Paulo Grand Prix", "Brazil", 11, 8, false, 70.0, 71),
        gp(22, "Las Vegas Grand Prix", "United States", 11, 21, false, 94.0, 50),
        gp(23, "Qatar Grand Prix", "Qatar", 11, 29, false, 83.0, 57),
        gp(24, "Abu Dhabi Grand Prix", "United Arab Emirates", 12, 6, false, 86.0, 58),
    ]
}

/// The next race on or after the given (month, day) in 2026 — the live "next GP".
pub fn next_gp(month: u8, day: u8) -> Option<GrandPrix> {
    let key = (month as u16) * 100 + day as u16;
    season_2026()
        .into_iter()
        .find(|g| (g.race_date.0 as u16) * 100 + g.race_date.1 as u16 >= key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn season_has_24_rounds_and_6_sprints() {
        let s = season_2026();
        assert_eq!(s.len(), 24);
        assert_eq!(s.iter().filter(|g| g.sprint).count(), 6);
        // rounds are sequential 1..=24
        for (i, g) in s.iter().enumerate() {
            assert_eq!(g.round as usize, i + 1);
        }
    }

    #[test]
    fn next_race_from_june_3_is_monaco() {
        let g = next_gp(6, 3).expect("a next race exists in June");
        assert_eq!(g.name, "Monaco Grand Prix");
        assert_eq!(g.round, 8);
    }

    #[test]
    fn after_finale_there_is_no_next_race() {
        assert!(next_gp(12, 20).is_none());
    }
}
