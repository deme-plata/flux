//! The 2026 FIA Formula 1 technical regulations, encoded as hard limits the
//! garage must respect. These numbers are the real headline 2026 figures:
//!
//! * Minimum car weight: **768 kg** (down from 800 kg in 2025).
//! * MGU-K deployable electrical power: **350 kW** (up from 120 kW; ~3×).
//! * Internal combustion engine: capped ~**400 kW** (down from >550 kW).
//! * MGU-H: **removed**.
//! * DRS: **removed** — replaced by driver-controlled active aero
//!   (Z-mode = low-drag straight, X-mode = high-downforce corner) plus a
//!   "manual override" electrical boost.
//! * Fuel: **100% sustainable**, energy-flow limited.
//!
//! The point of the game: push every part toward its regulated ceiling to go
//! as fast as possible — but never past it, or the car is disqualified.

use serde::{Deserialize, Serialize};

/// A single broken rule, surfaced to the player so they know *why* the car is illegal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Violation {
    pub rule: String,
    pub limit: String,
    pub actual: String,
}

/// The full 2026 rulebook the scrutineers check against.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Regulations {
    /// FIA minimum car weight including driver, kg. Going lighter is faster — and illegal.
    pub min_weight_kg: f64,
    /// MGU-K peak deployable electrical power, kW.
    pub max_mguk_kw: f64,
    /// Internal combustion engine peak power, kW.
    pub max_ice_kw: f64,
    /// Per-lap deployable battery energy, MJ.
    pub max_battery_mj: f64,
    /// Active-aero modes allowed (Z straight-mode + X corner-mode = 2). More flaps = illegal.
    pub max_aero_modes: u8,
    /// MGU-H is banned in 2026: true means the car must NOT carry one.
    pub mgu_h_banned: bool,
    /// Fuel must be 100% sustainable.
    pub sustainable_fuel_required: bool,
}

impl Regulations {
    /// The 2026 ruleset the game is balanced around.
    pub fn y2026() -> Self {
        Regulations {
            min_weight_kg: 768.0,
            max_mguk_kw: 350.0,
            max_ice_kw: 400.0,
            max_battery_mj: 8.5,
            max_aero_modes: 2,
            mgu_h_banned: true,
            sustainable_fuel_required: true,
        }
    }

    /// Combined peak power the rules permit, kW. ~750 kW ≈ ~1000 hp split 50/50.
    pub fn combined_power_kw(&self) -> f64 {
        self.max_mguk_kw + self.max_ice_kw
    }
}

impl Default for Regulations {
    fn default() -> Self {
        Self::y2026()
    }
}
