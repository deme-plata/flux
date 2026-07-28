//! flux-qwalk — an independent, *measured* check of the classical baseline in
//! **arXiv:2607.22818**, "Practical advantage beyond the quadratic speedup
//! limit with fully-quantum walks" (Incudini & Mazzola, SISSA, 2026-07-28).
//!
//! # Why this crate exists
//!
//! The paper's headline is a **sixth-degree polynomial** query speedup over the
//! best classical Markov chain for sampling the low-temperature Gibbs
//! distribution of dense Ising models, and a runtime-advantage crossover that
//! falls from ~10³ years to under a day. That headline is not a free-standing
//! claim: it is a **ratio of two fitted exponents**.
//!
//! Every chain in the comparison has a spectral gap that decays exponentially
//! in the number of spins,
//!
//! ```text
//!     δ_β(n) = c_β · 2^(−ν_β · n)
//! ```
//!
//! and the query cost of a walk goes like `1/δ` classically and `1/√δ` for a
//! Szegedy quantization. So the advertised advantage is, in exponent terms,
//!
//! ```text
//!     classical / fully-quantum  =  ν_classical / (ν_Hamiltonian / 2)
//!                                ≈  0.968 / (0.329 / 2)  ≈  5.9   ("sixth-degree")
//! ```
//!
//! The `ν_Hamiltonian` half needs a quantum proposal and cannot be checked here.
//! **`ν_classical` can be — exactly, on a laptop, with no quantum anything** —
//! and it is the denominator under the entire claim. If the classical baseline
//! were softer than reported, the advantage would shrink by the same factor.
//! This crate measures it.
//!
//! # What is measured
//!
//! For Sherrington–Kirkpatrick instances *with fields* (paper §III A, Eqs. 21–22)
//! we build the exact `2^n × 2^n` Metropolis transition matrix for a given
//! proposal, and extract the **absolute spectral gap** `δ = 1 − max_{i≥2}|λ_i|`
//! by full symmetric diagonalization. No sampling, no mixing-time estimator, no
//! autocorrelation heuristic — the gap is an eigenvalue, so we compute the
//! eigenvalue. Then we fit `log2 δ` against `n` over `n = 5..=10`, exactly the
//! window the paper fits, and compare `ν`.
//!
//! # The analytic anchor (the part that makes this falsifiable)
//!
//! The paper's fitted `ν` for the *uniform* proposal marches to 1.000 as β
//! grows (0.968 → 0.991 → 0.999 → 1.000 at β = 4, 10, 20, 40). That is not a
//! fit artifact, and it is worth saying plainly because it reframes the result:
//!
//! The uniform (independence) proposal draws a fresh configuration from the
//! `2^n` possibilities. At low temperature essentially all Gibbs mass sits on
//! one configuration, so leaving it requires *proposing* a specific target,
//! which happens with probability `2^(−n)`. Hence `δ → 2^(−n)`, i.e. **ν → 1
//! exactly**, and the "best classical walk" at low temperature is, in scaling
//! terms, a brute-force scan of configuration space. The quantized walk gets
//! `2^(n/2)` — Grover — and the paper's fully-quantum walk gets `2^(n/6)`.
//!
//! [`uniform_gap_low_temperature_limit`] states this limit, and the test
//! `uniform_nu_approaches_one_at_low_temperature` measures it. If our measured
//! `ν_uniform` did *not* approach 1, either the instrument or the paper would be
//! wrong, and we would know which by the analytic limit.
//!
//! # What this crate does NOT do
//!
//! - It does not verify the quantum side (`ν_Hamiltonian ≈ 0.329`). That needs
//!   Hamiltonian-simulation proposals; nothing here touches them.
//! - It does not check the fault-tolerant compilation or the gate-count model
//!   behind the "less than one day" crossover.
//! - `n ≤ 10` here is not a limitation borrowed from the paper for convenience:
//!   `2^n × 2^n` dense diagonalization is `O(8^n)`. `n = 10` is a 1024×1024
//!   eigenproblem; `n = 14` would be 16384×16384. The paper fits the same
//!   window for the same reason, and that *is* a real caveat on both sides —
//!   an exponent fitted over `n = 5..10` is an extrapolation to `n = 50`.

