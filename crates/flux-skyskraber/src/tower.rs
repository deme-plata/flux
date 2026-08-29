//! The building program — floors, zones, and the invariants an architect
//! would enforce with a red pen. `TowerSpec::validate()` IS that red pen.
//!
//! v0.2 encodes what the concept renders (skyskraper_quillon_graph_pictures)
//! actually specify: the tower rises as ONE body, then splits at the crown
//! into TWO asymmetric spires — the taller carrying the beacon — joined by a
//! garden **sky bridge** at two-thirds height. The facade carries the glowing
//! Quillon Graph shield sigil with a column of light down the central axis,
//! the podium spirals into the plaza, and the site is waterfront.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Above the split level, every floor belongs to one of the twin spires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Spire {
    /// The taller spire — carries the beacon.
    A,
    /// The shorter spire.
    B,
}

/// What a floor is *for*. The zoning of the tower.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Zone {
    /// Basement gold vault. May only exist below ground.
    GoldVault,
    /// Robot logistics: charging, staging, freight interchange.
    RobotBay,
    /// Ground-level lobby. Exactly one, at level 0.
    Lobby,
    /// Quillon Bank — the main organ of the building.
    BankHall,
    /// Office floors for the (mostly robot) workforce.
    Offices,
    /// The grand auditorium — big thinkers come here.
    Auditorium,
    /// Mechanical / life-safety floors.
    Mechanical,
    /// The garden deck spanning the gap between the twin spires. Also a
    /// refuge/transfer floor, so it counts toward the mechanical cadence.
    SkyBridge,
    /// Sky garden + executive commons at the crowns.
    SkyGarden,
}

/// One floor of the tower. Negative levels are basements. `spire: None`
/// means a full floor plate (below the split) or a bridge deck.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Floor {
    pub level: i32,
    pub zone: Zone,
    pub area_m2: u32,
    /// Design occupancy (robots + humans) for transport sizing.
    pub design_occupancy: u32,
    pub spire: Option<Spire>,
}

/// The facade identity from the renders: the shield sigil and the light column.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Facade {
    pub emblem: String,
    /// The glowing spear of light down the central axis.
    pub light_column: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TowerSpec {
    pub name: String,
    pub floors: Vec<Floor>,
    /// Human passenger cab shafts.
    pub human_shafts: u32,
    /// Robot freight shafts (the robot elevator transport system).
    pub robot_shafts: u32,
    pub facade: Facade,
    /// The site sits on the water (river plaza with lagoon pools).
    pub waterfront: bool,
}

#[derive(Debug, Error, PartialEq)]
pub enum TowerError {
    #[error("gold vault on level {0} — the vault must be below ground")]
    VaultAboveGround(i32),
    #[error("lobby must exist exactly once at level 0 (found {0})")]
    LobbyWrong(usize),
    #[error("duplicate floor: level {0} spire {1:?}")]
    DuplicateFloor(i32, Option<Spire>),
    #[error("no robot shafts — a robot workforce cannot move")]
    NoRobotShafts,
    #[error("auditorium levels are not contiguous")]
    AuditoriumSplit,
    #[error("more than {max} floors between mechanical/refuge levels (gap of {gap})")]
    MechanicalGap { gap: i32, max: i32 },
    #[error("floor {0} above the spire split must belong to a spire")]
    UnassignedAboveSplit(i32),
    #[error("spire floor at level {0} below another full floor plate")]
    SpireBelowPlate(i32),
    #[error("twin spires exist but no sky bridge joins them")]
    NoSkyBridge,
    #[error("sky bridge at level {0} outside the twin-spire overlap {1}..={2}")]
    BridgeOutsideOverlap(i32, i32, i32),
    #[error("spires must rise from the same split level (A from {0}, B from {1})")]
    SplitMismatch(i32, i32),
}

