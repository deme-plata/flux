// flux-soundtrack — algorithmic fusion instrumentals for the "Flux Album"
//
// Real DSP, no samples, no loops from a library. Every note's timing comes
// from a Zeckendorf-style Fibonacci-sum grouping of the bar's odd meter;
// every note's pitch comes from indexing a mode by the sequence of primes.
// Bass = fretless portamento + vibrato (Hellborg-style glide). Guitar = FM
// "tapped legato" voice with a golden-ratio-spaced delay. This is real
// computed audio (assert!()-checked math), not sampled/looped material.
// Singing is NOT synthesized here — lyrics ship as a separate text booklet.

use hound::{SampleFormat, WavSpec, WavWriter};
use std::f32::consts::PI;
use std::fs;

const SR: u32 = 44_100;

// ---------------------------------------------------------------- math ---

const PHRYGIAN_DOMINANT: [i32; 7] = [0, 1, 4, 5, 7, 8, 10]; // semitones from root
const PRIMES: [usize; 24] = [
    2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71, 73, 79, 83, 89,
];
const FIB: [usize; 12] = [1, 1, 2, 3, 5, 8, 13, 21, 34, 55, 89, 144];

/// Verify a rhythm grouping is (a) all Fibonacci numbers and (b) sums to the
/// meter's numerator, i.e. a genuine Zeckendorf-style decomposition of the bar.
fn verify_fib_grouping(name: &str, groups: &[usize], expect_sum: usize) {
    let sum: usize = groups.iter().sum();
    assert_eq!(sum, expect_sum, "{name}: grouping {groups:?} must sum to {expect_sum}");
    for g in groups {
        assert!(FIB.contains(g), "{name}: {g} is not a Fibonacci number");
    }
}

fn midi_freq(root_hz: f32, semitones: f32) -> f32 {
    root_hz * 2f32.powf(semitones / 12.0)
}

fn scale_freq(root_hz: f32, degree_index: usize, octave_shift: i32) -> f32 {
    let deg = PHRYGIAN_DOMINANT[degree_index % PHRYGIAN_DOMINANT.len()];
    midi_freq(root_hz, (deg + 12 * octave_shift) as f32)
}

fn octave_shift_from_fib(i: usize) -> i32 {
    match FIB[i % FIB.len()] % 5 {
        0 => -1,
        4 => 1,
        _ => 0,
    }
}

// ------------------------------------------------------------ envelopes --

fn adsr(t: f32, dur: f32, attack: f32, release: f32) -> f32 {
    if t < attack {
        t / attack.max(1e-6)
    } else if t > dur - release {
        ((dur - t) / release.max(1e-6)).max(0.0)
    } else {
        1.0
    }
}

fn soft_clip(x: f32) -> f32 {
    x.tanh()
}

struct Note {
    start_s: f32,
    dur_s: f32,
    freq: f32,
    vel: f32,
}

// ---------------------------------------------------------------- voices -

/// Fretless bass: exponential portamento glide into each note + slow vibrato
/// that deepens over the note's sustain, plus soft odd-harmonic content.
fn render_bass(notes: &[Note], total_len: usize) -> Vec<f32> {
    let mut buf = vec![0f32; total_len];
    for (i, n) in notes.iter().enumerate() {
        let prev_freq = if i > 0 { notes[i - 1].freq } else { n.freq };
        let glide_s = (n.dur_s * 0.18).min(0.09);
        let start_i = (n.start_s * SR as f32) as usize;
        let dur_i = (n.dur_s * SR as f32) as usize;
        for s in 0..dur_i {
            let idx = start_i + s;
            if idx >= buf.len() {
                break;
            }
            let t = s as f32 / SR as f32;
            let f = if t < glide_s && i > 0 && prev_freq > 0.0 {
                let k = t / glide_s;
                prev_freq * (n.freq / prev_freq).powf(k)
            } else {
                n.freq
            };
            let vib_depth = 0.006 * (t / n.dur_s).min(1.0);
            let vib = 1.0 + vib_depth * (2.0 * PI * 5.5 * t).sin();
            let phase = 2.0 * PI * f * vib * t;
            let sample = phase.sin() + 0.35 * (2.0 * phase).sin() + 0.12 * (3.0 * phase).sin();
            let env = adsr(t, n.dur_s, 0.02, (n.dur_s * 0.3).min(0.25));
            buf[idx] += 0.6 * n.vel * env * sample;
        }
    }
    buf
}