use nalgebra::{DMatrix, SymmetricEigen};

/// Deterministic PCG32. Reproducibility is the point: every number this crate
/// prints must be regenerable from a seed, so a reader can recompute the table
/// rather than trust it.
pub struct Pcg32 {
    state: u64,
    inc: u64,
}

impl Pcg32 {
    pub fn new(seed: u64) -> Self {
        let mut r = Pcg32 { state: 0, inc: (seed << 1) | 1 };
        r.next_u32();
        r.state = r.state.wrapping_add(seed);
        r.next_u32();
        r
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(6364136223846793005).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Uniform in the open interval (0, 1) — open, so `ln` in Box–Muller is safe.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u32() as f64 + 0.5) / 4_294_967_296.0
    }

    /// Standard normal via Box–Muller.
    pub fn next_gaussian(&mut self) -> f64 {
        let u1 = self.next_f64();
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

/// Which Metropolis proposal move the chain uses. These are the two *classical*
/// families the paper benchmarks; the third (Hamiltonian simulation) is quantum
/// and deliberately absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proposal {
    /// Flip one uniformly chosen spin: `q(y|x) = 1/n` on single-bit neighbours.
    Local,
    /// Draw a fresh configuration uniformly: `q(y|x) = 2^(−n)` everywhere.
    /// This is the best classical move in the low-temperature regime.
    Uniform,
}

/// A Sherrington–Kirkpatrick instance with fields, normalized per Eq. (21).
///
/// `h_i ~ N(0,1)` and `J_ij ~ N(0,1)` are drawn independently, then all
/// coefficients are scaled by `α = sqrt( n / (Σ_i h_i² + Σ_{i<j} J_ij²) )`.
/// That normalization is what gives the model its standard volume scaling: it
/// drives `J̃` to a width of order `1/√n`, so the extensive energy grows like
/// `n` — consistent with the paper's own Fig. 3, which reports an expected
/// operator norm of `n`.
#[derive(Clone, Debug)]
pub struct SkInstance {
    pub n: usize,
    /// Normalized fields `h̃_i`, length `n`.
    pub h: Vec<f64>,
    /// Normalized couplings `J̃`, upper triangle stored row-major as `j[i][k]`
    /// for `k > i`; entries with `k <= i` are zero and unused.
    pub j: Vec<Vec<f64>>,
}

impl SkInstance {
    /// Draw one instance for `n` spins from `rng`.
    pub fn sample(n: usize, rng: &mut Pcg32) -> Self {
        let h_raw: Vec<f64> = (0..n).map(|_| rng.next_gaussian()).collect();
        let mut j_raw = vec![vec![0.0f64; n]; n];
        for i in 0..n {
            for k in (i + 1)..n {
                j_raw[i][k] = rng.next_gaussian();
            }
        }

        let mut sum_sq: f64 = h_raw.iter().map(|v| v * v).sum();
        for i in 0..n {
            for k in (i + 1)..n {
                sum_sq += j_raw[i][k] * j_raw[i][k];
            }
        }
        // Eq. (21). sum_sq > 0 with probability 1; guard only against a
        // pathological all-zero draw rather than silently producing NaN.
        let alpha = if sum_sq > 0.0 { (n as f64 / sum_sq).sqrt() } else { 1.0 };

        let h = h_raw.iter().map(|v| alpha * v).collect();
        let mut j = j_raw;
        for i in 0..n {
            for k in (i + 1)..n {
                j[i][k] *= alpha;
            }
        }
        SkInstance { n, h, j }
    }

    /// Number of configurations, `2^n`.
    pub fn dim(&self) -> usize {
        1usize << self.n
    }

    /// Spin `i` of configuration `x`: bit clear → `+1`, bit set → `−1`.
    #[inline]
    fn spin(x: usize, i: usize) -> f64 {
        if (x >> i) & 1 == 0 {
            1.0
        } else {
            -1.0
        }
    }

