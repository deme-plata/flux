//! Electromagnetism in the revised SI, read from the CODATA 2022 adjustment.
//!
//! Since 20 May 2019 the elementary charge `e`, the Planck constant `h` and the speed of
//! light `c` are exact numbers. The price of that choice: the magnetic constant
//! `μ0 = 4π×10⁻⁷ N A⁻²` stopped being exact. `μ0 = 2hα/(e²c)` is now a MEASURED quantity,
//! and so are `ε0` and `Z0`. The whole electromagnetic block of the CODATA table hangs off
//! a handful of measured numbers: the fine-structure constant `α` (electron g−2 and atom
//! recoil), the Rydberg constant `R∞` (hydrogen spectroscopy), and — for the nuclear
//! magneton only — the electron–proton mass ratio.
//!
//! This module
//! 1. parses NIST's own ASCII listings (2022 and 2018, embedded byte for byte),
//! 2. recomputes the electromagnetic constants from `(α, R∞, mₑ/mₚ, aₑ)` and checks them
//!    against the printed table ([`consistency`]),
//! 3. measures, from the two tables, by which power of `α` each constant moved between
//!    2018 and 2022 ([`alpha_fingerprint`]) — the paper's Sec. XV list, made falsifiable,
//! 4. recomputes the 2022 `α` from its three input determinations and the expansion factor
//!    2.5 the Task Group applied ([`weighted_mean`]),
//! 5. tracks `μ0/(4π×10⁻⁷)` ([`mu0_drift`]), the electron-anomaly shift
//!    ([`anomaly_fingerprint`]), the proton-radius tension and the post-CODATA muon g−2.
//!
//! Sources (fetched 2026-11-13):
//! * NIST, "Fundamental Physical Constants — Complete Listing, 2022 CODATA adjustment",
//!   <https://physics.nist.gov/cuu/Constants/Table/allascii.txt>, and the 2018 listing
//!   <https://physics.nist.gov/cuu/Constants/ArchiveASCII/allascii_2018.txt>
//!   (BLAKE3 pinned in [`NIST_2022_BLAKE3`], [`NIST_2018_BLAKE3`]).
//! * P. J. Mohr, D. B. Newell, B. N. Taylor, E. Tiesinga, "CODATA Recommended Values of the
//!   Fundamental Physical Constants: 2022", Rev. Mod. Phys. 97, 025002 (2025),
//!   arXiv:2409.03787 — Sec. I.2.5 (aₑ, A1⁽¹⁰⁾), Table XVI (rₚ), Sec. VII (muon),
//!   Sec. XV (α, its inputs, μ0).
//! * Muon g−2 Theory Initiative, White Paper 2025, arXiv:2505.21476, eqs. (9.4)–(9.6);
//!   Fermilab Muon g−2 final result, 3 June 2025.

use std::f64::consts::PI;

/// NIST's 2022 listing, byte for byte.
pub const NIST_2022: &str = include_str!("../data/codata2022-allascii.txt");
/// NIST's 2018 listing, byte for byte.
pub const NIST_2018: &str = include_str!("../data/codata2018-allascii.txt");
/// BLAKE3 of the 2022 file as fetched from physics.nist.gov on 2026-11-13.
pub const NIST_2022_BLAKE3: &str =
    "d6e98c90cf2ea59fce421c1d694fd3186cb6cf1079ebdb693d828c05b2d4e700";
/// BLAKE3 of the 2018 file as fetched from physics.nist.gov on 2026-11-13.
pub const NIST_2018_BLAKE3: &str =
    "a72d727460c728f23b250a418b41d31e9b0b55c85d3b7056f65e5a67e5d75cb6";

/// The exact constants of the revised SI and the electrical quanta built from them.
pub mod si2019 {
    use std::f64::consts::PI;
    /// Elementary charge (C), exact.
    pub const E: f64 = 1.602_176_634e-19;
    /// Planck constant (J s), exact.
    pub const H: f64 = 6.626_070_15e-34;
    /// Speed of light in vacuum (m/s), exact.
    pub const C: f64 = 299_792_458.0;
    /// ħ = h/2π (J s), exact.
    pub fn hbar() -> f64 {
        H / (2.0 * PI)
    }
    /// Josephson constant K_J = 2e/h (Hz/V), exact: the voltage standard.
    pub fn josephson() -> f64 {
        2.0 * E / H
    }
    /// von Klitzing constant R_K = h/e² (Ω), exact: the resistance standard.
    pub fn von_klitzing() -> f64 {
        H / (E * E)
    }
    /// Magnetic flux quantum Φ0 = h/2e (Wb), exact.
    pub fn flux_quantum() -> f64 {
        H / (2.0 * E)
    }
    /// Conductance quantum G0 = 2e²/h (S), exact.
    pub fn conductance_quantum() -> f64 {
        2.0 * E * E / H
    }
    /// The magnetic constant as it was DEFINED before 20 May 2019: 4π×10⁻⁷ N A⁻².
    pub fn mu0_pre2019() -> f64 {
        4.0 * PI * 1e-7
    }
}
use si2019::{C, E, H};

