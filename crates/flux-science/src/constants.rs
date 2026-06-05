// Flux Science — Physical Constants
//
// Planck units, fundamental constants, scale hierarchies.
// Ported from research.md.

/// Speed of light in vacuum (m/s).
pub const SPEED_OF_LIGHT: f64 = 2.99792458e8;

/// Gravitational constant (m^3 kg^-1 s^-2).
pub const GRAVITATIONAL: f64 = 6.67430e-11;

/// Reduced Planck constant (m^2 kg / s).
pub const PLANCK_REDUCED: f64 = 1.054571817e-34;

/// Boltzmann constant (m^2 kg s^-2 K^-1).
pub const BOLTZMANN: f64 = 1.380649e-23;

/// Planck length (m).
pub fn planck_length() -> f64 {
    (GRAVITATIONAL * PLANCK_REDUCED / SPEED_OF_LIGHT.powi(3)).sqrt()
}

/// Planck time (s).
pub fn planck_time() -> f64 {
    planck_length() / SPEED_OF_LIGHT
}

/// Planck mass (kg).
pub fn planck_mass() -> f64 {
    (PLANCK_REDUCED * SPEED_OF_LIGHT / GRAVITATIONAL).sqrt()
}

/// Planck energy (J).
pub fn planck_energy() -> f64 {
    planck_mass() * SPEED_OF_LIGHT.powi(2)
}

/// Planck temperature (K).
pub fn planck_temperature() -> f64 {
    planck_energy() / BOLTZMANN
}

/// Schwarzschild radius for mass M (m).
pub fn schwarzschild_radius(mass: f64) -> f64 {
    2.0 * GRAVITATIONAL * mass / SPEED_OF_LIGHT.powi(2)
}

/// Hubble constant (km/s/Mpc) — approximate.
pub const HUBBLE_CONSTANT: f64 = 70.0;

/// Convert Hubble constant to SI (s^-1).
pub fn hubble_si() -> f64 {
    HUBBLE_CONSTANT * 1000.0 / 3.085677581e22 // km/s/Mpc → s^-1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_planck_units_order() {
        let lp = planck_length();
        let tp = planck_time();
        let mp = planck_mass();
        assert!(lp > 1e-36 && lp < 1e-34);
        assert!(tp > 1e-45 && tp < 1e-43);
        assert!(mp > 1e-9 && mp < 1e-7);
    }

    #[test]
    fn test_schwarzschild_solar() {
        let solar_mass = 1.989e30; // kg
        let rs = schwarzschild_radius(solar_mass);
        assert!((rs - 2954.0).abs() < 100.0, "Solar Schwarzschild radius ~2954m, got {}", rs);
    }
}