impl TowerSpec {
    /// The canonical blueprint, matched to the concept renders.
    ///
    /// B4–B3 gold vault · B2–B1 robot bays · 0 lobby · 1–5 Quillon Bank
    /// · 6–8 grand auditorium · 9–45 offices (mechanical 20, 40) — then the
    /// body splits: spire A 46–88 (offices, refuge 75, sky garden 76–88 with
    /// the beacon), spire B 46–78 (offices, sky garden 74–78), joined by the
    /// garden sky bridge decks at 60–61.
    pub fn quillon_default() -> Self {
        let mut floors = Vec::new();
        let plate = |level, zone, area, occ| Floor { level, zone, area_m2: area, design_occupancy: occ, spire: None };
        let sp = |level, zone, area, occ, s| Floor { level, zone, area_m2: area, design_occupancy: occ, spire: Some(s) };

        for level in [-4, -3] {
            floors.push(plate(level, Zone::GoldVault, 2_400, 6));
        }
        for level in [-2, -1] {
            floors.push(plate(level, Zone::RobotBay, 2_400, 120));
        }
        floors.push(plate(0, Zone::Lobby, 2_000, 200));
        for level in 1..=5 {
            floors.push(plate(level, Zone::BankHall, 1_800, 90));
        }
        for level in 6..=8 {
            floors.push(plate(level, Zone::Auditorium, 1_800, 400));
        }
        for level in 9..=45 {
            let zone = if level % 20 == 0 { Zone::Mechanical } else { Zone::Offices };
            let occ = if zone == Zone::Mechanical { 4 } else { 60 };
            floors.push(plate(level, zone, 1_500, occ));
        }
        // Spire A — the taller, beacon-bearing spire.
        for level in 46..=74 {
            floors.push(sp(level, Zone::Offices, 900, 36, Spire::A));
        }
        floors.push(sp(75, Zone::Mechanical, 900, 2, Spire::A));
        for level in 76..=88 {
            floors.push(sp(level, Zone::SkyGarden, 700, 24, Spire::A));
        }
        // Spire B — the shorter twin.
        for level in 46..=73 {
            floors.push(sp(level, Zone::Offices, 900, 36, Spire::B));
        }
        for level in 74..=78 {
            floors.push(sp(level, Zone::SkyGarden, 700, 24, Spire::B));
        }
        // The garden sky bridge spanning the gap, at two-thirds height.
        for level in [60, 61] {
            floors.push(plate(level, Zone::SkyBridge, 600, 80));
        }

        Self {
            name: "Quillon Graph Skyskraber".into(),
            floors,
            human_shafts: 6,
            robot_shafts: 4,
            facade: Facade { emblem: "Quillon Graph shield sigil".into(), light_column: true },
            waterfront: true,
        }
    }

    pub fn top_level(&self) -> i32 {
        self.floors.iter().map(|f| f.level).max().unwrap_or(0)
    }

    pub fn bottom_level(&self) -> i32 {
        self.floors.iter().map(|f| f.level).min().unwrap_or(0)
    }

    /// Sorted levels of a zone; deduplicated (twin spires share levels).
    pub fn levels_of(&self, zone: Zone) -> Vec<i32> {
        let mut v: Vec<i32> =
            self.floors.iter().filter(|f| f.zone == zone).map(|f| f.level).collect();
        v.sort();
        v.dedup();
        v
    }

    /// The level where the body splits into twin spires, if it does.
    pub fn split_level(&self) -> Option<i32> {
        self.floors.iter().filter(|f| f.spire.is_some()).map(|f| f.level).min()
    }

    pub fn spire_top(&self, s: Spire) -> Option<i32> {
        self.floors.iter().filter(|f| f.spire == Some(s)).map(|f| f.level).max()
    }

    /// The red pen. Every invariant here is a rule a structural or life-safety
    /// engineer would also insist on — encoded so the blueprint cannot drift.
    pub fn validate(&self) -> Result<(), TowerError> {
        let mut seen = std::collections::BTreeSet::new();
        for f in &self.floors {
            if !seen.insert((f.level, f.spire)) {
                return Err(TowerError::DuplicateFloor(f.level, f.spire));
            }
            if f.zone == Zone::GoldVault && f.level >= 0 {
                return Err(TowerError::VaultAboveGround(f.level));
            }
        }
        let lobbies: Vec<&Floor> =
            self.floors.iter().filter(|f| f.zone == Zone::Lobby).collect();
        if lobbies.len() != 1 || lobbies[0].level != 0 {
            return Err(TowerError::LobbyWrong(lobbies.len()));
        }
        if self.robot_shafts == 0 {
            return Err(TowerError::NoRobotShafts);
        }
        let aud = self.levels_of(Zone::Auditorium);
        if !aud.is_empty() {
            for w in aud.windows(2) {
                if w[1] - w[0] != 1 {
                    return Err(TowerError::AuditoriumSplit);
                }
            }
        }
        // Twin-spire discipline.
        if let Some(split) = self.split_level() {
            let a_min = self.floors.iter().filter(|f| f.spire == Some(Spire::A)).map(|f| f.level).min();
            let b_min = self.floors.iter().filter(|f| f.spire == Some(Spire::B)).map(|f| f.level).min();
            if let (Some(a), Some(b)) = (a_min, b_min) {
                if a != b {
                    return Err(TowerError::SplitMismatch(a, b));
                }
            }
            for f in &self.floors {
                if f.level >= split && f.spire.is_none() && f.zone != Zone::SkyBridge {
                    return Err(TowerError::UnassignedAboveSplit(f.level));
                }
                if f.spire.is_some() && f.level < split {
                    return Err(TowerError::SpireBelowPlate(f.level));
                }
            }
            let bridges = self.levels_of(Zone::SkyBridge);
            if bridges.is_empty() {
                return Err(TowerError::NoSkyBridge);
            }
            let overlap_top = Spire::iter_tops(self).unwrap_or(split);
            for b in bridges {
                if b < split || b > overlap_top {
                    return Err(TowerError::BridgeOutsideOverlap(b, split, overlap_top));
                }
            }
        }
        // Mechanical cadence: never more than 25 above-ground floors without a
        // mechanical or refuge (sky bridge) level.
        const MAX_GAP: i32 = 25;
        let mut mech = self.levels_of(Zone::Mechanical);
        mech.extend(self.levels_of(Zone::SkyBridge));
        mech.push(0); // lobby level anchors the bottom of the run
        mech.push(self.top_level());
        mech.sort();
        mech.dedup();
        for w in mech.windows(2) {
            let gap = w[1] - w[0];
            if gap > MAX_GAP {
                return Err(TowerError::MechanicalGap { gap, max: MAX_GAP });
            }
        }
        Ok(())
    }
}