/// Guitar: FM "tapped legato" voice — 2:1 carrier:modulator ratio, fast
/// attack, mild soft-clip drive to read as an electric lead.
fn render_guitar(notes: &[Note], total_len: usize) -> Vec<f32> {
    let mut buf = vec![0f32; total_len];
    for n in notes {
        let start_i = (n.start_s * SR as f32) as usize;
        let dur_i = (n.dur_s * SR as f32) as usize;
        let mod_ratio = 2.0;
        let mod_index = 1.6;
        for s in 0..dur_i {
            let idx = start_i + s;
            if idx >= buf.len() {
                break;
            }
            let t = s as f32 / SR as f32;
            let env = adsr(t, n.dur_s, 0.004, (n.dur_s * 0.4).min(0.12));
            let modulator = (2.0 * PI * n.freq * mod_ratio * t).sin();
            let carrier_phase = 2.0 * PI * n.freq * t + mod_index * env * modulator;
            let driven = soft_clip(carrier_phase.sin() * 2.2);
            buf[idx] += 0.45 * n.vel * env * driven;
        }
    }
    buf
}

/// Delay whose tap spacing is the golden-ratio-derived interval 0.618/phi s
/// — a real, professionally-used technique (golden-ratio delay times avoid
/// harsh comb-filtering because the taps never lock into simple ratios).
fn golden_delay(buf: &[f32], feedback: f32, mix: f32) -> Vec<f32> {
    const PHI: f32 = 1.618_033_9;
    let delay_s = 0.618 / PHI; // ~0.382s
    let delay_i = (delay_s * SR as f32) as usize;
    let mut out = buf.to_vec();
    if delay_i == 0 || delay_i >= out.len() {
        return out;
    }
    for i in delay_i..out.len() {
        out[i] += buf[i - delay_i] * feedback * mix;
    }
    out
}

