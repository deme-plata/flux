//! Autocorrelation-based tempo (BPM) estimation.
//!
//! The core estimator (`score_at_bpm`, `autocorr_bpm_raw`, `autocorr_bpm`,
//! `deposit_onset`) and the deterministic xorshift64* PRNG (`Rng`) are ported
//! verbatim from `crates/flux-arxiv-latex/src/bin/ai_dj_live.rs` ("The Beat
//! Budget"). That file only ever *simulates* an onset-strength envelope
//! (`synth_envelope`) to Monte-Carlo-test the estimator against a known true
//! BPM — the estimator itself (autocorrelation over an onset-strength
//! envelope) is exactly what real onset-based beat trackers run before any
//! learned model is layered on top, so it also works unmodified on a REAL
//! list of onset timestamps. This module keeps both paths:
//!   - [`build_envelope_from_onsets`] + [`estimate_tempo_from_onsets`] — the
//!     real path: turn a caller-supplied list of onset timestamps (e.g. from
//!     an actual onset detector) into a tempo estimate.
//!   - [`simulate_tempo_estimate`] — the paper's Monte-Carlo path: synthesize
//!     a click envelope from a true BPM + jitter/noise/miss/spurious
//!     parameters and recover it, for reproducing "how well does this
//!     estimator do under condition X" without needing real audio.

use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────── PRNG
//
// Deterministic xorshift64* — ported verbatim from ai_dj_live.rs::Rng. Kept
// as our own generator (no `rand` dependency) so a given seed reproduces
// byte-identical simulated envelopes across runs, matching the paper's
// reproducibility contract.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform float in [0, 1).
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + self.f64() * (hi - lo)
    }
    /// Standard-normal via Box-Muller.
    pub fn gaussian(&mut self) -> f64 {
        let u1 = self.f64().max(1e-12);
        let u2 = self.f64();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

/// Deposit one onset's percussive attack/decay tail into the envelope,
/// linearly split across its two nearest frames by fractional position
/// rather than hard-rounded to the nearest frame. Ported verbatim from
/// `ai_dj_live.rs::deposit_onset` (see that file's doc comment for why the
/// fractional split matters — hard-rounding at a coarse frame rate creates a
/// systematic bias that isn't a property of the audio being modelled).
pub fn deposit_onset(env: &mut [f64], t: f64, frame_hz: f64, amp: f64) {
    let pos = t * frame_hz;
    let base = pos.floor() as isize;
    let frac = pos - pos.floor();
    let decay = [1.0, 0.5, 0.22]; // percussive attack/decay tail, per tap
    for (k, d) in decay.iter().enumerate() {
        let w_lo = amp * d * (1.0 - frac);
        let w_hi = amp * d * frac;
        let j_lo = base + k as isize;
        let j_hi = j_lo + 1;
        if j_lo >= 0 && (j_lo as usize) < env.len() {
            env[j_lo as usize] += w_lo;
        }
        if j_hi >= 0 && (j_hi as usize) < env.len() {
            env[j_hi as usize] += w_hi;
        }
    }
}

/// Build a real onset-strength envelope from a caller-supplied list of onset
/// timestamps (seconds). `amplitudes` defaults to 1.0 per onset if `None` or
/// shorter than `onsets_sec`. This is the path a real onset detector's output
/// would feed into.
pub fn build_envelope_from_onsets(onsets_sec: &[f64], amplitudes: Option<&[f64]>, duration_s: f64, frame_hz: f64) -> Vec<f64> {
    let n_frames = (duration_s * frame_hz).max(1.0) as usize;
    let mut env = vec![0.0f64; n_frames + 8];
    for (i, &t) in onsets_sec.iter().enumerate() {
        if t < 0.0 {
            continue;
        }
        let amp = amplitudes.and_then(|a| a.get(i)).copied().unwrap_or(1.0);
        deposit_onset(&mut env, t, frame_hz, amp);
    }
    env.truncate(n_frames);
    env
}

/// Synthesize a Monte-Carlo click envelope corrupted by human timing jitter,
/// missed onsets, spurious syncopated onsets, and broadband noise. Ported
/// verbatim from `ai_dj_live.rs::synth_envelope`.
#[allow(clippy::too_many_arguments)]
pub fn synth_envelope(
    bpm: f64,
    duration_s: f64,
    frame_hz: f64,
    jitter_ms: f64,
    miss_prob: f64,
    spurious_prob: f64,
    noise_amp: f64,
    rng: &mut Rng,
) -> Vec<f64> {
    let n_frames = (duration_s * frame_hz) as usize;
    let mut env = vec![0.0f64; n_frames + 8];
    let beat_period = 60.0 / bpm;
    let mut t = 0.0;
    while t < duration_s {
        if rng.f64() >= miss_prob {
            let jitter_s = rng.gaussian() * (jitter_ms / 1000.0);
            deposit_onset(&mut env, (t + jitter_s).max(0.0), frame_hz, 1.0);
        }
        if rng.f64() < spurious_prob {
            let frac = rng.range(0.2, 0.8);
            deposit_onset(&mut env, t + frac * beat_period, frame_hz, 0.6);
        }
        t += beat_period;
    }
    for v in env.iter_mut() {
        *v += rng.f64() * noise_amp;
    }
    env.truncate(n_frames);
    env
}

/// Autocorrelation score at a candidate BPM. Ported verbatim from
/// `ai_dj_live.rs::score_at_bpm`.
pub fn score_at_bpm(env: &[f64], frame_hz: f64, bpm: f64) -> Option<f64> {
    let lag = (60.0 / bpm * frame_hz).round() as usize;
    if lag < 1 || lag >= env.len() {
        return None;
    }
    let n = env.len() - lag;
    let mut score = 0.0;
    for i in 0..n {
        score += env[i] * env[i + lag];
    }
    Some(score / n as f64)
}

/// Raw (undisambiguated) autocorrelation tempo pick — octave-ambiguous by
/// construction. Ported verbatim from `ai_dj_live.rs::autocorr_bpm_raw`.
pub fn autocorr_bpm_raw(env: &[f64], frame_hz: f64, bpm_lo: f64, bpm_hi: f64, step: f64) -> f64 {
    let mut best_bpm = bpm_lo;
    let mut best_score = f64::MIN;
    let mut bpm = bpm_lo;
    while bpm <= bpm_hi {
        if let Some(score) = score_at_bpm(env, frame_hz, bpm) {
            if score > best_score {
                best_score = score;
                best_bpm = bpm;
            }
        }
        bpm += step;
    }
    best_bpm
}

/// Standard real-world octave tie-break margin. Ported verbatim from
/// `ai_dj_live.rs::OCTAVE_BIAS`.
pub const OCTAVE_BIAS: f64 = 0.85;

/// Octave-disambiguated estimate: prefer the faster octave when its score is
/// within `OCTAVE_BIAS` of the raw winner's. Ported verbatim from
/// `ai_dj_live.rs::autocorr_bpm`.
pub fn autocorr_bpm(env: &[f64], frame_hz: f64, bpm_lo: f64, bpm_hi: f64, step: f64) -> f64 {
    let raw = autocorr_bpm_raw(env, frame_hz, bpm_lo, bpm_hi, step);
    let raw_score = score_at_bpm(env, frame_hz, raw).unwrap_or(f64::MIN);
    let doubled = raw * 2.0;
    if doubled <= bpm_hi {
        if let Some(s) = score_at_bpm(env, frame_hz, doubled) {
            if s >= OCTAVE_BIAS * raw_score {
                return doubled;
            }
        }
    }
    raw
}

/// Result of a tempo estimate: the octave-disambiguated BPM, the raw
/// (pre-disambiguation) BPM, and whether disambiguation actually changed the
/// answer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TempoEstimate {
    pub estimated_bpm: f64,
    pub raw_bpm: f64,
    pub octave_corrected: bool,
}

