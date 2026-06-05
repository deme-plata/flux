//! The lap-time integrator — a point-mass vehicle model that turns real road
//! geometry + the 2026 powertrain + tyre grip + weather into a lap time, the
//! honest way:
//!
//! * **Corners**: apex speed from `v = √(a_lat · r)` where lateral accel folds in
//!   tyre grip *and* aerodynamic downforce (which itself grows with v² — solved
//!   in a couple of iterations).
//! * **Straights**: accelerate (power-limited, drag-limited) from the last apex,
//!   then brake (under huge downforce-assisted deceleration) to the next apex.
//!   Top speed is whichever of power, drag or the braking point bites first.
//! * **Energy**: braking harvests into the battery; the straights spend it. Run
//!   the battery dry and you lose the boost — a time penalty.
//! * **Mass**: car + remaining fuel. Burn fuel and the car quickens through the
//!   stint, just like the real thing.

use crate::garage::{Car, PartKind};
use crate::powertrain::{lap_energy, AeroMode, EnergyState, LapEnergy, PowerUnit};
use crate::regs::Regulations;
use crate::track::Track;
use crate::tyres::TyreState;
use crate::weather::Weather;
use serde::{Deserialize, Serialize};

const G: f64 = 9.81;
const RHO: f64 = 1.225;

/// Result of simulating one lap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LapResult {
    pub lap_time_s: f64,
    pub top_speed_kmh: f64,
    pub avg_speed_kmh: f64,
    /// Apex speed (km/h) at each corner, in order — telemetry for the front-ends.
    pub apex_speeds_kmh: Vec<f64>,
    pub energy: LapEnergy,
    pub ran_dry: bool,
    pub tyre_grip: f64,
}

fn car_aero_quality(car: &Car) -> f64 {
    let fw = car.part(PartKind::FrontWing);
    let rw = car.part(PartKind::RearWing);
    let fl = car.part(PartKind::Floor);
    // average of condition × tune, normalised to 0..1
    let q = |p: &crate::garage::Part| (p.condition / 100.0) * (p.value / 100.0);
    ((q(fw) + q(rw) + q(fl)) / 3.0).clamp(0.3, 1.05)
}

fn engine_power_w(car: &Car, pu: &PowerUnit, regs: &Regulations, deploy: bool) -> f64 {
    let ice = car.part(PartKind::Ice);
    let mguk = car.part(PartKind::Mguk);
    let (ice_max, _) = PartKind::Ice.legal_max(regs);
    let (mguk_max, _) = PartKind::Mguk.legal_max(regs);
    let ice_eff = (ice.condition / 100.0) * (ice.value / ice_max).min(1.0);
    let ice_w = pu.ice_w() * ice_eff;
    if deploy {
        let mguk_eff = (mguk.condition / 100.0) * (mguk.value / mguk_max).min(1.0);
        ice_w + pu.mguk_kw * 1000.0 * mguk_eff
    } else {
        ice_w
    }
}

/// Apex speed (m/s) for a corner, solving the downforce↔speed coupling.
///
/// `v_cap` is the car's straight-line top speed: a fast, open kink isn't grip
/// limited at all — it's taken flat, capped by power/drag, not by the radius.
/// The geometric radius is opened up by the **racing line** (drivers use the
/// full track width), so the effective cornering radius is larger than the kerb.
fn apex_speed(radius_m: f64, tyre_grip: f64, aero_q: f64, mass: f64, v_cap: f64) -> f64 {
    let cla = AeroMode::Corner.downforce_cla() * aero_q;
    let r = radius_m * 1.5; // racing line opens the corner
    // Start mechanical-only, then add aero grip (grows with v²) over 3 passes.
    let mut v = (tyre_grip * G * r).sqrt();
    for _ in 0..3 {
        let downforce = 0.5 * RHO * cla * v * v;
        let a_lat = (G + downforce / mass) * tyre_grip;
        v = (a_lat * r).sqrt().min(v_cap); // a flat-out kink is capped by top speed
    }
    v.min(v_cap)
}

/// Power-limited terminal speed (m/s) in the low-drag straight mode.
fn v_max_power(power_w: f64) -> f64 {
    let cda = AeroMode::Straight.drag_cda();
    (2.0 * power_w / (RHO * cda)).powf(1.0 / 3.0)
}

