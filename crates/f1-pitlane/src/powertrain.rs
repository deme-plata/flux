//! The modern (2026) power unit, modelled as an energy system — this is the
//! "hands dirty in oil" layer. No MGU-H; a big 350 kW MGU-K; a finite battery
//! you must **harvest under braking** and **spend on deployment**; an ICE on
//! 100% sustainable fuel whose mass you burn down over a stint; and active aero
//! with a low-drag straight mode (Z) and a high-downforce corner mode (X).
//!
//! The interesting engineering: you can't deploy electrical power you haven't
//! banked. Over-deploy early in the lap and you run the battery flat before the
//! main straight — exactly the energy-management game real 2026 F1 will be.

use crate::regs::Regulations;
use serde::{Deserialize, Serialize};

/// Active-aero state. 2026 replaces DRS with driver-selectable modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AeroMode {
    /// Z-mode: wings shed drag for the straights (low Cd, low downforce).
    Straight,
    /// X-mode: wings load up for the corners (high downforce, high Cd).
    Corner,
}

impl AeroMode {
    /// Drag coefficient × frontal area (m²) — lower is faster in a straight line.
    pub fn drag_cda(&self) -> f64 {
        match self {
            AeroMode::Straight => 0.70, // shed drag
            AeroMode::Corner => 1.05,   // full wing
        }
    }
    /// Downforce coefficient × area — extra grip in the corners.
    pub fn downforce_cla(&self) -> f64 {
        match self {
            AeroMode::Straight => 1.8,
            AeroMode::Corner => 3.4,
        }
    }
}

/// Fixed power-unit specification (legal 2026 ceilings).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PowerUnit {
    pub ice_kw: f64,
    pub mguk_kw: f64,
    pub battery_capacity_mj: f64,
}

impl PowerUnit {
    pub fn y2026(regs: &Regulations) -> Self {
        PowerUnit {
            ice_kw: regs.max_ice_kw,
            mguk_kw: regs.max_mguk_kw,
            battery_capacity_mj: regs.max_battery_mj,
        }
    }
    /// Peak combined power (W) with full deployment.
    pub fn peak_combined_w(&self) -> f64 {
        (self.ice_kw + self.mguk_kw) * 1000.0
    }
    /// ICE-only power (W) — what you have when the battery is flat.
    pub fn ice_w(&self) -> f64 {
        self.ice_kw * 1000.0
    }
}

/// Mutable energy state across a stint.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EnergyState {
    /// Stored deployable electrical energy (MJ).
    pub battery_mj: f64,
    /// Fuel mass remaining (kg) — burns down, lightening (and quickening) the car.
    pub fuel_kg: f64,
}

impl EnergyState {
    /// A car starting the race: battery topped up, ~full lightweight 2026 tank.
    pub fn race_start(pu: &PowerUnit) -> Self {
        EnergyState { battery_mj: pu.battery_capacity_mj, fuel_kg: 100.0 }
    }
}

/// What the power unit did over one lap.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LapEnergy {
    pub harvested_mj: f64,
    pub deployed_mj: f64,
    pub fuel_burned_kg: f64,
    /// True if the driver asked for more deployment than the battery could give —
    /// i.e. ran out of electrical boost before the lap was done.
    pub ran_dry: bool,
    /// Fraction of the lap (0..1) that had MGU-K boost available.
    pub boost_fraction: f64,
}

/// Estimate how much energy can be harvested under braking on this track —
/// proportional to the number and severity of braking zones (slow corners).
pub fn harvestable_mj(braking_events: &[f64], pu: &PowerUnit, lap_time_s: f64) -> f64 {
    // Each braking event recovers kinetic energy, capped by the MGU-K's rate.
    let raw: f64 = braking_events.iter().sum();
    let mguk_cap = pu.mguk_kw * lap_time_s / 1000.0 * 0.5; // regen ~half the lap under braking
    raw.min(mguk_cap).min(pu.battery_capacity_mj)
}

