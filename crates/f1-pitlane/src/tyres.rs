//! Tyre thermodynamics. A tyre isn't a grip number — it's a lump of rubber with
//! a temperature window. Too cold and it's glassy; too hot and it greases up and
//! wears off a cliff. Softer compounds peak higher but in a narrow window and
//! die fast; hards are durable but never reach the same peak. Put slicks on a
//! wet track and you're a passenger; put wets on a dry one and they boil.
//!
//! This couples to [`crate::weather`] (track temp + rain) and feeds the grip term
//! the physics integrator uses for apex speed.

use crate::garage::Compound;
use crate::weather::Weather;
use serde::{Deserialize, Serialize};

/// (window low °C, window high °C, peak grip coefficient) for a compound.
pub fn window(compound: Compound) -> (f64, f64, f64) {
    match compound {
        Compound::Soft => (85.0, 110.0, 1.06),
        Compound::Medium => (90.0, 120.0, 1.00),
        Compound::Hard => (95.0, 130.0, 0.95),
        Compound::Intermediate => (50.0, 80.0, 0.88),
        Compound::Wet => (40.0, 70.0, 0.82),
    }
}

/// How well a compound suits the current wetness (0..~1). This is the crossover:
/// slicks rule the dry and die in the rain; wets/inters rule the wet and boil dry.
pub fn wet_suitability(compound: Compound, rain: f64) -> f64 {
    match compound {
        Compound::Soft | Compound::Medium | Compound::Hard => {
            // Slicks: perfect dry, falling off a cliff as it wets.
            (1.0 - 3.2 * rain).clamp(0.08, 1.0)
        }
        Compound::Intermediate => {
            // Best in damp/light rain (~0.15..0.5); poor when dry or flooded.
            let center = 0.32;
            (1.0 - (rain - center).abs() * 1.6).clamp(0.35, 1.0)
        }
        Compound::Wet => {
            // Best in heavy rain; overheats and greases when dry.
            if rain < 0.25 {
                (0.45 + rain).clamp(0.45, 0.85)
            } else {
                (0.75 + rain * 0.25).clamp(0.75, 1.0)
            }
        }
    }
}

/// Live tyre state during a stint.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TyreState {
    pub compound: Compound,
    /// Carcass core temperature (°C).
    pub core_temp_c: f64,
    /// Remaining tread, 0 (dead) .. 100 (fresh).
    pub condition_pct: f64,
}

impl TyreState {
    /// Fit a fresh set; core temp starts near the track temp (out-lap warm-up).
    pub fn fresh(compound: Compound, weather: &Weather) -> Self {
        let (lo, hi, _) = window(compound);
        // Out of the blankets, then settling toward track temp.
        let start = (weather.track_temp_c + 25.0).clamp(lo - 20.0, hi + 10.0);
        TyreState { compound, core_temp_c: start, condition_pct: 100.0 }
    }

    /// Seed from an existing car tyre condition (so the garage and the sim agree).
    pub fn from_condition(compound: Compound, condition_pct: f64, weather: &Weather) -> Self {
        let mut t = Self::fresh(compound, weather);
        t.condition_pct = condition_pct.clamp(0.0, 100.0);
        t
    }

    /// Temperature term: a bell around the window centre. 1.0 in the sweet spot,
    /// falling to ~0.5 well outside it.
    fn temp_factor(&self) -> f64 {
        let (lo, hi, _) = window(self.compound);
        let centre = (lo + hi) / 2.0;
        let half = (hi - lo) / 2.0;
        let d = ((self.core_temp_c - centre) / half).abs();
        (1.0 - 0.5 * d * d).clamp(0.45, 1.0)
    }

    /// Condition term with a cliff: grip is fine until ~25% then drops sharply.
    fn condition_factor(&self) -> f64 {
        let c = self.condition_pct / 100.0;
        if c > 0.25 {
            0.7 + 0.3 * c
        } else {
            // off the cliff
            (c / 0.25 * 0.7).max(0.1)
        }
    }

