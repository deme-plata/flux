//! Antimatter and gravity, 2022–2026, recomputed from the CODATA 2022 listing.
//!
//! **CODATA 2022 has no antimatter rows.** Every antiparticle number below is a matter row
//! carried over by CPT symmetry (same mass, same magnitude of magnetic moment, opposite
//! charge). The experiments are tests of exactly that carry-over, and this module asks of each
//! one: what did it print, can the printed numbers be rebuilt from CODATA, and what limits it.
//!
//! 1. **ALPHA-g (direct free fall).** The only direct measurement of gravity on neutral
//!    antimatter: a = (0.75 ± 0.13 ± 0.16) g, repulsion excluded. We rebuild the magnetic
//!    "bias" scale (1 g ≡ 1.77×10⁻³ T/m) from `m_H g/μ_B`, refit the published raw counts
//!    (Table 1) with the paper's own efficiency calibration, and show that counting statistics
//!    (≈0.06 g) are not what holds the result back: the systematics are.
//! 2. **The trapped states' magnetic moments.** The bias is defined with μ_B, but the two
//!    trapped hyperfine states move with μₑ ∓ μₚ (Breit–Rabi). Negligible at 20 %; at the 1 % goal
//!    the 0.18 % difference between them is about an eighth of the error budget.
//! 3. **BASE (antiproton clock).** q/m of p̄ vs p to 16 ppt; recomputing the H⁻ mass ratio from
//!    CODATA 2022 and the two gravity bounds (1.8×10⁻⁷ absolute, 0.030 annual).
//! 4. **GBAR.** Fall times and the gravitational quantum states it plans to use.
//!
//! Sources (fetched 2026-11-16; text extracted with pdftotext):
//! * ALPHA, "Observation of the effect of gravity on the motion of antimatter",
//!   Nature 621, 716–722 (2023), doi:10.1038/s41586-023-06527-1 — Table 1, Tables 2–3, eq. (1).
//! * BASE, "A 16 parts per trillion comparison of the antiproton-to-proton q/m ratios",
//!   Nature 601, 53–57 (2022), arXiv:2311.16006 — eqs. (3)–(13).
//! * R. Caravita et al., "CERN AD/ELENA Antimatter Program", arXiv:2503.22471 (2025) — §3.2
//!   (2023 result reproduced in the 2024 run; ≤1 % possible in the existing machine), §3.3
//!   (mean energy ~1 mK), §6 (GBAR: anti-ion not yet produced; 10⁻⁵ goal).
//! * ALPHA, "Be⁺ assisted, simultaneous confinement of more than 15000 antihydrogen atoms",
//!   Nat. Commun. 16, 10106 (2025) — in ALPHA-g, 2024: accumulation rate up more than 20×.
//! * CERN news, 27 May 2026: antihydrogen ground-state hyperfine splitting to 4 ppm (Nature).

use crate::electromagnetism::{si2019, CodataTable};
use si2019::{C, E, H};
use std::f64::consts::PI;

