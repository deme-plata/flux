//! ai_dj_live — "The Beat Budget": a computed latency, beat-tracking and
//! harmonic-mixing analysis of AI-assisted DJ / live-performance technology.
//!
//! Same dogfood contract as `thermodynamic_ledger` / `legibility_dividend` /
//! `idle_machine`: every quantitative claim in this paper is COMPUTED at
//! document-generation time by THIS binary, not quoted from a paper or
//! invented as prose. Concretely:
//!   - a real, seeded Monte-Carlo beat-tracking simulation (synthetic click
//!     envelopes with human timing jitter, missed/spurious onsets and
//!     broadband noise, run through an autocorrelation tempo estimator);
//!   - first-principles acoustic-propagation and audio-buffer latency
//!     arithmetic (speed of sound, sample-rate/buffer-size round trips);
//!   - a Camelot-wheel harmonic-compatibility graph built and greedily
//!     traversed in code (not hand-counted);
//!   - an FFT flop-count budget for the STFT/ISTFT front end of a real-time
//!     stem separator.
//! Related work is a live arXiv API sweep (35 papers, fetched via WebFetch on
//! export.arxiv.org/api/query) parsed and cited by flux-arxiv-latex.
//!
//! Usage: ai_dj_live [arxiv.json] [out_dir]

use flux_arxiv_latex::doc::{Block, Document};
use flux_arxiv_latex::{bibliography, latex_escape, parse_arxiv_json, related_work_section, ArxivPaper};

// ───────────────────────────────────────────────────────── formatting helpers

/// Format a number for math mode: plain when small, \times10^{n} otherwise.
fn sci(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return format!("{x}");
    }
    let exp = x.abs().log10().floor() as i32;
    if (-2..=4).contains(&exp) {
        let s = format!("{:.3}", x);
        let s = s.trim_end_matches('0').trim_end_matches('.');
        s.to_string()
    } else {
        let mant = x / 10f64.powi(exp);
        format!("{:.2}\\times10^{{{}}}", mant, exp)
    }
}

fn para(s: String) -> Block {
    Block::Raw(format!("{s}\n\n"))
}

// ───────────────────────────────────────────────────────────────────── PRNG
//
// A tiny deterministic xorshift64* generator. Using our own PRNG (rather than
// adding a `rand` dependency) keeps the crate's dependency surface unchanged
// and, more importantly, means every run of this binary with the same seed
// reproduces byte-identical figures — a requirement for "the paper corrects
// itself when you recompile it", not "the paper re-rolls the dice".

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform float in [0, 1).
    fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + self.f64() * (hi - lo)
    }
    /// Standard-normal via Box–Muller.
    fn gaussian(&mut self) -> f64 {
        let u1 = self.f64().max(1e-12);
        let u2 = self.f64();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

// ──────────────────────────────────────────────────── beat-tracking simulator
//
// Models the onset-strength envelope a real beat tracker works from: an
// impulse (with a short percussive decay tail) at every beat, corrupted by
// (a) human/production timing jitter, (b) missed onsets (quiet passages),
// (c) spurious syncopated onsets (hats/claps on off-beats), and (d) broadband
// texture noise. Tempo is then recovered by autocorrelation over the
// envelope, the same family of technique real onset-based trackers use
// before any learned model is layered on top.

/// Deposit one onset's percussive attack/decay tail into the envelope,
/// linearly split across its two nearest frames by fractional position
/// rather than hard-rounded to the nearest frame. Hard rounding at a coarse
/// 100 Hz frame rate turns non-integer beat periods (i.e.\ almost every real
/// BPM) into a systematic sub-frame drift that biases the autocorrelation
/// search toward an unrelated lag — a discretisation artifact of the
/// simulator, not a property of the audio it is modelling, so it is removed
/// here rather than left in to quietly shape the results.
fn deposit_onset(env: &mut [f64], t: f64, frame_hz: f64, amp: f64) {
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

fn synth_envelope(
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
            let frac = rng.range(0.2, 0.8); // an "and"/off-beat subdivision
            deposit_onset(&mut env, t + frac * beat_period, frame_hz, 0.6);
        }
        t += beat_period;
    }
    for v in env.iter_mut() {
        *v += rng.f64() * noise_amp; // broadband percussive-texture noise
    }
    env.truncate(n_frames);
    env
}

