//! flux-cowsim — the HEADLESS virtual-fence / cow-haptic-feedback sim core, the CPU backend for the
//! `cowherd` boilerplate. The browser shows the canvas; this is the real agent-based model that runs
//! on Epsilon's CPU (no GPU, no bevy) — and being a flux crate it's `flux_optimize`-able.
//!
//! The thing being simulated is a GPS **virtual fence** with a **collar state machine**:
//!
//! ```text
//!     Calm ──(approach fence: within the warn band)──▶ Sound  ──(cross the fence)──▶ Haptic
//!       ▲                                                │                              │
//!       └────────────(retreat inside)───────────────────┴──────────────────────────────┘
//! ```
//!
//! Real virtual-fence collars (Nofence, eShepherd, …) play an audio cue as the animal nears the
//! boundary and deliver a mild haptic pulse only if it keeps going. The whole welfare argument rests
//! on **associative learning**: the cow learns the sound *predicts* the pulse (Pavlovian), so a
//! trained herd turns at the sound and is almost never pulsed. So this model's job — and what
//! `flux_optimize` tunes the collar protocol for — is **maximize containment while minimizing the
//! number of haptic pulses**, by letting the herd learn the boundary as fast as possible.
//!
//! Movement is Boids (cohesion toward the grazing centre + separation + jitter); the fence response
//! is the collar push, scaled by how much each cow has learned. Deterministic: a seeded splitmix64
//! PRNG drives the jitter, so a given (params, seed) reproduces exactly — the chronos ethos.
//!
//! Pattern lifted from the q-narwhalknight void-walker agent-field + the q-robot-cli collar; this is
//! the part that decides whether the protocol is humane, distilled to a curve you can optimize.

use serde::{Deserialize, Serialize};

/// Deterministic splitmix64 — reproducible jitter so (params, seed) ⇒ identical run.
#[derive(Debug, Clone)]
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// uniform 0..1
    fn unit(&mut self) -> f64 { (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64 }
    /// signed −1..1
    fn signed(&mut self) -> f64 { self.unit() * 2.0 - 1.0 }
}

/// The collar state for a cow this step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Collar { Calm = 0, Sound = 2, Haptic = 3 }

/// Collar-protocol + herd parameters — what an agent/`flux_optimize` tunes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CowParams {
    pub n: usize,           // herd size
    pub field_w: f64,
    pub field_h: f64,
    pub fence_radius: f64,  // virtual-fence radius about the field centre
    pub warn_band: f64,     // width of the audio (Sound) zone just inside the fence
    // boids
    pub cohesion: f64,      // pull toward the HERD centroid (keeps the herd together, doesn't anchor it)
    pub separation: f64,    // push off close neighbours
    pub sep_dist: f64,      // neighbour radius for separation
    pub graze: f64,         // outward grazing/exploration drive — what makes the herd test the fence
    pub friction: f64,      // velocity damping (0..1), so wander is a bounded random walk not ballistic
    pub jitter: f64,        // random wander
    pub max_speed: f64,
    // collar protocol
    pub sound_push: f64,    // turn-back force from the audio cue
    pub haptic_push: f64,   // turn-back force from the haptic pulse (strong)
    pub learn_gain: f64,    // how much a learned cow amplifies its sound response
    pub learn_rate: f64,    // learning per step from hearing the sound (slow)
    pub shock_learn: f64,   // learning per haptic pulse (fast, aversive)
    // objective weighting
    pub shock_weight: f64,  // welfare priority: how heavily pulses count against the score
    pub seed: u64,
}

impl Default for CowParams {
    fn default() -> Self {
        CowParams {
            n: 60, field_w: 860.0, field_h: 540.0, fence_radius: 200.0, warn_band: 45.0,
            cohesion: 0.008, separation: 0.10, sep_dist: 16.0, graze: 0.05, friction: 0.85,
            jitter: 0.06, max_speed: 2.0,
            sound_push: 0.02, haptic_push: 0.7, learn_gain: 5.0, learn_rate: 0.002, shock_learn: 0.03,
            shock_weight: 50.0, seed: 42,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Cow { x: f64, y: f64, vx: f64, vy: f64, learn: f64, state: Collar }

/// One sampled point of the run (what the browser viz / a stream consumes).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HerdReport {
    pub step: u32,
    pub inside_frac: f64,   // fraction of the herd currently inside the fence
    pub mean_learn: f64,    // herd-average learning (0..1)
    pub learned_frac: f64,  // fraction that has learned the fence (learn > 0.5)
    pub shocks_cum: u64,    // cumulative haptic pulses so far
    pub sounds_cum: u64,    // cumulative audio cues so far
}

/// End-of-run summary — `welfare_score` is the scalar `flux_optimize` maximizes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HerdSummary {
    pub steps: u32,
    pub n: usize,
    pub shocks_total: u64,
    pub sounds_total: u64,
    pub mean_containment: f64,  // time-averaged inside_frac, 0..1
    pub final_mean_learn: f64,  // 0..1
    pub learned_frac: f64,      // 0..1 at the end
    pub shock_rate: f64,        // shocks per cow per step
    pub welfare_score: f64,     // containment − shock_weight·shock_rate  (maximize)
}

/// The herd. `step` advances the Boids + collar ABM one tick.
#[derive(Debug, Clone)]
pub struct Herd {
    pub p: CowParams,
    cows: Vec<Cow>,
    rng: Rng,
    pub t: u32,
    shocks: u64,
    sounds: u64,
    contain_sum: f64, // Σ inside_frac, for the time-average
}

impl Herd {
    pub fn new(p: CowParams) -> Self {
        let mut rng = Rng(p.seed.max(1));
        let (cx, cy) = (p.field_w / 2.0, p.field_h / 2.0);
        // spawn the herd INSIDE the fence (a paddock placement, uniform over the disk)
        let cows = (0..p.n).map(|_| {
            let ang = rng.unit() * std::f64::consts::TAU;
            let rad = p.fence_radius * rng.unit().sqrt();
            Cow { x: cx + rad * ang.cos(), y: cy + rad * ang.sin(),
                  vx: 0.0, vy: 0.0, learn: 0.0, state: Collar::Calm }
        }).collect();
        Herd { p, cows, rng, t: 0, shocks: 0, sounds: 0, contain_sum: 0.0 }
    }

