//! The quantum Chinese-remainder clock — arXiv:2608.07938 (Nagar, Hamma, Palmero,
//! Radzihovsky, Yang, Lloyd; MIT-CTP/6088, dated May 2026, v2 11 Aug 2026).
//!
//! **The picture.** An atomic clock is a phase: it knows the time modulo its period but
//! cannot count periods. Take `m` "hands" — truncated oscillators with pairwise-coprime
//! integer periods `x_1 … x_m`, `H_j = ω_j Σ n|n⟩⟨n|`, `ω_j = 2π/x_j` (their Eq. 1). Each hand
//! yields `t_j = t mod x_j`; the Chinese remainder theorem (Sunzi, 3rd–5th c.; Aryabhata's
//! algorithm) recovers `t` uniquely for `t < ∏ x_j` — "the Maya calendar range". Range grows
//! as the PRODUCT of the periods while each hand stays Heisenberg-limited.
//!
//! **The measurement.** With Buzek's optimal initial state (Eq. 5) and Holevo's covariant
//! phase measurement, the measured phase `φ_j` has the density of their Eq. (6),
//!
//! ```text
//! P_opt(φ | t) = 2 sin²(π/2n) cos²(nδ/2) cos²(δ/2) / ( n² sin²((δ+π/n)/2) sin²((δ−π/n)/2) ),
//! δ = φ − ω t,   n = Z·x_j states,
//! ```
//!
//! sharply peaked at `δ = 0` with width `π/n`, so `t̃_j = φ_j/ω_j` has error `Δt ≃ 1/(2Z)`
//! ("increasing phase resolution": keep `ω_j`, multiply the state count by `Z`).
//!
//! **A finding while re-deriving it** (Buzek state `√(2/n) Σ sin(π(k+½)/n)|k⟩` under the
//! Holevo POVM `dφ/2π |φ⟩⟨φ|`, geometric sums): the δ-dependence is exactly as printed, but
//! the printed constant integrates to `2π/n` over `[−π, π]`, not to 1 (π for n = 2, 0.42 for
//! n = 15 — measured by Simpson in the tests). The properly normalised density is
//! `sin²(π/2n) cos²(nδ/2) cos²(δ/2) / (π n sin²((δ+π/n)/2) sin²((δ−π/n)/2))`, i.e. the printed
//! form times `n/2π`. [`phase_density`] uses the normalised one; the shape — and therefore
//! every result of the paper — is unchanged, and the rejection sampler never cared.
//!
//! **The fault-tolerant protocol** (their §Protocol, verbatim here in [`reconstruct`]):
//! (1) measure `t̃_j`; (2) take the fractional parts `r_j`; if `max r − min r < ½` round every
//! `t̃_j` DOWN, otherwise the values straddle an integer boundary ("wrap around") and each is
//! rounded to the NEAREST integer; `r'_j = t̃_j − t'_j`, `r' = mean r'_j`; (3) CRT on the
//! integers `t'_j` gives `t'`; the answer is `t' + r'`. The integer part is right whenever
//! every `|t_j − t̃_j| < ¼`, which by Eq. (7) fails with probability `≃ 32/(3π²Z³)` per hand,
//! hence Eq. (8): `Z ≥ Z_min ≃ (32/(3π²) · 1/(1−p^{1/m}))^{1/3} ≃ (m/ε)^{1/3}`. Final
//! uncertainty `≈ 1/(2Z√m)`.
//!
//! Their Fig. 2 — periods {2,3,5,7,11}, `Z ∈ {1,3,5,7}`, 1000 trials uniform on `[0, 2310)`
//! — reports `P(|error| < 1)` = 0.138 / 0.903 / 0.991 / 0.996. [`figure2`] regenerates it and
//! a test pins the four numbers.
//!
//! **Their surprise** (p. 4, proof of optimality): entangling the hands does NOT help. The
//! optimal measurement is the tensor product of independent phase measurements — the
//! information is in the composition, not in any hand. That is the property the
//! decentralized clock in [`crate::p2p`] is built on.
//!
//! **Redundancy** (this crate's addition, classical RRNS): with more hands than the range
//! needs, a hand that lies is the one whose exclusion brings the reconstruction back inside
//! the known bound — [`reconstruct_bounded`].