/// Inputs that are NOT in the CODATA table, each with where it was read.
pub mod external {
    /// Hydrogen ground-state hyperfine frequency (Hz), hydrogen-maser value. Antihydrogen's is
    /// assumed equal (CPT); ALPHA measured it to 4 ppm in 2026, far finer than needed here.
    pub const NU_HF_HYDROGEN: f64 = 1_420_405_751.768;
    /// Electron affinity of H (eV), Lykke & Lineberger, as used by BASE eq. (10).
    pub const H_ELECTRON_AFFINITY_EV: f64 = 0.754_195;
    /// BASE eq. (12): Penning-trap polarizability shift α_{H⁻}B²/m_p = 7.689(18) ppt.
    pub const BASE_POLARIZABILITY_SHIFT: f64 = 7.689e-12;
    /// BASE eq. (8): 2mₑ/mₚ as BASE used it (their own weighted mean, not CODATA's).
    pub const BASE_TWO_ME_MP: f64 = 0.001_089_234_042_99;
    /// BASE eq. (9): Bₑ/mₚ (NIST ASD hydrogen binding energy).
    pub const BASE_BE_MP: f64 = 0.000_000_014_493_06;
    /// BASE eq. (10): Aₑ/mₚ.
    pub const BASE_AE_MP: f64 = 0.000_000_000_803_81;
    /// BASE eq. (13): theoretical proton-to-H⁻ cyclotron frequency ratio.
    pub const BASE_R_THEO: f64 = 1.001_089_218_753_80;
    /// BASE eq. (3): −(q/m)_p/(q/m)_p̄ − 1 = 3(16) ×10⁻¹².
    pub const BASE_R_EXP_MINUS_1: f64 = 3e-12;
    pub const BASE_R_SIGMA: f64 = 16e-12;
    /// BASE: Φ/c² of the local supercluster, used for the absolute bound.
    pub const BASE_SUPERCLUSTER_PHI_C2: f64 = 2.99e-5;
    /// BASE's printed bounds: |α_g − 1| < 1.8×10⁻⁷ (absolute), |α_g,D − 1| < 0.030 (annual, 68 % C.L.).
    pub const BASE_BOUND_ABSOLUTE: f64 = 1.8e-7;
    pub const BASE_BOUND_DIFFERENTIAL: f64 = 0.030;
    /// BASE: fraction of the annual peak-to-peak potential swing the data cover.
    pub const BASE_COVERAGE: f64 = 0.80;
    /// Heliocentric gravitational constant GM_sun (m³ s⁻², DE430/TDB). Known to ~10⁻¹⁰ from
    /// planetary ranging, so CODATA's G (22 ppm) never enters.
    pub const GM_SUN: f64 = 1.327_124_400_41e20;
    /// Astronomical unit (m), exact by IAU 2012 Resolution B2.
    pub const AU: f64 = 149_597_870_700.0;
    /// Earth's orbital eccentricity (J2000). BASE rounds it to 0.017.
    pub const EARTH_ECCENTRICITY: f64 = 0.016_71;
    /// ALPHA-g paper: "g = 9.81 m s⁻²", mirror peaks 25.6 cm apart, 1 T background solenoid.
    pub const ALPHA_G_LOCAL_G: f64 = 9.81;
    pub const ALPHA_G_PEAK_SEPARATION_M: f64 = 0.256;
    pub const ALPHA_G_BACKGROUND_FIELD_T: f64 = 1.0;
    /// ALPHA-g's printed scale: g ≡ 1.77×10⁻³ T/m; 4.53×10⁻⁴ T across the mirror peaks.
    pub const ALPHA_G_PRINTED_GRADIENT: f64 = 1.77e-3;
    pub const ALPHA_G_PRINTED_FIELD_DIFF: f64 = 4.53e-4;
    /// ALPHA-g result: a/g = 0.75 ± 0.13 (statistical + systematic) ± 0.16 (simulation).
    pub const ALPHA_G_RESULT: (f64, f64, f64) = (0.75, 0.13, 0.16);
    /// ALPHA-g: probability of a result at least this extreme with no gravity.
    pub const ALPHA_G_P_NO_GRAVITY: f64 = 2.9e-4;
    /// ALPHA-g Table 3 items: finite data size, detector-efficiency calibration, field model.
    pub const ALPHA_G_STAT_ITEM: f64 = 0.06;
    pub const ALPHA_G_EFFICIENCY_ITEM: f64 = 0.12;
    pub const ALPHA_G_FIELD_MODEL_ITEM: f64 = 0.16;
    /// AD/ELENA overview §3.3: laser-cooled trapped antihydrogen, mean energy "of order 1 mK".
    pub const ALPHA_COOLED_ENERGY_K: f64 = 1e-3;
    /// Nat. Commun. 16, 10106: Be⁺ assisted synthesis in ALPHA-g (2024), rate gain "more than 20".
    pub const ALPHA_G_RATE_GAIN_2024: f64 = 20.0;
    /// First two zeros of the Airy function Ai(−z) (gravitational quantum states).
    pub const AIRY_ZEROS: [f64; 2] = [2.338_107_410_459_767, 4.087_949_444_130_970];
}
use external as ext;

/// How many CODATA rows name an antiparticle. (2022: none.)
pub fn antimatter_rows(t: &CodataTable) -> usize {
    t.constants
        .iter()
        .filter(|c| {
            let n = c.name.to_lowercase();
            n.contains("anti") || n.contains("positron")
        })
        .count()
}