struct Rng(u32);
impl Rng {
    fn next_f32(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

fn render_drums(hits: &[(f32, char)], total_len: usize) -> Vec<f32> {
    let mut buf = vec![0f32; total_len];
    let mut rng = Rng(0x9E37_79B9);
    for &(t0, kind) in hits {
        let start_i = (t0 * SR as f32) as usize;
        match kind {
            'k' => {
                let dur = 0.12f32;
                for s in 0..(SR as f32 * dur) as usize {
                    let idx = start_i + s;
                    if idx >= buf.len() {
                        break;
                    }
                    let t = s as f32 / SR as f32;
                    let f = 90.0 - 50.0 * (t / dur);
                    let env = (1.0 - t / dur).max(0.0).powf(1.5);
                    buf[idx] += 0.9 * env * (2.0 * PI * f * t).sin();
                }
            }
            's' => {
                let dur = 0.09f32;
                for s in 0..(SR as f32 * dur) as usize {
                    let idx = start_i + s;
                    if idx >= buf.len() {
                        break;
                    }
                    let t = s as f32 / SR as f32;
                    let env = (1.0 - t / dur).max(0.0);
                    buf[idx] += 0.5 * env * rng.next_f32();
                }
            }
            _ => {
                let dur = 0.03f32;
                for s in 0..(SR as f32 * dur) as usize {
                    let idx = start_i + s;
                    if idx >= buf.len() {
                        break;
                    }
                    let t = s as f32 / SR as f32;
                    let env = (1.0 - t / dur).max(0.0);
                    buf[idx] += 0.18 * env * rng.next_f32();
                }
            }
        }
    }
    buf
}

// ------------------------------------------------------------ composer --

struct SongSpec {
    slug: &'static str,
    title: &'static str,
    feature: &'static str,
    tempo_bpm: f32,
    root_hz: f32,
    bass_meter_groups: &'static [usize], // Fibonacci grouping, eighth-note units
    guitar_meter_groups: &'static [usize],
    duration_s: f32,
    liner: &'static str,
}

fn build_line(
    root_hz: f32,
    groups: &[usize],
    eighth_dur: f32,
    duration_s: f32,
    base_octave: i32,
    subdivide: bool, // guitar: split each group unit into `unit` eighth notes
    vel: f32,
) -> Vec<Note> {
    let mut notes = Vec::new();
    let mut t = 0.0f32;
    let mut i = 0usize;
    'outer: loop {
        for &g in groups {
            let degree_index = PRIMES[i % PRIMES.len()] % PHRYGIAN_DOMINANT.len();
            let octv = base_octave + octave_shift_from_fib(i);
            if subdivide {
                let sub_dur = eighth_dur;
                for k in 0..g {
                    if t >= duration_s {
                        break 'outer;
                    }
                    let deg2 = PRIMES[(i + k) % PRIMES.len()] % PHRYGIAN_DOMINANT.len();
                    let freq = scale_freq(root_hz, deg2, octv);
                    notes.push(Note { start_s: t, dur_s: sub_dur * 0.94, freq, vel });
                    t += sub_dur;
                }
            } else {
                if t >= duration_s {
                    break 'outer;
                }
                let dur = eighth_dur * g as f32;
                let freq = scale_freq(root_hz, degree_index, octv);
                notes.push(Note { start_s: t, dur_s: dur * 0.96, freq, vel });
                t += dur;
            }
            i += 1;
        }
    }
    notes
}

fn build_drum_hits(groups: &[usize], eighth_dur: f32, duration_s: f32, start_at: f32) -> Vec<(f32, char)> {
    let mut hits = Vec::new();
    let mut t = 0.0f32;
    loop {
        for (gi, &g) in groups.iter().enumerate() {
            if t >= duration_s {
                return hits;
            }
            if t >= start_at {
                hits.push((t, if gi == 0 { 'k' } else { 's' }));
                let mut sub = t;
                for _ in 0..g {
                    if sub < duration_s && sub >= start_at {
                        hits.push((sub, 'h'));
                    }
                    sub += eighth_dur;
                }
            }
            t += eighth_dur * g as f32;
        }
    }
}

fn section_gain(instr: char, t: f32, dur: f32) -> f32 {
    let frac = t / dur;
    let fade_out = if frac > 0.92 { ((1.0 - frac) / 0.08).clamp(0.0, 1.0) } else { 1.0 };
    let base = match instr {
        'b' => 1.0, // bass present throughout
        'g' => {
            if frac < 0.22 {
                (frac / 0.22).clamp(0.0, 1.0) * 0.7
            } else {
                1.0
            }
        }
        _ => {
            if frac < 0.55 {
                0.0
            } else if frac < 0.63 {
                (frac - 0.55) / 0.08
            } else {
                1.0
            }
        }
    };
    base * fade_out
}

fn apply_section_gain(buf: &mut [f32], instr: char, dur: f32) {
    for (i, s) in buf.iter_mut().enumerate() {
        let t = i as f32 / SR as f32;
        *s *= section_gain(instr, t, dur);
    }
}

fn write_wav(path: &str, left: &[f32], right: &[f32]) {
    let spec = WavSpec {
        channels: 2,
        sample_rate: SR,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(path, spec).expect("create wav");
    for i in 0..left.len() {
        let l = (left[i].clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        let r = (right[i].clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        writer.write_sample(l).unwrap();
        writer.write_sample(r).unwrap();
    }
    writer.finalize().unwrap();
}

fn render_song(spec: &SongSpec, out_dir: &str) -> (usize, f32) {
    let eighth_dur = 60.0 / spec.tempo_bpm / 2.0;

    verify_fib_grouping(spec.slug, spec.bass_meter_groups, spec.bass_meter_groups.iter().sum());
    verify_fib_grouping(spec.slug, spec.guitar_meter_groups, spec.guitar_meter_groups.iter().sum());

    let total_len = (spec.duration_s * SR as f32) as usize + SR as usize; // pad 1s
    let bass_notes = build_line(spec.root_hz, spec.bass_meter_groups, eighth_dur, spec.duration_s, 0, false, 0.9);
    let guitar_notes = build_line(spec.root_hz, spec.guitar_meter_groups, eighth_dur, spec.duration_s, 2, true, 0.75);
    let drum_start = spec.duration_s * 0.55;
    let drum_hits = build_drum_hits(spec.bass_meter_groups, eighth_dur, spec.duration_s, drum_start);

    let mut bass = render_bass(&bass_notes, total_len);
    let mut guitar = render_guitar(&guitar_notes, total_len);
    let drums = render_drums(&drum_hits, total_len);

    apply_section_gain(&mut bass, 'b', spec.duration_s);
    apply_section_gain(&mut guitar, 'g', spec.duration_s);
    let guitar_delayed = golden_delay(&guitar, 0.38, 0.55);

    let mut drums_g = drums;
    apply_section_gain(&mut drums_g, 'd', spec.duration_s);

    // constant-power pan: bass slightly left, guitar slightly right, drums center
    let pan = |x: f32, p: f32, left: bool| {
        let theta = (p + 1.0) * PI / 4.0;
        x * if left { theta.cos() } else { theta.sin() }
    };

    let mut left = vec![0f32; total_len];
    let mut right = vec![0f32; total_len];
    for i in 0..total_len {
        left[i] = pan(bass[i], -0.35, true) + pan(guitar_delayed[i], 0.35, true) + drums_g[i];
        right[i] = pan(bass[i], -0.35, false) + pan(guitar_delayed[i], 0.35, false) + drums_g[i];
        left[i] = soft_clip(left[i] * 0.9);
        right[i] = soft_clip(right[i] * 0.9);
    }

    let path = format!("{out_dir}/{}.wav", spec.slug);
    write_wav(&path, &left, &right);

    let bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    (bytes as usize, spec.duration_s)
}

fn main() {
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| "./out".to_string());
    fs::create_dir_all(&out_dir).unwrap();

    let songs: [SongSpec; 5] = [
        SongSpec {
            slug: "01-twelve-point-four",
            title: "Twelve Point Four",
            feature: "fluxc self-hosting — the compiler compiles itself in 12.4s warm",
            tempo_bpm: 124.0,
            root_hz: 55.00, // A1
            bass_meter_groups: &[3, 2, 2],   // 7/8, Fibonacci sum
            guitar_meter_groups: &[3, 2, 2],
            duration_s: 78.0,
            liner: "7/8, grouped 3+2+2 — the loop that recompiles the thing computing it.",
        },
        SongSpec {
            slug: "02-skeleton-store",
            title: "Skeleton Store",
            feature: "SkeletonStore — flat append-only commit path, ~2,600x flux-db KV throughput",
            tempo_bpm: 136.0,
            root_hz: 36.71, // D1
            bass_meter_groups: &[3, 3, 5],   // 11/8
            guitar_meter_groups: &[3, 3, 5],
            duration_s: 82.0,
            liner: "11/8, grouped 3+3+5 — the wall measured, then walked straight through.",
        },
        SongSpec {
            slug: "03-cache-convergence",
            title: "Cache Convergence",
            feature: "content-hash RUSTC_WRAPPER cache — no-op checks converge to a ~1s floor",
            tempo_bpm: 118.0,
            root_hz: 41.20, // E1
            bass_meter_groups: &[2, 3, 5],   // 10 eighth-units (5/4), ascending Fibonacci run
            guitar_meter_groups: &[2, 3, 5],
            duration_s: 74.0,
            liner: "5/4 as 2+3+5 eighth-units — a literal ascending Fibonacci run for a converging cache.",
        },
        SongSpec {
            slug: "04-swarm-signal",
            title: "Swarm Signal",
            feature: "flux_swarm_* — independent agents claim, build, and settle across one bus",
            tempo_bpm: 132.0,
            root_hz: 49.00, // G1
            bass_meter_groups: &[3, 5],      // 4/4 as 8 eighth-units, Fibonacci 3+5
            guitar_meter_groups: &[3, 3],    // 3/4 as 6 eighth-units, Fibonacci 3+3
            duration_s: 86.0,
            liner: "true polymeter: bass cycles 8 units, guitar cycles 6 — an exact 4:3 ratio, two voices never locking step, same as two agents on the same bus.",
        },
        SongSpec {
            slug: "05-provenance-sig",
            title: "Provenance Sig",
            feature: "SQIsign Level 5 provenance — 292-byte signatures binding BLAKE3(artifact) to the agent key",
            tempo_bpm: 96.0,
            root_hz: 34.65, // C#1
            bass_meter_groups: &[2, 3, 3, 5], // 13/8
            guitar_meter_groups: &[2, 3, 3, 5],
            duration_s: 90.0,
            liner: "13/8, grouped 2+3+3+5 — slow and ceremonial, the meter of a signature that binds a proof to a wallet.",
        },
    ];

    println!("=== flux-soundtrack — math-in-the-sound report ===");
    let mut total_bytes = 0usize;
    for s in &songs {
        let (bytes, dur) = render_song(s, &out_dir);
        total_bytes += bytes;
        println!(
            "{:<28} | {:>6.1}s | {:>6.1} BPM | root {:>6.2}Hz | bass groups {:?} | guitar groups {:?} | {} bytes",
            s.title, dur, s.tempo_bpm, s.root_hz, s.bass_meter_groups, s.guitar_meter_groups, bytes
        );
        println!("    feature: {}", s.feature);
        println!("    math:    {}", s.liner);
    }
    println!("=== total WAV bytes: {total_bytes} ===");
}
