//! The building program — floors, zones, and the invariants an architect
//! would enforce with a red pen. `TowerSpec::validate()` IS that red pen.

use serde::{Deserialize, Serialize};
use thiserror::Error;

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
    /// Sky garden + executive commons at the crown.
    SkyGarden,
}

/// One floor of the tower. Negative levels are basements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Floor {
    pub level: i32,
    pub zone: Zone,
    pub area_m2: u32,
    /// Design occupancy (robots + humans) for transport sizing.
    pub design_occupancy: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TowerSpec {
    pub name: String,
    pub floors: Vec<Floor>,
    /// Human passenger cab shafts.
    pub human_shafts: u32,
    /// Robot freight shafts (the robot elevator transport system).
    pub robot_shafts: u32,
}

#[derive(Debug, Error, PartialEq)]
pub enum TowerError {
    #[error("gold vault on level {0} — the vault must be below ground")]
    VaultAboveGround(i32),
    #[error("lobby must exist exactly once at level 0 (found {0})")]
    LobbyWrong(usize),
    #[error("duplicate floor level {0}")]
    DuplicateLevel(i32),
    #[error("no robot shafts — a robot workforce cannot move")]
    NoRobotShafts,
    #[error("auditorium levels are not contiguous")]
    AuditoriumSplit,
    #[error("more than {max} floors between mechanical levels (gap of {gap})")]
    MechanicalGap { gap: i32, max: i32 },
}

impl TowerSpec {
    /// The canonical blueprint: 88 floors above ground, 4 basements.
    ///
    /// B4–B3 gold vault · B2–B1 robot bays · 0 lobby · 1–5 Quillon Bank
    /// · 6–8 grand auditorium · offices · mechanical every 20 · 81–88 sky
    /// garden and crown.
    pub fn quillon_default() -> Self {
        let mut floors = Vec::new();
        for level in [-4, -3] {
            floors.push(Floor { level, zone: Zone::GoldVault, area_m2: 2_400, design_occupancy: 6 });
        }
        for level in [-2, -1] {
            floors.push(Floor { level, zone: Zone::RobotBay, area_m2: 2_400, design_occupancy: 120 });
        }
        floors.push(Floor { level: 0, zone: Zone::Lobby, area_m2: 2_000, design_occupancy: 200 });
        for level in 1..=5 {
            floors.push(Floor { level, zone: Zone::BankHall, area_m2: 1_800, design_occupancy: 90 });
        }
        for level in 6..=8 {
            floors.push(Floor { level, zone: Zone::Auditorium, area_m2: 1_800, design_occupancy: 400 });
        }
        for level in 9..=80 {
            let zone = if level % 20 == 0 { Zone::Mechanical } else { Zone::Offices };
            let occ = if zone == Zone::Mechanical { 4 } else { 60 };
            floors.push(Floor { level, zone, area_m2: 1_500, design_occupancy: occ });
        }
        for level in 81..=88 {
            floors.push(Floor { level, zone: Zone::SkyGarden, area_m2: 1_200, design_occupancy: 40 });
        }
        Self { name: "Quillon Graph Skyskraber".into(), floors, human_shafts: 6, robot_shafts: 4 }
    }

    pub fn top_level(&self) -> i32 {
        self.floors.iter().map(|f| f.level).max().unwrap_or(0)
    }

    pub fn bottom_level(&self) -> i32 {
        self.floors.iter().map(|f| f.level).min().unwrap_or(0)
    }

    pub fn levels_of(&self, zone: Zone) -> Vec<i32> {
        let mut v: Vec<i32> =
            self.floors.iter().filter(|f| f.zone == zone).map(|f| f.level).collect();
        v.sort();
        v
    }

    /// The red pen. Every invariant here is a rule a structural or life-safety
    /// engineer would also insist on — encoded so the blueprint cannot drift.
    pub fn validate(&self) -> Result<(), TowerError> {
        let mut seen = std::collections::BTreeSet::new();
        for f in &self.floors {
            if !seen.insert(f.level) {
                return Err(TowerError::DuplicateLevel(f.level));
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
        // Mechanical cadence: never more than 25 above-ground floors without one.
        const MAX_GAP: i32 = 25;
        let mut mech = self.levels_of(Zone::Mechanical);
        mech.insert(0, 0); // lobby level anchors the bottom of the run
        mech.push(self.top_level());
        for w in mech.windows(2) {
            let gap = w[1] - w[0];
            if gap > MAX_GAP {
                return Err(TowerError::MechanicalGap { gap, max: MAX_GAP });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_blueprint_validates() {
        let spec = TowerSpec::quillon_default();
        assert!(spec.validate().is_ok());
        assert_eq!(spec.top_level(), 88);
        assert_eq!(spec.bottom_level(), -4);
        assert_eq!(spec.levels_of(Zone::GoldVault), vec![-4, -3]);
        assert_eq!(spec.levels_of(Zone::Auditorium), vec![6, 7, 8]);
    }

    #[test]
    fn vault_above_ground_is_rejected() {
        let mut spec = TowerSpec::quillon_default();
        spec.floors.push(Floor { level: 89, zone: Zone::GoldVault, area_m2: 100, design_occupancy: 1 });
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
}
