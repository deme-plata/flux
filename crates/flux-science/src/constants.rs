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

/// Inverse fine-structure constant, CODATA 2022: α⁻¹ = 137.035999177(21).
pub const FINE_STRUCTURE_INV: f64 = 137.035999177;

/// Fine-structure constant α, CODATA 2022.
pub fn fine_structure() -> f64 {
    1.0 / FINE_STRUCTURE_INV
}

/// Standard acceleration of gravity (m/s²), exact by definition (CGPM 1901).
pub const STANDARD_GRAVITY: f64 = 9.80665;

// ── CODATA 2022 particle-physics block (added 2026-09-02, closes kappa-hep audit
// fix #2: "no eV→J, no lepton mass"). Values from physics.nist.gov/constants.
/// Elementary charge (C), exact since the 2019 SI redefinition. Numerically
/// equal to one electron-volt in joules.
pub const ELEMENTARY_CHARGE: f64 = 1.602_176_634e-19;
/// One electron-volt in joules, exact (= ELEMENTARY_CHARGE · 1 V).
pub const ELECTRON_VOLT: f64 = 1.602_176_634e-19;
/// Proton mass (kg), CODATA 2022: 1.672 621 925 95(52) e-27, u_r = 3.1e-10.
pub const PROTON_MASS: f64 = 1.672_621_925_95e-27;
/// Electron mass (kg), CODATA 2022: 9.109 383 7139(28) e-31, u_r = 3.1e-10.
pub const ELECTRON_MASS: f64 = 9.109_383_713_9e-31;
/// Planck constant h (J s), exact: 6.626 070 15 e-34.
pub const PLANCK_H: f64 = 6.626_070_15e-34;

/// Convert an energy in eV to joules (exact).
pub fn ev_to_joule(ev: f64) -> f64 {
    ev * ELECTRON_VOLT
}
/// Convert an energy in joules to eV (exact).
pub fn joule_to_ev(j: f64) -> f64 {
    j / ELECTRON_VOLT
}
/// Dimensionless gravitational coupling of a particle of mass m:
/// α_G = G m² / (ħ c). For the proton this is 5.906e-39 — the "nominal
/// Planck-suppressed coupling" that some earlier papers wrote as α₀.
pub fn gravitational_coupling(mass: f64) -> f64 {
    GRAVITATIONAL * mass * mass / (PLANCK_REDUCED * SPEED_OF_LIGHT)
}
/// Landauer erasure bound k_B T ln 2 (J per bit) at temperature T (K).
pub fn landauer_bound(temperature_k: f64) -> f64 {
    BOLTZMANN * temperature_k * std::f64::consts::LN_2
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
    fn test_fine_structure_codata_2022() {
        // α ≈ 7.297352564e-3; the folklore Z≈137 element sits at 1/α.
        let a = fine_structure();
        assert!((a - 7.2973525643e-3).abs() < 1e-11, "alpha = {a}");
        assert!(FINE_STRUCTURE_INV > 137.0 && FINE_STRUCTURE_INV < 137.04);
    }

    #[test]
    fn test_codata_particle_block() {
        // exactness / self-consistency
        assert_eq!(ELECTRON_VOLT, ELEMENTARY_CHARGE);
        assert!((PLANCK_H / (2.0 * std::f64::consts::PI) - PLANCK_REDUCED).abs() / PLANCK_REDUCED < 1e-9);
        assert!((joule_to_ev(ev_to_joule(4.07e6)) - 4.07e6).abs() < 1e-3);
        // proton gravitational coupling, textbook 5.906e-39
        let a = gravitational_coupling(PROTON_MASS);
        assert!((a - 5.906e-39).abs() / 5.906e-39 < 1e-3, "alpha_G(p) = {a}");
        // proton/electron mass ratio 1836.152673...
        let r = PROTON_MASS / ELECTRON_MASS;
        assert!((r - 1836.152_673).abs() < 1e-3, "m_p/m_e = {r}");
        // Landauer at 300 K = 2.871e-21 J = 17.9 meV
        let l = landauer_bound(300.0);
        assert!((l - 2.871e-21).abs() / 2.871e-21 < 1e-3, "landauer = {l}");
    }

    #[test]
    fn test_schwarzschild_solar() {
        let solar_mass = 1.989e30; // kg
        let rs = schwarzschild_radius(solar_mass);
        assert!((rs - 2954.0).abs() < 100.0, "Solar Schwarzschild radius ~2954m, got {}", rs);
    }
}