    /// Grip coefficient delivered right now (≈ effective μ contribution), folding
    /// in compound peak, temperature, wear, wet-suitability and surface grip.
    pub fn grip(&self, weather: &Weather) -> f64 {
        let (_, _, peak) = window(self.compound);
        peak * self.temp_factor()
            * self.condition_factor()
            * wet_suitability(self.compound, weather.rain_intensity())
            * weather.grip_multiplier()
    }

    /// Whether the tyre is in its working window.
    pub fn in_window(&self) -> bool {
        let (lo, hi, _) = window(self.compound);
        self.core_temp_c >= lo && self.core_temp_c <= hi
    }

    /// One lap of thermal + wear evolution. `load` (0..1) is how hard the lap was
    /// (street circuits / high tyre-stress push temps and wear up).
    pub fn run_lap(&mut self, load: f64, weather: &Weather) {
        let (lo, hi, _) = window(self.compound);
        let centre = (lo + hi) / 2.0;
        // Heat toward an equilibrium set by track temp + working load; wind/rain cool.
        // Tuned so a warmed slick settles in the ~100-110°C working window.
        let equilibrium = weather.track_temp_c + 50.0 + 25.0 * load
            - weather.wind_kmh * 0.2
            - weather.rain_intensity() * 25.0;
        self.core_temp_c += (equilibrium - self.core_temp_c) * 0.55;
        // Wear: base rate by compound, amplified by load and by overheating.
        let base = match self.compound {
            Compound::Soft => 2.4,
            Compound::Medium => 1.6,
            Compound::Hard => 1.0,
            Compound::Intermediate => 1.8,
            Compound::Wet => 1.3,
        };
        let overheat = if self.core_temp_c > hi { (self.core_temp_c - hi) / 30.0 } else { 0.0 };
        let _ = centre;
        let wear = base * (0.6 + load) * (1.0 + overheat);
        self.condition_pct = (self.condition_pct - wear).max(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn softs_peak_higher_than_hards_in_the_dry() {
        let w = Weather::dry_warm();
        let mut soft = TyreState::fresh(Compound::Soft, &w);
        let mut hard = TyreState::fresh(Compound::Hard, &w);
        // settle both into their windows
        for _ in 0..3 {
            soft.run_lap(0.5, &w);
            hard.run_lap(0.5, &w);
        }
        assert!(soft.grip(&w) > hard.grip(&w), "softs must out-grip hards when warm");
    }

    #[test]
    fn softs_wear_faster_than_hards() {
        let w = Weather::dry_warm();
        let mut soft = TyreState::fresh(Compound::Soft, &w);
        let mut hard = TyreState::fresh(Compound::Hard, &w);
        for _ in 0..15 {
            soft.run_lap(0.7, &w);
            hard.run_lap(0.7, &w);
        }
        assert!(soft.condition_pct < hard.condition_pct, "softs should be more worn");
    }

    #[test]
    fn slicks_are_useless_in_heavy_rain() {
        let wet = Weather::from_open_meteo(15.0, 95.0, 6.0, 65, 15.0);
        let slick = TyreState::fresh(Compound::Medium, &wet);
        let wets = TyreState::fresh(Compound::Wet, &wet);
        assert!(wets.grip(&wet) > slick.grip(&wet), "wets must beat slicks in the rain");
        assert!(slick.grip(&wet) < 0.4, "slicks in heavy rain have almost no grip");
    }

    #[test]
    fn wets_overheat_and_lose_out_in_the_dry() {
        let dry = Weather::dry_warm();
        let slick = TyreState::fresh(Compound::Soft, &dry);
        let wets = TyreState::fresh(Compound::Wet, &dry);
        assert!(slick.grip(&dry) > wets.grip(&dry), "slicks must beat wets in the dry");
    }

    #[test]
    fn worn_tyre_falls_off_the_cliff() {
        let w = Weather::dry_warm();
        let mut t = TyreState::fresh(Compound::Soft, &w);
        let fresh_grip = {
            t.run_lap(0.5, &w);
            t.grip(&w)
        };
        t.condition_pct = 15.0; // past the cliff
        assert!(t.grip(&w) < fresh_grip * 0.7, "a cliffed tyre loses a lot of grip");
    }
}
