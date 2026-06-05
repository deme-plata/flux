// Flux Science — Cosmological Inflation (Starobinsky Model)
//
// Starobinsky R^2 inflation: V(φ) = V₀(1 - e^(-φ/α))²
// Ported from research.md: CosmologicalInflation class.

use super::constants::*;

/// Starobinsky cosmological inflation model.
pub struct CosmologicalInflation {
    v0: f64,        // Potential scale V₀
    alpha: f64,     // Starobinsky parameter α = √(2/3) M_pl
    phi_ini: f64,   // Initial field value
}

impl CosmologicalInflation {
    /// Create with default Starobinsky parameters.
    pub fn new() -> Self {
        let m_pl = planck_mass();
        CosmologicalInflation {
            v0: 1.2e-10 * m_pl.powi(4),
            alpha: (2.0_f64 / 3.0_f64).sqrt() * m_pl,
            phi_ini: 5.5 * m_pl,
        }
    }

    /// Create with custom parameters.
    pub fn with_params(v0: f64, alpha: f64, phi_ini: f64) -> Self {
        CosmologicalInflation { v0, alpha, phi_ini }
    }

    /// Starobinsky potential: V(φ) = V₀(1 - e^(-φ/α))²
    pub fn potential(&self, phi: f64) -> f64 {
        let x = (phi / self.alpha).clamp(-50.0, 50.0);
        self.v0 * (1.0 - (-x).exp()).powi(2)
    }

    /// First derivative V'(φ).
    pub fn potential_derivative(&self, phi: f64) -> f64 {
        let x = (phi / self.alpha).clamp(-50.0, 50.0);
        let exp_term = (-x).exp();
        2.0 * self.v0 * exp_term * (1.0 - exp_term) / self.alpha
    }

    /// Second derivative V''(φ).
    pub fn potential_second_derivative(&self, phi: f64) -> f64 {
        let x = phi / self.alpha;
        let exp_term = (-x).exp();
        2.0 * self.v0 * exp_term * (2.0 * exp_term - 1.0) / self.alpha.powi(2)
    }

    /// Hubble parameter: H = √(V / 3M_pl²)
    pub fn hubble_parameter(&self, phi: f64) -> f64 {
        let v = self.potential(phi).max(1e-100);
        let m_pl = planck_mass();
        (v / (3.0 * m_pl.powi(2))).sqrt()
    }

    /// First slow-roll parameter: ε = (M_pl²/2)(V'/V)²
    pub fn slow_roll_epsilon(&self, phi: f64) -> f64 {
        let m_pl = planck_mass();
        let v = self.potential(phi).max(1e-100);
        let v_prime = self.potential_derivative(phi);
        (m_pl.powi(2) / 2.0) * (v_prime / v).powi(2)
    }

    /// Second slow-roll parameter: η = M_pl² V''/V
    pub fn slow_roll_eta(&self, phi: f64) -> f64 {
        let m_pl = planck_mass();
        let v = self.potential(phi).max(1e-100);
        let v_second = self.potential_second_derivative(phi);
        m_pl.powi(2) * v_second / v
    }

    /// Solve inflation by integrating e-foldings.
    /// Returns (n_folds, phi_values, observables).
    pub fn solve_inflation(&self, n_steps: usize, dn: f64) -> InflationResults {
        let m_pl = planck_mass();
        let mut n = vec![0.0];
        let mut phi = vec![self.phi_ini];

        for _i in 1..n_steps {
            let phi_curr = *phi.last().unwrap();
            let h = self.hubble_parameter(phi_curr);
            let v_prime = self.potential_derivative(phi_curr);

            // Field update from slow-roll: dφ = -(V'/3H²) dN
            let dphi = -(v_prime / (3.0 * h.powi(2))) * dn;
            let phi_next = phi_curr + dphi;
            n.push(n.last().unwrap() + dn);
            phi.push(phi_next);

            let epsilon = self.slow_roll_epsilon(phi_next);
            if epsilon > 1.0 {
                break; // Slow-roll ended
            }
            if phi_next < 0.1 * m_pl {
                break; // Field too small
            }
        }

        // Compute observables at horizon crossing (N* ≈ 55 from end)
        let n_end = *n.last().unwrap();
        let n_star = (n_end - 55.0).max(n[0]);
        let idx_star = n.iter().position(|&ni| ni >= n_star).unwrap_or(n.len() - 1);
        let phi_star = phi[idx_star];

        let epsilon_star = self.slow_roll_epsilon(phi_star);
        let eta_star = self.slow_roll_eta(phi_star);

        // Normalize V₀ to match Planck amplitude A_s = 2.1e-9
        let v_star = self.potential(phi_star);
        let a_s_target = 2.1e-9;
        let a_s_computed = v_star / (24.0 * std::f64::consts::PI.powi(2) * m_pl.powi(4) * epsilon_star.max(1e-100));

        InflationResults {
            n_folds: n_end,
            n_s: 1.0 - 6.0 * epsilon_star + 2.0 * eta_star,
            r: 16.0 * epsilon_star,
            a_s: a_s_target,
            epsilon: epsilon_star,
            eta: eta_star,
            phi_star: phi_star / m_pl,
            scale_factor: a_s_target / a_s_computed.max(1e-100),
        }
    }
}

/// Results from an inflation simulation.
#[derive(Debug, Clone)]
pub struct InflationResults {
    /// Total number of e-foldings.
    pub n_folds: f64,
    /// Scalar spectral index n_s.
    pub n_s: f64,
    /// Tensor-to-scalar ratio r.
    pub r: f64,
    /// Scalar amplitude A_s.
    pub a_s: f64,
    /// Slow-roll parameter ε at horizon crossing.
    pub epsilon: f64,
    /// Slow-roll parameter η at horizon crossing.
    pub eta: f64,
    /// Field value at horizon crossing (in M_pl units).
    pub phi_star: f64,
    /// Scale factor needed to match Planck normalization.
    pub scale_factor: f64,
}

impl Default for CosmologicalInflation {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_potential_positive() {
        let inflation = CosmologicalInflation::new();
        let v = inflation.potential(5.0 * planck_mass());
        assert!(v > 0.0);
        assert!(v < 1e60); // Should be sub-Planckian
    }

    #[test]
    fn test_slow_roll_epsilon_small() {
        let inflation = CosmologicalInflation::new();
        let eps = inflation.slow_roll_epsilon(5.0 * planck_mass());
        assert!(eps < 1.0, "ε should be <1 during inflation");
    }

    #[test]
    fn test_solve_inflation_produces_efolds() {
        let inflation = CosmologicalInflation::new();
        let results = inflation.solve_inflation(1000, 0.1);
        assert!(results.n_folds > 40.0, "Should produce >40 e-folds");
        assert!(results.n_s > 0.9 && results.n_s < 1.0, "n_s should be near 0.96");
        assert!(results.r < 0.1, "r should be small for Starobinsky");
    }
}
