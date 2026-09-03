//! The droplet: a bounded, fully deterministic agent state machine.
//!
//! Ported from Quillon Graph `void-walker::droplet::DropletField`, with three
//! defects fixed:
//!
//! 1. **Non-determinism.** The original seeded its own `StdRng` but then reached
//!    for `rand::thread_rng()` inside `k_parameter::generate_quantum_noise`, so
//!    two runs from the same seed diverged. Here every source of randomness is
//!    the droplet's own `ChaCha8Rng`, and `rand_distr` is dropped in favour of an
//!    inline Box–Muller transform.
//! 2. **Unit lie.** Every timestamp in the original was labelled "attoseconds"
//!    but computed as `as_nanos() / 1_000_000_000` — i.e. whole **seconds**, off
//!    by 10^18. Nothing here claims a precision it does not have: the clock is
//!    an explicit monotonic `tick` count supplied by the caller.
//! 3. **Unbounded history.** `eeg_history` grew with `Vec::remove(0)` (O(n) per
//!    push). Here it is a fixed-capacity ring.

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::f64::consts::{PI, TAU};

use crate::species::Species;

/// How many recent stimulus samples a droplet remembers.
pub const STIMULUS_WINDOW: usize = 64;

/// A stimulus applied to a droplet.
///
/// The source paper calls this an EEG amplitude in microvolts. Nothing in the
/// model actually requires it to come from a brain — it is simply a bounded
/// scalar drive, and naming it honestly is what lets the same droplet model
/// carry P2P peer telemetry (see [`crate::mesh`]).
#[derive(Copy, Clone, Debug, Default, Serialize, Deserialize)]
pub struct Stimulus {
    /// Drive amplitude. Saturates through `tanh`, so any finite value is safe.
    pub amplitude: f64,
}

impl Stimulus {
    pub fn new(amplitude: f64) -> Self {
        Self { amplitude }
    }
}

/// Electrowetting-on-dielectric actuation. Contact-angle shift goes as V², and
/// saturates — a real physical effect and the one term in the source model that
/// is dimensionally honest.
#[derive(Copy, Clone, Debug, Default, Serialize, Deserialize)]
pub struct EwodDrive {
    pub volts: f64,
}

/// 32-byte isotopic fingerprint identifying a droplet's water.
pub type IsotopicSig = [u8; 32];

/// Signed topological charge, bounded to ±[`crate::score::GainWeights::charge_cap`].
pub type TopoCharge = i32;

fn isotopic_hash(seed: u64) -> IsotopicSig {
    let mut h = blake3::Hasher::new();
    h.update(b"flux-aqua/isotopic-signature/v1");
    h.update(&seed.to_le_bytes());
    *h.finalize().as_bytes()
}

/// A fixed-capacity ring of recent stimulus samples.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StimulusRing {
    buf: Vec<f64>,
    head: usize,
    len: usize,
}

impl StimulusRing {
    pub fn new() -> Self {
        Self {
            buf: vec![0.0; STIMULUS_WINDOW],
            head: 0,
            len: 0,
        }
    }

    pub fn push(&mut self, v: f64) {
        if self.buf.is_empty() {
            self.buf = vec![0.0; STIMULUS_WINDOW];
        }
        self.buf[self.head] = v;
        self.head = (self.head + 1) % STIMULUS_WINDOW;
        self.len = (self.len + 1).min(STIMULUS_WINDOW);
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Mean over the retained window; 0.0 when empty.
    pub fn mean(&self) -> f64 {
        if self.len == 0 {
            return 0.0;
        }
        let n = self.len;
        let mut sum = 0.0;
        for i in 0..n {
            // Walk backwards from head so we only read initialised slots.
            let idx = (self.head + STIMULUS_WINDOW - 1 - i) % STIMULUS_WINDOW;
            sum += self.buf[idx];
        }
        sum / n as f64
    }
}

/// A single water robot.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Droplet {
    /// Which species this droplet belongs to. Drives capacity and lifespan.
    pub species: Species,
    /// Quantum phase, always in `[0, 2π)`.
    pub phase: f64,
    /// Coherence in `[0, 1]`. Decays under thermal noise, recovers under EWOD.
    pub coherence: f64,
    /// Normalised energy in `[0, 1]`.
    pub energy: f64,
    /// Temperature in kelvin.
    pub temperature_k: f64,
    /// Isotopic fingerprint — this droplet's identity.
    pub iso_sig: IsotopicSig,
    /// DNA-encoded payload (2 bits per base, 4 bases per byte).
    pub dna: Vec<u8>,
    /// Recent stimulus history.
    pub stimulus: StimulusRing,
    /// Monotonic tick counter, incremented by the caller's clock.
    pub tick: u64,
    /// Completed lifecycle cycles; a droplet retires at `species.lifespan_cycles()`.
    pub cycles: u64,
    seed: u64,
    #[serde(skip)]
    rng: Option<ChaCha8Rng>,
}