    /// Classical Hamiltonian, Eq. (22): `H(x) = −Σ_i h̃_i x_i − Σ_{i<j} J̃_ij x_i x_j`.
    pub fn energy(&self, x: usize) -> f64 {
        let mut e = 0.0;
        for i in 0..self.n {
            let si = Self::spin(x, i);
            e -= self.h[i] * si;
            for k in (i + 1)..self.n {
                e -= self.j[i][k] * si * Self::spin(x, k);
            }
        }
        e
    }

    /// All `2^n` energies, computed once and reused across β.
    pub fn energies(&self) -> Vec<f64> {
        (0..self.dim()).map(|x| self.energy(x)).collect()
    }
}

/// Build the **discriminant matrix** of the Metropolis chain at inverse
/// temperature `beta`.
///
/// For a reversible chain, `D = Π^{1/2} P Π^{-1/2}` is symmetric and shares
/// every eigenvalue with `P`. Writing it out with `π ∝ e^{−βH}` and a symmetric
/// proposal `q`, the off-diagonal entries collapse to a strikingly stable form:
///
/// ```text
///     D_xy = q · exp(−β·|H(y) − H(x)| / 2)          (y ≠ x)
/// ```
///
/// This matters numerically, not just aesthetically. The naive route computes
/// `sqrt(π_x/π_y)`, which at β = 40 overflows long before it produces an
/// answer — the Gibbs weights span hundreds of orders of magnitude. The form
/// above never evaluates anything larger than 1, so the low-temperature regime
/// (exactly the regime the paper's claim lives in) is computed in the same
/// arithmetic as the high-temperature one, with no rescaling tricks.
///
/// The diagonal is fixed by stochasticity: `D_xx = P_xx = 1 − Σ_{y≠x} q·A(x→y)`.
pub fn discriminant(inst: &SkInstance, energies: &[f64], beta: f64, prop: Proposal) -> DMatrix<f64> {
    let dim = inst.dim();
    let mut d = DMatrix::<f64>::zeros(dim, dim);

    let q = match prop {
        Proposal::Local => 1.0 / inst.n as f64,
        Proposal::Uniform => 1.0 / dim as f64,
    };

    for x in 0..dim {
        let mut leave = 0.0; // Σ_{y≠x} q · A(x→y)
        match prop {
            Proposal::Local => {
                for i in 0..inst.n {
                    let y = x ^ (1usize << i);
                    let delta = energies[y] - energies[x];
                    let accept = if delta <= 0.0 { 1.0 } else { (-beta * delta).exp() };
                    leave += q * accept;
                    d[(x, y)] = q * (-beta * delta.abs() / 2.0).exp();
                }
            }
            Proposal::Uniform => {
                for y in 0..dim {
                    if y == x {
                        continue;
                    }
                    let delta = energies[y] - energies[x];
                    let accept = if delta <= 0.0 { 1.0 } else { (-beta * delta).exp() };
                    leave += q * accept;
                    d[(x, y)] = q * (-beta * delta.abs() / 2.0).exp();
                }
            }
        }
        d[(x, x)] = 1.0 - leave;
    }
    d
}

/// The **absolute spectral gap** `δ = 1 − max_{i≥2} |λ_i|`.
///
/// The stationary eigenvalue `λ₁ = 1` is dropped by removing the single
/// eigenvalue closest to 1 — not by dropping the largest, which would silently
/// do the wrong thing if the chain were reducible and had a degenerate unit
/// eigenvalue. `absolute` gap (rather than `1 − λ₂`) is what bounds mixing,
/// because a near `−1` eigenvalue means a near-periodic chain.
pub fn absolute_spectral_gap(d: &DMatrix<f64>) -> f64 {
    let eig = SymmetricEigen::new(d.clone());
    let mut vals: Vec<f64> = eig.eigenvalues.iter().copied().collect();

    let mut drop_at = 0usize;
    let mut best = f64::INFINITY;
    for (i, v) in vals.iter().enumerate() {
        let dist = (v - 1.0).abs();
        if dist < best {
            best = dist;
            drop_at = i;
        }
    }
    vals.remove(drop_at);

    let max_abs = vals.iter().fold(0.0f64, |a, v| a.max(v.abs()));
    1.0 - max_abs
}

/// Convenience: gap for one instance at one β under one proposal.
pub fn gap_for(inst: &SkInstance, beta: f64, prop: Proposal) -> f64 {
    let e = inst.energies();
    let d = discriminant(inst, &e, beta, prop);
    absolute_spectral_gap(&d)
}