/// The CODATA matter numbers that every antimatter quantity here is mirrored from.
#[derive(Clone, Debug)]
pub struct Matter {
    pub year: u16,
    pub m_p: f64,
    pub m_e: f64,
    pub me_over_mp: f64,
    pub mu_b: f64,
    /// |μₑ|/μ_B (the positron's, by CPT).
    pub mu_e_over_mu_b: f64,
    /// μₚ/μ_B (the antiproton's magnitude, by CPT).
    pub mu_p_over_mu_b: f64,
    /// mₚc² in eV.
    pub mp_c2_ev: f64,
    pub ryd_hc_ev: f64,
    pub alpha: f64,
    pub k_b: f64,
    /// Standard gravity gₙ (exact, conventional).
    pub g_n: f64,
    pub big_g: f64,
    pub big_g_rel_sigma: f64,
}

impl Matter {
    pub fn from_table(t: &CodataTable) -> Self {
        let v = |n: &str| t.must(n).value;
        Self {
            year: t.year,
            m_p: v("proton mass"),
            m_e: v("electron mass"),
            me_over_mp: v("electron-proton mass ratio"),
            mu_b: v("Bohr magneton"),
            mu_e_over_mu_b: v("electron mag. mom. to Bohr magneton ratio").abs(),
            mu_p_over_mu_b: v("proton mag. mom. to Bohr magneton ratio"),
            mp_c2_ev: v("proton mass energy equivalent in MeV") * 1e6,
            ryd_hc_ev: v("Rydberg constant times hc in eV"),
            alpha: 1.0 / t.alpha_inverse().value,
            k_b: v("Boltzmann constant"),
            g_n: v("standard acceleration of gravity"),
            big_g: v("Newtonian constant of gravitation"),
            big_g_rel_sigma: t.must("Newtonian constant of gravitation").rel_sigma(),
        }
    }

    pub fn codata_2022() -> Self {
        Self::from_table(&CodataTable::nist_2022())
    }

    /// (Anti)hydrogen 1S binding energy (eV): R∞hc·μ/mₑ with the leading Dirac term (1+α²/4).
    /// It omits the 1S Lamb shift (≈3.4×10⁻⁵ eV), so it sits ≈2.5 ppm above NIST's value.
    pub fn hydrogen_binding_ev(&self) -> f64 {
        self.ryd_hc_ev / (1.0 + self.me_over_mp) * (1.0 + self.alpha * self.alpha / 4.0)
    }

    /// (Anti)hydrogen atom mass (kg): mₚ + mₑ − B/c².
    pub fn hydrogen_mass(&self) -> f64 {
        self.m_p + self.m_e - self.hydrogen_binding_ev() * E / (C * C)
    }
}

// ── ALPHA-g: the scale of the experiment ─────────────────────────────────────────────────

/// Vertical field gradient that mimics `g` on a ground-state (anti)hydrogen atom (T/m):
/// μ_B ∂B/∂z = m_H g.
pub fn bias_gradient(m: &Matter, g: f64) -> f64 {
    m.hydrogen_mass() * g / m.mu_b
}

/// Field difference between two mirror peaks `sep` metres apart that mimics `g` (T).
pub fn mirror_difference(m: &Matter, g: f64, sep: f64) -> f64 {
    bias_gradient(m, g) * sep
}

/// Electric field whose force on an (anti)proton equals its weight (V/m).
pub fn equivalent_electric_field(m: &Matter, g: f64) -> f64 {
    m.m_p * g / E
}

/// Magnetic field whose Lorentz force on an (anti)proton moving at its thermal speed
/// √(3kT/m) equals its weight (T).
pub fn equivalent_magnetic_field(m: &Matter, g: f64, temp_k: f64) -> f64 {
    let v = (3.0 * m.k_b * temp_k / m.m_p).sqrt();
    m.m_p * g / (E * v)
}

/// Gravitational energy of an (anti)hydrogen atom over `height`, in kelvin.
pub fn gravity_step_kelvin(m: &Matter, g: f64, height: f64) -> f64 {
    m.hydrogen_mass() * g * height / m.k_b
}

// ── The trapped states' magnetic moments (Breit–Rabi) ────────────────────────────────────

/// dE/dB of the two low-field-seeking ground states, in units of μ_B, at field `b`.
#[derive(Clone, Copy, Debug)]
pub struct TrappedMoments {
    pub field_t: f64,
    /// Breit–Rabi field parameter x = 2(μₑ+μₚ)B/(hν_hf).
    pub x: f64,
    /// |d⟩ (F=1, m_F=+1): a pure state, slope μₑ − μₚ at every field.
    pub d: f64,
    /// |c⟩ (F=1, m_F=0): slope (μₑ+μₚ)·x/√(1+x²), reaching μₑ+μₚ only at high field.
    pub c: f64,
}