impl Droplet {
    /// Birth a droplet of `species` from `seed`. Fully deterministic.
    pub fn new(species: Species, seed: u64) -> Self {
        Self {
            species,
            phase: 0.0,
            coherence: 0.92,
            energy: 0.80,
            temperature_k: 295.0,
            iso_sig: isotopic_hash(seed),
            dna: Vec::new(),
            stimulus: StimulusRing::new(),
            tick: 0,
            cycles: 0,
            seed,
            rng: Some(ChaCha8Rng::seed_from_u64(seed ^ 0xAF23_19C0_DD41_55E1)),
        }
    }

    /// Restore the RNG after a deserialize round-trip. Idempotent.
    pub fn rehydrate(&mut self) {
        if self.rng.is_none() {
            self.rng = Some(ChaCha8Rng::seed_from_u64(self.seed ^ 0xAF23_19C0_DD41_55E1));
        }
    }

    fn rng(&mut self) -> &mut ChaCha8Rng {
        self.rehydrate();
        self.rng.as_mut().expect("rehydrated above")
    }

    /// Standard-normal sample via Box–Muller, from the droplet's own stream.
    fn normal(&mut self, sigma: f64) -> f64 {
        let rng = self.rng();
        // Guard u1 away from 0 so ln() stays finite.
        let u1: f64 = rng.gen_range(f64::MIN_POSITIVE..1.0);
        let u2: f64 = rng.gen_range(0.0..1.0);
        sigma * (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
    }

    /// Apply a stimulus: phase advances by `tanh(amplitude) · coherence · π`,
    /// and energy relaxes towards the drive with a 0.9 retention factor.
    pub fn stimulate(&mut self, s: Stimulus) {
        self.stimulus.push(s.amplitude);
        let delta = s.amplitude.tanh() * self.coherence * PI;
        self.phase = (self.phase + delta).rem_euclid(TAU);
        let drive = (s.amplitude / 50.0).clamp(0.0, 1.0);
        self.energy = (self.energy * 0.9 + drive * 0.1).clamp(0.0, 1.0);
        self.tick += 1;
    }

    /// EWOD actuation: extra phase nudge ∝ V², saturating at π/2, plus a small
    /// coherence (mobility) boost.
    pub fn ewod(&mut self, drive: EwodDrive) {
        let v2 = drive.volts.max(0.0).powi(2);
        let delta = (v2 / (v2 + 100.0)) * 0.5 * PI;
        self.phase = (self.phase + delta).rem_euclid(TAU);
        self.coherence = (self.coherence + (drive.volts / 20.0).min(0.1)).min(1.0);
        self.tick += 1;
    }

    /// Thermal/Brownian dephasing over `dt_s` seconds at `temp_k`.
    /// Coherence only ever decreases here.
    pub fn thermal_dephase(&mut self, temp_k: f64, dt_s: f64) {
        self.temperature_k = temp_k;
        let sigma = (temp_k / 300.0).sqrt() * (dt_s / 0.01).sqrt() * 0.02;
        let kick = self.normal(sigma).abs();
        self.coherence = (self.coherence - kick).clamp(0.0, 1.0);
        self.tick += 1;
    }

    /// Observe a nearby droplet's water signature, with a few bytes perturbed —
    /// the model's stand-in for an imperfect measurement.
    pub fn sniff(&mut self) -> IsotopicSig {
        let mut sig = self.iso_sig;
        let count = self.rng().gen_range(1..=3);
        for _ in 0..count {
            let i = self.rng().gen_range(0..sig.len());
            let b: u8 = self.rng().gen();
            sig[i] ^= b;
        }
        sig
    }

    /// Similarity of two isotopic signatures, in `[0, 1]`, as normalised
    /// **bit** agreement: `1 - hamming_bits / 256`.
    ///
    /// The source used byte equality — `matches / 32` — which is a metric that
    /// only works in the one case the source ever used it. Its `sniff_parallel_water`
    /// returned a copy of the droplet's *own* signature with 1–3 bytes flipped,
    /// so the comparison was always self-to-near-self and scored ~0.91. Between
    /// two genuinely independent BLAKE3 digests, byte equality returns
    /// `~32/256 = 0.004` with probability ≈ 1: the metric is pinned at zero and
    /// carries no signal at all, so no pair of distinct droplets could ever
    /// bridge. Bit agreement is 1.0 for identical, ~0.5 for independent, 0.0 for
    /// complementary — informative across the whole range.
    pub fn affinity(a: &IsotopicSig, b: &IsotopicSig) -> f64 {
        let differing: u32 = a
            .iter()
            .zip(b)
            .map(|(x, y)| (x ^ y).count_ones())
            .sum();
        1.0 - (differing as f64 / 256.0)
    }

    /// Encode `data` into the DNA store: 2 bits per base, 4 bases per byte,
    /// little-endian within the byte (A=00, T=01, G=10, C=11).
    ///
    /// Refuses to exceed the species' DNA capacity and returns how many bytes
    /// were actually stored — the original silently grew without limit despite
    /// the whitepaper specifying a per-species base-pair budget.
    pub fn store_dna(&mut self, data: &[u8]) -> usize {
        let cap_bases = self.species.dna_load_bp();
        let room_bases = cap_bases.saturating_sub(self.dna.len());
        let take = data.len().min(room_bases / 4);
        for byte in &data[..take] {
            for i in 0..4 {
                self.dna.push(match (byte >> (i * 2)) & 0b11 {
                    0 => b'A',
                    1 => b'T',
                    2 => b'G',
                    _ => b'C',
                });
            }
        }
        take
    }

    /// Decode `len_bytes` bytes starting at byte index `byte_offset`.
    ///
    /// The source API took a *base* offset and a *byte* length in the same call,
    /// which is why its own doc example only ever worked at offset 0. Both
    /// arguments here are in bytes.
    pub fn read_dna(&self, byte_offset: usize, len_bytes: usize) -> Vec<u8> {
        let start = byte_offset * 4;
        if start >= self.dna.len() {
            return Vec::new();
        }
        let end = (start + len_bytes * 4).min(self.dna.len());
        self.dna[start..end]
            .chunks_exact(4)
            .map(|chunk| {
                let mut byte = 0u8;
                for (i, &base) in chunk.iter().enumerate() {
                    let bits = match base {
                        b'T' => 1u8,
                        b'G' => 2,
                        b'C' => 3,
                        _ => 0,
                    };
                    byte |= bits << (i * 2);
                }
                byte
            })
            .collect()
    }

    /// Complete one lifecycle cycle. Returns `true` when the droplet has reached
    /// its species' lifespan and should divide or retire.
    pub fn advance_cycle(&mut self) -> bool {
        self.cycles += 1;
        self.cycles >= self.species.lifespan_cycles()
    }

    /// A droplet is "hot" when it is both energetic and coherent — the state in
    /// which a bridge attempt is worth making.
    pub fn is_hot(&self) -> bool {
        self.energy > 0.8 && self.coherence > 0.9
    }

    /// Short human-readable state line.
    pub fn summary(&self) -> String {
        format!(
            "{} φ={:.3} coh={:.3} E={:.3} T={:.1}K dna={}B tick={}",
            self.species.name(),
            self.phase,
            self.coherence,
            self.energy,
            self.temperature_k,
            self.dna.len(),
            self.tick
        )
    }
}

/// Map a drive energy to a bounded topological charge, deterministically.
///
/// The source used `rand::thread_rng()` here, so the same input gave different
/// charges on every call and no test could pin it. This version derives its
/// jitter from the droplet's own stream.
pub fn tune_topology(rng: &mut ChaCha8Rng, tev_scale: f64, cap: i32) -> TopoCharge {
    let mean = tev_scale.tanh() * (cap as f64 - 1.5);
    let jitter: f64 = rng.gen_range(-1.0..=1.0);
    (mean + jitter).round().clamp(-(cap as f64), cap as f64) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn birth_is_in_range() {
        let d = Droplet::new(Species::Processor, 1337);
        assert_eq!(d.phase, 0.0);
        assert!((0.0..=1.0).contains(&d.coherence));
        assert!((0.0..=1.0).contains(&d.energy));
        assert_eq!(d.cycles, 0);
    }

    #[test]
    fn same_seed_gives_identical_trajectory() {
        // This is the property the source crate did not have: it mixed a seeded
        // StdRng with rand::thread_rng(), so runs diverged.
        let run = |seed: u64| {
            let mut d = Droplet::new(Species::Scout, seed);
            for i in 0..50 {
                d.stimulate(Stimulus::new(i as f64 % 40.0));
                d.thermal_dephase(300.0, 0.01);
                let _ = d.sniff();
            }
            (d.phase, d.coherence, d.energy)
        };
        assert_eq!(run(99), run(99));
        assert_ne!(run(99), run(100));
    }

    #[test]
    fn phase_stays_wrapped() {
        let mut d = Droplet::new(Species::Messenger, 7);
        for _ in 0..500 {
            d.stimulate(Stimulus::new(9.0));
            d.ewod(EwodDrive { volts: 30.0 });
            assert!((0.0..TAU).contains(&d.phase), "phase escaped: {}", d.phase);
        }
    }

    #[test]
    fn thermal_dephasing_never_increases_coherence() {
        let mut d = Droplet::new(Species::Guardian, 5);
        let mut prev = d.coherence;
        for _ in 0..200 {
            d.thermal_dephase(350.0, 0.02);
            assert!(d.coherence <= prev + f64::EPSILON);
            prev = d.coherence;
        }
    }

    #[test]
    fn ewod_saturates() {
        let mut a = Droplet::new(Species::Processor, 1);
        let mut b = Droplet::new(Species::Processor, 1);
        a.ewod(EwodDrive { volts: 1_000.0 });
        b.ewod(EwodDrive { volts: 1_000_000.0 });
        // Both are deep in saturation, so the phase nudge must agree to well
        // under a milliradian despite a 1000x voltage difference.
        assert!((a.phase - b.phase).abs() < 1e-3);
    }

    #[test]
    fn dna_roundtrip() {
        let mut d = Droplet::new(Species::Memory, 456);
        let msg = b"Every drop contains the ocean";
        let stored = d.store_dna(msg);
        assert_eq!(stored, msg.len());
        assert_eq!(d.read_dna(0, msg.len()), msg);
    }

    #[test]
    fn dna_respects_species_capacity() {
        // Messenger has the smallest budget (10^4 bp) — a big write must clip,
        // not grow without bound.
        let mut d = Droplet::new(Species::Messenger, 3);
        let big = vec![0xABu8; 100_000];
        let stored = d.store_dna(&big);
        assert!(stored < big.len());
        assert!(d.dna.len() <= Species::Messenger.dna_load_bp());
    }

    #[test]
    fn dna_read_at_offset() {
        let mut d = Droplet::new(Species::Memory, 11);
        d.store_dna(b"ABCDEFGH");
        assert_eq!(d.read_dna(4, 4), b"EFGH");
    }

    #[test]
    fn stimulus_ring_is_bounded_and_averages() {
        let mut d = Droplet::new(Species::Scout, 2);
        for _ in 0..1000 {
            d.stimulate(Stimulus::new(10.0));
        }
        assert_eq!(d.stimulus.len(), STIMULUS_WINDOW);
        assert!((d.stimulus.mean() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn affinity_spans_the_whole_unit_interval() {
        let a = [0xAAu8; 32];
        assert_eq!(Droplet::affinity(&a, &a), 1.0, "identical must score 1");
        assert_eq!(
            Droplet::affinity(&a, &[0x55u8; 32]),
            0.0,
            "bitwise complement must score 0"
        );
    }

    #[test]
    fn independent_signatures_score_near_one_half() {
        // The property the source metric could not have: two unrelated droplets
        // must land near 0.5, not pinned at ~0.004. Without this, no pair of
        // distinct droplets can ever clear the stability bar.
        let mut sum = 0.0;
        let n = 200;
        for seed in 0..n {
            let a = Droplet::new(Species::Scout, seed).iso_sig;
            let b = Droplet::new(Species::Scout, seed + 10_000).iso_sig;
            sum += Droplet::affinity(&a, &b);
        }
        let mean = sum / n as f64;
        assert!((mean - 0.5).abs() < 0.03, "mean affinity was {mean}");
    }

    #[test]
    fn topology_is_deterministic_and_bounded() {
        let mut r1 = ChaCha8Rng::seed_from_u64(4);
        let mut r2 = ChaCha8Rng::seed_from_u64(4);
        assert_eq!(tune_topology(&mut r1, 2.0, 8), tune_topology(&mut r2, 2.0, 8));
        let mut r = ChaCha8Rng::seed_from_u64(9);
        for _ in 0..500 {
            assert!(tune_topology(&mut r, 5.0, 8).abs() <= 8);
        }
    }

    #[test]
    fn serde_roundtrip_rehydrates_rng() {
        let mut d = Droplet::new(Species::Trader, 21);
        d.stimulate(Stimulus::new(12.0));
        let json = serde_json::to_string(&d).unwrap();
        let mut back: Droplet = serde_json::from_str(&json).unwrap();
        back.rehydrate();
        assert_eq!(back.phase, d.phase);
        assert_eq!(back.sniff().len(), 32);
    }

    #[test]
    fn lifespan_retires_droplet() {
        let mut d = Droplet::new(Species::Messenger, 1);
        let life = Species::Messenger.lifespan_cycles();
        for _ in 0..life - 1 {
            assert!(!d.advance_cycle());
        }
        assert!(d.advance_cycle());
    }
}