use crate::PI;
use serde::{Deserialize, Serialize};

/// One hand of the clock: a truncated oscillator with integer period `period` and `Z·period`
/// states (Z = the paper's truncation-scale multiplier).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hand {
    pub period: u64,
    pub z: u32,
}

impl Hand {
    pub fn new(period: u64, z: u32) -> Self { Hand { period, z: z.max(1) } }
    /// Number of states in the truncated oscillator, `n = Z·x`.
    pub fn states(&self) -> u64 { self.period * self.z as u64 }
    /// `ω = 2π/x`.
    pub fn omega(&self) -> f64 { 2.0 * PI / self.period as f64 }
}

pub fn gcd(a: u128, b: u128) -> u128 { if b == 0 { a } else { gcd(b, a % b) } }

pub fn pairwise_coprime(periods: &[u64]) -> bool {
    for i in 0..periods.len() {
        for j in (i + 1)..periods.len() {
            if gcd(periods[i] as u128, periods[j] as u128) != 1 { return false; }
        }
    }
    true
}

/// The Maya-calendar range `∏ x_j`.
pub fn range(periods: &[u64]) -> u128 { periods.iter().map(|&p| p as u128).product() }

fn ext_gcd(a: i128, b: i128) -> (i128, i128, i128) {
    if b == 0 { (a, 1, 0) } else { let (g, x, y) = ext_gcd(b, a % b); (g, y, x - (a / b) * y) }
}

/// Modular inverse of `a` modulo `m` (coprime required).
pub fn mod_inverse(a: u128, m: u128) -> Option<u128> {
    let (g, x, _) = ext_gcd(a as i128, m as i128);
    if g != 1 { return None; }
    Some(x.rem_euclid(m as i128) as u128)
}

/// Sunzi's theorem: the unique `t < ∏ x_j` with `t ≡ r_j (mod x_j)`. `None` if the moduli are
/// not pairwise coprime or the lengths differ.
pub fn crt(residues: &[u64], periods: &[u64]) -> Option<u128> {
    if residues.len() != periods.len() || periods.is_empty() || !pairwise_coprime(periods) { return None; }
    let m = range(periods);
    let mut t: u128 = 0;
    for (&r, &x) in residues.iter().zip(periods) {
        let mi = m / x as u128;
        let yi = mod_inverse(mi % x as u128, x as u128)?;
        // (r · M_i · y_i) mod M — keep every partial product under M to avoid u128 overflow:
        // r < x, mi·yi < M·x is fine only while M·x fits; do it in steps.
        let term = mulmod(mulmod(r as u128 % m, mi, m), yi, m);
        t = (t + term) % m;
    }
    Some(t)
}

/// `(a·b) mod m` without overflow for `m < 2^127`.
pub fn mulmod(a: u128, b: u128, m: u128) -> u128 {
    let (mut a, mut b, mut r) = (a % m, b % m, 0u128);
    while b > 0 {
        if b & 1 == 1 { r = (r + a) % m; }
        a = (a << 1) % m;
        b >>= 1;
    }
    r
}

/// Result of the fault-tolerant reconstruction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reconstruction {
    /// `t' + r'`.
    pub t: f64,
    pub integer: u128,
    pub fractional: f64,
    /// The fractional parts straddled an integer boundary (nearest-rounding branch).
    pub wrapped: bool,
    /// `max r_j − min r_j` before the branch decision.
    pub spread: f64,
    /// Sample std of the residuals `r'_j` (the unbiased estimator of the paper, Δ).
    pub residual_std: f64,
    /// `Δ/√m`.
    pub uncertainty: f64,
    pub range: u128,
}