impl TrappedMoments {
    /// Equal-population mean.
    pub fn mean(&self) -> f64 {
        0.5 * (self.c + self.d)
    }
    pub fn spread(&self) -> f64 {
        (self.c - self.d).abs()
    }
}

/// Free-particle moments; the bound-state correction to g (≈α²/3 ≈ 1.8×10⁻⁵) is ignored.
pub fn trapped_moments(m: &Matter, b: f64, nu_hf: f64) -> TrappedMoments {
    let sum = m.mu_e_over_mu_b + m.mu_p_over_mu_b;
    let x = 2.0 * sum * m.mu_b * b / (H * nu_hf);
    TrappedMoments { field_t: b, x, d: m.mu_e_over_mu_b - m.mu_p_over_mu_b, c: sum * x / (1.0 + x * x).sqrt() }
}

// ── ALPHA-g: the published counts ────────────────────────────────────────────────────────

/// One row of ALPHA-g's Table 1 (background-subtracted, NOT efficiency-corrected).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReleaseTrial {
    /// Nominal bias label (g). The paper: labels, not the derived biases of its Fig. 5.
    pub bias: f64,
    pub trials: u8,
    pub n_up: f64,
    pub n_dn: f64,
    /// Events during the long-octupole ramp-down (used to normalise efficiencies).
    pub n_loc: f64,
}

const fn row(bias: f64, trials: u8, n_up: f64, n_dn: f64, n_loc: f64) -> ReleaseTrial {
    ReleaseTrial { bias, trials, n_up, n_dn, n_loc }
}

/// ALPHA-g 2023, Table 1, the eleven gravity trials (20 s ramp-down).
pub const ALPHA_G_2023_TABLE1: [ReleaseTrial; 11] = [
    row(-3.0, 7, 151.7, 16.5, 199.2),
    row(-2.0, 7, 128.7, 33.5, 168.2),
    row(-1.5, 6, 128.9, 57.7, 192.0),
    row(-1.0, 7, 69.7, 62.5, 183.2),
    row(-0.5, 7, 55.7, 67.5, 201.2),
    row(0.0, 7, 36.7, 94.5, 144.2),
    row(0.5, 7, 36.7, 124.5, 177.2),
    row(1.0, 7, 17.7, 119.5, 185.2),
    row(1.5, 6, 13.9, 180.7, 234.0),
    row(2.0, 7, 6.7, 163.5, 228.2),
    row(3.0, 7, 7.7, 147.5, 199.2),
];
/// The −10 g calibration (everything leaves through the top).
pub const ALPHA_G_2023_CAL_UP: ReleaseTrial = row(-10.0, 6, 142.9, 0.7, 169.0);
/// The +10 g calibration (everything leaves through the bottom).
pub const ALPHA_G_2023_CAL_DN: ReleaseTrial = row(10.0, 6, -0.1, 185.7, 213.0);

/// Up and down detection efficiencies relative to the octupole-ramp count, with binomial errors.
#[derive(Clone, Copy, Debug)]
pub struct Efficiency {
    pub up: f64,
    pub up_sigma: f64,
    pub dn: f64,
    pub dn_sigma: f64,
}

impl Efficiency {
    /// κ = ε_dn/ε_up.
    pub fn ratio(&self) -> f64 {
        self.dn / self.up
    }
    pub fn ratio_rel_sigma(&self) -> f64 {
        ((self.up_sigma / self.up).powi(2) + (self.dn_sigma / self.dn).powi(2)).sqrt()
    }
}

pub fn efficiency_2023() -> Efficiency {
    let e = |n: f64, tot: f64| {
        let p = n / tot;
        (p, (p * (1.0 - p) / tot).sqrt())
    };
    let (up, up_sigma) = e(ALPHA_G_2023_CAL_UP.n_up, ALPHA_G_2023_CAL_UP.n_loc);
    let (dn, dn_sigma) = e(ALPHA_G_2023_CAL_DN.n_dn, ALPHA_G_2023_CAL_DN.n_loc);
    Efficiency { up, up_sigma, dn, dn_sigma }
}