/// One row of a NIST listing.
#[derive(Clone, Debug, PartialEq)]
pub struct Constant {
    /// NIST's name, e.g. `"vacuum mag. permeability"`.
    pub name: String,
    pub value: f64,
    /// Standard uncertainty (0 for exact constants).
    pub sigma: f64,
    /// One unit in the last printed digit of `value`.
    pub ulp: f64,
    /// Printed "(exact)".
    pub exact: bool,
    /// Printed with "..." (an exact value cut off, not rounded).
    pub truncated: bool,
    pub unit: String,
}

impl Constant {
    /// Relative standard uncertainty.
    pub fn rel_sigma(&self) -> f64 {
        if self.value == 0.0 {
            0.0
        } else {
            self.sigma / self.value.abs()
        }
    }
    /// Largest possible gap between the printed value and the number it stands for.
    pub fn rounding(&self) -> f64 {
        if self.truncated {
            self.ulp
        } else if self.exact {
            0.0
        } else {
            0.5 * self.ulp
        }
    }
    /// [`Self::rounding`] relative to the value.
    pub fn rel_rounding(&self) -> f64 {
        self.rounding() / self.value.abs()
    }
}

/// Why a listing did not parse.
#[derive(Clone, Debug, PartialEq)]
pub enum ParseError {
    NoYear,
    NoHeader,
    BadRow { line: usize, what: &'static str },
}

/// A parsed NIST listing.
#[derive(Clone, Debug)]
pub struct CodataTable {
    pub year: u16,
    pub constants: Vec<Constant>,
}

/// Parse one NIST number field: `"6.626 070 15 e-34"`, `"-2.002 319 304 360 92"`,
/// `"483 597.848 4... e9"`. Returns (value, unit of the last printed digit, truncated?).
pub fn parse_number(field: &str) -> Option<(f64, f64, bool)> {
    let truncated = field.contains("...");
    let compact: String = field.chars().filter(|c| !c.is_whitespace()).collect();
    let compact = compact.replace("...", "");
    if compact.is_empty() {
        return None;
    }
    let (mantissa, exponent) = match compact.find('e') {
        Some(i) => (&compact[..i], compact[i + 1..].parse::<i32>().ok()?),
        None => (compact.as_str(), 0),
    };
    let value: f64 = format!("{mantissa}e{exponent}").parse().ok()?;
    let decimals = mantissa.find('.').map_or(0, |i| mantissa.len() - i - 1) as i32;
    Some((value, 10f64.powi(exponent - decimals), truncated))
}

impl CodataTable {
    /// Parse NIST's fixed-column listing (name 0..60, value 60..85, uncertainty 85..110,
    /// unit 110..). Never panics on foreign input: a non-ASCII or short row is an error.
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let year = text
            .lines()
            .find_map(|l| {
                l.trim()
                    .strip_suffix("CODATA adjustment")
                    .and_then(|y| y.trim().parse::<u16>().ok())
            })
            .ok_or(ParseError::NoYear)?;
        let mut constants = Vec::new();
        let mut in_body = false;
        for (i, line) in text.lines().enumerate() {
            if line.starts_with("-----") {
                in_body = true;
                continue;
            }
            if !in_body || line.trim().is_empty() {
                continue;
            }
            let bad = |what: &'static str| ParseError::BadRow { line: i + 1, what };
            let name = line.get(..60).ok_or_else(|| bad("name field"))?.trim();
            let vfield = line.get(60..85).ok_or_else(|| bad("value field"))?;
            let ufield = line.get(85..line.len().min(110)).ok_or_else(|| bad("uncertainty field"))?;
            let unit = line.get(110..).unwrap_or("").trim();
            let (value, ulp, truncated) = parse_number(vfield).ok_or_else(|| bad("value"))?;
            let (sigma, exact) = if ufield.trim() == "(exact)" {
                (0.0, true)
            } else {
                (parse_number(ufield).ok_or_else(|| bad("uncertainty"))?.0, false)
            };
            constants.push(Constant {
                name: name.to_string(),
                value,
                sigma,
                ulp,
                exact,
                truncated,
                unit: unit.to_string(),
            });
        }
        if !in_body {
            return Err(ParseError::NoHeader);
        }
        Ok(Self { year, constants })
    }

