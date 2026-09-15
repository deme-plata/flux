//! The planet as a hand.
//!
//! The Earth Rotation Angle is a PHASE: it tells the time of day modulo one turn and cannot
//! count turns — exactly the paper's atom. `ERA = 2π(0.7790572732640 + 1.00273781191135448·
//! (JD_UT1 − 2451545.0))` (IERS Conventions 2010, eq. 5.15); `UT1 = UTC + (UT1−UTC)` with
//! UT1−UTC from the IERS finals via the sigil-earth feed (`/v1/earth/latest → today.ut1utc`).
//!
//! The Earth also has slower hands — the annual cycle in the length of day, the 433-day
//! Chandler wobble of the pole — with nearly-coprime periods (365, 433) whose CRT range is
//! 158,045 days ≈ 433 years: a natural Maya Long Count. Whether they can be COMPOSED is a
//! question of resolution: the paper needs every hand's error under ¼ unit. [`fit_hands`]
//! fits the phases from the feed's own series and reports each hand's `σ_t` and equivalent
//! `Z = 1/(2σ_t)`; [`calendar_verdict`] says honestly whether the composition works.

use crate::crt;
use crate::PI;
use serde::{Deserialize, Serialize};

pub const JD_UNIX_EPOCH: f64 = 2_440_587.5;
pub const JD_J2000: f64 = 2_451_545.0;
/// Turns per UT1 day (IERS 2010 eq. 5.15).
pub const TURNS_PER_DAY: f64 = 1.002_737_811_911_354_48;
pub const ERA_J2000_TURNS: f64 = 0.779_057_273_264_0;
/// The stellar day — one full rotation against the ICRF — = 86400 / TURNS_PER_DAY =
/// 86164.0989 s. (The *sidereal* day, 86164.0905 s, is measured against the precessing
/// equinox and is 8.4 ms shorter; the ERA hand turns with the stellar day.)
pub const STELLAR_DAY_S: f64 = 86_400.0 / TURNS_PER_DAY;
pub const SIDEREAL_DAY_S: f64 = 86_164.0905;
pub const DAYS_YEAR: f64 = 365.25;
pub const CHANDLER_DAYS: f64 = 433.0;
pub const MJD_UNIX: f64 = 40_587.0;

pub fn jd_ut1(unix_utc_s: f64, ut1_minus_utc_s: f64) -> f64 {
    (unix_utc_s + ut1_minus_utc_s) / 86_400.0 + JD_UNIX_EPOCH
}

/// Earth Rotation Angle in radians, `[0, 2π)`.
pub fn era_rad(jd_ut1: f64) -> f64 {
    let f = ERA_J2000_TURNS + TURNS_PER_DAY * (jd_ut1 - JD_J2000);
    2.0 * PI * (f - f.floor())
}

pub fn era_deg(jd_ut1: f64) -> f64 { era_rad(jd_ut1).to_degrees() }

/// The rotation hand: fraction of the current turn and seconds into the sidereal day.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotationHand {
    pub jd_ut1: f64,
    pub era_deg: f64,
    pub turn_fraction: f64,
    pub seconds_into_sidereal_day: f64,
    pub ut1_minus_utc_s: f64,
    /// Whole turns since J2000 — the count the phase alone cannot give (comes from the calendar).
    pub turns_since_j2000: i64,
}

pub fn rotation_hand(unix_utc_s: f64, ut1_minus_utc_s: f64) -> RotationHand {
    let jd = jd_ut1(unix_utc_s, ut1_minus_utc_s);
    let f = ERA_J2000_TURNS + TURNS_PER_DAY * (jd - JD_J2000);
    let frac = f - f.floor();
    RotationHand {
        jd_ut1: jd,
        era_deg: frac * 360.0,
        turn_fraction: frac,
        seconds_into_sidereal_day: frac * STELLAR_DAY_S,
        ut1_minus_utc_s,
        turns_since_j2000: f.floor() as i64,
    }
}

/// A slow Earth hand fitted from a series: `value(t) = … + a cos(ωt) + b sin(ωt)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EarthHand {
    pub name: String,
    pub period_days: f64,
    pub amplitude: f64,
    /// Phase `φ = atan2(b, a)`; the cycle peaks at `t = φ/ω` days after the fit origin.
    pub phase_rad: f64,
    /// Days since the last maximum — the hand's remainder `t mod P`.
    pub remainder_days: f64,
    /// Phase uncertainty propagated to days: `σ_t = (σ_R/R)·P/2π`, `σ_R ≈ σ_resid·√(2/n)`.
    pub sigma_t_days: f64,
    /// The paper's resolution multiplier this hand is equivalent to: `Z = 1/(2σ_t)`.
    pub z_equivalent: f64,
    pub n_samples: usize,
    pub resid_std: f64,
}

