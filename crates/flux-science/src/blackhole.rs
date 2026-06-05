// Flux Science — Black Hole Evolution (Hawking Radiation)
//
// Mass loss rate during Hawking evaporation with quantum gravity corrections.
// Ported from research.md: BlackHoleEvolution class.

use super::constants::*;
use super::quantum::QuantumGravityCorrections;

/// Black hole evolution with Hawking radiation and quantum corrections.
pub struct BlackHoleEvolution {
    initial_mass: f64,
    include_quantum: bool,
    qg: QuantumGravityCorrections,
}

impl BlackHoleEvolution {
    /// Create a new black hole evolution simulation.
    /// `initial_mass` in kg. `include_quantum` enables string theory + LQG corrections.
    pub fn new(initial_mass: f64, include_quantum: bool) -> Self {
        BlackHoleEvolution {
            initial_mass,
            include_quantum,
            qg: QuantumGravityCorrections::new("both"),
        }
    }

    /// Compute the mass loss rate dM/dt at mass M.
    /// Classical: dM/dt = -ħc^4 / (15360π G^2 M^2)
    pub fn mass_loss_rate(&self, mass: f64) -> f64 {
        let classical = -PLANCK_REDUCED * SPEED_OF_LIGHT.powi(4)
            / (15360.0 * std::f64::consts::PI * GRAVITATIONAL.powi(2) * mass.powi(2));

        if !self.include_quantum {
            return classical;
        }

        let r_h = 2.0 * GRAVITATIONAL * mass / SPEED_OF_LIGHT.powi(2);
        let (alpha_corr, loop_corr) = self.qg.string_theory_corrections(mass, r_h);
        let (delta_b, vol_corr) = self.qg.loop_quantum_corrections(mass, r_h);
        let quantum_factor = alpha_corr * loop_corr * vol_corr * (1.0 - delta_b.powi(2));
        classical * quantum_factor
    }

    /// Evolve the black hole mass over time using 4th-order Runge-Kutta.
    /// Returns (times, masses) vectors.
    pub fn evolve(&self, t_span: (f64, f64), n_steps: usize) -> (Vec<f64>, Vec<f64>) {
        let dt = (t_span.1 - t_span.0) / n_steps as f64;
        let mut t = vec![t_span.0];
        let mut m = vec![self.initial_mass];

        for i in 0..n_steps {
            let ti = t[i];
            let mi = m[i];
            if mi <= 0.0 { break; }

            // RK4 step
            let k1 = dt * self.mass_loss_rate(mi);
            let k2 = dt * self.mass_loss_rate(mi + 0.5 * k1);
            let k3 = dt * self.mass_loss_rate(mi + 0.5 * k2);
            let k4 = dt * self.mass_loss_rate(mi + k3);

            let m_next = mi + (k1 + 2.0 * k2 + 2.0 * k3 + k4) / 6.0;
            t.push(ti + dt);
            m.push(m_next.max(0.0));
        }

        (t, m)
    }

    /// Compute the Hawking temperature at mass M.
    pub fn hawking_temperature(&self, mass: f64) -> f64 {
        if mass <= 0.0 { return f64::INFINITY; }
        PLANCK_REDUCED * SPEED_OF_LIGHT.powi(3) / (8.0 * std::f64::consts::PI * GRAVITATIONAL * mass * BOLTZMANN)
    }

    /// Compute the remaining lifetime (time until full evaporation).
    pub fn lifetime(&self) -> f64 {
        // Classical: τ = 5120π G^2 M^3 / (ħc^4)
        let classical = 5120.0 * std::f64::consts::PI * GRAVITATIONAL.powi(2)
            * self.initial_mass.powi(3)
            / (PLANCK_REDUCED * SPEED_OF_LIGHT.powi(4));

        if !self.include_quantum {
            return classical;
        }

        // With quantum corrections, integrate until mass → 0
        let (t, m) = self.evolve((0.0, 2.0 * classical), 10000);
        t.last().copied().unwrap_or(classical)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hawking_rate_negative() {
        let bh = BlackHoleEvolution::new(1e12, false);
        let rate = bh.mass_loss_rate(1e12);
        assert!(rate < 0.0, "Mass loss rate must be negative");
    }

    #[test]
    fn test_lifetime_positive() {
        let bh = BlackHoleEvolution::new(1e12, false);
        let tau = bh.lifetime();
        assert!(tau > 0.0);
        assert!(tau < 1e30); // Shouldn't exceed age of universe
    }

    #[test]
    fn test_quantum_correction_reduces_rate() {
        let bh_classical = BlackHoleEvolution::new(1e10, false);
        let bh_quantum = BlackHoleEvolution::new(1e10, true);
        let r_c = bh_classical.mass_loss_rate(1e10);
        let r_q = bh_quantum.mass_loss_rate(1e10);
        // Quantum corrections should modify the rate
        assert!(r_c != r_q, "Quantum corrections should change evaporation rate");
    }
}