    /// The embedded CODATA 2022 listing.
    pub fn nist_2022() -> Self {
        Self::parse(NIST_2022).expect("embedded NIST 2022 listing parses")
    }

    /// The embedded CODATA 2018 listing.
    pub fn nist_2018() -> Self {
        Self::parse(NIST_2018).expect("embedded NIST 2018 listing parses")
    }

    /// Look a constant up by its exact NIST name.
    pub fn get(&self, name: &str) -> Option<&Constant> {
        self.constants.iter().find(|c| c.name == name)
    }

    /// Like [`Self::get`], for the names this module relies on.
    pub fn must(&self, name: &str) -> &Constant {
        self.get(name)
            .unwrap_or_else(|| panic!("CODATA {} listing has no row {name:?}", self.year))
    }

    /// The row α is read from. α⁻¹ is printed with a relatively finer last digit
    /// (±3.6×10⁻¹²) than α itself (±6.9×10⁻¹²), so it is the better source.
    pub fn alpha_inverse(&self) -> &Constant {
        self.must("inverse fine-structure constant")
    }
}

/// The measured numbers the electromagnetic block is built from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inputs {
    pub alpha: f64,
    /// R∞ (m⁻¹).
    pub rydberg: f64,
    /// mₑ/mₚ.
    pub me_over_mp: f64,
    /// Electron magnetic-moment anomaly aₑ.
    pub a_e: f64,
}

impl Inputs {
    pub fn from_table(t: &CodataTable) -> Self {
        Self {
            alpha: 1.0 / t.alpha_inverse().value,
            rydberg: t.must("Rydberg constant").value,
            me_over_mp: t.must("electron-proton mass ratio").value,
            a_e: t.must("electron mag. mom. anomaly").value,
        }
    }
}

fn mu0(i: &Inputs) -> f64 {
    2.0 * H * i.alpha / (E * E * C)
}
fn eps0(i: &Inputs) -> f64 {
    E * E / (2.0 * H * C * i.alpha)
}
fn z0(i: &Inputs) -> f64 {
    2.0 * H * i.alpha / (E * E)
}
fn bohr_radius(i: &Inputs) -> f64 {
    i.alpha / (4.0 * PI * i.rydberg)
}
fn compton_wavelength(i: &Inputs) -> f64 {
    i.alpha * i.alpha / (2.0 * i.rydberg)
}
fn classical_electron_radius(i: &Inputs) -> f64 {
    i.alpha.powi(3) / (4.0 * PI * i.rydberg)
}
fn thomson_cross_section(i: &Inputs) -> f64 {
    i.alpha.powi(6) / (6.0 * PI * i.rydberg * i.rydberg)
}
fn electron_mass(i: &Inputs) -> f64 {
    2.0 * H * i.rydberg / (C * i.alpha * i.alpha)
}
fn bohr_magneton(i: &Inputs) -> f64 {
    E * C * i.alpha * i.alpha / (8.0 * PI * i.rydberg)
}
fn nuclear_magneton(i: &Inputs) -> f64 {
    bohr_magneton(i) * i.me_over_mp
}
fn hartree_energy(i: &Inputs) -> f64 {
    2.0 * H * C * i.rydberg
}
fn atomic_unit_field(i: &Inputs) -> f64 {
    hartree_energy(i) / (E * bohr_radius(i))
}
fn electron_g(i: &Inputs) -> f64 {
    -2.0 * (1.0 + i.a_e)
}
fn electron_moment(i: &Inputs) -> f64 {
    -(1.0 + i.a_e) * bohr_magneton(i)
}

/// How one tabulated constant follows from [`Inputs`].
pub struct Law {
    pub symbol: &'static str,
    pub nist_name: &'static str,
    /// Exponents of (α, R∞, mₑ/mₚ) when the constant is a pure power of them; `None`
    /// when it is not (aₑ enters through QED, not as a power).
    pub powers: Option<(i32, i32, i32)>,
    pub derive: fn(&Inputs) -> f64,
}