/// Efficiency-corrected probability to leave through the bottom.
pub fn p_down(t: &ReleaseTrial, kappa: f64) -> f64 {
    let dn = t.n_dn / kappa;
    dn / (dn + t.n_up)
}

/// Shape of the escape curve P_dn(bias). The paper takes the shape from simulation; we try two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Logistic,
    Probit,
}

/// erfc(x), Numerical Recipes `erfcc` (fractional error < 1.2×10⁻⁷).
pub fn erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let poly = -1.265_512_23
        + t * (1.000_023_68
            + t * (0.374_091_96
                + t * (0.096_784_18
                    + t * (-0.186_288_06
                        + t * (0.278_868_07
                            + t * (-1.135_203_98 + t * (1.488_515_87 + t * (-0.822_152_23 + t * 0.170_872_77))))))));
    let ans = t * (-z * z + poly).exp();
    if x >= 0.0 {
        ans
    } else {
        2.0 - ans
    }
}

impl Shape {
    /// Cumulative F(η) and density f(η) = F′(η).
    fn cdf_pdf(self, eta: f64) -> (f64, f64) {
        match self {
            Shape::Logistic => {
                let f = 1.0 / (1.0 + (-eta).exp());
                (f, f * (1.0 - f))
            }
            Shape::Probit => (0.5 * erfc(-eta / 2f64.sqrt()), (-0.5 * eta * eta).exp() / (2.0 * PI).sqrt()),
        }
    }
}

/// Maximum-likelihood escape curve P_dn = F(β₀ + β₁·bias) on efficiency-corrected counts.
#[derive(Clone, Copy, Debug)]
pub struct EscapeFit {
    pub shape: Shape,
    pub kappa: f64,
    pub beta0: f64,
    pub beta1: f64,
    /// Bias at which P_dn = ½ (g). In the paper's one-dimensional picture, gravity a sits at −a.
    pub balance: f64,
    /// Counting-statistics (Fisher) error on `balance`.
    pub balance_sigma: f64,
    /// Total efficiency-corrected events used.
    pub events: f64,
    pub nll: f64,
}

impl EscapeFit {
    /// The naive acceleration a/g = −balance.
    pub fn naive_acceleration(&self) -> f64 {
        -self.balance
    }
}

fn counts(kappa: f64) -> Vec<(f64, f64, f64)> {
    ALPHA_G_2023_TABLE1.iter().map(|t| (t.bias, t.n_dn / kappa, t.n_up)).collect()
}

fn nll(shape: Shape, data: &[(f64, f64, f64)], b0: f64, b1: f64) -> f64 {
    data.iter()
        .map(|&(x, s, f)| {
            let (p, _) = shape.cdf_pdf(b0 + b1 * x);
            let p = p.clamp(1e-300, 1.0 - 1e-16);
            -(s * p.ln() + f * (1.0 - p).ln())
        })
        .sum()
}

/// Fisher scoring. Converges in a handful of steps for both shapes.
pub fn fit_escape(shape: Shape, kappa: f64) -> EscapeFit {
    let data = counts(kappa);
    let (mut b0, mut b1) = (0.5, 1.0);
    let mut info = [[0.0; 2]; 2];
    for _ in 0..200 {
        let mut score = [0.0; 2];
        info = [[0.0; 2]; 2];
        for &(x, s, f) in &data {
            let (p, d) = shape.cdf_pdf(b0 + b1 * x);
            let n = s + f;
            let v = p * (1.0 - p);
            let u = (s - n * p) * d / v;
            let w = n * d * d / v;
            score[0] += u;
            score[1] += u * x;
            info[0][0] += w;
            info[0][1] += w * x;
            info[1][1] += w * x * x;
        }
        info[1][0] = info[0][1];
        let det = info[0][0] * info[1][1] - info[0][1] * info[1][0];
        let d0 = (info[1][1] * score[0] - info[0][1] * score[1]) / det;
        let d1 = (-info[1][0] * score[0] + info[0][0] * score[1]) / det;
        b0 += d0;
        b1 += d1;
        if d0.abs() < 1e-13 && d1.abs() < 1e-13 {
            break;
        }
    }
    let det = info[0][0] * info[1][1] - info[0][1] * info[1][0];
    let cov = [[info[1][1] / det, -info[0][1] / det], [-info[1][0] / det, info[0][0] / det]];
    let j = [-1.0 / b1, b0 / (b1 * b1)];
    let var = j[0] * j[0] * cov[0][0] + 2.0 * j[0] * j[1] * cov[0][1] + j[1] * j[1] * cov[1][1];
    EscapeFit {
        shape,
        kappa,
        beta0: b0,
        beta1: b1,
        balance: -b0 / b1,
        balance_sigma: var.sqrt(),
        events: data.iter().map(|&(_, s, f)| s + f).sum(),
        nll: nll(shape, &data, b0, b1),
    }
}