/// Time over a straight of length `l`, going from `v_in` to `v_apex_next`,
/// returning (time_s, top_speed, harvested_braking_energy_mj).
#[allow(clippy::too_many_arguments)]
fn straight_segment(
    l: f64,
    v_in: f64,
    v_apex_next: f64,
    power_w: f64,
    mass: f64,
    tyre_grip: f64,
    v_cap: f64,
) -> (f64, f64, f64) {
    // Representative longitudinal accel: traction-limited low down, power-limited up high.
    let v_ref = v_in.max(20.0);
    let traction = tyre_grip * G * 1.45; // rear grip + downforce-assisted traction
    let a_acc = (power_w / (mass * v_ref)).min(traction).max(2.0);
    // Braking: downforce-assisted, very strong.
    let a_brake = tyre_grip * G * 4.6;
    let vmax = v_max_power(power_w).min(v_cap);

    // Unconstrained top speed if length allowed full accel then brake.
    let k = 1.0 / (2.0 * a_acc) + 1.0 / (2.0 * a_brake);
    let v_top_uncon =
        ((l + v_in * v_in / (2.0 * a_acc) + v_apex_next * v_apex_next / (2.0 * a_brake)) / k).sqrt();
    let v_top = v_top_uncon.min(vmax).max(v_apex_next.max(v_in));

    let d_acc = ((v_top * v_top - v_in * v_in) / (2.0 * a_acc)).max(0.0);
    let d_brake = ((v_top * v_top - v_apex_next * v_apex_next) / (2.0 * a_brake)).max(0.0);
    let d_cruise = (l - d_acc - d_brake).max(0.0);

    let t_acc = if a_acc > 0.0 { (v_top - v_in).max(0.0) / a_acc } else { 0.0 };
    let t_brake = if a_brake > 0.0 { (v_top - v_apex_next).max(0.0) / a_brake } else { 0.0 };
    let t_cruise = if v_top > 0.0 { d_cruise / v_top } else { 0.0 };

    // Braking energy recovered (MJ), at ~55% regen efficiency, MGU-K only on the rear.
    let harvested = (0.5 * mass * (v_top * v_top - v_apex_next * v_apex_next)).max(0.0) * 0.55 / 1e6;

    (t_acc + t_cruise + t_brake, v_top, harvested)
}

/// How hard this track works the tyres (0..1), from its danger + corner mix.
fn tyre_load(track: &Track) -> f64 {
    (0.4 + 0.5 * track.danger).clamp(0.3, 1.0)
}

/// Simulate one flying lap. Mutates tyre + energy state across the stint.
pub fn simulate_lap(
    track: &Track,
    car: &Car,
    tyre: &mut TyreState,
    weather: &Weather,
    pu: &PowerUnit,
    energy: &mut EnergyState,
    regs: &Regulations,
    deploy_aggression: f64,
) -> LapResult {
    let aero_q = car_aero_quality(car);
    let tyre_grip = tyre.grip(weather);
    let mass = car.weight_kg + energy.fuel_kg;
    let deploy = energy.battery_mj > 0.05 && deploy_aggression > 0.0;
    let power = engine_power_w(car, pu, regs, deploy);
    // Top speed: lesser of power/drag terminal and the gearing limit (~345 km/h).
    let v_cap = v_max_power(power).min(96.0);

    // 1) apex speeds for every corner (racing line + downforce, capped at top speed)
    let apex: Vec<f64> = track
        .corners
        .iter()
        .map(|c| apex_speed(c.radius_m.max(8.0), tyre_grip, aero_q, mass, v_cap))
        .collect();

    // 2) straights + corners, lap loop (corner i is preceded by its straight,
    //    entered at the previous corner's apex/exit speed).
    let n = track.corners.len();
    let mut lap_time = 0.0;
    let mut top_speed = 0.0f64;
    let mut braking_events = Vec::with_capacity(n);

    for i in 0..n {
        let c = &track.corners[i];
        let v_in = apex[(i + n - 1) % n];
        let v_apex = apex[i];
        let (t_s, v_top, harvest) =
            straight_segment(c.straight_before_m.max(20.0), v_in, v_apex, power, mass, tyre_grip, v_cap);
        lap_time += t_s;
        top_speed = top_speed.max(v_top);
        braking_events.push(harvest);
        // corner arc traversed slightly above apex (drivers feed in throttle on exit)
        lap_time += c.arc_len_m.max(10.0) / (v_apex.max(5.0) * 1.12);
    }

    // 3) energy balance for the lap
    let energy_report = lap_energy(pu, energy, &braking_events, lap_time, deploy_aggression);
    // Running the battery dry mid-lap costs the deployment benefit on part of the lap.
    if energy_report.ran_dry {
        lap_time *= 1.0 + 0.012 * (1.0 - energy_report.boost_fraction);
    }

    // 4) tyre evolution
    tyre.run_lap(tyre_load(track), weather);

    let lap_dist_m = track.length_km * 1000.0;
    let avg_speed = lap_dist_m / lap_time;
    LapResult {
        lap_time_s: lap_time,
        top_speed_kmh: top_speed * 3.6,
        avg_speed_kmh: avg_speed * 3.6,
        apex_speeds_kmh: apex.iter().map(|v| v * 3.6).collect(),
        energy: energy_report,
        ran_dry: energy_report.ran_dry,
        tyre_grip,
    }
}