/// Real path: estimate tempo from a caller-supplied list of onset
/// timestamps (seconds). Returns `None` for an empty onset list.
pub fn estimate_tempo_from_onsets(
    onsets_sec: &[f64],
    amplitudes: Option<&[f64]>,
    duration_s: Option<f64>,
    frame_hz: f64,
    bpm_lo: f64,
    bpm_hi: f64,
    bpm_step: f64,
) -> Option<TempoEstimate> {
    if onsets_sec.is_empty() {
        return None;
    }
    let max_t = onsets_sec.iter().cloned().fold(f64::MIN, f64::max);
    let duration = duration_s.unwrap_or(max_t + 2.0).max(1.0);
    let env = build_envelope_from_onsets(onsets_sec, amplitudes, duration, frame_hz);
    let raw = autocorr_bpm_raw(&env, frame_hz, bpm_lo, bpm_hi, bpm_step);
    let est = autocorr_bpm(&env, frame_hz, bpm_lo, bpm_hi, bpm_step);
    Some(TempoEstimate {
        estimated_bpm: est,
        raw_bpm: raw,
        octave_corrected: (est - raw).abs() > 1e-9,
    })
}

/// Parameters for the paper's Monte-Carlo simulate-and-recover mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulateParams {
    pub true_bpm: f64,
    #[serde(default = "default_duration_s")]
    pub duration_s: f64,
    #[serde(default = "default_frame_hz")]
    pub frame_hz: f64,
    #[serde(default)]
    pub jitter_ms: f64,
    #[serde(default = "default_miss_prob")]
    pub miss_prob: f64,
    #[serde(default = "default_spurious_prob")]
    pub spurious_prob: f64,
    #[serde(default)]
    pub noise_amp: f64,
    #[serde(default = "default_seed")]
    pub seed: u64,
}