/// The low-temperature limit of the uniform-proposal gap: `δ → 2^(−n)`.
///
/// As β → ∞ the Gibbs measure concentrates on the ground state; escaping it
/// requires the independence sampler to *propose* a specific configuration out
/// of `2^n`. So the exponent `ν` in `δ = C·2^(−νn)` tends to exactly 1, and the
/// best classical low-temperature walk is a configuration-space scan in
/// disguise. This is the anchor the measured fit is checked against.
pub fn uniform_gap_low_temperature_limit(n: usize) -> f64 {
    (2.0f64).powi(-(n as i32))
}

/// Least-squares fit of `δ(n) = C · 2^(−ν·n)`, i.e. a straight line through
/// `(n, log2 δ)`. Returns `(log2_c, nu)`. `nu` is negated slope, so a faster
/// decay is a larger `nu` — the paper's convention.
pub fn fit_nu(points: &[(usize, f64)]) -> (f64, f64) {
    let m = points.len() as f64;
    let xs: Vec<f64> = points.iter().map(|(n, _)| *n as f64).collect();
    let ys: Vec<f64> = points.iter().map(|(_, d)| d.log2()).collect();
    let mean_x = xs.iter().sum::<f64>() / m;
    let mean_y = ys.iter().sum::<f64>() / m;
    let mut num = 0.0;
    let mut den = 0.0;
    for i in 0..points.len() {
        num += (xs[i] - mean_x) * (ys[i] - mean_y);
        den += (xs[i] - mean_x) * (xs[i] - mean_x);
    }
    let slope = if den != 0.0 { num / den } else { 0.0 };
    (mean_y - slope * mean_x, -slope)
}

