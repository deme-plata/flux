//! Observables — the quantities that can be checked against a laboratory.
//!
//! Each of these has a published experimental value for liquid argon, which is
//! the point: an MD code that only reports its own energy is unfalsifiable.

use crate::{System, V3};

/// Mean-squared displacement (nm²) from the **unwrapped** coordinates.
///
/// Using wrapped coordinates here is the classic silent bug: atoms teleport
/// across the periodic boundary, the MSD saturates at the box size, and the
/// diffusion coefficient comes out plausibly small and completely wrong.
pub fn msd(reference: &[V3], current: &[V3]) -> f64 {
    debug_assert_eq!(reference.len(), current.len());
    if reference.is_empty() {
        return 0.0;
    }
    let s: f64 = reference
        .iter()
        .zip(current)
        .map(|(r0, r)| r.sub(r0).norm2())
        .sum();
    s / reference.len() as f64
}

/// Self-diffusion coefficient (nm²/ps) from the Einstein relation
/// `MSD(t) = 6 D t`, fitted by least squares through the origin over the
/// supplied `(time_ps, msd_nm2)` samples.
///
/// Callers should discard the early ballistic regime before fitting; the
/// binary does this by starting the fit after equilibration.
pub fn diffusion_coefficient(samples: &[(f64, f64)]) -> f64 {
    let (mut num, mut den) = (0.0, 0.0);
    for &(t, m) in samples {
        num += t * m;
        den += t * t;
    }
    if den == 0.0 {
        return 0.0;
    }
    (num / den) / 6.0
}

/// Convert nm²/ps to cm²/s, the unit experimental tables use.
pub fn nm2_ps_to_cm2_s(d: f64) -> f64 {
    // 1 nm²/ps = 1e-14 cm² / 1e-12 s = 1e-2 cm²/s
    d * 1e-2
}

/// Histogram accumulator for the radial distribution function.
#[derive(Debug, Clone)]
pub struct RdfAccumulator {
    pub bins: Vec<f64>,
    pub r_max: f64,
    pub samples: usize,
}

impl RdfAccumulator {
    pub fn new(n_bins: usize, r_max: f64) -> Self {
        Self { bins: vec![0.0; n_bins], r_max, samples: 0 }
    }

    /// Accumulate all pair separations of the current configuration.
    pub fn sample(&mut self, sys: &System) {
        let n = sys.n();
        let dr = self.r_max / self.bins.len() as f64;
        for i in 0..n {
            for j in (i + 1)..n {
                let r = sys.min_image(sys.pos[i], sys.pos[j]).norm();
                if r < self.r_max {
                    let b = (r / dr) as usize;
                    if b < self.bins.len() {
                        self.bins[b] += 2.0; // i-j and j-i
                    }
                }
            }
        }
        self.samples += 1;
    }
}

/// Normalise the histogram into `g(r)`: pairs observed divided by pairs
/// expected in an ideal gas of the same density.
pub fn rdf(acc: &RdfAccumulator, sys: &System) -> Vec<(f64, f64)> {
    let n_bins = acc.bins.len();
    let dr = acc.r_max / n_bins as f64;
    let rho = sys.number_density();
    let n = sys.n() as f64;
    let mut out = Vec::with_capacity(n_bins);
    for (b, &count) in acc.bins.iter().enumerate() {
        let r_lo = b as f64 * dr;
        let r_hi = r_lo + dr;
        let r_mid = 0.5 * (r_lo + r_hi);
        // Volume of the spherical shell.
        let shell = 4.0 / 3.0 * std::f64::consts::PI * (r_hi.powi(3) - r_lo.powi(3));
        let ideal = shell * rho * n * acc.samples as f64;
        let g = if ideal > 0.0 { count / ideal } else { 0.0 };
        out.push((r_mid, g));
    }
    out
}

/// Published reference values for liquid argon at 94.4 K, 1.374 g/cm³ — the
/// state point Rahman used in the first molecular-dynamics study of a liquid
/// (Phys. Rev. 136, A405, 1964). These are the numbers this crate is graded
/// against; they are *inputs to the test*, never outputs of the program.
pub mod reference {
    /// Self-diffusion coefficient, cm²/s.
    pub const ARGON_D_CM2_S: f64 = 2.43e-5;
    /// Position of the first peak of g(r), nm.
    pub const ARGON_RDF_FIRST_PEAK_NM: f64 = 0.37;
    /// State point.
    pub const ARGON_T_K: f64 = 94.4;
    pub const ARGON_RHO_G_CM3: f64 = 1.374;
}