fn default_duration_s() -> f64 {
    20.0
}
fn default_frame_hz() -> f64 {
    100.0
}
fn default_miss_prob() -> f64 {
    0.05
}
fn default_spurious_prob() -> f64 {
    0.15
}
fn default_seed() -> u64 {
    0x5EED_5EED
}

/// Simulate one synthetic run at the given condition and recover its tempo —
/// the same single-trial mechanics `ai_dj_live.rs::run_condition` averages
/// over many trials for the paper's sweep tables.
pub fn simulate_tempo_estimate(p: &SimulateParams) -> TempoEstimate {
    let mut rng = Rng::new(p.seed);
    let env = synth_envelope(p.true_bpm, p.duration_s, p.frame_hz, p.jitter_ms, p.miss_prob, p.spurious_prob, p.noise_amp, &mut rng);
    let raw = autocorr_bpm_raw(&env, p.frame_hz, 60.0, 200.0, 0.5);
    let est = autocorr_bpm(&env, p.frame_hz, 60.0, 200.0, 0.5);
    TempoEstimate {
        estimated_bpm: est,
        raw_bpm: raw,
        octave_corrected: (est - raw).abs() > 1e-9,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clean, low-jitter 124 BPM click track should recover close to 124.
    #[test]
    fn estimates_clean_house_tempo() {
        let true_bpm = 124.0;
        let beat_period = 60.0 / true_bpm;
        let mut onsets = Vec::new();
        let mut t = 0.0;
        while t < 20.0 {
            onsets.push(t);
            t += beat_period;
        }
        let est = estimate_tempo_from_onsets(&onsets, None, Some(20.0), 100.0, 60.0, 200.0, 0.5).unwrap();
        let pct_err = (est.estimated_bpm - true_bpm).abs() / true_bpm * 100.0;
        assert!(pct_err < 4.0, "expected close to {true_bpm} BPM, got {} ({pct_err:.2}% err)", est.estimated_bpm);
    }

    #[test]
    fn empty_onsets_returns_none() {
        assert!(estimate_tempo_from_onsets(&[], None, None, 100.0, 60.0, 200.0, 0.5).is_none());
    }

    /// Reproduces (a smaller-scale, single-trial version of) the paper's
    /// clean-condition Monte-Carlo run: same seed => same estimate, every
    /// time (the reproducibility contract this whole module inherits from
    /// ai_dj_live.rs).
    #[test]
    fn simulate_is_deterministic_given_seed() {
        let params = SimulateParams {
            true_bpm: 124.0,
            duration_s: 20.0,
            frame_hz: 100.0,
            jitter_ms: 5.0,
            miss_prob: 0.05,
            spurious_prob: 0.15,
            noise_amp: 0.1,
            seed: 0x51DE_51DE,
        };
        let a = simulate_tempo_estimate(&params);
        let b = simulate_tempo_estimate(&params);
        assert_eq!(a, b, "same seed must reproduce byte-identical estimate");
    }

    #[test]
    fn simulate_clean_condition_recovers_true_bpm() {
        let params = SimulateParams {
            true_bpm: 124.0,
            duration_s: 20.0,
            frame_hz: 100.0,
            jitter_ms: 5.0,
            miss_prob: 0.05,
            spurious_prob: 0.15,
            noise_amp: 0.1,
            seed: 0x51DE_51DE,
        };
        let est = simulate_tempo_estimate(&params);
        let pct_err = (est.estimated_bpm - params.true_bpm).abs() / params.true_bpm * 100.0;
        assert!(pct_err < 4.0, "clean condition should recover close to true BPM, got {pct_err:.2}% err");
    }

    /// The octave-disambiguation step must be able to change the raw pick —
    /// exercised at the exact condition ai_dj_live.rs isolates for this
    /// (House tempo, ~no jitter).
    #[test]
    fn octave_disambiguation_can_change_the_pick() {
        let mut any_corrected = false;
        for k in 0..50u64 {
            let mut rng = Rng::new(0x0C7A_0C7A ^ (k.wrapping_mul(0x9E37_79B1)));
            let env = synth_envelope(124.0, 20.0, 100.0, 1.0, 0.05, 0.15, 0.2, &mut rng);
            let raw = autocorr_bpm_raw(&env, 100.0, 60.0, 200.0, 0.5);
            let est = autocorr_bpm(&env, 100.0, 60.0, 200.0, 0.5);
            if (est - raw).abs() > 1e-9 {
                any_corrected = true;
                break;
            }
        }
        assert!(any_corrected, "expected at least one trial where disambiguation changes the raw pick");
    }
}
