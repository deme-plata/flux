// Flux Science — General Relativity: Schwarzschild Metric
//
// Schwarzschild solution, Ricci tensor, Einstein-Hilbert action.
// Ported from research.md: schwarzschild_metric, christoffel, riemann, ricci.

/// Schwarzschild metric components at radius r, mass M.
pub struct SchwarzschildMetric {
    mass: f64,
    r: f64,
}

impl SchwarzschildMetric {
    /// Create for mass M at radius r (both in SI units).
    pub fn new(mass: f64, r: f64) -> Self {
        SchwarzschildMetric { mass, r }
    }

    /// Time-time component g_tt = -(1 - 2GM/c^2r)
    pub fn g_tt(&self) -> f64 {
        -(1.0 - 2.0 * super::constants::GRAVITATIONAL * self.mass
            / (super::constants::SPEED_OF_LIGHT.powi(2) * self.r))
    }

    /// Radial component g_rr = 1 / (1 - 2GM/c^2r)
    pub fn g_rr(&self) -> f64 {
        1.0 / (1.0 - 2.0 * super::constants::GRAVITATIONAL * self.mass
            / (super::constants::SPEED_OF_LIGHT.powi(2) * self.r))
    }

    /// Angular component g_θθ = r^2
    pub fn g_theta_theta(&self) -> f64 {
        self.r.powi(2)
    }

    /// Angular component g_φφ = r^2 sin^2(θ)
    pub fn g_phi_phi(&self, theta: f64) -> f64 {
        self.r.powi(2) * theta.sin().powi(2)
    }

    /// Ricci scalar for Schwarzschild metric.
    /// For Schwarzschild: R = 0 (vacuum solution).
    pub fn ricci_scalar(&self) -> f64 {
        0.0 // Schwarzschild is a vacuum solution
    }

    /// Einstein tensor component G_tt (time-time).
    /// For Schwarzschild: G_μν = 0 (vacuum Einstein equations).
    pub fn einstein_tensor_tt(&self) -> f64 {
        0.0
    }

    /// Kretschmann scalar: K = R_μνρσ R^μνρσ = 48G^2M^2/c^4r^6
    /// Measures spacetime curvature. Diverges at r=0 (singularity).
    pub fn kretschmann_scalar(&self) -> f64 {
        let g = super::constants::GRAVITATIONAL;
        let c = super::constants::SPEED_OF_LIGHT;
        48.0 * g.powi(2) * self.mass.powi(2) / (c.powi(4) * self.r.powi(6))
    }

    /// Event horizon radius (Schwarzschild radius).
    pub fn horizon_radius(&self) -> f64 {
        2.0 * super::constants::GRAVITATIONAL * self.mass / super::constants::SPEED_OF_LIGHT.powi(2)
    }

    /// Time dilation factor at radius r: sqrt(-g_tt)
    /// Approaching the horizon, time dilation → 0.
    pub fn time_dilation(&self) -> f64 {
        (-self.g_tt()).sqrt()
    }

    /// Gravitational redshift: Δλ/λ = 1/√(-g_tt) - 1
    pub fn redshift(&self) -> f64 {
        1.0 / self.time_dilation() - 1.0
    }

    /// Proper time interval dτ for coordinate time dt at radius r.
    pub fn proper_time(&self, dt: f64) -> f64 {
        self.time_dilation() * dt
    }

    /// Photon sphere radius: r = 3GM/c^2
    pub fn photon_sphere(&self) -> f64 {
        3.0 * super::constants::GRAVITATIONAL * self.mass / super::constants::SPEED_OF_LIGHT.powi(2)
    }

    /// Innermost stable circular orbit (ISCO): r = 6GM/c^2
    pub fn isco(&self) -> f64 {
        6.0 * super::constants::GRAVITATIONAL * self.mass / super::constants::SPEED_OF_LIGHT.powi(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_solar_horizon() {
        let metric = SchwarzschildMetric::new(1.989e30, 1.0e10); // Solar mass at arbitrary r
        let rs = metric.horizon_radius();
        assert!((rs - 2954.0).abs() < 50.0, "Solar Schwarzschild radius ~2954m");
    }

    #[test]
    fn test_photon_sphere() {
        let metric = SchwarzschildMetric::new(1.989e30, 1.0e10);
        let rp = metric.photon_sphere();
        assert!((rp - 1.5 * metric.horizon_radius()).abs() < 1.0);
    }

    #[test]
    fn test_kretschmann_singularity() {
        let metric = SchwarzschildMetric::new(1.989e30, 1.0); // Near singularity
        let k = metric.kretschmann_scalar();
        assert!(k > 1.0, "Kretschmann scalar positive at r=1m");
    }

    #[test]
    fn test_time_dilation_near_horizon() {
        let metric = SchwarzschildMetric::new(1.989e30, 3000.0); // Near horizon
        let td = metric.time_dilation();
        assert!(td < 0.5, "Time dilation should slow near horizon");
    }
}