/// Run the lap's energy balance given a deployment aggression (0 = lift-and-coast,
/// 1 = send everything). Mutates the [`EnergyState`].
pub fn lap_energy(
    pu: &PowerUnit,
    state: &mut EnergyState,
    braking_events: &[f64],
    lap_time_s: f64,
    deploy_aggression: f64,
) -> LapEnergy {
    let aggression = deploy_aggression.clamp(0.0, 1.0);
    let harvested = harvestable_mj(braking_events, pu, lap_time_s);

    // Maximum the MGU-K could physically deploy over the lap.
    let max_deploy = pu.mguk_kw * lap_time_s / 1000.0;
    let want = aggression * max_deploy;
    let available = state.battery_mj + harvested;
    let deployed = want.min(available).max(0.0);
    let ran_dry = want > available + 1e-6;

    state.battery_mj = (state.battery_mj + harvested - deployed).clamp(0.0, pu.battery_capacity_mj);

    // Fuel: ICE energy over the lap, in kg (≈ 43 MJ/kg, ~30% thermal efficiency
    // for a 2026 PU running near 400 kW).
    let ice_energy_mj = pu.ice_kw * lap_time_s / 1000.0;
    let fuel_burned = ice_energy_mj / (43.0 * 0.30);
    state.fuel_kg = (state.fuel_kg - fuel_burned).max(0.0);

    let boost_fraction = if max_deploy > 0.0 { (deployed / max_deploy).clamp(0.0, 1.0) } else { 0.0 };

    LapEnergy {
        harvested_mj: harvested,
        deployed_mj: deployed,
        fuel_burned_kg: fuel_burned,
        ran_dry,
        boost_fraction,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pu() -> PowerUnit {
        PowerUnit::y2026(&Regulations::y2026())
    }

    #[test]
    fn pu_matches_2026_ceilings() {
        let p = pu();
        assert_eq!(p.mguk_kw, 350.0);
        assert_eq!(p.ice_kw, 400.0);
        assert!((p.peak_combined_w() - 750_000.0).abs() < 1.0);
    }

    #[test]
    fn straight_mode_has_less_drag_than_corner_mode() {
        assert!(AeroMode::Straight.drag_cda() < AeroMode::Corner.drag_cda());
        assert!(AeroMode::Corner.downforce_cla() > AeroMode::Straight.downforce_cla());
    }

    #[test]
    fn over_deploying_runs_the_battery_dry() {
        let p = pu();
        let mut st = EnergyState { battery_mj: 1.0, fuel_kg: 100.0 };
        // few braking zones to harvest from, but max aggression
        let e = lap_energy(&p, &mut st, &[0.2, 0.2], 75.0, 1.0);
        assert!(e.ran_dry, "asking for full deploy on an empty battery must run dry");
        assert!(st.battery_mj < 0.5);
        assert!(e.boost_fraction < 1.0);
    }

    #[test]
    fn lifting_and_coasting_recharges_the_battery() {
        let p = pu();
        let mut st = EnergyState { battery_mj: 2.0, fuel_kg: 100.0 };
        // lots of braking, zero deploy → battery climbs back up
        let e = lap_energy(&p, &mut st, &[1.0, 1.0, 1.0, 1.0], 90.0, 0.0);
        assert_eq!(e.deployed_mj, 0.0);
        assert!(st.battery_mj > 2.0, "harvesting with no deploy must recharge");
        assert!(st.battery_mj <= p.battery_capacity_mj);
    }

    #[test]
    fn fuel_burns_down_over_a_lap() {
        let p = pu();
        let mut st = EnergyState::race_start(&p);
        let before = st.fuel_kg;
        lap_energy(&p, &mut st, &[0.5, 0.5], 80.0, 0.7);
        assert!(st.fuel_kg < before, "ICE must burn fuel");
        assert!(before - st.fuel_kg < 5.0, "but not absurd amounts per lap");
    }
}