    fn center(&self) -> (f64, f64) { (self.p.field_w / 2.0, self.p.field_h / 2.0) }

    /// Advance one tick. Returns the sampled report for this step.
    pub fn step(&mut self) -> HerdReport {
        let (cx, cy) = self.center(); // the FIXED geofence centre (a GPS geofence, not the herd)
        let p = self.p;
        // snapshot positions so neighbour separation is order-independent (deterministic)
        let snap: Vec<(f64, f64)> = self.cows.iter().map(|c| (c.x, c.y)).collect();
        // herd centroid drives boids cohesion (keeps the herd together without anchoring its location)
        let n = snap.len().max(1) as f64;
        let hx = snap.iter().map(|p| p.0).sum::<f64>() / n;
        let hy = snap.iter().map(|p| p.1).sum::<f64>() / n;
        let mut inside = 0usize;
        for i in 0..self.cows.len() {
            let (mut vx, mut vy) = (self.cows[i].vx, self.cows[i].vy);
            let (px, py) = (self.cows[i].x, self.cows[i].y);
            // cohesion toward the herd centroid
            let gx = hx - px; let gy = hy - py;
            let gd = (gx * gx + gy * gy).sqrt().max(1e-9);
            vx += gx / gd * p.cohesion; vy += gy / gd * p.cohesion;
            // separation from close neighbours
            for (j, &(ox, oy)) in snap.iter().enumerate() {
                if j == i { continue; }
                let ex = px - ox; let ey = py - oy;
                let ed = (ex * ex + ey * ey).sqrt();
                if ed < p.sep_dist && ed > 0.0 { vx += ex / ed * p.separation; vy += ey / ed * p.separation; }
            }
            // radial geometry about the geofence centre
            let rx = px - cx; let ry = py - cy;          // outward radial vector
            let d = (rx * rx + ry * ry).sqrt().max(1e-9); // distance from fence centre
            // grazing/exploration drive: cows wander OUTWARD — this is what tests the fence
            vx += rx / d * p.graze; vy += ry / d * p.graze;
            // deterministic jitter
            vx += self.rng.signed() * p.jitter; vy += self.rng.signed() * p.jitter;

            // collar state machine, turn-back is INWARD (−radial), scaled by what this cow has learned
            let from_fence = p.fence_radius - d; // >0 inside, <0 crossed
            let resp = 1.0 + p.learn_gain * self.cows[i].learn;
            if from_fence < 0.0 {
                // crossed the boundary → haptic pulse (strong turn-back + fast aversive learning)
                self.cows[i].state = Collar::Haptic;
                vx -= rx / d * p.haptic_push; vy -= ry / d * p.haptic_push;
                self.cows[i].learn = (self.cows[i].learn + p.shock_learn).min(1.0);
                self.shocks += 1;
            } else if from_fence < p.warn_band {
                // in the audio band → sound cue (turn-back scaled by learning + slow learning)
                self.cows[i].state = Collar::Sound;
                vx -= rx / d * p.sound_push * resp; vy -= ry / d * p.sound_push * resp;
                self.cows[i].learn = (self.cows[i].learn + p.learn_rate).min(1.0);
                self.sounds += 1;
            } else {
                self.cows[i].state = Collar::Calm;
            }

            // friction (bounded random walk), speed cap, then move
            vx *= p.friction; vy *= p.friction;
            let s = (vx * vx + vy * vy).sqrt();
            if s > p.max_speed { vx *= p.max_speed / s; vy *= p.max_speed / s; }
            let (nx, ny) = (px + vx, py + vy);
            self.cows[i].vx = vx; self.cows[i].vy = vy; self.cows[i].x = nx; self.cows[i].y = ny;

            let nd = ((cx - nx).powi(2) + (cy - ny).powi(2)).sqrt();
            if nd < p.fence_radius { inside += 1; }
        }
        let n = self.cows.len().max(1) as f64;
        let inside_frac = inside as f64 / n;
        self.contain_sum += inside_frac;
        self.t += 1;
        HerdReport {
            step: self.t, inside_frac,
            mean_learn: self.cows.iter().map(|c| c.learn).sum::<f64>() / n,
            learned_frac: self.cows.iter().filter(|c| c.learn > 0.5).count() as f64 / n,
            shocks_cum: self.shocks, sounds_cum: self.sounds,
        }
    }