/// Counting-statistics significance against a curve whose balance sits at `balance`
/// (slope re-fitted): z = √(2 ΔNLL). Systematics are NOT included.
pub fn stat_only_z(fit: &EscapeFit, balance: f64) -> f64 {
    let data = counts(fit.kappa);
    let f = |b1: f64| nll(fit.shape, &data, -balance * b1, b1);
    // Golden-section search on the slope.
    let (mut lo, mut hi) = (0.01, 20.0);
    let r = (5f64.sqrt() - 1.0) / 2.0;
    for _ in 0..200 {
        let a = hi - r * (hi - lo);
        let b = lo + r * (hi - lo);
        if f(a) < f(b) {
            hi = b;
        } else {
            lo = a;
        }
    }
    (2.0 * (f(0.5 * (lo + hi)) - fit.nll)).max(0.0).sqrt()
}

/// Shift of the balance point when κ moves by ±1σ (half the difference), counting only.
pub fn efficiency_shift(shape: Shape) -> f64 {
    let e = efficiency_2023();
    let k = e.ratio();
    let s = e.ratio_rel_sigma();
    0.5 * (fit_escape(shape, k * (1.0 + s)).balance - fit_escape(shape, k * (1.0 - s)).balance).abs()
}

/// Events needed for a counting error of `target` (g), same curve shape.
pub fn events_for(fit: &EscapeFit, target: f64) -> f64 {
    fit.events * (fit.balance_sigma / target).powi(2)
}

// ── BASE: the antiproton as a clock ──────────────────────────────────────────────────────

/// The terms of BASE's eq. (7)/(13), recomputed.
#[derive(Clone, Copy, Debug)]
pub struct HMinusRatio {
    pub two_me_mp: f64,
    pub be_mp: f64,
    pub ae_mp: f64,
    pub polarizability: f64,
    /// R = m_{H⁻}/m_p = ν_p/ν_{H⁻}.
    pub r: f64,
}

pub fn hminus_ratio(m: &Matter) -> HMinusRatio {
    let two_me_mp = 2.0 * m.me_over_mp;
    let be_mp = m.hydrogen_binding_ev() / m.mp_c2_ev;
    let ae_mp = ext::H_ELECTRON_AFFINITY_EV / m.mp_c2_ev;
    let polarizability = ext::BASE_POLARIZABILITY_SHIFT;
    HMinusRatio { two_me_mp, be_mp, ae_mp, polarizability, r: 1.0 + two_me_mp - be_mp - ae_mp + polarizability }
}

/// Clock-WEP bound from a frequency-ratio error σ in a potential Φ/c²: |α_g − 1| < σ/(3Φ/c²)
/// (BASE eq. (4), after Hughes & Holzscheiter).
pub fn clock_bound(sigma: f64, phi_c2: f64) -> f64 {
    sigma / (3.0 * phi_c2)
}

/// The Sun's potential at 1 au, over c².
pub fn solar_potential_c2() -> f64 {
    ext::GM_SUN / (C * C * ext::AU)
}

/// Peak-to-peak fractional swing of the solar potential over Earth's orbit: 2ε/(1−ε²).
pub fn annual_fractional_swing() -> f64 {
    let e = ext::EARTH_ECCENTRICITY;
    2.0 * e / (1.0 - e * e)
}

/// One-line estimate of the annual bound: σ/(3·coverage·ΔΦ/c²). BASE's time-series fit is weaker.
pub fn annual_naive_bound() -> f64 {
    clock_bound(ext::BASE_R_SIGMA, ext::BASE_COVERAGE * annual_fractional_swing() * solar_potential_c2())
}

// ── GBAR: free fall and gravitational quantum states ─────────────────────────────────────

pub fn fall_time(height: f64, accel: f64) -> f64 {
    (2.0 * height / accel).sqrt()
}