/// The electromagnetic block, in the order of the paper's Sec. XV list.
pub const LAWS: &[Law] = &[
    Law { symbol: "mu0", nist_name: "vacuum mag. permeability", powers: Some((1, 0, 0)), derive: mu0 },
    Law { symbol: "eps0", nist_name: "vacuum electric permittivity", powers: Some((-1, 0, 0)), derive: eps0 },
    Law { symbol: "Z0", nist_name: "characteristic impedance of vacuum", powers: Some((1, 0, 0)), derive: z0 },
    Law { symbol: "a0", nist_name: "Bohr radius", powers: Some((1, -1, 0)), derive: bohr_radius },
    Law { symbol: "lambda_C", nist_name: "Compton wavelength", powers: Some((2, -1, 0)), derive: compton_wavelength },
    Law { symbol: "r_e", nist_name: "classical electron radius", powers: Some((3, -1, 0)), derive: classical_electron_radius },
    Law { symbol: "sigma_e", nist_name: "Thomson cross section", powers: Some((6, -2, 0)), derive: thomson_cross_section },
    Law { symbol: "m_e", nist_name: "electron mass", powers: Some((-2, 1, 0)), derive: electron_mass },
    Law { symbol: "mu_B", nist_name: "Bohr magneton", powers: Some((2, -1, 0)), derive: bohr_magneton },
    Law { symbol: "mu_N", nist_name: "nuclear magneton", powers: Some((2, -1, 1)), derive: nuclear_magneton },
    Law { symbol: "E_h", nist_name: "Hartree energy", powers: Some((0, 1, 0)), derive: hartree_energy },
    Law { symbol: "E_au", nist_name: "atomic unit of electric field", powers: Some((-1, 2, 0)), derive: atomic_unit_field },
    Law { symbol: "g_e", nist_name: "electron g factor", powers: None, derive: electron_g },
    Law { symbol: "mu_e", nist_name: "electron mag. mom.", powers: None, derive: electron_moment },
];

/// Relative standard uncertainty of α in 2022 before rounding (Sec. XV: "actually
/// 1.558 × 10⁻¹⁰"). The printed 0.000 000 0011 e-3 rounds it to 1.51 × 10⁻¹⁰.
pub const ALPHA_REL_SIGMA_2022_UNROUNDED: f64 = 1.558e-10;

/// One row of the consistency check.
#[derive(Clone, Debug)]
pub struct Check {
    pub symbol: &'static str,
    pub nist_name: &'static str,
    pub table: f64,
    pub derived: f64,
    pub sigma: f64,
    /// (derived − table) / σ(table).
    pub z: f64,
    pub rel_sigma: f64,
}

/// Recompute every [`LAWS`] entry from the table's own inputs and compare with the
/// printed value. A self-consistent table gives |z| ≪ 1 (only printing rounding is left).
pub fn consistency(t: &CodataTable) -> Vec<Check> {
    let inputs = Inputs::from_table(t);
    LAWS.iter()
        .map(|law| {
            let row = t.must(law.nist_name);
            let derived = (law.derive)(&inputs);
            Check {
                symbol: law.symbol,
                nist_name: law.nist_name,
                table: row.value,
                derived,
                sigma: row.sigma,
                z: (derived - row.value) / row.sigma,
                rel_sigma: row.rel_sigma(),
            }
        })
        .collect()
}

/// An exact electrical quantum, recomputed from e and h, beside NIST's truncated print.
#[derive(Clone, Debug)]
pub struct ExactCheck {
    pub nist_name: &'static str,
    pub computed: f64,
    pub printed: f64,
    pub ulp: f64,
}

/// The exact quanta of the revised SI: K_J, R_K, G0, Φ0, ħ.
pub fn exact_checks(t: &CodataTable) -> Vec<ExactCheck> {
    [
        ("Josephson constant", si2019::josephson()),
        ("von Klitzing constant", si2019::von_klitzing()),
        ("conductance quantum", si2019::conductance_quantum()),
        ("mag. flux quantum", si2019::flux_quantum()),
        ("reduced Planck constant", si2019::hbar()),
    ]
    .into_iter()
    .map(|(nist_name, computed)| {
        let row = t.must(nist_name);
        ExactCheck { nist_name, computed, printed: row.value, ulp: row.ulp }
    })
    .collect()
}

/// How one constant moved between two adjustments, measured in powers of α.
#[derive(Clone, Debug)]
pub struct Shift {
    pub symbol: &'static str,
    pub nist_name: &'static str,
    /// The power of α the constant depends on (paper, Sec. XV).
    pub n_theory: i32,
    /// ln(X_new/X_old), with the R∞ and mₑ/mₚ parts divided out, over ln(α_new/α_old).
    pub n_measured: f64,
    /// Worst-case error of `n_measured` from the printed rounding of every number used.
    pub bound: f64,
    /// (X_new − X_old) / σ_old: the move in units of the old uncertainty.
    pub shift_in_old_sigma: f64,
}

/// ln(new/old) computed without cancellation.
fn ln_ratio(old: &Constant, new: &Constant) -> f64 {
    ((new.value - old.value) / old.value).ln_1p()
}

/// ln(α_new/α_old), read from the α⁻¹ rows.
pub fn ln_alpha_ratio(old: &CodataTable, new: &CodataTable) -> f64 {
    -ln_ratio(old.alpha_inverse(), new.alpha_inverse())
}