    /// Run `steps` ticks, returning every `sample`-th report plus the end summary.
    pub fn run(&mut self, steps: u32, sample: u32) -> (Vec<HerdReport>, HerdSummary) {
        let sample = sample.max(1);
        let mut out = Vec::new();
        for _ in 0..steps {
            let r = self.step();
            if r.step % sample == 0 { out.push(r); }
        }
        (out, self.summary(steps))
    }

    fn summary(&self, steps: u32) -> HerdSummary {
        let n = self.cows.len().max(1);
        let mean_containment = if steps > 0 { self.contain_sum / steps as f64 } else { 0.0 };
        let shock_rate = self.shocks as f64 / (n as f64 * steps.max(1) as f64);
        HerdSummary {
            steps, n,
            shocks_total: self.shocks, sounds_total: self.sounds,
            mean_containment,
            final_mean_learn: self.cows.iter().map(|c| c.learn).sum::<f64>() / n as f64,
            learned_frac: self.cows.iter().filter(|c| c.learn > 0.5).count() as f64 / n as f64,
            shock_rate,
            welfare_score: mean_containment - self.p.shock_weight * shock_rate,
        }
    }
}

/// Containment − welfare penalty after `steps` — the scalar `flux_optimize` MAXIMIZES when tuning
/// the collar protocol (warn band, sound strength, learning) for a humane fence.
pub fn welfare_score(p: CowParams, steps: u32) -> f64 {
    let mut h = Herd::new(p);
    h.run(steps, steps).1.welfare_score
}

/// Total haptic pulses delivered over `steps` — the welfare cost you want to drive down.
pub fn total_shocks(p: CowParams, steps: u32) -> u64 {
    let mut h = Herd::new(p);
    h.run(steps, steps).1.shocks_total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_is_deterministic_for_a_seed() {
        let a = total_shocks(CowParams::default(), 1500);
        let b = total_shocks(CowParams::default(), 1500);
        assert_eq!(a, b, "same (params, seed) ⇒ identical run");
    }

    #[test]
    fn the_fence_contains_the_herd() {
        // with a working collar, most of the herd stays inside; with no collar response they wander out.
        let on = Herd::new(CowParams::default()).run(2500, 2500).1;
        let off = Herd::new(CowParams { sound_push: 0.0, haptic_push: 0.0, ..Default::default() })
            .run(2500, 2500).1;
        assert!(on.mean_containment > off.mean_containment,
            "the collar contains the herd ({} > {})", on.mean_containment, off.mean_containment);
        assert!(on.mean_containment > 0.6, "a working fence keeps most of the herd in ({})", on.mean_containment);
    }

    #[test]
    fn the_herd_learns_and_gets_shocked_less_over_time() {
        // Pavlovian: as the herd learns the sound predicts the pulse, pulses-per-step should fall.
        let mut h = Herd::new(CowParams::default());
        let (series, sum) = h.run(4000, 250);
        // shocks in the first quarter vs the last quarter of the run
        let first = series.iter().find(|r| r.step >= 1000).unwrap().shocks_cum;
        let q3 = series.iter().find(|r| r.step >= 3000).unwrap().shocks_cum;
        let last = sum.shocks_total;
        let early_rate = first as f64 / 1000.0;
        let late_rate = (last - q3) as f64 / 1000.0;
        assert!(sum.final_mean_learn > 0.0, "the herd learns the fence ({})", sum.final_mean_learn);
        assert!(late_rate < early_rate,
            "a trained herd is pulsed less ({late_rate}/step late < {early_rate}/step early)");
    }

    #[test]
    fn a_wider_warn_band_means_fewer_pulses() {
        // a wider audio zone warns cows earlier ⇒ they turn before crossing ⇒ fewer haptic pulses.
        let narrow = total_shocks(CowParams { warn_band: 15.0, ..Default::default() }, 3000);
        let wide = total_shocks(CowParams { warn_band: 70.0, ..Default::default() }, 3000);
        assert!(wide < narrow, "wider warn band ⇒ fewer pulses ({wide} < {narrow})");
    }

    #[test]
    fn a_humane_protocol_scores_above_a_shock_only_one() {
        // sound + learning (humane) should beat shock-only (no audio cue, never learns the warning):
        // both may contain the herd, but the shock-only one pays a heavy welfare penalty in pulses.
        let humane = welfare_score(CowParams::default(), 3000);
        let shock_only = welfare_score(CowParams { warn_band: 0.0, sound_push: 0.0, ..Default::default() }, 3000);
        assert!(humane > shock_only,
            "the humane collar scores higher ({humane} > {shock_only})");
    }
}