/// Least squares by normal equations (Gauss–Jordan). `x[i]` = basis row for sample `i`.
pub fn lstsq(x: &[Vec<f64>], y: &[f64]) -> Vec<f64> {
    let n = x.first().map(|r| r.len()).unwrap_or(0);
    let mut m = vec![vec![0.0; n + 1]; n];
    for (row, &yy) in x.iter().zip(y) {
        for i in 0..n {
            for j in 0..n { m[i][j] += row[i] * row[j]; }
            m[i][n] += row[i] * yy;
        }
    }
    for c in 0..n {
        let piv = (c..n).max_by(|&a, &b| m[a][c].abs().partial_cmp(&m[b][c].abs()).unwrap()).unwrap();
        m.swap(c, piv);
        if m[c][c].abs() < 1e-300 { continue; }
        for r in 0..n {
            if r != c {
                let f = m[r][c] / m[c][c];
                for k in c..=n { m[r][k] -= f * m[c][k]; }
            }
        }
    }
    (0..n).map(|i| if m[i][i] != 0.0 { m[i][n] / m[i][i] } else { 0.0 }).collect()
}

/// Fit `[1, t, (cos ωt, sin ωt) per cycle]` to `(t_days, value)` samples, `t` relative to
/// `t_now`, and turn each cycle into a hand.
pub fn fit_hands(samples: &[(f64, f64)], t_now: f64, cycles: &[(&str, f64)]) -> Vec<EarthHand> {
    if samples.len() < 2 * cycles.len() + 4 { return vec![]; }
    let rows: Vec<Vec<f64>> = samples.iter().map(|&(t, _)| {
        let tt = t - t_now;
        let mut b = vec![1.0, tt / DAYS_YEAR];
        for &(_, p) in cycles { let w = 2.0 * PI / p; b.push((w * tt).cos()); b.push((w * tt).sin()); }
        b
    }).collect();
    let y: Vec<f64> = samples.iter().map(|&(_, v)| v).collect();
    let coef = lstsq(&rows, &y);
    let n = y.len() as f64;
    let resid_var = rows.iter().zip(&y).map(|(r, yy)| (yy - r.iter().zip(&coef).map(|(a, c)| a * c).sum::<f64>()).powi(2)).sum::<f64>() / (n - coef.len() as f64).max(1.0);
    let resid_std = resid_var.sqrt();
    cycles.iter().enumerate().map(|(k, &(name, p))| {
        let (a, b) = (coef[2 + 2 * k], coef[3 + 2 * k]);
        let amp = (a * a + b * b).sqrt();
        let phase = b.atan2(a);
        let w = 2.0 * PI / p;
        let t_max = phase / w; // days after t_now
        let remainder = (-t_max).rem_euclid(p);
        let sigma_r = resid_std * (2.0 / n).sqrt();
        let sigma_t = if amp > 0.0 { (sigma_r / amp) * p / (2.0 * PI) } else { f64::INFINITY };
        EarthHand { name: name.to_string(), period_days: p, amplitude: amp, phase_rad: phase, remainder_days: remainder, sigma_t_days: sigma_t, z_equivalent: 1.0 / (2.0 * sigma_t), n_samples: y.len(), resid_std }
    }).collect()
}

/// Can the planet's own hands be CRT-composed at day resolution? The paper's condition is
/// every `|t_j − t̃_j| < ¼` unit; with Gaussian `σ_t` that holds with probability
/// `erf(0.25/(σ_t√2))` per hand.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarVerdict {
    pub periods_days: Vec<u64>,
    pub pairwise_coprime: bool,
    pub range_days: u128,
    pub range_years: f64,
    pub needed_sigma_days: f64,
    pub per_hand_ok_probability: Vec<f64>,
    pub composition_ok_probability: f64,
    pub feasible_at_day_resolution: bool,
    pub z_min_for_99pct: f64,
    pub verdict: String,
}

fn erf(x: f64) -> f64 {
    // Abramowitz–Stegun 7.1.26, |ε| < 1.5e-7
    let t = 1.0 / (1.0 + 0.3275911 * x.abs());
    let y = 1.0 - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t * (-x * x).exp();
    if x >= 0.0 { y } else { -y }
}