/// For every pure-power law: by which power of α did the constant move from `old` to
/// `new`? If the laws are right and the tables correctly rounded, every `n_measured`
/// lies within `bound` of its integer `n_theory`.
pub fn alpha_fingerprint(old: &CodataTable, new: &CodataTable) -> Vec<Shift> {
    let (a0, a1) = (old.alpha_inverse(), new.alpha_inverse());
    let (r0, r1) = (old.must("Rydberg constant"), new.must("Rydberg constant"));
    let (q0, q1) = (old.must("electron-proton mass ratio"), new.must("electron-proton mass ratio"));
    let dla = ln_alpha_ratio(old, new);
    LAWS.iter()
        .filter_map(|law| {
            let (n, k, j) = law.powers?;
            let (x0, x1) = (old.must(law.nist_name), new.must(law.nist_name));
            let resid = ln_ratio(x0, x1) - k as f64 * ln_ratio(r0, r1) - j as f64 * ln_ratio(q0, q1);
            let rounding = x0.rel_rounding()
                + x1.rel_rounding()
                + k.unsigned_abs() as f64 * (r0.rel_rounding() + r1.rel_rounding())
                + j.unsigned_abs() as f64 * (q0.rel_rounding() + q1.rel_rounding())
                + n.unsigned_abs() as f64 * (a0.rel_rounding() + a1.rel_rounding());
            Some(Shift {
                symbol: law.symbol,
                nist_name: law.nist_name,
                n_theory: n,
                n_measured: resid / dla,
                bound: rounding / dla.abs(),
                shift_in_old_sigma: (x1.value - x0.value) / x0.sigma,
            })
        })
        .collect()
}

/// Mass-independent QED coefficients A1⁽²⁾, A1⁽⁴⁾, A1⁽⁶⁾, A1⁽⁸⁾ of
/// aₑ = Σ A1⁽²ⁿ⁾ (α/π)ⁿ (standard values: Schwinger, Petermann/Sommerfield,
/// Laporta–Remiddi, Laporta 2017).
pub const QED_A1: [f64; 4] = [0.5, -0.328_478_965_579_193_8, 1.181_241_456_587, -1.912_245_764_926];
/// Tenth-order coefficient A1⁽¹⁰⁾ in the 2018 expression (CODATA 2022, Sec. I.2.5).
pub const QED_A1_10_2018: f64 = 6.675;
/// Tenth-order coefficient A1⁽¹⁰⁾ in the 2022 expression (CODATA 2022, Sec. I.2.5).
pub const QED_A1_10_2022: f64 = 6.08;

/// The mass-independent QED part of aₑ at a given α and tenth-order coefficient.
pub fn a_e_qed(alpha: f64, a1_10: f64) -> f64 {
    let x = alpha / PI;
    QED_A1.iter().chain(std::iter::once(&a1_10)).enumerate().map(|(i, c)| c * x.powi(i as i32 + 1)).sum()
}

/// d ln aₑ / d ln α of the mass-independent QED series ("aₑ ≈ α/2π" made exact).
pub fn a_e_log_slope(alpha: f64, a1_10: f64) -> f64 {
    let x = alpha / PI;
    let num: f64 = QED_A1
        .iter()
        .chain(std::iter::once(&a1_10))
        .enumerate()
        .map(|(i, c)| (i as f64 + 1.0) * c * x.powi(i as i32 + 1))
        .sum();
    num / a_e_qed(alpha, a1_10)
}

/// How the electron anomaly moved, in powers of α, against two predictions.
#[derive(Clone, Debug)]
pub struct AnomalyShift {
    /// ln(aₑ_new/aₑ_old) / ln(α_new/α_old), from the tables.
    pub n_measured: f64,
    /// Worst-case printing-rounding error of `n_measured`.
    pub bound: f64,
    /// Prediction if the QED theory had stayed fixed: d ln aₑ/d ln α ≈ 0.9985.
    pub n_fixed_theory: f64,
    /// Prediction including the tenth-order coefficient update 6.675 → 6.08.
    pub n_with_a10_update: f64,
}

/// The 2018→2022 move of aₑ. Other theory pieces (hadronic, weak, mass-dependent QED)
/// also changed slightly and are NOT separated here.
pub fn anomaly_fingerprint(old: &CodataTable, new: &CodataTable) -> AnomalyShift {
    let (e0, e1) = (old.must("electron mag. mom. anomaly"), new.must("electron mag. mom. anomaly"));
    let (a0, a1) = (old.alpha_inverse(), new.alpha_inverse());
    let dla = ln_alpha_ratio(old, new);
    let alpha = 1.0 / a1.value;
    let n_fixed_theory = a_e_log_slope(alpha, QED_A1_10_2022);
    let a10_shift = (QED_A1_10_2022 - QED_A1_10_2018) * (alpha / PI).powi(5) / e1.value;
    AnomalyShift {
        n_measured: ln_ratio(e0, e1) / dla,
        bound: (e0.rel_rounding() + e1.rel_rounding() + a0.rel_rounding() + a1.rel_rounding()) / dla.abs(),
        n_fixed_theory,
        n_with_a10_update: n_fixed_theory + a10_shift / dla,
    }
}