/// The paper's protocol, steps (2)–(3). `measured[j] = t̃_j ∈ [0, x_j)`.
pub fn reconstruct(measured: &[f64], periods: &[u64]) -> Result<Reconstruction, String> {
    if measured.len() != periods.len() || measured.is_empty() { return Err("hands and periods differ in length".into()); }
    if !pairwise_coprime(periods) { return Err("periods are not pairwise coprime".into()); }
    let fr: Vec<f64> = measured.iter().map(|v| v - v.floor()).collect();
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for &f in &fr { lo = lo.min(f); hi = hi.max(f); }
    let spread = hi - lo;
    let wrapped = spread > 0.5;
    let mut ints = Vec::with_capacity(measured.len());
    let mut resid = Vec::with_capacity(measured.len());
    for (&v, &x) in measured.iter().zip(periods) {
        let tp = if wrapped { v.round() } else { v.floor() };
        resid.push(v - tp);
        // rounding up may land on x_j: that is 0 mod x_j
        ints.push((tp.max(0.0) as u64) % x);
    }
    let integer = crt(&ints, periods).ok_or("CRT failed")?;
    let m = resid.len() as f64;
    let mean = resid.iter().sum::<f64>() / m;
    let var = if resid.len() > 1 { resid.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (m - 1.0) } else { 0.0 };
    let std = var.sqrt();
    Ok(Reconstruction { t: integer as f64 + mean, integer, fractional: mean, wrapped, spread, residual_std: std, uncertainty: std / m.sqrt(), range: range(periods) })
}

/// Reconstruction with a known bound on the time and redundant hands: if the full CRT lands
/// outside `bound`, every leave-one-out subset whose range still covers `bound` is tried; the
/// hand whose exclusion brings the answer inside the bound is reported as the suspect.
///
/// The check is only as good as the margin between the bound and the subsets' ranges: a
/// subset that still CONTAINS the liar lands inside the bound by accident with probability
/// `≈ bound / range(subset)`, reported as `false_accept_p`. Three hands whose pairwise products
/// barely clear the bound cannot name anyone; four hands near 10^4 against a 10^8 bound leave
/// 10^12-range triples and a false-accept of 10^-4.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundedReconstruction {
    pub reconstruction: Option<Reconstruction>,
    pub consistent: bool,
    /// Index of the hand whose remainder is inconsistent with the others (if identifiable).
    pub suspect: Option<usize>,
    pub bound: u128,
    /// Probability that a subset still containing a liar lands inside the bound by chance.
    pub false_accept_p: f64,
    pub note: String,
}

pub fn reconstruct_bounded(measured: &[f64], periods: &[u64], bound: u128) -> BoundedReconstruction {
    let full = match reconstruct(measured, periods) {
        Ok(r) => r,
        Err(e) => return BoundedReconstruction { reconstruction: None, consistent: false, suspect: None, bound, false_accept_p: 1.0, note: e },
    };
    if full.integer < bound {
        return BoundedReconstruction { reconstruction: Some(full), consistent: true, suspect: None, bound, false_accept_p: 0.0, note: "all hands agree inside the bound".into() };
    }
    let mut inside: Vec<(usize, Reconstruction)> = Vec::new();
    let mut min_range = u128::MAX;
    for skip in 0..periods.len() {
        let p: Vec<u64> = periods.iter().enumerate().filter(|(i, _)| *i != skip).map(|(_, &x)| x).collect();
        let m: Vec<f64> = measured.iter().enumerate().filter(|(i, _)| *i != skip).map(|(_, &v)| v).collect();
        let r_sub = range(&p);
        if r_sub < bound { continue; }
        min_range = min_range.min(r_sub);
        if let Ok(r) = reconstruct(&m, &p) { if r.integer < bound { inside.push((skip, r)); } }
    }
    let fap = if min_range == u128::MAX { 1.0 } else { (bound as f64 / min_range as f64).min(1.0) };
    match inside.len() {
        1 => { let (skip, r) = inside.remove(0); BoundedReconstruction { reconstruction: Some(r), consistent: false, suspect: Some(skip), bound, false_accept_p: fap, note: format!("hand {skip} (period {}) is inconsistent with the others; excluded (false-accept {fap:.1e})", periods[skip]) } }
        0 => BoundedReconstruction { reconstruction: Some(full), consistent: false, suspect: None, bound, false_accept_p: fap, note: "outside the bound and no single hand explains it (need more redundancy)".into() },
        _ => BoundedReconstruction { reconstruction: None, consistent: false, suspect: None, bound, false_accept_p: fap, note: format!("{} different exclusions land inside the bound — ambiguous (false-accept {fap:.1e}); use hands whose subsets dwarf the bound", inside.len()) },
    }
}

