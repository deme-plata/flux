//! flux-whitehole — a *measured* module for the black-hole → white-hole
//! scenario, built to the same instrument discipline as `flux-qwalk`: every
//! number is computed here from CODATA constants, and every headline quantity
//! is checked against an independent published value or an exact closed form.
//!
//! # The literature this encodes
//!
//! The loop-quantum-gravity "black-to-white hole" picture gives a hierarchy of
//! timescales, all powers of the mass in Planck units:
//!
//! | timescale | scaling | source |
//! |---|---|---|
//! | bounce (tunnelling to a white hole) | `~ M²` | Haggard & Rovelli, arXiv:1407.0989 |
//! | Hawking evaporation | `~ M³` | Hawking (semiclassical) |
//! | white-hole remnant lifetime | `~ M⁵` | Martin-Dussaud, arXiv:2504.05492 |
//!
//! The `M⁵` entry is recent and *revises* the previously quoted `M⁴`: the older
//! estimate is argued to neglect the white hole's internal dynamics. Both are
//! carried here ([`RemnantLaw`]) precisely because the exponent is contested —
//! an instrument that hardcoded the winner would be useless for asking what the
//! disagreement actually costs.
//!
//! # Why the hierarchy is the whole story
//!
//! For large `M`, `M² ≪ M³ ≪ M⁵`, so the ordering is not a detail — it decides
//! the physics. If the bounce really goes as `M²`, a black hole tunnels into a
//! white hole *long before* it finishes evaporating, which is what makes the
//! scenario observationally interesting at all (the "Planck star" route to fast
//! radio bursts). If instead the bounce scaled like `M³` or slower, evaporation
//! would get there first and the white hole would never be reached from a
//! primordial population. [`dominant_channel`] answers that comparison for a
//! given mass, and it is the single most falsifiable thing in this crate.
//!
//! # What is honest here, and what is not
//!
//! - The evaporation law is **exact within its own assumptions** and is checked
//!   two ways: closed form vs. adaptive numerical integration of `dM/dt`.
//! - The evaporation model is **photon-only** (a single massless species, pure
//!   Stefan–Boltzmann on the horizon, no greybody factors). This is stated
//!   loudly because it moves real numbers: the textbook "primordial black hole
//!   exploding today" mass is often quoted as `~5×10¹¹ kg`, which counts many
//!   emitted species; photon-only gives `~1.7×10¹¹ kg`. Both are "right"; they
//!   answer different questions. [`evaporation_lifetime`] documents which.
//! - The bounce and remnant laws are **dimensional-analysis scalings with an
//!   unknown O(1) coefficient**. Nobody has derived those coefficients. So this
//!   crate never pretends to predict a date — it computes how the answer moves
//!   as the coefficient and exponent move ([`flood`]), which is the only
//!   defensible thing to do with an undetermined prefactor.

/// CODATA-2018 constants, SI.
pub mod consts {
    /// Gravitational constant, m³ kg⁻¹ s⁻².
    pub const G: f64 = 6.674_30e-11;
    /// Speed of light, m s⁻¹ (exact).
    pub const C: f64 = 299_792_458.0;
    /// Reduced Planck constant, J s.
    pub const HBAR: f64 = 1.054_571_817e-34;
    /// Boltzmann constant, J K⁻¹ (exact).
    pub const KB: f64 = 1.380_649e-23;
    /// Solar mass, kg.
    pub const M_SUN: f64 = 1.988_47e30;
    /// Julian year, s.
    pub const YEAR: f64 = 3.155_76e7;
    /// Age of the universe, s (Planck 2018: 13.797 Gyr).
    pub const AGE_UNIVERSE: f64 = 13.797e9 * YEAR;
}

use consts::*;

/// Planck mass, `sqrt(ħc/G)` ≈ 2.176×10⁻⁸ kg.
pub fn planck_mass() -> f64 {
    (HBAR * C / G).sqrt()
}

/// Planck time, `sqrt(ħG/c⁵)` ≈ 5.391×10⁻⁴⁴ s.
pub fn planck_time() -> f64 {
    (HBAR * G / C.powi(5)).sqrt()
}

/// Planck length, `sqrt(ħG/c³)` ≈ 1.616×10⁻³⁵ m.
pub fn planck_length() -> f64 {
    (HBAR * G / C.powi(3)).sqrt()
}