pub fn calendar_verdict(hands: &[EarthHand]) -> CalendarVerdict {
    let periods: Vec<u64> = hands.iter().map(|h| h.period_days.round() as u64).collect();
    let coprime = crt::pairwise_coprime(&periods);
    let range = if coprime { crt::range(&periods) } else { 0 };
    let per: Vec<f64> = hands.iter().map(|h| erf(0.25 / (h.sigma_t_days * std::f64::consts::SQRT_2))).collect();
    let all: f64 = per.iter().product();
    let feasible = coprime && all > 0.99;
    let worst = hands.iter().map(|h| h.sigma_t_days).fold(0.0, f64::max);
    let verdict = if !coprime { "periods are not pairwise coprime — no CRT composition".to_string() }
        else if feasible { format!("composable: P(integer part right) = {all:.3} over {range} days") }
        else { format!("NOT composable at day resolution: worst hand σ_t = {worst:.1} d, the protocol needs < 0.25 d (Z_equiv {:.2}); P(integer part right) = {all:.3}", 1.0 / (2.0 * worst)) };
    CalendarVerdict { periods_days: periods, pairwise_coprime: coprime, range_days: range, range_years: range as f64 / DAYS_YEAR, needed_sigma_days: 0.25, per_hand_ok_probability: per, composition_ok_probability: all, feasible_at_day_resolution: feasible, z_min_for_99pct: crt::z_min(hands.len().max(1), 0.99), verdict }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn era_at_j2000_and_the_sidereal_day() {
        // ERA(J2000.0) = 2π·0.7790572732640 = 280.46°
        assert!((era_deg(JD_J2000) - 280.4606).abs() < 1e-3);
        assert!((STELLAR_DAY_S - 86_164.0989).abs() < 1e-3, "{STELLAR_DAY_S}");
        assert!(STELLAR_DAY_S - SIDEREAL_DAY_S > 0.008 && STELLAR_DAY_S - SIDEREAL_DAY_S < 0.009);
        // one UT1 day later the angle advanced by 360.9856°
        let d = (era_deg(JD_J2000 + 1.0) - era_deg(JD_J2000)).rem_euclid(360.0);
        assert!((d - 0.9856).abs() < 1e-3, "{d}");
    }

    #[test]
    fn rotation_hand_is_a_phase_not_a_counter() {
        let h1 = rotation_hand(1_789_494_976.0, -0.0050609);
        let h2 = rotation_hand(1_789_494_976.0 + STELLAR_DAY_S, -0.0050609);
        assert!((h1.turn_fraction - h2.turn_fraction).abs() < 1e-6, "same phase one sidereal day later");
        assert_eq!(h2.turns_since_j2000, h1.turns_since_j2000 + 1);
        assert!(h1.era_deg >= 0.0 && h1.era_deg < 360.0);
    }

    #[test]
    fn fitted_hands_recover_a_known_phase_and_the_verdict_is_honest() {
        // synthetic LOD-like series: annual amplitude 0.35 ms peaking 40 d before now, noise 0.2 ms
        let mut rng = crt::Rng::new(3);
        let w = 2.0 * PI / DAYS_YEAR;
        let samples: Vec<(f64, f64)> = (0..2000).map(|i| {
            let t = i as f64 - 2000.0;
            let noise = (rng.f64() + rng.f64() + rng.f64() - 1.5) * 0.4;
            (t, 1.0 + 0.35 * (w * (t + 40.0)).cos() + noise)
        }).collect();
        let hands = fit_hands(&samples, 0.0, &[("annual", DAYS_YEAR)]);
        assert_eq!(hands.len(), 1);
        let h = &hands[0];
        assert!((h.amplitude - 0.35).abs() < 0.03, "{}", h.amplitude);
        // last max was 40 d ago → remainder ≈ 40
        assert!((h.remainder_days - 40.0).abs() < 4.0, "{}", h.remainder_days);
        assert!(h.sigma_t_days > 0.25, "a real Earth hand is coarser than ¼ day: {}", h.sigma_t_days);
        let v = calendar_verdict(&hands);
        assert!(!v.feasible_at_day_resolution);
        assert!(v.verdict.starts_with("NOT composable"));
        // and with a Chandler hand the periods ARE coprime and the range is 433 years
        let two = vec![h.clone(), EarthHand { name: "chandler".into(), period_days: CHANDLER_DAYS, ..h.clone() }];
        let v2 = calendar_verdict(&two);
        assert!(v2.pairwise_coprime);
        assert_eq!(v2.range_days, 365 * 433);
    }
}