fn score_at_bpm(env: &[f64], frame_hz: f64, bpm: f64) -> Option<f64> {
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

/// Raw (undisambiguated) autocorrelation tempo pick: the global-max-score lag
/// in [bpm_lo, bpm_hi]. A periodic onset train autocorrelates near-equally at
/// every integer multiple of its true period, so whenever the search band
/// spans more than one such multiple this bare argmax is octave-ambiguous BY
/// CONSTRUCTION — not a bug, the defining failure mode of naive periodicity
/// tempo estimation, and the reason every production beat tracker adds an
/// explicit disambiguation stage (see `autocorr_bpm` below).
fn autocorr_bpm_raw(env: &[f64], frame_hz: f64, bpm_lo: f64, bpm_hi: f64, step: f64) -> f64 {
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

/// Octave-disambiguated estimate: take the raw argmax, then prefer the
/// FASTER octave (double the tempo, i.e.\ half the lag) whenever its score is
/// within `OCTAVE_BIAS` of the raw winner's — the standard real-world
/// tie-break (near-equal periodicity strength is resolved toward the
/// shorter/faster candidate rather than accepted at face value).
const OCTAVE_BIAS: f64 = 0.85;

fn autocorr_bpm(env: &[f64], frame_hz: f64, bpm_lo: f64, bpm_hi: f64, step: f64) -> f64 {
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

struct SweepResult {
    accuracy_pct: f64,
    octave_err_pct: f64,
    mape_pct: f64,
}

/// Run `trials` independent, seeded simulations at one (bpm, jitter, noise)
/// condition and return the aggregate accuracy statistics.
fn run_condition(true_bpm: f64, jitter_ms: f64, noise_amp: f64, trials: usize, seed0: u64) -> SweepResult {
    let duration_s = 20.0;
    let frame_hz = 100.0;
    let mut correct = 0usize;
    let mut octave = 0usize;
    let mut abs_err_sum = 0.0;
    for k in 0..trials {
        let mut rng = Rng::new(seed0.wrapping_add(k as u64 * 0x9E37_79B1));
        let env = synth_envelope(true_bpm, duration_s, frame_hz, jitter_ms, 0.05, 0.15, noise_amp, &mut rng);
        let est = autocorr_bpm(&env, frame_hz, 60.0, 200.0, 0.5);
        let pct_err = (est - true_bpm).abs() / true_bpm * 100.0;
        abs_err_sum += pct_err;
        if pct_err < 4.0 {
            correct += 1;
        } else if (est - 2.0 * true_bpm).abs() / (2.0 * true_bpm) < 0.04
            || (est - true_bpm / 2.0).abs() / (true_bpm / 2.0) < 0.04
        {
            octave += 1;
        }
    }
    SweepResult {
        accuracy_pct: correct as f64 / trials as f64 * 100.0,
        octave_err_pct: octave as f64 / trials as f64 * 100.0,
        mape_pct: abs_err_sum / trials as f64,
    }
}

// ────────────────────────────────────────────────────── Camelot-wheel graph
//
// The Camelot system (used by rekordbox, Serato, Mixed In Key, engine DJ,
// ...) relabels the 24 major/minor keys as 1A..12A (minor) and 1B..12B
// (major) arranged so that harmonically "safe" transitions are adjacency on
// a wheel: the SAME code, an ADJACENT number with the same letter (a
// perfect-fifth move), or the SAME number with the other letter (relative
// major/minor). We build this as an explicit graph over the 24 codes and
// count/traverse it in code rather than asserting the percentages by hand.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Camelot {
    num: i16,
    letter: u8, // 0 = A (minor), 1 = B (major)
}

fn all_camelot() -> Vec<Camelot> {
    let mut v = Vec::with_capacity(24);
    for n in 1..=12i16 {
        for l in [0u8, 1u8] {
            v.push(Camelot { num: n, letter: l });
        }
    }
    v
}

fn compatible_strict(a: Camelot, b: Camelot) -> bool {
    if a == b {
        return false;
    }
    let d = (a.num - b.num).rem_euclid(12);
    (a.letter == b.letter && (d == 1 || d == 11)) || (a.num == b.num && a.letter != b.letter)
}

fn compatible_extended(a: Camelot, b: Camelot) -> bool {
    if compatible_strict(a, b) {
        return true;
    }
    let d = (a.num - b.num).rem_euclid(12);
    a.letter == b.letter && (d == 2 || d == 10) // the "energy boost" jump some curators allow
}

#[derive(Clone, Copy)]
struct Track {
    camelot: Camelot,
    bpm: f64,
}

fn build_library(n: usize, bpm_lo: f64, bpm_hi: f64, rng: &mut Rng) -> Vec<Track> {
    (0..n)
        .map(|_| {
            let num = 1 + (rng.f64() * 12.0) as i16;
            let num = num.clamp(1, 12);
            let letter = if rng.f64() < 0.5 { 0 } else { 1 };
            Track { camelot: Camelot { num, letter }, bpm: rng.range(bpm_lo, bpm_hi) }
        })
        .collect()
}

/// Greedily consume `library`: from the current track, jump to the nearest-
/// BPM harmonically-compatible remaining track; if none exists within
/// `tol_pct`, take a "hard cut" (start a fresh run from an arbitrary
/// remaining track). Returns (mean run length before a cut, number of cuts).
fn greedy_set(library: &[Track], tol_pct: f64, extended: bool) -> (f64, usize) {
    let mut remaining: Vec<usize> = (0..library.len()).collect();
    let mut cur = remaining.remove(0);
    let mut run_lengths = vec![1usize];
    let mut hard_cuts = 0usize;
    while !remaining.is_empty() {
        let cur_t = library[cur];
        let mut best: Option<(usize, f64)> = None;
        for (pos, &idx) in remaining.iter().enumerate() {
            let t = library[idx];
            let ok = if extended { compatible_extended(cur_t.camelot, t.camelot) } else { compatible_strict(cur_t.camelot, t.camelot) };
            if ok {
                let bd = (t.bpm - cur_t.bpm).abs() / cur_t.bpm * 100.0;
                if bd <= tol_pct && best.map_or(true, |(_, bb)| bd < bb) {
                    best = Some((pos, bd));
                }
            }
        }
        match best {
            Some((pos, _)) => {
                cur = remaining.remove(pos);
                *run_lengths.last_mut().unwrap() += 1;
            }
            None => {
                hard_cuts += 1;
                cur = remaining.remove(0);
                run_lengths.push(1);
            }
        }
    }
    let avg = run_lengths.iter().sum::<usize>() as f64 / run_lengths.len() as f64;
    (avg, hard_cuts)
}

// ───────────────────────────────────────────────────────── first-principles

fn fft_flops(n: f64) -> f64 {
    5.0 * n * n.log2() // standard radix-2 complex-FFT flop approximation
}

fn buffer_latency_ms(buf: f64, sr: f64, stages: f64) -> f64 {
    buf / sr * 1000.0 * stages
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let json_path = args.get(1).map(String::as_str).unwrap_or("crates/flux-arxiv-latex/dj_ai_live.arxiv.json");
    let out_dir = args.get(2).map(String::as_str).unwrap_or("/tmp/ai-dj-live");

    let papers: Vec<ArxivPaper> = std::fs::read_to_string(json_path)
        .ok()
        .and_then(|j| parse_arxiv_json(&j).ok())
        .unwrap_or_default();

    // ============================================================ SECTION 1
    // Acoustic propagation + audio-buffer round-trip latency.
    let speed_of_sound = 343.0_f64; // m/s, dry air at 20C
    let ms_per_m = 1000.0 / speed_of_sound;
    let venue_depths = [5.0, 10.0, 20.0, 40.0, 80.0];
    let venue_ms: Vec<f64> = venue_depths.iter().map(|d| d * ms_per_m).collect();

    let sample_rates = [44_100.0, 48_000.0, 96_000.0];
    let buffer_sizes = [32.0, 64.0, 128.0, 256.0, 512.0, 1024.0];
    let sync_threshold_ms = 20.0_f64; // ASSUMED: commonly used tight-coupling engineering budget
    let max_buf_for_sr: Vec<f64> = sample_rates
        .iter()
        .map(|&sr| (sync_threshold_ms / 1000.0 * sr / 2.0).floor())
        .collect();

    // ============================================================ SECTION 2
    // Beat-tracking Monte Carlo sweep.
    let genres: [(&str, f64); 3] = [("Hip-Hop", 90.0), ("House", 124.0), ("Drum \\& Bass", 174.0)];
    let jitters = [0.0, 10.0, 20.0, 30.0, 40.0];
    let trials = 60usize;
    let mut jitter_rows: Vec<(String, f64, SweepResult)> = Vec::new();
    for (name, bpm) in genres {
        for &j in &jitters {
            let r = run_condition(bpm, j, 0.2, trials, ((bpm as u64) << 20) ^ (j as u64) ^ 0xABCD);
            jitter_rows.push((name.to_string(), j, r));
        }
    }
    let noise_levels = [0.0, 0.2, 0.4, 0.6];
    let mut noise_rows: Vec<(f64, SweepResult)> = Vec::new();
    for &na in &noise_levels {
        let r = run_condition(124.0, 15.0, na, trials, 0x1357_2468 ^ (na * 1000.0) as u64);
        noise_rows.push((na, r));
    }
    // headline pull-quote numbers
    let clean_result = run_condition(124.0, 5.0, 0.1, 200, 0x51DE_51DE);
    let harsh_result = run_condition(124.0, 35.0, 0.6, 200, 0xBEEF_CAFE);

    // Illustrative raw-vs-disambiguated comparison at a single, otherwise-easy
    // condition (House, essentially no jitter) to isolate and quantify the
    // octave-ambiguity failure mode from the timing-jitter failure mode.
    let raw_trials = 200usize;
    let mut raw_correct = 0usize;
    let mut disamb_correct = 0usize;
    for k in 0..raw_trials {
        let mut rng = Rng::new(0x0C7A_0C7A ^ (k as u64 * 0x9E37_79B1));
        let env = synth_envelope(124.0, 20.0, 100.0, 1.0, 0.05, 0.15, 0.2, &mut rng);
        let raw_est = autocorr_bpm_raw(&env, 100.0, 60.0, 200.0, 0.5);
        let disamb_est = autocorr_bpm(&env, 100.0, 60.0, 200.0, 0.5);
        if (raw_est - 124.0).abs() / 124.0 < 0.04 {
            raw_correct += 1;
        }
        if (disamb_est - 124.0).abs() / 124.0 < 0.04 {
            disamb_correct += 1;
        }
    }
    let raw_correct_pct = raw_correct as f64 / raw_trials as f64 * 100.0;
    let disamb_correct_pct = disamb_correct as f64 / raw_trials as f64 * 100.0;

    // ============================================================ SECTION 3
    // Microphone-array time-of-flight geometry for crowd-response sensing.
    let mic_spacings = [0.10, 0.30, 1.0, 3.0];
    let tau_max_ms: Vec<f64> = mic_spacings.iter().map(|d| d / speed_of_sound * 1000.0).collect();
    let sr_sense = 48_000.0_f64;
    let sample_period_ms = 1000.0 / sr_sense;
    let samples_across: Vec<f64> = tau_max_ms.iter().map(|t| t / sample_period_ms).collect();
    let n_elements = 8.0_f64;
    let crowd_freq_lo = 200.0_f64; // Hz, bass-driven "drop" transient energy
    let crowd_freq_hi = 2000.0_f64; // Hz, cheering/vocal energy
    let wavelength_lo = speed_of_sound / crowd_freq_hi;
    let wavelength_hi = speed_of_sound / crowd_freq_lo;
    let beamwidth_deg_narrow = (wavelength_lo / (n_elements * 0.30)).to_degrees();
    let beamwidth_deg_wide = (wavelength_hi / (n_elements * 0.30)).to_degrees();

    // ============================================================ SECTION 4
    // Camelot-wheel compatibility graph.
    let nodes = all_camelot();
    let mut strict_edges = 0usize;
    let mut ext_edges = 0usize;
    for &a in &nodes {
        for &b in &nodes {
            if a != b {
                if compatible_strict(a, b) {
                    strict_edges += 1;
                }
                if compatible_extended(a, b) {
                    ext_edges += 1;
                }
            }
        }
    }
    let total_ordered = nodes.len() * (nodes.len() - 1);
    let strict_pct = strict_edges as f64 / total_ordered as f64 * 100.0;
    let ext_pct = ext_edges as f64 / total_ordered as f64 * 100.0;

    let mut rng_lib = Rng::new(0x600D_600D);
    let within_genre = build_library(500, 120.0, 140.0, &mut rng_lib);
    let cross_genre = build_library(500, 85.0, 175.0, &mut rng_lib);
    let (within_avg_strict, within_cuts_strict) = greedy_set(&within_genre, 6.0, false);
    let (within_avg_ext, within_cuts_ext) = greedy_set(&within_genre, 6.0, true);
    let (cross_avg_strict, cross_cuts_strict) = greedy_set(&cross_genre, 6.0, false);
    let (cross_avg_ext, cross_cuts_ext) = greedy_set(&cross_genre, 6.0, true);
    let tol_sweep = [3.0, 6.0, 10.0];
    let tol_results: Vec<(f64, f64, usize)> = tol_sweep
        .iter()
        .map(|&tol| {
            let (avg, cuts) = greedy_set(&within_genre, tol, false);
            (tol, avg, cuts)
        })
        .collect();

    // ============================================================ SECTION 5
    // STFT/ISTFT compute budget for a real-time stem separator front end.
    let windows = [1024.0, 2048.0, 4096.0];
    let sr_audio = 44_100.0_f64;
    let track_s = 180.0_f64; // 3-minute track
    let transforms_per_frame = 5.0_f64; // 1 forward (mixture) + 4 inverse (stems)
    let throughput_classes: [(&str, f64); 3] = [
        ("scalar core (illustrative, $\\sim 2$\\,GFLOP/s)", 2.0e9),
        ("SIMD-vectorized core (illustrative, $\\sim 30$\\,GFLOP/s)", 3.0e10),
        ("parallel/accelerated (illustrative, $\\sim 500$\\,GFLOP/s)", 5.0e11),
    ];
    let mut stft_rows: Vec<(f64, f64, Vec<(String, f64, f64)>)> = Vec::new();
    for &w in &windows {
        let hop = w / 4.0;
        let frames = (track_s * sr_audio / hop).ceil();
        let total_flops = frames * transforms_per_frame * fft_flops(w);
        let mut per_class = Vec::new();
        for (name, thr) in throughput_classes {
            let time_s = total_flops / thr;
            let rtf = track_s / time_s;
            per_class.push((name.to_string(), time_s, rtf));
        }
        stft_rows.push((w, frames, per_class));
    }

    // ============================================================ SECTION 6
    // Full-chain latency budget (compose the pieces above).
    let chosen_buf = 256.0_f64;
    let chosen_sr = 48_000.0_f64;
    let buf_latency = buffer_latency_ms(chosen_buf, chosen_sr, 2.0);
    let sense_latency = tau_max_ms[1]; // 0.30 m spacing
    let decision_latency_ms = 5.0_f64; // ASSUMED: one control-loop tick for a curation/BPM-sync decision
    let total_chain_ms = sense_latency + decision_latency_ms + buf_latency;
    let venue_20m_ms = 20.0 * ms_per_m;

    // ================================================================= tex
    let mut doc = Document::new("article")
        .option("11pt")
        .package_opt("inputenc", &["utf8"])
        .package_opt("geometry", &["margin=1.1in"])
        .package("amsmath")
        .package("amssymb")
        .package("booktabs")
        .package_opt("hyperref", &["hidelinks"])
        .preamble(concat!(
            "\\providecommand{\\textcite}[1]{\\cite{#1}}\n",
            "\\title{The Beat Budget\\\\[6pt]\\large A Computed Latency, Beat-Tracking and Harmonic-Mixing Analysis of AI-Assisted DJ and Live-Performance Systems}\n",
            "\\author{The Flux Foundation\\\\\\small simulated and computed by a Rust binary in \\texttt{flux-arxiv-latex}, ",
            "related work fetched live from arXiv}\n",
            "\\date{\\today}"
        ))
        .add(Block::Raw("\\maketitle".into()))
        .add(Block::Raw(
            "\\begin{abstract}\nEvery number in this paper is \\emph{computed at document-generation time} by the binary \
             that typeset it: a seeded Monte-Carlo simulation of autocorrelation-based beat tracking under human timing \
             jitter and mix noise; first-principles acoustic-propagation and audio-buffer latency arithmetic; an explicit \
             graph of Camelot-wheel harmonic compatibility, built and greedily traversed in code rather than asserted by \
             hand; and an FFT flop-count budget for the STFT/ISTFT front end of a real-time stem separator. Nothing below \
             is a headline statistic copied from a vendor deck. Where a figure is an assumption rather than a derivation \
             --- a perceptual latency threshold, a mic-array spacing --- it is labelled as such, and the arithmetic that \
             follows from it is shown in full so it can be recomputed with a different assumption.\n\\end{abstract}\n".to_string(),
        ))
        .add(Block::Section("Introduction".into()))
        .add(para(
            "\"AI-assisted DJing\" is usually pitched as a single fuzzy capability. It is not: it is at least five \
             separate, individually tractable signal-processing and combinatorial problems bolted onto a hard real-time \
             constraint. This paper takes six of them apart --- tempo/beat estimation, harmonic (key-compatible) track \
             sequencing, crowd-response sensing, stem separation, and the buffer/acoustic latency budget that constrains \
             all of them --- and computes, rather than asserts, what each one actually costs and how well it actually \
             works under conditions a live performance imposes: human timing imperfection, venue geometry, and a hard \
             wall-clock deadline. The generator is a Rust binary; rerunning it re-simulates every figure from its random \
             seed and re-derives every formula from its inputs.".to_string(),
        ))
        // ---------------------------------------------------------- SEC 1
        .add(Block::Section("The Physical Floor: Buffer Round-Trips and the Speed of Sound".into()))
        .add(para(format!(
            "Two latency floors bound any live audio-AI pipeline, and neither is negotiable by better software. The first \
             is electronic: a digital audio pipeline moves data in fixed-size buffers, and a minimal input+output round \
             trip costs $2b/f_s$ seconds for a buffer of $b$ frames at sample rate $f_s$ (ignoring processing time and any \
             extra driver-level buffering, which only adds to this). Table~\\ref{{tab:buflatency}} computes this for the \
             common buffer sizes and sample rates. Taking a widely used engineering budget for ``feels instantaneous'' \
             tight-coupling latency of ${}$\\,ms (ASSUMED, not derived) as the ceiling, the largest admissible buffer at \
             ${}$\\,Hz is ${}$ frames; at ${}$\\,Hz it drops to ${}$ frames.",
            sci(sync_threshold_ms), sci(sample_rates[1]), sci(max_buf_for_sr[1]), sci(sample_rates[2]), sci(max_buf_for_sr[2])
        )))
        .add(Block::Raw({
            let mut s = String::from(
                "\\begin{center}\\begin{tabular}{r|rrrrrr}\\toprule\n\\textbf{Buffer (frames)}",
            );
            for b in &buffer_sizes {
                s.push_str(&format!(" & {}", *b as i64));
            }
            s.push_str("\\\\\\midrule\n");
            for &sr in &sample_rates {
                s.push_str(&format!("${:.0}$\\,Hz round trip (ms)", sr));
                for &b in &buffer_sizes {
                    s.push_str(&format!(" & ${:.2}$", buffer_latency_ms(b, sr, 2.0)));
                }
                s.push_str("\\\\\n");
            }
            s.push_str("\\bottomrule\\end{tabular}\\end{center}\n\\label{tab:buflatency}\n\n");
            s
        }))
        .add(para(format!(
            "The second floor is acoustic and cannot be buffered away at any price: sound in air at ${}$\\,K travels at \
             ${}$\\,m/s, i.e.\\ ${:.3}$\\,ms per metre. Table~\\ref{{tab:venue}} shows the pure propagation delay from a \
             booth to the back of a venue at several depths. At ${:.0}$\\,m --- an unremarkable club floor --- propagation \
             alone already costs ${:.1}$\\,ms, more than the entire ${}$\\,ms electronic budget above; at ${:.0}$\\,m it \
             costs ${:.1}$\\,ms. Optimising the software pipeline below a couple of milliseconds is therefore moot beyond \
             the front-of-house position: geometry, not code, is the dominant term for anyone standing more than a few \
             metres from the speaker stack.",
            293.0, sci(speed_of_sound), ms_per_m, venue_depths[2], venue_ms[2], sci(sync_threshold_ms), venue_depths[4], venue_ms[4]
        )))
        .add(Block::Raw({
            let mut s = String::from(
                "\\begin{center}\\begin{tabular}{rr}\\toprule\n\\textbf{Distance (m)} & \\textbf{Propagation delay (ms)}\\\\\\midrule\n",
            );
            for (d, ms) in venue_depths.iter().zip(venue_ms.iter()) {
                s.push_str(&format!("${:.0}$ & ${:.2}$\\\\\n", d, ms));
            }
            s.push_str("\\bottomrule\\end{tabular}\\end{center}\n\\label{tab:venue}\n\n");
            s
        }))
        // ---------------------------------------------------------- SEC 2
        .add(Block::Section("Beat Tracking Under Human Timing: a Monte-Carlo Simulation".into()))
        .add(para(
            "Autocorrelation-based tempo estimation is the workhorse underneath essentially every beat tracker, learned \
             or not: build an onset-strength envelope, then find the lag that best explains the envelope's own periodicity. \
             To find out how such an estimator actually degrades under the conditions a live set imposes --- human \
             performers or DJs are not metronomes, mixes have syncopated hats and claps, and dense productions add \
             broadband texture noise --- this paper does not cite a benchmark number; it runs one. For each condition below \
             we synthesise a 20-second onset-strength envelope at 100\\,Hz frame rate from a true tempo, corrupt it with \
             Gaussian timing jitter (std.\\ dev.\\ as given), a 5\\% missed-onset rate, a 15\\% spurious syncopated-onset \
             rate and additive broadband noise, then recover the tempo by autocorrelation search over 60--200\\,BPM in \
             0.5\\,BPM steps. Each cell is the mean of 60 independent, seeded trials.".to_string(),
        ))
        .add(para(format!(
            "Before the sweep: a bare autocorrelation argmax is \\emph{{octave-ambiguous by construction}}, not by bad \
             luck. A periodic onset train correlates with itself almost equally at every integer multiple of its true \
             period, so whenever the search band (here 60--200\\,BPM) contains both a tempo and its double, the argmax is \
             a coin flip between them, biased only by numerical detail. Isolating exactly this effect --- House tempo, \
             essentially no jitter (${}$\\,ms) --- the raw, undisambiguated argmax computed in this run recovers the \
             correct tempo in only ${:.1}\\%$ of ${}$ trials (it is not \\emph{{wrong}} so much as \\emph{{indifferent}}: it \
             is finding a real periodicity, just not always the fundamental one). Adding the single standard fix --- prefer \
             the faster/shorter-lag octave whenever its score is within ${:.0}\\%$ of the winner's --- lifts that to \
             ${:.1}\\%$ with no other change to the algorithm. Every other table in this section uses the disambiguated \
             estimator; this comparison is reported once to show what the disambiguation step is actually buying.",
            sci(1.0), raw_correct_pct, raw_trials, OCTAVE_BIAS * 100.0, disamb_correct_pct
        )))
        .add(Block::Raw({
            let mut s = String::from(
                "\\begin{center}\\begin{tabular}{lrrrr}\\toprule\n\\textbf{Genre (BPM)} & \\textbf{Jitter (ms)} & \\textbf{Accuracy (\\%)} & \\textbf{Octave-error (\\%)} & \\textbf{Mean abs.\\ error (\\%)}\\\\\\midrule\n",
            );
            for (name, j, r) in &jitter_rows {
                s.push_str(&format!(
                    "{} & ${:.0}$ & ${:.1}$ & ${:.1}$ & ${:.2}$\\\\\n",
                    name, j, r.accuracy_pct, r.octave_err_pct, r.mape_pct
                ));
            }
            s.push_str("\\bottomrule\\end{tabular}\\end{center}\n\\label{tab:jitter}\n\n");
            s
        }))
        .add(para(format!(
            "A useful headline pair, computed at the extremes of the sweep: a tight, clean signal (jitter ${}$\\,ms, low \
             noise) recovers tempo correctly (within a $4\\%$ tolerance) in ${:.1}\\%$ of ${}$ trials, mean absolute error \
             ${:.2}\\%$; a loose, busy one (jitter ${}$\\,ms, high noise) recovers correctly in only ${:.1}\\%$, with \
             ${:.1}\\%$ landing on an octave error (half or double tempo) rather than a near-miss --- the estimator is not \
             failing gracefully, it is failing to a \\emph{{specific, structured}} wrong answer, which is exactly why \
             production beat trackers keep an explicit half/double-tempo disambiguation stage rather than trusting the raw \
             autocorrelation peak.",
            sci(5.0), clean_result.accuracy_pct, sci(200.0), clean_result.mape_pct,
            sci(35.0), harsh_result.accuracy_pct, harsh_result.octave_err_pct
        )))
        .add(para(format!(
            "Isolating the noise axis alone (fixed jitter $15$\\,ms, House tempo) turns up a genuinely counter-intuitive \
             result, reported as computed rather than smoothed into the expected shape: accuracy is \\emph{{lowest}} at \
             \\emph{{zero}} added noise (${:.1}\\%$, with ${:.1}\\%$ octave error) and \\emph{{improves}} once a little \
             broadband noise is added (${:.1}$--${:.1}\\%$ across the tested range). The mechanism is the same one \
             Section~2's octave-disambiguation discussion already exposed: a perfectly clean periodic envelope produces \
             near-tied autocorrelation scores at a tempo and its octave, and the ${}\\%$ disambiguation margin sometimes \
             sits on the wrong side of that tie; a small amount of broadband noise perturbs the two candidate scores \
             independently and, more often than not, breaks the tie in the disambiguator's favour --- a small-scale \
             instance of the same dithering effect used deliberately in other detection systems. This is not an argument \
             for adding noise on purpose; it is a warning that ``accuracy vs.\\ signal cleanliness'' is not guaranteed \
             monotonic for an estimator built on exact periodicity, and any real deployment should sweep its own noise \
             axis rather than assume cleaner is always better.",
            noise_rows[0].1.accuracy_pct, noise_rows[0].1.octave_err_pct,
            noise_rows[1..].iter().map(|(_, r)| r.accuracy_pct).fold(f64::INFINITY, f64::min),
            noise_rows[1..].iter().map(|(_, r)| r.accuracy_pct).fold(f64::NEG_INFINITY, f64::max),
            OCTAVE_BIAS * 100.0
        )))
        .add(Block::Raw({
            let mut s = String::from(
                "\\begin{center}\\begin{tabular}{rrrr}\\toprule\n\\textbf{Noise amplitude} & \\textbf{Accuracy (\\%)} & \\textbf{Octave-error (\\%)} & \\textbf{Mean abs.\\ error (\\%)}\\\\\\midrule\n",
            );
            for (na, r) in &noise_rows {
                s.push_str(&format!("${:.1}$ & ${:.1}$ & ${:.1}$ & ${:.2}$\\\\\n", na, r.accuracy_pct, r.octave_err_pct, r.mape_pct));
            }
            s.push_str("\\bottomrule\\end{tabular}\\end{center}\n\\label{tab:noise}\n\n");
            s
        }))
        // ---------------------------------------------------------- SEC 3
        .add(Block::Section("Crowd-Response Sensing: Microphone-Array Geometry".into()))
        .add(para(format!(
            "A microphone array reading crowd energy (cheer/impact detection for automated set-curation feedback) is \
             bounded by the same speed of sound as Section~1, applied at array scale. For a pair of omnidirectional \
             elements spaced $d$ apart, the maximum inter-element time-of-flight is $\\tau_{{\\max}}=d/c$; \
             Table~\\ref{{tab:array}} computes it, and how many samples at ${:.0}$\\,kHz it spans (more samples means finer \
             sub-sample time-difference-of-arrival resolution for beamforming). At a practical $30$\\,cm spacing, \
             $\\tau_{{\\max}}={:.3}$\\,ms spans ${:.1}$ samples --- comfortably enough for interpolated TDOA estimation \
             without needing an unusually high sense sample rate.",
            sr_sense / 1000.0, tau_max_ms[1], samples_across[1]
        )))
        .add(Block::Raw({
            let mut s = String::from(
                "\\begin{center}\\begin{tabular}{rrr}\\toprule\n\\textbf{Spacing (m)} & \\textbf{$\\tau_{\\max}$ (ms)} & \\textbf{Samples spanned (48\\,kHz)}\\\\\\midrule\n",
            );
            for ((d, t), s2) in mic_spacings.iter().zip(tau_max_ms.iter()).zip(samples_across.iter()) {
                s.push_str(&format!("${:.2}$ & ${:.3}$ & ${:.1}$\\\\\n", d, t, s2));
            }
            s.push_str("\\bottomrule\\end{tabular}\\end{center}\n\\label{tab:array}\n\n");
            s
        }))
        .add(para(format!(
            "Angular resolution for a uniform linear array follows the standard beamwidth approximation \
             $\\Delta\\theta\\approx\\lambda/(Nd)$ for $N$ elements. Crowd-response energy spans roughly ${:.0}$--${:.0}$\\,Hz \
             (bass-driven transient ``drop'' impacts through cheering/vocal energy), i.e.\\ wavelengths of \
             ${:.2}$--${:.2}$\\,m at ${}$\\,m/s. An 8-element array at $30$\\,cm spacing therefore resolves direction to \
             roughly ${:.0}^\\circ$ at the high-frequency end and ${:.0}^\\circ$ at the low-frequency end --- coarse \
             compared to visual crowd sensing, but adequate for the coarse-grained question a live-performance system \
             actually needs answered (``did the left or right section of the room respond?''), not fine source \
             localisation.",
            crowd_freq_lo, crowd_freq_hi, wavelength_lo, wavelength_hi, sci(speed_of_sound), beamwidth_deg_narrow, beamwidth_deg_wide
        )))
        // ---------------------------------------------------------- SEC 4
        .add(Block::Section("Harmonic Mixing as a Graph Problem: the Camelot Wheel".into()))
        .add(para(format!(
            "The Camelot system used by most DJ software relabels the 24 major/minor keys onto a wheel so that a \
             ``harmonically safe'' transition is graph adjacency: the same code, an adjacent number with the same letter \
             (a perfect-fifth move), or the same number with the other letter (relative major/minor). Building this as an \
             explicit 24-node graph and counting edges in code (rather than asserting the fraction by hand) gives \
             ${}$ of the ${}$ possible ordered key-pairs (${:.1}\\%$) compatible under the strict three-rule set; an \
             extended rule set some curators also allow (a same-letter $\\pm 2$ ``energy jump'') raises that to \
             ${}$ pairs (${:.1}\\%$).",
            strict_edges, total_ordered, strict_pct, ext_edges, ext_pct
        )))
        .add(para(
            "Compatibility alone does not make a set sequenceable: a transition also needs a BPM match. To measure how \
             far ``harmonically valid neighbours exist'' actually gets an automated curator, we simulate a greedy \
             AI set-builder over a synthetic 500-track library: from the current track, jump to the nearest-BPM \
             harmonically-compatible remaining track within a tolerance window; if none qualifies, take a hard cut \
             (re-seed from an arbitrary remaining track) and continue. We run this over two libraries --- a single-genre \
             pool (BPM $120$--$140$, e.g.\\ house/techno) and a cross-genre pool (BPM $85$--$175$, spanning hip-hop through \
             drum \\& bass) --- at a $6\\%$ BPM tolerance.".to_string(),
        ))
        .add(Block::Raw(format!(
            "\\begin{{center}}\\begin{{tabular}}{{lrrrr}}\\toprule\n\\textbf{{Library}} & \\textbf{{Rule set}} & \\textbf{{Mean run length}} & \\textbf{{Hard cuts}} & \\textbf{{Cuts / 100 tracks}}\\\\\\midrule\n\
             Single-genre (120--140) & strict & ${:.1}$ & ${}$ & ${:.1}$\\\\\n\
             Single-genre (120--140) & extended & ${:.1}$ & ${}$ & ${:.1}$\\\\\n\
             Cross-genre (85--175) & strict & ${:.1}$ & ${}$ & ${:.1}$\\\\\n\
             Cross-genre (85--175) & extended & ${:.1}$ & ${}$ & ${:.1}$\\\\\\bottomrule\n\
             \\end{{tabular}}\\end{{center}}\n\\label{{tab:camelot}}\n\n",
            within_avg_strict, within_cuts_strict, within_cuts_strict as f64 / 5.0,
            within_avg_ext, within_cuts_ext, within_cuts_ext as f64 / 5.0,
            cross_avg_strict, cross_cuts_strict, cross_cuts_strict as f64 / 5.0,
            cross_avg_ext, cross_cuts_ext, cross_cuts_ext as f64 / 5.0,
        )))
        .add(para(
            "The gap between the single-genre and cross-genre rows is the computational restatement of something every \
             working DJ already knows: harmonic compatibility is necessary but the BPM window does almost all of the \
             actual constraining, and a genuinely cross-genre AI-curated set needs either a much wider BPM tolerance \
             (which a human ear will register as an audible tempo ramp) or a time-stretching/pitch-shifting stage in the \
             mix engine, not just a bigger key-compatibility table.".to_string(),
        ))
        .add(Block::Raw({
            let mut s = String::from(
                "\\begin{center}\\begin{tabular}{rrr}\\toprule\n\\textbf{BPM tolerance (\\%)} & \\textbf{Mean run length} & \\textbf{Hard cuts (of 500)}\\\\\\midrule\n",
            );
            for (tol, avg, cuts) in &tol_results {
                s.push_str(&format!("${:.0}$ & ${:.1}$ & ${}$\\\\\n", tol, avg, cuts));
            }
            s.push_str("\\bottomrule\\end{tabular}\\end{center}\n\\label{tab:tolsweep}\n\n");
            s
        }))
        // ---------------------------------------------------------- SEC 5
        .add(Block::Section("Compute Budget for Real-Time Stem Separation".into()))
        .add(para(format!(
            "Automated stem separation for live remixing has two cost centres: the neural inference itself (architecture-\
             specific, and this paper deliberately does not fabricate a FLOP count for an unspecified network), and the \
             deterministic STFT/ISTFT front-and-back end every mask-based separator needs regardless of network choice. \
             The second is fully computable from first principles: an $N$-point complex FFT costs approximately \
             $5N\\log_2 N$ floating-point operations, a 3-minute track at ${}$\\,Hz produces \
             $\\lceil \\text{{duration}}\\cdot f_s/\\text{{hop}}\\rceil$ analysis frames, and a 4-stem mask-based separator \
             needs one forward transform (the mixture) plus four inverse transforms (the stems) per frame --- five \
             transforms in total.",
            sr_audio
        )))
        .add(Block::Raw({
            let mut s = String::from(
                "\\begin{center}\\begin{tabular}{r r r r r}\\toprule\n\\textbf{Window} & \\textbf{Frames} & \\textbf{Total FLOPs} & \\textbf{Time @ class} & \\textbf{RTF}\\\\\\midrule\n",
            );
            for (w, frames, classes) in &stft_rows {
                let total = *frames * transforms_per_frame * fft_flops(*w);
                for (i, (name, time_s, rtf)) in classes.iter().enumerate() {
                    if i == 0 {
                        s.push_str(&format!(
                            "${:.0}$ & ${:.0}$ & ${}$ & {} & ",
                            w, frames, sci(total), name
                        ));
                        s.push_str(&format!("${:.4}$\\,s / ${:.0}\\times$\\\\\n", time_s, rtf));
                    } else {
                        s.push_str(&format!(" & & & {} & ${:.4}$\\,s / ${:.0}\\times$\\\\\n", name, time_s, rtf));
                    }
                }
            }
            s.push_str("\\bottomrule\\end{tabular}\\end{center}\n\\label{tab:stft}\n\n");
            s
        }))
        .add(para(
            "Read this as a floor, not a forecast: the STFT/ISTFT front end alone is real-time-capable (RTF $>1$) by a \
             wide margin at every illustrative throughput class above the conservative scalar-core one, at every window \
             size tested. Whatever total real-time budget a stem-separation product has is therefore overwhelmingly spent \
             on the learned mask network, not on the transform pair around it --- which means optimisation effort belongs \
             on the model, not on a hand-rolled FFT.".to_string(),
        ))
        // ---------------------------------------------------------- SEC 6
        .add(Block::Section("Putting It Together: a Full-Chain Latency Budget".into()))
        .add(para(format!(
            "Chaining a plausible \"AI listens to the crowd, decides, and re-syncs the mix\" loop: microphone-array \
             sensing at $30$\\,cm spacing (${:.3}$\\,ms, Section~3) $+$ a control-loop decision tick (${:.1}$\\,ms, ASSUMED) \
             $+$ an audio buffer round trip at a practical ${}$-frame / ${}$\\,Hz setting (${:.2}$\\,ms, Section~1) totals \
             ${:.2}$\\,ms end-to-end --- inside the ${}$\\,ms tight-coupling budget, with room to spare for the network \
             inference this paper deliberately left unpriced. Compare that to plain propagation delay at only \
             $20$\\,m from the stack (${:.1}$\\,ms, Section~1): in any venue larger than a small room, the venue's own \
             air is a bigger latency term than the entire AI decision loop computed here.",
            sense_latency, decision_latency_ms, chosen_buf as i64, chosen_sr as i64, buf_latency, total_chain_ms, sync_threshold_ms, venue_20m_ms
        )));

    // Related work — straight from the live arXiv sweep.
    if !papers.is_empty() {
        doc = doc.add(Block::Raw(related_work_section(&papers)));
    }

    doc = doc
        .add(Block::Section("Reproducibility".into()))
        .add(para(
            "This PDF is the output of a Rust binary (\\texttt{ai\\_dj\\_live}) in the \\texttt{flux-arxiv-latex} crate, \
             built and run through \\texttt{fluxc} in the Flux tree. It fetches nothing at run time except the supplied \
             arXiv JSON (itself pulled from a live \\texttt{export.arxiv.org} API sweep at corpus-build time, not hand-\
             curated abstracts). Every simulation uses a fixed seed, so recompiling reproduces byte-identical figures; \
             changing a seed, a jitter model, a BPM tolerance, or a throughput class and recompiling changes the numbers \
             --- which is the point. We consider this the only honest way to publish a claim about how well an algorithm \
             performs: run the algorithm.".to_string(),
        ))
        .add(Block::Section("Coda".into()))
        .add(para(
            "None of the results above required access to a real dataset, a real venue, or a real crowd --- and that is \
             also their limit. The beat-tracking sweep models a genuinely standard failure mode (jitter, missed onsets, \
             syncopation, texture noise) with parameters chosen to be plausible rather than measured from a specific \
             corpus; the Camelot-graph simulation captures real, well-documented DJ theory exactly, because that theory is \
             itself a fixed combinatorial object, not a statistic. Readers building an actual product on any of this \
             should replace the assumed inputs --- the $20$\\,ms sync threshold, the $5\\%$/$15\\%$ miss/spurious rates, \
             the $6\\%$ BPM tolerance --- with numbers measured from their own hardware and their own library, and \
             recompile. The formulas and the simulator do not change; only the inputs do.".to_string(),
        ));

    if !papers.is_empty() {
        let mut bib = String::from("\\begin{thebibliography}{99}\n");
        for p in &papers {
            let mut authors: Vec<String> = p.authors.iter().take(3).map(|a| latex_escape(a)).collect();
            if p.authors.len() > 3 {
                authors.push("et al.".into());
            }
            let year = p.published.get(0..4).unwrap_or("n.d.");
            bib.push_str(&format!(
                "\\bibitem{{{}}} {}: \\emph{{{}}}. arXiv:{} ({}). \\url{{{}}}\n",
                p.cite_key(),
                authors.join(", "),
                latex_escape(&p.title),
                p.id,
                year,
                if p.url.is_empty() { format!("https://arxiv.org/abs/{}", p.id) } else { p.url.clone() }
            ));
        }
        bib.push_str("\\end{thebibliography}\n");
        doc = doc.add(Block::Raw(bib));
    }

    std::fs::create_dir_all(out_dir).expect("out dir");
    std::fs::write(format!("{out_dir}/ai_dj_live.bib"), bibliography(&papers)).expect("bib");
    let res = doc.compile_pdf(out_dir, "ai_dj_live");
    if res.success {
        println!("OK {}", res.pdf_path.unwrap());
    } else {
        let tail: String = res.log.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        eprintln!("FAILED\n{tail}");
        std::process::exit(1);
    }
}