/// One input determination of α.
#[derive(Clone, Copy, Debug)]
pub struct AlphaInput {
    pub label: &'static str,
    pub method: &'static str,
    pub alpha_inv: f64,
    pub sigma: f64,
}

/// The three key determinations behind the 2022 α, as the paper lists them (Sec. XV).
pub const ALPHA_INPUTS_2022: [AlphaInput; 3] = [
    AlphaInput { label: "Berkeley-18", method: "atom recoil h/m(133Cs), Parker et al. 2018", alpha_inv: 137.035_999_046, sigma: 27e-9 },
    AlphaInput { label: "LKB-20", method: "atom recoil h/m(87Rb), Morel et al. 2020", alpha_inv: 137.035_999_206, sigma: 11e-9 },
    AlphaInput { label: "NW-23", method: "electron g-2, a_e, Fan et al. 2023", alpha_inv: 137.035_999_166, sigma: 15e-9 },
];

/// The expansion factor CODATA 2022 applied to the α data (items D1–D6, Table XXV).
pub const ALPHA_EXPANSION_FACTOR_2022: f64 = 2.5;

/// An inverse-variance weighted mean and its goodness of fit.
#[derive(Clone, Debug)]
pub struct WeightedMean {
    pub mean: f64,
    pub sigma: f64,
    pub chi2: f64,
    pub dof: usize,
    /// √(χ²/ν): the factor by which the scatter exceeds the stated uncertainties.
    pub birge_ratio: f64,
    /// (xᵢ − mean)/(expansion·σᵢ) per input.
    pub normalized_residuals: Vec<f64>,
}

/// Weighted mean of the inputs with every uncertainty multiplied by `expansion`.
pub fn weighted_mean(inputs: &[AlphaInput], expansion: f64) -> WeightedMean {
    assert!(inputs.len() >= 2, "a weighted mean needs at least two inputs");
    let base = inputs[0].alpha_inv;
    let w: Vec<f64> = inputs.iter().map(|x| 1.0 / (x.sigma * expansion).powi(2)).collect();
    let sw: f64 = w.iter().sum();
    let offset = inputs.iter().zip(&w).map(|(x, w)| w * (x.alpha_inv - base)).sum::<f64>() / sw;
    let mean = base + offset;
    let normalized_residuals: Vec<f64> =
        inputs.iter().map(|x| (x.alpha_inv - mean) / (x.sigma * expansion)).collect();
    let chi2: f64 = normalized_residuals.iter().map(|r| r * r).sum();
    let dof = inputs.len() - 1;
    WeightedMean { mean, sigma: 1.0 / sw.sqrt(), chi2, dof, birge_ratio: (chi2 / dof as f64).sqrt(), normalized_residuals }
}

/// (a − b) in units of their combined standard uncertainty.
pub fn tension(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0) / a.1.hypot(b.1)
}

/// χ² survival probability for exactly two degrees of freedom: p = e^(−χ²/2).
pub fn chi2_p_dof2(chi2: f64) -> f64 {
    (-chi2 / 2.0).exp()
}

/// μ0/(4π×10⁻⁷ N A⁻²) − 1 as printed in Sec. XV, in units of 10⁻¹¹: (year, value, σ).
pub const MU0_DRIFT_PAPER: [(u16, f64, f64); 3] = [(2017, 20.0, 23.0), (2018, 55.0, 15.0), (2022, -13.0, 16.0)];

/// μ0/(4π×10⁻⁷) − 1 and its standard uncertainty, from the table's μ0 row.
pub fn mu0_drift(t: &CodataTable) -> (f64, f64) {
    let m = t.must("vacuum mag. permeability");
    let old = si2019::mu0_pre2019();
    ((m.value - old) / old, m.sigma / old)
}

/// A radius in femtometres with its standard uncertainty.
pub type Radius = (f64, f64);

/// Proton rms charge radius, CODATA 2022 Table XVI, "complete" adjustment.
pub const R_P_2022_COMPLETE: Radius = (0.840_75, 0.000_64);
/// The same from the muonic-atom data alone (Table XVI).
pub const R_P_2022_MUONIC: Radius = (0.840_60, 0.000_66);
/// The same from the electronic H/D data alone (Table XVI).
pub const R_P_2022_ELECTRONIC: Radius = (0.852_9, 0.004_3);