impl Spire {
    /// Highest level present in BOTH spires — the bridge must land inside it.
    fn iter_tops(spec: &TowerSpec) -> Option<i32> {
        let a = spec.spire_top(Spire::A)?;
        let b = spec.spire_top(Spire::B)?;
        Some(a.min(b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_blueprint_validates() {
        let spec = TowerSpec::quillon_default();
        assert_eq!(spec.validate(), Ok(()));
        assert_eq!(spec.top_level(), 88);
        assert_eq!(spec.bottom_level(), -4);
        assert_eq!(spec.levels_of(Zone::GoldVault), vec![-4, -3]);
        assert_eq!(spec.levels_of(Zone::Auditorium), vec![6, 7, 8]);
        assert_eq!(spec.split_level(), Some(46));
        assert_eq!(spec.spire_top(Spire::A), Some(88));
        assert_eq!(spec.spire_top(Spire::B), Some(78));
        assert_eq!(spec.levels_of(Zone::SkyBridge), vec![60, 61]);
        assert!(spec.waterfront);
        assert!(spec.facade.light_column);
    }

    #[test]
    fn vault_above_ground_is_rejected() {
        let mut spec = TowerSpec::quillon_default();
        spec.floors.push(Floor { level: 89, zone: Zone::GoldVault, area_m2: 100, design_occupancy: 1, spire: Some(Spire::A) });
        assert_eq!(spec.validate(), Err(TowerError::VaultAboveGround(89)));
    }

    #[test]
    fn robotless_tower_is_rejected() {
        let mut spec = TowerSpec::quillon_default();
        spec.robot_shafts = 0;
        assert_eq!(spec.validate(), Err(TowerError::NoRobotShafts));
    }

    #[test]
    fn split_auditorium_is_rejected() {
        let mut spec = TowerSpec::quillon_default();
        // Move floor 7 out of the auditorium: 6 and 8 remain, no longer contiguous.
        for f in spec.floors.iter_mut() {
            if f.level == 7 {
                f.zone = Zone::Offices;
            }
        }
        assert_eq!(spec.validate(), Err(TowerError::AuditoriumSplit));
    }

    #[test]
    fn twin_spires_without_a_bridge_are_rejected() {
        let mut spec = TowerSpec::quillon_default();
        spec.floors.retain(|f| f.zone != Zone::SkyBridge);
        assert_eq!(spec.validate(), Err(TowerError::NoSkyBridge));
    }

    #[test]
    fn bridge_above_the_shorter_spire_is_rejected() {
        let mut spec = TowerSpec::quillon_default();
        // Spire B tops out at 78 — a bridge at 85 would hang in the air.
        for f in spec.floors.iter_mut() {
            if f.zone == Zone::SkyBridge && f.level == 61 {
                f.level = 85;
            }
        }
        assert_eq!(spec.validate(), Err(TowerError::BridgeOutsideOverlap(85, 46, 78)));
    }

    #[test]
    fn full_plate_above_the_split_is_rejected() {
        let mut spec = TowerSpec::quillon_default();
        spec.floors.push(Floor { level: 50, zone: Zone::Offices, area_m2: 100, design_occupancy: 1, spire: None });
        assert_eq!(spec.validate(), Err(TowerError::UnassignedAboveSplit(50)));
    }
}