// ───────────────────────────── the measurement (Eq. 6) ─────────────────────────────

/// Eq. (6) of the paper for `n = Z·x` states, as a function of `δ = φ − ωt ∈ [−π, π]`,
/// normalised to integrate to 1 (see the module note: the printed constant is off by `2π/n`).
/// The two removable singularities at `δ = ±π/n` evaluate to `n/4π`.
pub fn phase_density(delta: f64, n_states: u64) -> f64 {
    let n = n_states as f64;
    let a = PI / n;
    let dp = (delta + a) / 2.0;
    let dm = (delta - a) / 2.0;
    if dm.abs() < 1e-9 || dp.abs() < 1e-9 { return n / (4.0 * PI); }
    let num = (a / 2.0).sin().powi(2) * (n * delta / 2.0).cos().powi(2) * (delta / 2.0).cos().powi(2);
    let den = PI * n * dp.sin().powi(2) * dm.sin().powi(2);
    num / den
}

/// The printed Eq. (6) constant, kept for the record: `phase_density · 2π/n`.
pub fn phase_density_as_printed(delta: f64, n_states: u64) -> f64 {
    phase_density(delta, n_states) * 2.0 * PI / n_states as f64
}

/// Peak of the density (at `δ = 0`), used as the rejection-sampling bound.
pub fn phase_density_peak(n_states: u64) -> f64 {
    let n = n_states as f64;
    1.0 / (PI * n * (PI / (2.0 * n)).sin().powi(2))
}

/// splitmix64 — deterministic, dependency-free.
#[derive(Debug, Clone)]
pub struct Rng(pub u64);
impl Rng {
    pub fn new(seed: u64) -> Self { Rng(seed) }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `[0, 1)`.
    pub fn f64(&mut self) -> f64 { (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64 }
}

/// Draw `δ` from Eq. (6) by rejection sampling against the uniform proposal on `[−π, π]`.
pub fn sample_phase_error(rng: &mut Rng, n_states: u64) -> f64 {
    let peak = phase_density_peak(n_states) * 1.0001;
    loop {
        let d = (rng.f64() * 2.0 - 1.0) * PI;
        if rng.f64() * peak <= phase_density(d, n_states) { return d; }
    }
}

/// Measure one hand at time `t`: `t̃ = (t + δ/ω) mod x`, `δ` from Eq. (6).
pub fn measure_hand(t: f64, hand: &Hand, rng: &mut Rng) -> f64 {
    let d = sample_phase_error(rng, hand.states());
    let x = hand.period as f64;
    (t + d / hand.omega()).rem_euclid(x)
}

/// Eq. (8): the truncation multiplier for success probability `p` with `m` hands.
pub fn z_min(m: usize, p: f64) -> f64 {
    let m = m as f64;
    (32.0 / (3.0 * PI * PI) / (1.0 - p.powf(1.0 / m))).cbrt()
}

/// The paper's `(m/ε)^{1/3}` shorthand for Eq. (8).
pub fn z_min_approx(m: usize, eps: f64) -> f64 { (m as f64 / eps).cbrt() }

/// Final uncertainty `1/(2Z√m)`.
pub fn uncertainty(z: u32, m: usize) -> f64 { 1.0 / (2.0 * z as f64 * (m as f64).sqrt()) }

/// One Monte-Carlo cell of the paper's Fig. 2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationReport {
    pub periods: Vec<u64>,
    pub z: u32,
    pub trials: u32,
    pub range: u128,
    /// `P(|error| < 1)` — the number printed in each panel of Fig. 2.
    pub p_err_lt_1: f64,
    /// Trials whose INTEGER part was wrong (errors of order the range).
    pub catastrophic: u32,
    /// Std of the error over the non-catastrophic trials.
    pub sigma_fractional: f64,
    /// The paper's prediction for that std, `1/(2Z√m)`.
    pub sigma_predicted: f64,
    pub wrapped_fraction: f64,
    pub seed: u64,
}

