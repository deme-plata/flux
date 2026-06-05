// Flux Science — Quantum Gravity Corrections
//
// String theory and Loop Quantum Gravity corrections to classical gravity.
// Ported from research.md: QuantumGravityCorrections class.

use super::constants::*;

/// Quantum gravity corrections from both string theory and LQG.
pub struct QuantumGravityCorrections {
    theory_type: String,
    alpha_prime: f64,   // String length parameter
    gamma_lqg: f64,     // Immirzi parameter for LQG
    beta: f64,          // String coupling constant
}

impl QuantumGravityCorrections {
    /// Create with theory: "string", "lqg", or "both".
    pub fn new(theory_type: &str) -> Self {
        QuantumGravityCorrections {
            theory_type: theory_type.to_string(),
            alpha_prime: planck_length().powi(2),
            gamma_lqg: 0.2375,
            beta: 1.0 / (2.0 * std::f64::consts::PI),
        }
    }

    /// String theory corrections for horizon physics.
    /// Returns (alpha_correction, loop_correction).
    pub fn string_theory_corrections(&self, mass: f64, radius: f64) -> (f64, f64) {
        if self.theory_type == "lqg" {
            return (1.0, 1.0);
        }

        let r_squared = (2.0 * GRAVITATIONAL * mass / (SPEED_OF_LIGHT.powi(2) * radius.powi(3))).powi(2);
        let alpha_correction = 1.0 + self.alpha_prime * r_squared / (16.0 * std::f64::consts::PI);
        let loop_correction = 1.0 + self.beta * GRAVITATIONAL * PLANCK_REDUCED
            / (SPEED_OF_LIGHT.powi(3) * radius.powi(2));

        (alpha_correction, loop_correction)
    }

    /// Loop Quantum Gravity corrections.
    /// Returns (delta_b, volume_correction).
    pub fn loop_quantum_corrections(&self, mass: f64, radius: f64) -> (f64, f64) {
        if self.theory_type == "string" {
            return (0.0, 1.0);
        }

        let mu_0 = (3.0 * 3.0_f64.sqrt() * self.gamma_lqg / 2.0).sqrt() * planck_length();
        let delta_b = mu_0 / radius;
        let volume_correction = (1.0 + (planck_length() / radius).powi(2)).powf(self.gamma_lqg);

        (delta_b, volume_correction)
    }

    /// Compute the corrected gravitational potential at radius r from mass M.
    pub fn corrected_potential(&self, mass: f64, radius: f64) -> f64 {
        let classical = -GRAVITATIONAL * mass / radius;
        let (alpha, _) = self.string_theory_corrections(mass, radius);
        let (delta, vol) = self.loop_quantum_corrections(mass, radius);
        classical * alpha * vol * (1.0 - delta.powi(2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn test_string_corrections_solar() {
        let qg = QuantumGravityCorrections::new("string");
        let ms = 1.989e30;
        let rs = 6.957e8;
        let (alpha, loop_corr) = qg.string_theory_corrections(ms, rs);
        assert_relative_eq!(alpha, 1.0, epsilon = 1e-5); // Tiny correction for solar scale
        assert_relative_eq!(loop_corr, 1.0, epsilon = 1e-5);
    }

    #[test]
    fn test_lqg_corrections_planck() {
        let qg = QuantumGravityCorrections::new("lqg");
        let mp = planck_mass();
        let rp = planck_length();
        let (delta, vol) = qg.loop_quantum_corrections(mp, rp);
        assert!(delta > 0.0);
        assert!(vol > 1.0, "LQG volume correction at Planck scale should be >1");
    }
}
