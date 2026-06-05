// Flux Science — Holographic Theory (AdS/CFT)
//
// Wilson loops, entanglement entropy, holographic bounds.
// Ported from research.md: HolographicTheory class.

use super::constants::*;

/// Holographic theory computations: Wilson loops, entanglement entropy.
pub struct HolographicTheory {
    lambda_t: f64,  // 't Hooft coupling
}

impl HolographicTheory {
    /// Create with default coupling.
    pub fn new() -> Self {
        HolographicTheory { lambda_t: 0.01 }
    }

    /// Create with custom 't Hooft coupling.
    pub fn with_coupling(lambda_t: f64) -> Self {
        HolographicTheory { lambda_t }
    }

    /// Wilson loop expectation value: ⟨W⟩ = exp(-S_NG)
    /// S_NG = λ T r for a rectangular loop of size T × r.
    pub fn wilson_loop(&self, r: f64, t: f64) -> f64 {
        let r_scaled = enforce_length_scale(r);
        let t_scaled = enforce_energy_scale_inverse(t);
        let s_ng = (self.lambda_t * t_scaled * r_scaled).clamp(0.0, 100.0);
        (-s_ng).exp()
    }

    /// Entanglement entropy for a region of size L at depth z.
    /// S = (c/4G) * ln(L/z)  with holographic bound.
    pub fn entanglement_entropy(&self, l: f64, z: f64) -> f64 {
        let l_scaled = enforce_length_scale(l);
        let z_uv = z.max(planck_length()); // UV cutoff at Planck length
        let c = l_scaled / (4.0 * GRAVITATIONAL * z_uv);
        let entropy = c * (l_scaled / z_uv).ln();

        // Enforce holographic bound: S ≤ A/4G
        let area = l_scaled.powi(2);
        let max_entropy = area / (4.0 * GRAVITATIONAL * planck_length().powi(2));
        entropy.min(max_entropy).max(0.0)
    }

    /// Ryu-Takayanagi formula for entanglement entropy in AdS/CFT.
    /// S = Area(γ_A) / 4G  where γ_A is the minimal surface.
    pub fn ryu_takayanagi(&self, boundary_length: f64, bulk_depth: f64) -> f64 {
        let area = boundary_length * bulk_depth;
        area / (4.0 * GRAVITATIONAL)
    }

    /// Holographic bound: maximum entropy in a region of size L.
    /// S_max = A/4l_p² = πL²/l_p²
    pub fn holographic_bound(&self, length: f64) -> f64 {
        std::f64::consts::PI * length.powi(2) / planck_length().powi(2)
    }

    /// C-theorem: central charge decreases along RG flow.
    /// c_UV > c_IR (monotonic decrease).
    pub fn central_charge(&self, scale: f64) -> f64 {
        // c ~ R^3/G in AdS, where R is the AdS radius
        let r_ads = enforce_length_scale(scale);
        r_ads.powi(3) / GRAVITATIONAL
    }

    /// Compute the holographic stress tensor expectation value.
    pub fn stress_tensor_tt(&self, temperature: f64) -> f64 {
        // ⟨T_tt⟩ = (π²/8) N² T⁴  in N=4 SYM
        let n = (self.lambda_t * 10.0).sqrt(); // N ~ √λ
        std::f64::consts::PI.powi(2) / 8.0 * n.powi(2) * temperature.powi(4)
    }

    /// Holographic renormalization: counterterm needed to cancel divergences.
    pub fn counterterm(&self, cutoff: f64) -> f64 {
        let r = enforce_length_scale(cutoff);
        1.0 / (8.0 * std::f64::consts::PI * GRAVITATIONAL * r)
    }
}

impl Default for HolographicTheory {
    fn default() -> Self { Self::new() }
}

// ── Scale Helpers ──

fn enforce_length_scale(length: f64) -> f64 {
    length.clamp(planck_length(), 1e3 * planck_length())
}

fn enforce_energy_scale_inverse(time: f64) -> f64 {
    let t_p = planck_time();
    time.clamp(t_p, 1e10 * t_p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wilson_loop_bounds() {
        let ht = HolographicTheory::new();
        let w = ht.wilson_loop(1e-15, 1.0);
        assert!(w > 0.0 && w <= 1.0, "Wilson loop must be in (0,1]");
    }

    #[test]
    fn test_entropy_positive() {
        let ht = HolographicTheory::new();
        let s = ht.entanglement_entropy(1e-15, 1e-35);
        assert!(s > 0.0, "Entropy must be positive");
    }

    #[test]
    fn test_holographic_bound_large() {
        let ht = HolographicTheory::new();
        let bound = ht.holographic_bound(1e-10);
        assert!(bound > 1e30, "Holographic bound for macroscopic objects should be enormous");
    }

    #[test]
    fn test_stress_tensor_nonzero() {
        let ht = HolographicTheory::with_coupling(1.0);
        let t = ht.stress_tensor_tt(300.0); // Room temperature ~300K
        assert!(t > 0.0);
    }
}