/// Gravitational length ℓ_g = (ħ²/(2m²g))^{1/3} for an (anti)hydrogen atom above a mirror.
pub fn gravitational_length(m: &Matter, g: f64) -> f64 {
    let mh = m.hydrogen_mass();
    (si2019::hbar().powi(2) / (2.0 * mh * mh * g)).cbrt()
}

/// Energy of the n-th (1-based, n ≤ 2) gravitational quantum state, in Hz (E/h).
pub fn quantum_state_hz(m: &Matter, g: f64, n: usize) -> f64 {
    m.hydrogen_mass() * g * gravitational_length(m, g) * ext::AIRY_ZEROS[n - 1] / H
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m22() -> Matter {
        Matter::codata_2022()
    }

    #[test]
    fn codata_2022_has_no_antimatter_rows() {
        let t = CodataTable::nist_2022();
        assert!(t.constants.len() > 300);
        assert_eq!(antimatter_rows(&t), 0);
        assert_eq!(antimatter_rows(&CodataTable::nist_2018()), 0);
    }

    #[test]
    fn hydrogen_binding_within_3ppm_of_nist() {
        let m = m22();
        let rel = (m.hydrogen_binding_ev() / m.mp_c2_ev - ext::BASE_BE_MP) / ext::BASE_BE_MP;
        assert!(rel > 0.0 && rel < 3e-6, "{rel:e}");
    }

    #[test]
    fn alpha_g_scale_reproduced() {
        let m = m22();
        let grad = bias_gradient(&m, ext::ALPHA_G_LOCAL_G);
        let diff = mirror_difference(&m, ext::ALPHA_G_LOCAL_G, ext::ALPHA_G_PEAK_SEPARATION_M);
        assert!((grad - ext::ALPHA_G_PRINTED_GRADIENT).abs() < 0.005e-3, "{grad:e}");
        assert!((diff - ext::ALPHA_G_PRINTED_FIELD_DIFF).abs() < 0.005e-4, "{diff:e}");
    }

    #[test]
    fn equivalent_fields_match_the_paper_orders() {
        let m = m22();
        let e = equivalent_electric_field(&m, ext::ALPHA_G_LOCAL_G);
        let b = equivalent_magnetic_field(&m, ext::ALPHA_G_LOCAL_G, 10.0);
        assert!((e - 1.024e-7).abs() < 0.001e-7, "{e:e}"); // paper: "about 10⁻⁷ V m⁻¹"
        assert!((b - 2.06e-10).abs() < 0.01e-10, "{b:e}"); // paper: "of order 10⁻¹⁰ T"
    }

    #[test]
    fn trapped_moments_at_one_tesla() {
        let m = m22();
        let tm = trapped_moments(&m, ext::ALPHA_G_BACKGROUND_FIELD_T, ext::NU_HF_HYDROGEN);
        assert!((tm.x - 19.76).abs() < 0.01, "{}", tm.x);
        assert!((tm.d - 0.999_639).abs() < 1e-6, "{}", tm.d);
        assert!((tm.c - 1.001_40).abs() < 1e-5, "{}", tm.c);
        assert!((tm.spread() - 1.76e-3).abs() < 0.01e-3);
        assert!((tm.mean() - 1.0).abs() < 1e-3); // ALPHA: moment known to < 1 part per thousand
        let high = trapped_moments(&m, 100.0, ext::NU_HF_HYDROGEN);
        assert!((high.c - (m.mu_e_over_mu_b + m.mu_p_over_mu_b)).abs() < 1e-6);
    }

    #[test]
    fn efficiency_calibration() {
        let e = efficiency_2023();
        assert!((e.ratio() - 1.031).abs() < 0.001, "{}", e.ratio());
        assert!((e.ratio_rel_sigma() - 0.042).abs() < 0.001, "{}", e.ratio_rel_sigma());
    }

    #[test]
    fn refit_of_table1_lands_near_the_paper() {
        let k = efficiency_2023().ratio();
        let lg = fit_escape(Shape::Logistic, k);
        let pb = fit_escape(Shape::Probit, k);
        for f in [&lg, &pb] {
            let a = f.naive_acceleration();
            assert!(a > 0.65 && a < 0.90, "{a}");
            // Paper Table 3, "finite data size": 0.06 g.
            assert!(f.balance_sigma > 0.04 && f.balance_sigma < 0.08, "{}", f.balance_sigma);
        }
        assert!((lg.balance - pb.balance).abs() < 0.05);
        // The raw (uncorrected) fit sits a little lower: κ > 1 means "down" was seen better.
        assert!(fit_escape(Shape::Logistic, 1.0).balance < lg.balance);
    }

    #[test]
    fn counting_alone_would_exclude_no_gravity_and_repulsion() {
        let lg = fit_escape(Shape::Logistic, efficiency_2023().ratio());
        assert!(stat_only_z(&lg, 0.0) > 8.0); // the paper, WITH systematics: p = 2.9e-4
        assert!(stat_only_z(&lg, 1.0) > 20.0); // repulsion: balance at +1 g; paper: p < 1e-15
        assert!(stat_only_z(&lg, lg.balance) < 1e-6);
    }

    #[test]
    fn one_percent_is_not_a_counting_problem() {
        let lg = fit_escape(Shape::Logistic, efficiency_2023().ratio());
        let ratio = events_for(&lg, 0.01) / lg.events;
        assert!(ratio > 25.0 && ratio < 50.0, "{ratio}");
        assert!(efficiency_shift(Shape::Logistic) < ext::ALPHA_G_EFFICIENCY_ITEM);
    }

    #[test]
    fn gravity_is_a_third_of_a_cooled_atoms_energy() {
        let m = m22();
        let step = gravity_step_kelvin(&m, ext::ALPHA_G_LOCAL_G, ext::ALPHA_G_PEAK_SEPARATION_M);
        assert!((step - 3.04e-4).abs() < 0.01e-4, "{step:e}");
        assert!((gravity_step_kelvin(&m, ext::ALPHA_G_LOCAL_G, 0.01) - 1.19e-5).abs() < 0.01e-5);
    }

    #[test]
    fn base_ratio_rebuilt_from_codata_2022() {
        let r = hminus_ratio(&m22());
        assert!((r.two_me_mp - ext::BASE_TWO_ME_MP).abs() < 2e-14, "{:e}", r.two_me_mp - ext::BASE_TWO_ME_MP);
        assert!((r.ae_mp - ext::BASE_AE_MP).abs() < 5e-15); // printed to 1e-14
        assert!((r.r - ext::BASE_R_THEO).abs() < 1e-13, "{:e}", r.r - ext::BASE_R_THEO);
        // CODATA 2018 → 2022 moved R by far less than BASE's 16 ppt.
        let r18 = hminus_ratio(&Matter::from_table(&CodataTable::nist_2018()));
        assert!((r.r - r18.r).abs() < 1e-14);
    }

    #[test]
    fn base_gravity_bounds() {
        assert!((clock_bound(ext::BASE_R_SIGMA, ext::BASE_SUPERCLUSTER_PHI_C2) - ext::BASE_BOUND_ABSOLUTE).abs() < 0.05e-7);
        assert!((annual_fractional_swing() - 0.033).abs() < 0.001); // BASE: "≈ 0.03"
        assert!((solar_potential_c2() - 9.87e-9).abs() < 0.01e-9);
        let naive = annual_naive_bound();
        assert!(naive > 0.015 && naive < ext::BASE_BOUND_DIFFERENTIAL, "{naive}");
        assert!((m22().big_g_rel_sigma - 2.247e-5).abs() < 0.001e-5);
    }

    #[test]
    fn gbar_free_fall_and_quantum_states() {
        let m = m22();
        let t1 = fall_time(0.2, m.g_n);
        let t2 = fall_time(0.2, 0.75 * m.g_n);
        assert!((t1 - 0.2020).abs() < 0.0005 && (t2 - t1 - 0.0312).abs() < 0.0005, "{t1} {t2}");
        let l = gravitational_length(&m, m.g_n);
        assert!((l - 5.87e-6).abs() < 0.01e-6, "{l:e}");
        let e1 = quantum_state_hz(&m, m.g_n, 1);
        let e2 = quantum_state_hz(&m, m.g_n, 2);
        assert!((e1 - 340.0).abs() < 2.0 && (e2 - e1 - 254.0).abs() < 2.0, "{e1} {e2}");
    }

    #[test]
    fn erfc_is_accurate() {
        assert!((erfc(0.0) - 1.0).abs() < 1e-7);
        assert!((erfc(1.0) - 0.157_299_207).abs() < 1e-7);
        assert!((erfc(-1.0) - 1.842_700_793).abs() < 1e-7);
    }
}