/// Schwarzschild radius `r_s = 2GM/c²`, metres.
pub fn schwarzschild_radius(mass_kg: f64) -> f64 {
    2.0 * G * mass_kg / (C * C)
}

/// Hawking temperature `T = ħc³ / (8π G M k_B)`, kelvin.
///
/// Note `T·M` is a constant — the hotter a black hole gets, the less of it
/// there is. That exact relation is a test below, because it is the cheapest
/// way to catch a transcription error in this formula.
pub fn hawking_temperature(mass_kg: f64) -> f64 {
    HBAR * C.powi(3) / (8.0 * std::f64::consts::PI * G * mass_kg * KB)
}

/// Mass-loss rate `dM/dt = −ħc⁴ / (15360 π G² M²)`, kg s⁻¹ (negative).
///
/// Single massless boson species, Stefan–Boltzmann across the horizon area, no
/// greybody suppression. See the crate docs for what that costs.
pub fn mass_loss_rate(mass_kg: f64) -> f64 {
    -HBAR * C.powi(4) / (15360.0 * std::f64::consts::PI * G * G * mass_kg * mass_kg)
}

/// Total evaporation time `τ = 5120 π G² M³ / (ħ c⁴)`, seconds.
///
/// This is the exact integral of [`mass_loss_rate`] from `M` to 0, and the
/// `M³` entry of the hierarchy. **Photon-only**: adding the other species a
/// real black hole radiates shortens `τ` at fixed `M`, which is why the
/// literature's "exploding today" mass (~5×10¹¹ kg) is larger than the one
/// this function implies (~1.7×10¹¹ kg).
pub fn evaporation_lifetime(mass_kg: f64) -> f64 {
    5120.0 * std::f64::consts::PI * G * G * mass_kg.powi(3) / (HBAR * C.powi(4))
}

/// Evaporation time obtained by **numerically integrating** `dM/dt`, seconds.
///
/// This exists purely to audit [`evaporation_lifetime`]. A closed form and a
/// numerical integration of the same ODE are independent enough that agreeing
/// to ten digits means neither has a transcription error — and disagreeing
/// tells you immediately which layer to distrust.
///
/// # Quadrature, and a bug this function used to have
///
/// The integrand is `dt = dM / |dM/dt| = K·M² dM`, a pure quadratic in `M`.
///
/// The original implementation stepped uniformly in `u = M³` (so that `du/dt`
/// is constant) and then evaluated each step with the **midpoint in mass**,
/// `Δt ≈ ΔM · K · ((M + M_next)/2)²`. That is the wrong quadrature for a
/// quadratic: the exact step is `K·(M³ − M_next³)/3`, and the midpoint rule
/// gives `K·ΔM·((M+M_next)/2)²`. For the final step, where `M_next = 0`, those
/// are `K·M³/3` versus `K·M³/4` — a **25 % error on that step**. Because the
/// `u`-uniform schedule makes the last mass steps the largest, the error piled
/// up there and the whole integral came out `1.29e-5` low, which is precisely
/// what the cross-check test caught. The docstring meanwhile claimed the result
/// was "exact up to floating point". It was not.
///
/// The fix is **Simpson's rule**, which is exact for polynomials up to cubic
/// and therefore exact for this integrand, to floating point:
/// `∫ f = (Δm/6)·[f(a) + 4·f(mid) + f(b)]`. Measured residual is ~3e-13.
///
/// Simpson is used rather than simply substituting `Δt = Δu/(3A)` on purpose:
/// substituting the analytic step would make this function re-derive the closed
/// form it is supposed to be auditing, and the two would agree by construction
/// instead of by agreement. This still genuinely integrates `1/(dM/dt)`.
pub fn evaporation_lifetime_integrated(mass_kg: f64, steps: u32) -> f64 {
    // Reciprocal of the mass-loss rate: the dt/dM integrand, seconds per kg.
    let integrand = |m: f64| -> f64 {
        if m <= 0.0 {
            0.0
        } else {
            1.0 / (-mass_loss_rate(m))
        }
    };

    let mut m = mass_kg;
    let mut t = 0.0f64;
    // Equal decrements in u = M³ keep the step count meaningful in time.
    let u0 = m.powi(3);
    let du = u0 / steps as f64;
    for _ in 0..steps {
        let u_next = (m.powi(3) - du).max(0.0);
        let m_next = u_next.cbrt();
        let m_mid = 0.5 * (m + m_next);
        // Simpson over [m_next, m] — exact for the quadratic integrand.
        t += (m - m_next) / 6.0
            * (integrand(m) + 4.0 * integrand(m_mid) + integrand(m_next));
        m = m_next;
        if m <= 0.0 {
            break;
        }
    }
    t
}

