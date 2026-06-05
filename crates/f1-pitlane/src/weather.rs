//! Weather — and it's hooked to the *real world*. [`Weather::from_open_meteo`]
//! ingests a live open-meteo reading at a circuit's coordinates, so the next
//! Grand Prix can be raced in the conditions actually happening there right now.
//!
//! Weather drives three things the physics layer cares about:
//! * **surface grip** (cold + wet = less grip = slower + more crashes),
//! * **track temperature** (tyre operating window, see [`crate::tyres`]),
//! * **the right tyre** (slick → intermediate → wet crossover).

use crate::garage::Compound;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Weather {
    pub air_temp_c: f64,
    pub track_temp_c: f64,
    pub rain_mm: f64,
    pub humidity_pct: f64,
    pub wind_kmh: f64,
    /// WMO weather code (0 = clear, 1-3 cloud, 51-67 rain/drizzle, 80-82 showers…).
    pub wmo_code: u16,
}

impl Weather {
    /// A dry, warm reference race day.
    pub fn dry_warm() -> Self {
        Weather { air_temp_c: 25.0, track_temp_c: 38.0, rain_mm: 0.0, humidity_pct: 45.0, wind_kmh: 8.0, wmo_code: 0 }
    }

    /// Build from a live open-meteo `current` reading. Track temp is derived from
    /// air temp + solar load (clear skies bake the asphalt well above air).
    pub fn from_open_meteo(air_temp_c: f64, humidity_pct: f64, rain_mm: f64, wmo_code: u16, wind_kmh: f64) -> Self {
        let solar_gain = match wmo_code {
            0 => 14.0,          // clear — asphalt much hotter than air
            1 | 2 => 9.0,       // mainly clear / partly cloudy
            3 => 5.0,           // overcast
            45 | 48 => 4.0,     // fog
            c if c >= 51 => 1.0, // any precipitation — track cools toward air
            _ => 7.0,
        };
        let track_temp_c = air_temp_c + solar_gain - (wind_kmh * 0.15);
        Weather { air_temp_c, track_temp_c, rain_mm, humidity_pct, wind_kmh, wmo_code }
    }

    /// 0.0 (bone dry) .. 1.0 (full wet) — from rain amount and the WMO code.
    pub fn rain_intensity(&self) -> f64 {
        let from_code: f64 = match self.wmo_code {
            0..=3 => 0.0,
            45 | 48 => 0.05,
            51 | 53 | 55 => 0.20,   // drizzle
            56 | 57 => 0.30,        // freezing drizzle
            61 => 0.35,             // slight rain
            63 => 0.55,             // moderate rain
            65 => 0.80,             // heavy rain
            66 | 67 => 0.70,        // freezing rain
            71..=77 => 0.40,        // snow
            80 => 0.45,             // slight showers
            81 => 0.65,             // moderate showers
            82 => 0.90,             // violent showers
            95..=99 => 0.85,        // thunderstorm
            _ => 0.0,
        };
        let from_mm = (self.rain_mm / 5.0).clamp(0.0, 1.0);
        from_code.max(from_mm)
    }

    pub fn is_wet(&self) -> bool {
        self.rain_intensity() > 0.08
    }

    /// Surface grip multiplier (independent of tyre choice). Peaks on a warm dry
    /// track; cold or wet asphalt offers less mechanical grip.
    pub fn grip_multiplier(&self) -> f64 {
        // Temperature term: best around 35-45°C track temp.
        let temp_term = 1.0 - ((self.track_temp_c - 40.0).abs() / 100.0).min(0.18);
        // Wet term: a soaked track loses up to ~45% surface grip.
        let wet_term = 1.0 - 0.45 * self.rain_intensity();
        (temp_term * wet_term).clamp(0.45, 1.02)
    }

    /// The compound the conditions call for.
    pub fn recommended_compound(&self) -> Compound {
        let r = self.rain_intensity();
        if r >= 0.55 {
            Compound::Wet
        } else if r >= 0.12 {
            Compound::Intermediate
        } else if self.track_temp_c < 18.0 {
            Compound::Soft // warm them up on the softest rubber when it's cold
        } else if self.track_temp_c > 45.0 {
            Compound::Hard // save them from overheating when it's baking
        } else {
            Compound::Medium
        }
    }

    pub fn describe(&self) -> String {
        let sky = match self.wmo_code {
            0 => "clear",
            1 | 2 => "partly cloudy",
            3 => "overcast",
            45 | 48 => "foggy",
            51..=57 => "drizzle",
            61..=67 => "rain",
            71..=77 => "snow",
            80..=82 => "showers",
            95..=99 => "thunderstorm",
            _ => "mixed",
        };
        format!(
            "{}, air {:.0}°C / track {:.0}°C, {}, wind {:.0} km/h — {} → recommend {}",
            sky,
            self.air_temp_c,
            self.track_temp_c,
            if self.is_wet() { format!("WET {:.0}%", self.rain_intensity() * 100.0) } else { "dry".into() },
            self.wind_kmh,
            if self.is_wet() { "wet/inter race" } else { "slick race" },
            self.recommended_compound().label(),
        )
    }
}

impl Default for Weather {
    fn default() -> Self {
        Self::dry_warm()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_monaco_clear_reading_is_dry_and_slick() {
        // The actual reading pulled 2026-06-03: 23.9°C air, 42% RH, 0mm, clear.
        let w = Weather::from_open_meteo(23.9, 42.0, 0.0, 0, 8.9);
        assert!(!w.is_wet());
        assert!(w.track_temp_c > w.air_temp_c, "clear skies should bake the track above air temp");
        assert!(matches!(w.recommended_compound(), Compound::Medium | Compound::Soft));
        assert!(w.grip_multiplier() > 0.9);
    }

    #[test]
    fn heavy_rain_drops_grip_and_calls_for_wets() {
        let w = Weather::from_open_meteo(16.0, 95.0, 6.0, 65, 20.0);
        assert!(w.is_wet());
        assert!(w.rain_intensity() > 0.7);
        assert_eq!(w.recommended_compound(), Compound::Wet);
        assert!(w.grip_multiplier() < Weather::dry_warm().grip_multiplier());
    }

    #[test]
    fn light_drizzle_is_intermediate_territory() {
        let w = Weather::from_open_meteo(19.0, 80.0, 0.4, 51, 10.0);
        assert!(w.is_wet());
        assert_eq!(w.recommended_compound(), Compound::Intermediate);
    }

    #[test]
    fn cold_track_loses_grip() {
        let cold = Weather::from_open_meteo(5.0, 60.0, 0.0, 3, 25.0);
        assert!(cold.grip_multiplier() < Weather::dry_warm().grip_multiplier());
    }
}