pub fn simulate(periods: &[u64], z: u32, trials: u32, seed: u64) -> SimulationReport {
    let hands: Vec<Hand> = periods.iter().map(|&p| Hand::new(p, z)).collect();
    let m = range(periods);
    let mut rng = Rng::new(seed);
    let (mut ok, mut cat, mut wrapped) = (0u32, 0u32, 0u32);
    let mut errs: Vec<f64> = Vec::new();
    for _ in 0..trials {
        let t = rng.f64() * m as f64;
        let measured: Vec<f64> = hands.iter().map(|h| measure_hand(t, h, &mut rng)).collect();
        let r = reconstruct(&measured, periods).expect("coprime by construction");
        if r.wrapped { wrapped += 1; }
        // error on the circle of circumference ∏x
        let mut e = r.t - t;
        let mf = m as f64;
        if e > mf / 2.0 { e -= mf; }
        if e < -mf / 2.0 { e += mf; }
        if e.abs() < 1.0 { ok += 1; errs.push(e); } else { cat += 1; }
    }
    let n = errs.len().max(1) as f64;
    let mean = errs.iter().sum::<f64>() / n;
    let sigma = (errs.iter().map(|e| (e - mean).powi(2)).sum::<f64>() / n).sqrt();
    SimulationReport {
        periods: periods.to_vec(), z, trials, range: m,
        p_err_lt_1: ok as f64 / trials as f64, catastrophic: cat,
        sigma_fractional: sigma, sigma_predicted: uncertainty(z, periods.len()),
        wrapped_fraction: wrapped as f64 / trials as f64, seed,
    }
}

/// The periods of the paper's Fig. 2.
pub const FIGURE2_PERIODS: [u64; 5] = [2, 3, 5, 7, 11];
pub const FIGURE2_Z: [u32; 4] = [1, 3, 5, 7];
/// The four `P(|error|<1)` values printed in Fig. 2.
pub const FIGURE2_P: [f64; 4] = [0.138, 0.903, 0.991, 0.996];