/// Invert [`evaporation_lifetime`]: the mass whose evaporation time is exactly
/// `t` seconds. Closed form, since `τ ∝ M³`.
pub fn mass_evaporating_in(t_seconds: f64) -> f64 {
    (t_seconds * HBAR * C.powi(4) / (5120.0 * std::f64::consts::PI * G * G)).cbrt()
}

/// Which power law the white-hole remnant lifetime follows. The exponent is
/// genuinely disputed, so it is a parameter, not a constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemnantLaw {
    /// The older estimate, `τ ~ M⁴`.
    M4,
    /// Martin-Dussaud (arXiv:2504.05492), `τ ~ M⁵`, arguing the `M⁴` estimate
    /// neglects the white hole's internal dynamics.
    M5,
}

impl RemnantLaw {
    pub fn exponent(self) -> i32 {
        match self {
            RemnantLaw::M4 => 4,
            RemnantLaw::M5 => 5,
        }
    }
}

/// A power-law timescale in Planck units: `τ = k · (M/m_P)^p · t_P`, seconds.
///
/// Every quantum-gravity timescale in this crate has this shape. `k` is the
/// undetermined O(1) coefficient nobody has derived; it is an explicit
/// argument so that no caller can accidentally treat a guess as a prediction.
pub fn planck_power_law(mass_kg: f64, exponent: i32, k: f64) -> f64 {
    k * (mass_kg / planck_mass()).powi(exponent) * planck_time()
}

/// Bounce time, the `M²` entry: `τ_bounce = k · (M/m_P)² · t_P`.
pub fn bounce_time(mass_kg: f64, k: f64) -> f64 {
    planck_power_law(mass_kg, 2, k)
}

/// White-hole remnant lifetime under the chosen law.
pub fn remnant_lifetime(mass_kg: f64, law: RemnantLaw, k: f64) -> f64 {
    planck_power_law(mass_kg, law.exponent(), k)
}

/// Which fate reaches a black hole of this mass first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// Tunnels to a white hole before evaporating — the scenario is live.
    Bounce,
    /// Evaporates first; no white hole is reached from this mass.
    Evaporation,
}

/// Compare `τ_bounce` against `τ_evap` for a given mass and bounce coefficient.
///
/// This is the crate's central falsifiable statement. Because `τ_bounce ~ M²`
/// and `τ_evap ~ M³`, the bounce wins for all masses above a crossover that
/// depends only on `k` — and that crossover is absurdly small, which is the
/// point: for any astrophysical mass the bounce dominates by a colossal margin,
/// so the scenario cannot be dismissed on timescale grounds alone.
pub fn dominant_channel(mass_kg: f64, k_bounce: f64) -> Channel {
    if bounce_time(mass_kg, k_bounce) < evaporation_lifetime(mass_kg) {
        Channel::Bounce
    } else {
        Channel::Evaporation
    }
}

/// The mass at which bounce and evaporation take equally long, kg.
///
/// Setting `k (M/m_P)² t_P = 5120π G² M³ / (ħc⁴)` and solving for `M` gives a
/// closed form: both sides are pure powers of `M`, so the crossover is
/// `M = k · (m_P³ t_P ħ c⁴) / (m_P² · 5120π G²)` — evaluated here directly
/// rather than by root-finding, so there is nothing to converge or fail.
pub fn bounce_evaporation_crossover(k_bounce: f64) -> f64 {
    let m_p = planck_mass();
    let t_p = planck_time();
    // k (M/m_P)^2 t_P = A M^3, with A = 5120π G²/(ħc⁴)
    // => M = k t_P / (m_P² A)
    let a = 5120.0 * std::f64::consts::PI * G * G / (HBAR * C.powi(4));
    k_bounce * t_p / (m_p * m_p * a)
}

/// The mass whose *bounce* happens after exactly `t` seconds — i.e. the
/// primordial black holes going off right now, if the scenario is right.
///
/// This is the Planck-star route to fast radio bursts: a population formed in
/// the early universe reaches its bounce time today, and the emitted radiation
/// has a wavelength set by the object's own horizon scale.
pub fn mass_bouncing_after(t_seconds: f64, k_bounce: f64) -> f64 {
    planck_mass() * (t_seconds / (k_bounce * planck_time())).sqrt()
}