/// Convenience: a fresh-tyre, full-energy single flying lap (qualifying sim).
pub fn quali_lap(track: &Track, car: &Car, weather: &Weather, regs: &Regulations) -> LapResult {
    let pu = PowerUnit::y2026(regs);
    let mut energy = EnergyState::race_start(&pu);
    let mut tyre = TyreState::fresh(weather.recommended_compound(), weather);
    // out-lap(s) to bring the tyres into their working window before the flyer
    for _ in 0..4 {
        let _ = simulate_lap(track, car, &mut tyre, weather, &pu, &mut energy, regs, 0.4);
    }
    energy = EnergyState::race_start(&pu);
    simulate_lap(track, car, &mut tyre, weather, &pu, &mut energy, regs, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::{monaco, silverstone};

    fn perfect_car() -> (Car, Regulations) {
        let r = Regulations::y2026();
        let mut c = Car::new_2026(&r);
        c.perfect_all(&r);
        (c, r)
    }

    #[test]
    fn perfect_car_laps_monaco_in_a_realistic_time() {
        let (car, regs) = perfect_car();
        let w = Weather::dry_warm();
        let lap = quali_lap(&monaco(), &car, &w, &regs);
        // Real 2026 Monaco pole is ~71s; allow a generous realistic band.
        assert!(lap.lap_time_s > 60.0 && lap.lap_time_s < 90.0,
            "Monaco lap should be realistic, got {:.2}s", lap.lap_time_s);
        // Monaco top speed is low (tunnel ~ 290 km/h); never silly.
        assert!(lap.top_speed_kmh > 180.0 && lap.top_speed_kmh < 360.0,
            "Monaco top speed unrealistic: {:.0} km/h", lap.top_speed_kmh);
    }

    #[test]
    fn silverstone_is_faster_on_average_than_monaco() {
        let (car, regs) = perfect_car();
        let w = Weather::dry_warm();
        let monaco_lap = quali_lap(&monaco(), &car, &w, &regs);
        let silver_lap = quali_lap(&silverstone(), &car, &w, &regs);
        assert!(silver_lap.avg_speed_kmh > monaco_lap.avg_speed_kmh,
            "Silverstone ({:.0}) should average faster than Monaco ({:.0})",
            silver_lap.avg_speed_kmh, monaco_lap.avg_speed_kmh);
    }

    #[test]
    fn rain_makes_the_lap_slower() {
        let (car, regs) = perfect_car();
        let dry = Weather::dry_warm();
        let wet = Weather::from_open_meteo(15.0, 95.0, 6.0, 65, 12.0);
        let dry_lap = quali_lap(&monaco(), &car, &dry, &regs);
        let wet_lap = quali_lap(&monaco(), &car, &wet, &regs);
        assert!(wet_lap.lap_time_s > dry_lap.lap_time_s,
            "wet ({:.2}) must be slower than dry ({:.2})", wet_lap.lap_time_s, dry_lap.lap_time_s);
    }

    #[test]
    fn worn_tyres_slow_the_car_down() {
        let (car, regs) = perfect_car();
        let w = Weather::dry_warm();
        let pu = PowerUnit::y2026(&regs);
        let mut energy = EnergyState::race_start(&pu);
        let mut fresh = TyreState::fresh(crate::garage::Compound::Soft, &w);
        let fresh_lap = simulate_lap(&monaco(), &car, &mut fresh, &w, &pu, &mut energy, &regs, 0.8);
        let mut worn = TyreState::from_condition(crate::garage::Compound::Soft, 12.0, &w);
        let mut energy2 = EnergyState::race_start(&pu);
        let worn_lap = simulate_lap(&monaco(), &car, &mut worn, &w, &pu, &mut energy2, &regs, 0.8);
        assert!(worn_lap.lap_time_s > fresh_lap.lap_time_s,
            "worn tyres ({:.2}) must be slower than fresh ({:.2})", worn_lap.lap_time_s, fresh_lap.lap_time_s);
    }

    #[test]
    fn fuel_burns_down_and_car_lightens_over_a_stint() {
        let (car, regs) = perfect_car();
        let w = Weather::dry_warm();
        let pu = PowerUnit::y2026(&regs);
        let mut energy = EnergyState::race_start(&pu);
        let mut tyre = TyreState::fresh(crate::garage::Compound::Medium, &w);
        let start_fuel = energy.fuel_kg;
        for _ in 0..10 {
            simulate_lap(&monaco(), &car, &mut tyre, &w, &pu, &mut energy, &regs, 0.6);
        }
        assert!(energy.fuel_kg < start_fuel - 5.0, "fuel must burn over 10 laps");
    }
}