pub fn figure2(seed: u64) -> Vec<SimulationReport> {
    FIGURE2_Z.iter().map(|&z| simulate(&FIGURE2_PERIODS, z, 1000, seed ^ z as u64)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sunzi_by_brute_force() {
        let p = [3u64, 5, 7];
        for t in 0..105u64 {
            let r: Vec<u64> = p.iter().map(|&x| t % x).collect();
            assert_eq!(crt(&r, &p), Some(t as u128));
        }
        assert_eq!(crt(&[1, 1], &[4, 6]), None, "not coprime");
        assert_eq!(range(&FIGURE2_PERIODS), 2310);
        // the footnote: change one remainder by 1 and the answer jumps by the whole range
        assert_eq!(crt(&[1, 3], &[7, 5]), Some(8));
        assert_eq!(crt(&[1, 4], &[7, 5]), Some(29));
    }

    #[test]
    fn eq6_is_a_normalised_density_with_the_stated_width() {
        for &n in &[2u64, 6, 15, 55, 77] {
            // Simpson over [−π, π]
            let steps = 200_000;
            let h = 2.0 * PI / steps as f64;
            let mut s = 0.0;
            for i in 0..=steps {
                let d = -PI + i as f64 * h;
                let w = if i == 0 || i == steps { 1.0 } else if i % 2 == 1 { 4.0 } else { 2.0 };
                s += w * phase_density(d, n);
            }
            let integral = s * h / 3.0;
            assert!((integral - 1.0).abs() < 2e-3, "n={n}: ∫P = {integral}");
            // the printed constant integrates to 2π/n instead
            let printed = integral * 2.0 * PI / n as f64;
            assert!((printed - 2.0 * PI / n as f64).abs() < 5e-3, "n={n}: printed ∫ = {printed}");
            assert!((phase_density(PI / n as f64, n) - n as f64 / (4.0 * PI)).abs() < 1e-6);
            assert!((phase_density_as_printed(PI / n as f64, n) - 0.5).abs() < 1e-6);
            let peak = phase_density_peak(n);
            assert!((phase_density(0.0, n) - peak).abs() < 1e-9);
            assert!(phase_density(0.3, n) <= peak * 1.0001);
        }
    }

    #[test]
    fn measurement_error_scales_as_one_over_2z() {
        let mut rng = Rng::new(7);
        for &z in &[1u32, 3, 7] {
            let hand = Hand::new(11, z);
            let n = 4000;
            let mut acc = 0.0;
            for _ in 0..n {
                let t = 5.0;
                let mut e = measure_hand(t, &hand, &mut rng) - t;
                if e > 5.5 { e -= 11.0; }
                if e < -5.5 { e += 11.0; }
                acc += e * e;
            }
            let sigma = (acc / n as f64).sqrt();
            // the paper: Δt_j ≃ 1/(2Z) (the density has heavier tails than a Gaussian, so
            // the raw std sits above the ¼-width figure; the ORDER must match)
            let pred = 1.0 / (2.0 * z as f64);
            assert!(sigma > 0.5 * pred && sigma < 2.5 * pred, "Z={z}: σ={sigma} vs {pred}");
        }
    }

    #[test]
    fn figure2_is_reproduced() {
        let reps = figure2(20260811);
        let tol = [0.06, 0.05, 0.02, 0.015];
        for (rep, (&p, &tl)) in reps.iter().zip(FIGURE2_P.iter().zip(tol.iter())) {
            assert!((rep.p_err_lt_1 - p).abs() <= tl, "Z={}: P(|err|<1) = {} vs paper {}", rep.z, rep.p_err_lt_1, p);
            assert_eq!(rep.range, 2310);
        }
        // Z = 1 fails mostly by catastrophic integer errors; Z = 7 almost never.
        assert!(reps[0].catastrophic > 700);
        assert!(reps[3].catastrophic < 15);
    }

    #[test]
    fn eq8_z_min() {
        // m = 5, ε = 1 %: exact 8.1, shorthand (m/ε)^{1/3} = 7.9
        assert!((z_min(5, 0.99) - 8.13).abs() < 0.05, "{}", z_min(5, 0.99));
        assert!((z_min_approx(5, 0.01) - 7.94).abs() < 0.02);
        assert!((uncertainty(5, 5) - 0.0447).abs() < 1e-3);
    }

    #[test]
    fn wrap_around_is_handled() {
        // true t = 100.9 on periods {7, 11, 13}; errors of ±0.15 straddle 101
        let p = [7u64, 11, 13];
        let t: f64 = 100.9;
        let m: Vec<f64> = p.iter().zip([0.15f64, -0.1, 0.12]).map(|(&x, e)| (t + e).rem_euclid(x as f64)).collect();
        let r = reconstruct(&m, &p).unwrap();
        assert!(r.wrapped);
        assert!((r.t - t).abs() < 0.2, "{}", r.t);
        assert_eq!(r.integer, 101);
    }

    #[test]
    fn a_lying_hand_is_identified_with_redundancy() {
        // heights are below 1e8; four hands near 1e4: any three multiply to ~1e12, so one
        // hand is redundant and a liar is named with false-accept ≈ 1e-4
        let p = [10_007u64, 10_009, 10_037, 10_039];
        let h: u64 = 19_040_287;
        let mut m: Vec<f64> = p.iter().map(|&x| (h % x) as f64).collect();
        let ok = reconstruct_bounded(&m, &p, 100_000_000);
        assert!(ok.consistent);
        assert_eq!(ok.reconstruction.unwrap().integer, h as u128);
        m[1] = ((h % p[1]) as f64 + 3.0) % p[1] as f64; // a follower 3 blocks off
        let bad = reconstruct_bounded(&m, &p, 100_000_000);
        assert!(!bad.consistent);
        assert_eq!(bad.suspect, Some(1), "{}", bad.note);
        assert!(bad.false_accept_p < 2e-4, "{}", bad.false_accept_p);
        assert_eq!(bad.reconstruction.unwrap().integer, h as u128);
        // three hands that barely clear the bound cannot name anyone — and say so
        let p3 = [10_007u64, 10_009, 10_037];
        let mut m3: Vec<f64> = p3.iter().map(|&x| (h % x) as f64).collect();
        m3[1] += 3.0;
        let weak = reconstruct_bounded(&m3, &p3, 100_000_000);
        assert!(!weak.consistent);
        assert!(weak.false_accept_p > 0.9, "{}", weak.false_accept_p);
    }
}