/// Characteristic emitted wavelength of a bouncing object, metres.
///
/// Taken as the horizon scale itself, `λ ≈ 2 r_s` — the standard order-of-
/// magnitude estimate in this literature. It is an estimate, not a spectrum:
/// no radiative transfer, no redshift from the emission epoch. Reported because
/// it is the quantity that decides whether the signal lands anywhere near a
/// radio band, and *that* is what makes the scenario testable at all.
pub fn bounce_wavelength(mass_kg: f64) -> f64 {
    2.0 * schwarzschild_radius(mass_kg)
}

/// One row of a parameter flood.
#[derive(Clone, Debug)]
pub struct FloodRow {
    pub k: f64,
    /// Mass whose bounce time equals the age of the universe, kg.
    pub mass_bouncing_today: f64,
    /// Its characteristic emitted wavelength, m.
    pub wavelength: f64,
    /// Whether that wavelength lands in the observed FRB band (roughly
    /// 0.1 m – 3 m, i.e. ~100 MHz – 3 GHz).
    pub in_frb_band: bool,
    /// Its remnant lifetime under the chosen law, seconds.
    pub remnant_lifetime: f64,
    /// Whether such a remnant would still exist today.
    pub remnant_survives: bool,
}

/// **The flood.** Sweep the undetermined coefficient `k` across `decades` orders
/// of magnitude and report where the scenario's prediction stops being
/// observable.
///
/// The point is not to pick a `k`. It is to show how wide a range of `k` still
/// puts the signal in the radio band — because if that range were razor-thin,
/// the "Planck stars explain FRBs" claim would be a coincidence rather than a
/// prediction, and if it is broad, the claim survives our ignorance of the
/// prefactor. Either answer is informative; guessing `k = 1` and reporting one
/// number would be neither.
pub fn flood(k_min_log10: i32, k_max_log10: i32, per_decade: u32, law: RemnantLaw) -> Vec<FloodRow> {
    let mut rows = Vec::new();
    let total = ((k_max_log10 - k_min_log10) as u32) * per_decade;
    for i in 0..=total {
        let logk = k_min_log10 as f64 + i as f64 / per_decade as f64;
        let k = 10f64.powf(logk);
        let m = mass_bouncing_after(AGE_UNIVERSE, k);
        let lambda = bounce_wavelength(m);
        let tau_r = remnant_lifetime(m, law, 1.0);
        rows.push(FloodRow {
            k,
            mass_bouncing_today: m,
            wavelength: lambda,
            in_frb_band: (0.1..=3.0).contains(&lambda),
            remnant_lifetime: tau_r,
            remnant_survives: tau_r > AGE_UNIVERSE,
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(a: f64, b: f64) -> f64 {
        (a - b).abs() / b.abs()
    }

    /// Planck units must reproduce their CODATA values. If these drift, every
    /// quantum-gravity timescale below is silently wrong.
    #[test]
    fn planck_units_match_codata() {
        assert!(rel(planck_mass(), 2.176_434e-8) < 1e-5, "m_P = {}", planck_mass());
        assert!(rel(planck_time(), 5.391_247e-44) < 1e-5, "t_P = {}", planck_time());
        assert!(rel(planck_length(), 1.616_255e-35) < 1e-5, "l_P = {}", planck_length());
    }

    /// A solar-mass black hole: r_s ≈ 2.95 km, T ≈ 6.17×10⁻⁸ K. Both are
    /// textbook values computed independently of this code.
    #[test]
    fn solar_mass_black_hole_matches_textbook_values() {
        let rs = schwarzschild_radius(M_SUN);
        assert!(rel(rs, 2953.0) < 1e-3, "r_s = {rs} m");
        let t = hawking_temperature(M_SUN);
        assert!(rel(t, 6.17e-8) < 1e-2, "T_H = {t} K");
    }

    /// `T·M` is exactly constant. The cheapest possible check that the
    /// temperature formula has not been transcribed with a stray factor.
    #[test]
    fn temperature_times_mass_is_invariant() {
        let a = hawking_temperature(1e12) * 1e12;
        let b = hawking_temperature(M_SUN) * M_SUN;
        assert!(rel(a, b) < 1e-12, "T*M not invariant: {a} vs {b}");
    }

    /// **The instrument audit.** The closed-form evaporation time and the
    /// numerically integrated one must agree. They are derived differently; if
    /// they agree to 8 digits, neither is mistyped.
    #[test]
    fn closed_form_and_numerical_evaporation_agree() {
        for m in [1e11f64, 1e12, M_SUN] {
            let closed = evaporation_lifetime(m);
            let numeric = evaporation_lifetime_integrated(m, 20_000);
            assert!(
                rel(numeric, closed) < 1e-8,
                "M={m:e}: closed {closed:e} vs numeric {numeric:e} (rel {})",
                rel(numeric, closed)
            );
        }
    }

    /// A solar-mass black hole evaporates in ~2.1×10⁶⁷ years — the standard
    /// published figure for the photon-only law.
    #[test]
    fn solar_mass_evaporation_time_is_2e67_years() {
        let years = evaporation_lifetime(M_SUN) / YEAR;
        assert!(rel(years, 2.1e67) < 0.05, "tau = {years:e} yr");
    }

    /// Round-trip: the inverse must invert.
    #[test]
    fn mass_evaporating_in_inverts_the_lifetime() {
        let m = 3.7e11;
        assert!(rel(mass_evaporating_in(evaporation_lifetime(m)), m) < 1e-12);
    }

    /// The primordial black hole expiring right now comes out at ~1.7×10¹¹ kg
    /// under the photon-only law. The often-quoted ~5×10¹¹ kg counts many
    /// emitted species; this test pins OUR model's number, and the gap between
    /// the two is documented rather than fudged.
    #[test]
    fn pbh_exploding_today_is_1e11_kg_photon_only() {
        let m = mass_evaporating_in(AGE_UNIVERSE);
        assert!(
            (1.0e11..3.0e11).contains(&m),
            "photon-only PBH mass expiring today = {m:e} kg, expected ~1.7e11"
        );
    }

    /// The hierarchy `M² ≪ M³` means the bounce beats evaporation for every
    /// astrophysical mass, by an overwhelming margin. If this ever flipped, the
    /// whole scenario would be dead and we would want a loud failure.
    #[test]
    fn bounce_beats_evaporation_for_astrophysical_masses() {
        for m in [1e11f64, 1e15, M_SUN, 1e6 * M_SUN] {
            assert_eq!(
                dominant_channel(m, 1.0),
                Channel::Bounce,
                "M = {m:e} kg should bounce before evaporating"
            );
        }
    }

    /// The crossover formula must actually be the crossing point: the two
    /// timescales are equal there.
    #[test]
    fn crossover_mass_equalizes_the_two_timescales() {
        for k in [1e-3f64, 1.0, 1e3] {
            let m = bounce_evaporation_crossover(k);
            assert!(
                rel(bounce_time(m, k), evaporation_lifetime(m)) < 1e-9,
                "k={k}: bounce {:e} vs evap {:e}",
                bounce_time(m, k),
                evaporation_lifetime(m)
            );
        }
    }

    /// Round-trip on the bounce law too.
    #[test]
    fn mass_bouncing_after_inverts_bounce_time() {
        let m = 1e12;
        let t = bounce_time(m, 7.0);
        assert!(rel(mass_bouncing_after(t, 7.0), m) < 1e-9);
    }

    /// M⁵ remnants must outlive M⁴ remnants for any mass above a Planck mass —
    /// the direction of the Martin-Dussaud revision.
    #[test]
    fn m5_remnants_outlive_m4_remnants() {
        for m in [1e-6f64, 1.0, 1e11] {
            let t4 = remnant_lifetime(m, RemnantLaw::M4, 1.0);
            let t5 = remnant_lifetime(m, RemnantLaw::M5, 1.0);
            assert!(t5 > t4, "M={m:e}: M5 {t5:e} should exceed M4 {t4:e}");
        }
    }

    /// The flood must actually contain the interesting transition: some `k`
    /// puts the signal in the FRB band and some does not. A sweep where every
    /// row agrees is a sweep that measured nothing.
    #[test]
    fn flood_brackets_the_frb_band() {
        let rows = flood(-6, 12, 4, RemnantLaw::M5);
        assert!(rows.iter().any(|r| r.in_frb_band), "no k lands in the FRB band");
        assert!(rows.iter().any(|r| !r.in_frb_band), "every k lands in the band — sweep too narrow");
    }
}