/// Mean gap over `instances` independent draws at size `n`, plus the geometric
/// mean.
///
/// Both are reported because the paper says only "the mean over instances", and
/// which mean you take genuinely moves the fitted exponent: the gap is
/// log-distributed across disorder realizations, so the arithmetic mean is
/// dominated by the least-gapped instances while the geometric mean tracks the
/// typical one. Reporting a single number here would be hiding a judgement call
/// inside an instrument that exists to check someone else's judgement calls.
pub fn gap_statistics(n: usize, beta: f64, prop: Proposal, instances: usize, seed: u64)
    -> (f64, f64)
{
    let mut rng = Pcg32::new(seed);
    let mut arith = 0.0;
    let mut log_sum = 0.0;
    for _ in 0..instances {
        let inst = SkInstance::sample(n, &mut rng);
        let g = gap_for(&inst, beta, prop);
        arith += g;
        log_sum += g.log2();
    }
    let m = instances as f64;
    (arith / m, (log_sum / m).exp2())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The discriminant matrix must be symmetric — if it is not, the chain is
    /// not reversible and every eigenvalue below is meaningless.
    #[test]
    fn discriminant_is_symmetric_for_both_proposals() {
        let mut rng = Pcg32::new(7);
        let inst = SkInstance::sample(6, &mut rng);
        let e = inst.energies();
        for prop in [Proposal::Local, Proposal::Uniform] {
            for beta in [0.5, 4.0, 20.0] {
                let d = discriminant(&inst, &e, beta, prop);
                for x in 0..inst.dim() {
                    for y in 0..inst.dim() {
                        assert!(
                            (d[(x, y)] - d[(y, x)]).abs() < 1e-12,
                            "asymmetry at ({x},{y}) for {prop:?} beta={beta}"
                        );
                    }
                }
            }
        }
    }

    /// The chain is stochastic and stationary: an eigenvalue of exactly 1 must
    /// exist, and nothing may exceed 1 in absolute value.
    #[test]
    fn spectrum_has_unit_eigenvalue_and_is_contractive() {
        let mut rng = Pcg32::new(11);
        let inst = SkInstance::sample(6, &mut rng);
        let e = inst.energies();
        for prop in [Proposal::Local, Proposal::Uniform] {
            let d = discriminant(&inst, &e, 4.0, prop);
            let eig = SymmetricEigen::new(d.clone());
            let vals: Vec<f64> = eig.eigenvalues.iter().copied().collect();
            let max = vals.iter().fold(f64::NEG_INFINITY, |a, v| a.max(*v));
            let max_abs = vals.iter().fold(0.0f64, |a, v| a.max(v.abs()));
            assert!((max - 1.0).abs() < 1e-9, "no unit eigenvalue for {prop:?}: {max}");
            assert!(max_abs <= 1.0 + 1e-9, "spectral radius exceeds 1 for {prop:?}");
        }
    }

    /// Rows of the transition matrix sum to 1 (recovered from `D` via
    /// `P_xy = D_xy · sqrt(π_y/π_x)`, done in log space so β = 20 is safe).
    #[test]
    fn transition_rows_are_normalized() {
        let mut rng = Pcg32::new(3);
        let inst = SkInstance::sample(5, &mut rng);
        let e = inst.energies();
        let beta = 20.0;
        let d = discriminant(&inst, &e, beta, Proposal::Uniform);
        for x in 0..inst.dim() {
            let mut s = 0.0;
            for y in 0..inst.dim() {
                // P_xy = D_xy · exp(−β(H_y − H_x)/2)
                s += d[(x, y)] * (-beta * (e[y] - e[x]) / 2.0).exp();
            }
            assert!((s - 1.0).abs() < 1e-9, "row {x} sums to {s}");
        }
    }

    /// A planted exponent must come back out of the fitter.
    #[test]
    fn fit_recovers_a_planted_exponent() {
        let pts: Vec<(usize, f64)> =
            (5..=10).map(|n| (n, 3.0 * (2.0f64).powf(-0.75 * n as f64))).collect();
        let (log2_c, nu) = fit_nu(&pts);
        assert!((nu - 0.75).abs() < 1e-9, "nu = {nu}");
        assert!((log2_c - 3.0f64.log2()).abs() < 1e-9);
    }

    /// **The anchor.** At low temperature the uniform proposal must escape the
    /// ground state by proposing 1 configuration out of `2^n`, so the gap
    /// approaches `2^(−n)` and the fitted exponent approaches 1 — the paper
    /// reports 0.999 at β = 20. Measured here on small sizes.
    #[test]
    fn uniform_nu_approaches_one_at_low_temperature() {
        let pts: Vec<(usize, f64)> = (4..=8)
            .map(|n| (n, gap_statistics(n, 20.0, Proposal::Uniform, 3, 42 + n as u64).1))
            .collect();
        let (_, nu) = fit_nu(&pts);
        assert!(
            (nu - 1.0).abs() < 0.10,
            "uniform nu at beta=20 should approach 1 (paper: 0.999), measured {nu}"
        );
    }

    /// The gap itself must sit near `2^(−n)` at low temperature, not merely
    /// have the right slope — a slope test alone would pass on a wrong constant.
    #[test]
    fn uniform_gap_matches_the_analytic_low_temperature_limit() {
        let n = 7;
        let (_, geo) = gap_statistics(n, 30.0, Proposal::Uniform, 4, 99);
        let limit = uniform_gap_low_temperature_limit(n);
        let ratio = geo / limit;
        assert!(
            (0.25..=4.0).contains(&ratio),
            "gap {geo:e} should be within a small factor of 2^-n = {limit:e} (ratio {ratio})"
        );
    }

    /// Sanity on the instance generator: normalization must produce the
    /// standard SK volume scaling, i.e. an operator norm growing like `n`
    /// (the paper's Fig. 3 reports exactly this).
    #[test]
    fn normalization_gives_extensive_energy() {
        let mut rng = Pcg32::new(5150);
        for n in [8usize, 10] {
            let mut worst: f64 = 0.0;
            for _ in 0..8 {
                let inst = SkInstance::sample(n, &mut rng);
                let e = inst.energies();
                let m = e.iter().fold(0.0f64, |a, v| a.max(v.abs()));
                worst += m;
            }
            worst /= 8.0;
            let per_spin = worst / n as f64;
            assert!(
                (0.3..=3.0).contains(&per_spin),
                "operator norm should scale like n; got {worst} at n={n} ({per_spin}/spin)"
            );
        }
    }
}