/// How far the electronic H/D radius sits above the muonic one, in σ (paper: 2.8σ).
pub fn proton_radius_tension() -> f64 {
    tension(R_P_2022_ELECTRONIC, R_P_2022_MUONIC)
}

/// The muon anomaly after the CODATA 2022 data cutoff (31 December 2022).
pub mod muon_g2 {
    /// Fermilab final result, 3 June 2025: aμ = 0.001 165 920 705 (114 stat)(91 syst),
    /// quoted at 127 ppb.
    pub const FNAL_2025: f64 = 1.165_920_705e-3;
    pub const FNAL_2025_STAT: f64 = 1.14e-10;
    pub const FNAL_2025_SYST: f64 = 9.1e-11;
    /// Experimental average E821 + E989 runs 1–6, WP25 eq. (9.5): 116 592 071.5(14.5) × 10⁻¹¹.
    pub const EXP_AVERAGE_2025: (f64, f64) = (116_592_071.5e-11, 14.5e-11);
    /// Standard Model, Theory Initiative White Paper 2025 eq. (9.4): 116 592 033(62) × 10⁻¹¹.
    pub const SM_WP25: (f64, f64) = (116_592_033e-11, 62e-11);
    /// The experiment–theory discrepancy CODATA 2022 stated (Sec. VII).
    pub const CODATA_2022_DISCREPANCY_SIGMA: f64 = 4.2;
    /// Experiment minus WP25 theory in σ (the paper rows give 38(63) × 10⁻¹¹).
    pub fn wp25_tension() -> f64 {
        super::tension(EXP_AVERAGE_2025, SM_WP25)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_tables_are_the_nist_bytes() {
        assert_eq!(blake3::hash(NIST_2022.as_bytes()).to_hex().as_str(), NIST_2022_BLAKE3);
        assert_eq!(blake3::hash(NIST_2018.as_bytes()).to_hex().as_str(), NIST_2018_BLAKE3);
    }

    #[test]
    fn both_listings_parse_completely() {
        let (t22, t18) = (CodataTable::nist_2022(), CodataTable::nist_2018());
        assert_eq!((t22.year, t22.constants.len()), (2022, 355));
        assert_eq!((t18.year, t18.constants.len()), (2018, 354));
        for c in t22.constants.iter().chain(&t18.constants) {
            assert!(c.value.is_finite() && c.sigma >= 0.0 && c.ulp > 0.0, "{c:?}");
            assert_eq!(c.exact, c.sigma == 0.0, "{c:?}");
        }
    }

    #[test]
    fn parse_number_reads_nist_formats() {
        let (v, ulp, t) = parse_number("6.626 070 15 e-34       ").unwrap();
        assert_eq!((v, t), (6.626_070_15e-34, false));
        assert!((ulp / 1e-42 - 1.0).abs() < 1e-12);
        let (v, ulp, _) = parse_number("-2.002 319 304 360 92    ").unwrap();
        assert_eq!(v, -2.002_319_304_360_92);
        assert!((ulp / 1e-14 - 1.0).abs() < 1e-12);
        let (v, ulp, t) = parse_number("483 597.848 4... e9      ").unwrap();
        assert!(t && v == 483_597.848_4e9 && (ulp / 1e5 - 1.0).abs() < 1e-12);
        assert!(parse_number("   ").is_none());
        assert_eq!(CodataTable::parse("no header here").unwrap_err(), ParseError::NoYear);
    }

    #[test]
    fn reads_the_headline_values() {
        let (t22, t18) = (CodataTable::nist_2022(), CodataTable::nist_2018());
        let a = t22.alpha_inverse();
        assert_eq!((a.value, a.sigma), (137.035_999_177, 2.1e-8));
        assert_eq!(t18.alpha_inverse().value, 137.035_999_084);
        assert_eq!(t22.must("vacuum mag. permeability").value, 1.256_637_061_27e-6);
        let e = t22.must("elementary charge");
        assert!(e.exact && e.value == si2019::E);
    }

    #[test]
    fn every_em_constant_follows_from_alpha_and_rydberg() {
        for t in [CodataTable::nist_2022(), CodataTable::nist_2018()] {
            for c in consistency(&t) {
                assert!(c.z.abs() < 0.25, "{} {}: z = {}", t.year, c.symbol, c.z);
            }
        }
    }

    #[test]
    fn uncertainty_scales_with_the_power_of_alpha() {
        let t = CodataTable::nist_2022();
        for (law, c) in LAWS.iter().zip(consistency(&t)) {
            let Some((n, _, _)) = law.powers else { continue };
            let multiple = c.rel_sigma / ALPHA_REL_SIGMA_2022_UNROUNDED;
            let n = n.unsigned_abs() as f64;
            assert!((multiple - n).abs() < 0.05 * n + 0.03, "{}: {multiple} vs {n}", c.symbol);
        }
    }

    #[test]
    fn exact_quanta_match_their_truncated_print() {
        for x in exact_checks(&CodataTable::nist_2022()) {
            let gap = (x.computed - x.printed) / x.ulp;
            assert!((-1e-6..1.0).contains(&gap), "{}: gap {gap} ulp", x.nist_name);
        }
        // The vacuum impedance is 2α von Klitzing constants.
        let t = CodataTable::nist_2022();
        let ratio = t.must("characteristic impedance of vacuum").value / si2019::von_klitzing();
        assert!((ratio / (2.0 / t.alpha_inverse().value) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn alpha_fingerprint_matches_sec_xv() {
        let shifts = alpha_fingerprint(&CodataTable::nist_2018(), &CodataTable::nist_2022());
        assert_eq!(shifts.len(), 12);
        for s in &shifts {
            assert!(s.bound < 0.25, "{}: bound {} too loose to pin an integer", s.symbol, s.bound);
            assert!((s.n_measured - s.n_theory as f64).abs() <= s.bound, "{}: n = {} ± {} vs {}", s.symbol, s.n_measured, s.bound, s.n_theory);
            assert_eq!(s.n_measured.round() as i32, s.n_theory, "{}", s.symbol);
        }
        // α fell by about 4.4 of its 2018 standard uncertainties.
        let t18 = CodataTable::nist_2018();
        let shift = (CodataTable::nist_2022().alpha_inverse().value - t18.alpha_inverse().value) / t18.alpha_inverse().sigma;
        assert!((4.3..4.5).contains(&shift), "{shift}");
    }

    #[test]
    fn electron_anomaly_shift_needs_the_tenth_order_update() {
        let s = anomaly_fingerprint(&CodataTable::nist_2018(), &CodataTable::nist_2022());
        assert!((s.n_fixed_theory - 0.9985).abs() < 1e-3, "{s:?}");
        // The printed tables exclude "fixed theory" and admit the A1(10) update.
        assert!((s.n_measured - s.n_fixed_theory).abs() > s.bound, "{s:?}");
        assert!((s.n_measured - s.n_with_a10_update).abs() <= s.bound, "{s:?}");
    }

    #[test]
    fn alpha_inputs_reproduce_the_2022_value() {
        let m = weighted_mean(&ALPHA_INPUTS_2022, ALPHA_EXPANSION_FACTOR_2022);
        // Paper: 137.035 999 178(21).
        assert!((m.mean - 137.035_999_178).abs() < 0.5e-9, "{}", m.mean);
        assert!((m.sigma * 1e9 - 21.0).abs() < 0.5, "{}", m.sigma);
        let raw = weighted_mean(&ALPHA_INPUTS_2022, 1.0);
        assert!(raw.birge_ratio > 3.5 && chi2_p_dof2(raw.chi2) < 1e-6, "{raw:?}");
        let cs = (ALPHA_INPUTS_2022[0].alpha_inv, ALPHA_INPUTS_2022[0].sigma);
        let rb = (ALPHA_INPUTS_2022[1].alpha_inv, ALPHA_INPUTS_2022[1].sigma);
        assert!(tension(rb, cs) > 5.0);
    }

    #[test]
    fn mu0_drift_matches_the_paper() {
        for (t, year) in [(CodataTable::nist_2018(), 2018), (CodataTable::nist_2022(), 2022)] {
            let (d, s) = mu0_drift(&t);
            let (_, pv, ps) = MU0_DRIFT_PAPER.iter().copied().find(|r| r.0 == year).unwrap();
            assert!((d * 1e11 - pv).abs() <= 1.0, "{year}: {}", d * 1e11);
            assert!((s * 1e11 - ps).abs() <= 1.0, "{year}: σ {}", s * 1e11);
        }
    }

    #[test]
    fn proton_radius_and_muon_status() {
        assert!((proton_radius_tension() - 2.8).abs() < 0.05);
        let t22 = CodataTable::nist_2022();
        assert!((t22.must("proton rms charge radius").value * 1e15 - R_P_2022_COMPLETE.0).abs() < 1e-12);
        assert!(muon_g2::wp25_tension().abs() < 1.0);
        assert!((muon_g2::EXP_AVERAGE_2025.0 - muon_g2::SM_WP25.0 - 38.5e-11).abs() < 1e-13);
    }
}
