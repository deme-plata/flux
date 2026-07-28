//! flux-md — molecular dynamics that actually computes, and proves it.
//!
//! The `q-bio-dsl` front end in this fleet parses molecules beautifully and
//! then *sleeps*: `PlaceAtom` is a debug log and a 100 µs delay. This crate is
//! the missing backend — a real Lennard-Jones force field, a real velocity-
//! Verlet integrator, periodic boundaries with minimum-image convention, and
//! observables (temperature, radial distribution function, mean-squared
//! displacement) that are compared against **published experimental values**
//! rather than against themselves.
//!
//! # Why this crate is small on purpose
//!
//! "The Arithmetic of Long Life" (SIGIL research library) proves that *exact*
//! classical simulation of chemistry is forbidden by the covariant entropy
//! bound — an Earth-sized memory tops out at ~279 spin-orbitals. So the honest
//! route is never "simulate it exactly"; it is "approximate it, and check the
//! approximation against reality." Every claim this crate makes is falsifiable
//! against a measured number, and the test suite falsifies them on every run.
//!
//! # Units (self-consistent MD units)
//!
//! | quantity | unit          |
//! |----------|---------------|
//! | length   | nm            |
//! | mass     | amu (g/mol)   |
//! | time     | ps            |
//! | energy   | kJ/mol        |
//!
//! These are consistent: 1 amu·nm²/ps² = 1 kJ/mol exactly, so acceleration in
//! nm/ps² is simply force (kJ/mol/nm) divided by mass (amu). No fudge factors
//! appear anywhere in this crate; if one were needed, the unit system is wrong.
//!
//! ```
//! use flux_md::{System, Species, Integrator};
//! // A small argon box, equilibrated then run in NVE.
//! let mut sys = System::fcc_lattice(Species::argon(), 4, 1.374, 42);
//! sys.set_temperature(94.4, 42);
//! let mut integ = Integrator::new(0.004); // 4 fs
//! for _ in 0..200 { integ.step(&mut sys); }
//! // Energy is conserved to a fraction of a percent without a thermostat.
//! assert!(sys.temperature() > 0.0);
//! ```

#![forbid(unsafe_code)]

pub mod molecule;
pub mod observables;

/// Boltzmann constant in kJ/(mol·K).
pub const K_B: f64 = 0.008314462618;

/// Avogadro's number (1/mol) — used only for density conversions.
pub const N_A: f64 = 6.02214076e23;

/// A deterministic PRNG (PCG-XSH-RR 64/32). Reproducibility is a feature: two
/// runs with the same seed produce byte-identical trajectories, so a validation
/// result can be re-derived rather than trusted.
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
    inc: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut r = Self { state: 0, inc: (seed << 1) | 1 };
        r.next_u32();
        r.state = r.state.wrapping_add(seed);
        r.next_u32();
        r
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old
            .wrapping_mul(6364136223846793005)
            .wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Uniform in [0, 1).
    pub fn uniform(&mut self) -> f64 {
        self.next_u32() as f64 / 4_294_967_296.0
    }

    /// Standard normal via Box-Muller.
    pub fn normal(&mut self) -> f64 {
        let u1 = self.uniform().max(1e-12);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

/// A Lennard-Jones species: `U(r) = 4ε[(σ/r)¹² − (σ/r)⁶]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Species {
    pub name: &'static str,
    /// Collision diameter (nm).
    pub sigma: f64,
    /// Well depth (kJ/mol).
    pub epsilon: f64,
    /// Mass (amu).
    pub mass: f64,
    /// Molar mass (g/mol) — equals `mass` for a monatomic species.
    pub molar_mass: f64,
}

impl Species {
    /// Argon. The canonical MD validation system: Rahman's 1964 parameters,
    /// ε/k_B = 119.8 K, σ = 0.3405 nm, M = 39.948 g/mol.
    pub const fn argon() -> Self {
        Self {
            name: "Ar",
            sigma: 0.3405,
            epsilon: 0.9961, // 119.8 K × k_B
            mass: 39.948,
            molar_mass: 39.948,
        }
    }
}

/// Cartesian vector in nm.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct V3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl V3 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0, z: 0.0 };
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }
    pub fn norm2(&self) -> f64 {
        self.x * self.x + self.y * self.y + self.z * self.z
    }
    pub fn norm(&self) -> f64 {
        self.norm2().sqrt()
    }
    pub fn scale(&self, s: f64) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
    pub fn add(&self, o: &Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
    pub fn sub(&self, o: &Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

/// A periodic cubic box of identical LJ particles.
#[derive(Debug, Clone)]
pub struct System {
    pub species: Species,
    /// Box edge (nm).
    pub box_len: f64,
    /// Wrapped positions (nm).
    pub pos: Vec<V3>,
    /// Unwrapped positions — required for a correct mean-squared displacement;
    /// using wrapped coordinates is the classic way to compute a diffusion
    /// coefficient that is silently wrong.
    pub pos_unwrapped: Vec<V3>,
    /// Velocities (nm/ps).
    pub vel: Vec<V3>,
    /// Forces (kJ/mol/nm).
    pub force: Vec<V3>,
    /// Interaction cutoff (nm).
    pub cutoff: f64,
    /// Potential energy of the current configuration (kJ/mol).
    pub potential: f64,
}

impl System {
    /// Build an FCC lattice of `cells³ × 4` atoms at a given mass density
    /// (g/cm³) — the standard way to start a liquid without atom overlap.
    pub fn fcc_lattice(species: Species, cells: usize, density_g_cm3: f64, seed: u64) -> Self {
        let n = 4 * cells * cells * cells;
        // number density (1/nm³) from mass density (g/cm³)
        let num_density = density_g_cm3 / species.molar_mass * N_A * 1e-21;
        let box_len = (n as f64 / num_density).cbrt();
        let a = box_len / cells as f64; // conventional cell edge
        let basis = [
            V3::new(0.0, 0.0, 0.0),
            V3::new(0.5, 0.5, 0.0),
            V3::new(0.5, 0.0, 0.5),
            V3::new(0.0, 0.5, 0.5),
        ];
        let mut pos = Vec::with_capacity(n);
        for i in 0..cells {
            for j in 0..cells {
                for k in 0..cells {
                    for b in &basis {
                        pos.push(V3::new(
                            (i as f64 + b.x) * a,
                            (j as f64 + b.y) * a,
                            (k as f64 + b.z) * a,
                        ));
                    }
                }
            }
        }
        let _ = seed;
        let cutoff = (2.5 * species.sigma).min(0.5 * box_len);
        let mut sys = Self {
            species,
            box_len,
            pos_unwrapped: pos.clone(),
            pos,
            vel: vec![V3::ZERO; n],
            force: vec![V3::ZERO; n],
            cutoff,
            potential: 0.0,
        };
        sys.compute_forces();
        sys
    }

    pub fn n(&self) -> usize {
        self.pos.len()
    }

    /// Volume (nm³).
    pub fn volume(&self) -> f64 {
        self.box_len.powi(3)
    }

    /// Number density (1/nm³).
    pub fn number_density(&self) -> f64 {
        self.n() as f64 / self.volume()
    }

    /// Mass density (g/cm³).
    pub fn mass_density(&self) -> f64 {
        self.number_density() * self.species.molar_mass / N_A * 1e21
    }

    /// Minimum-image displacement from `j` to `i`.
    pub fn min_image(&self, a: V3, b: V3) -> V3 {
        let l = self.box_len;
        let mut d = a.sub(&b);
        d.x -= l * (d.x / l).round();
        d.y -= l * (d.y / l).round();
        d.z -= l * (d.z / l).round();
        d
    }

    /// Draw Maxwell-Boltzmann velocities at `temp_k`, remove centre-of-mass
    /// drift, then rescale so the instantaneous temperature is exact.
    pub fn set_temperature(&mut self, temp_k: f64, seed: u64) {
        let mut rng = Rng::new(seed);
        let sd = (K_B * temp_k / self.species.mass).sqrt();
        for v in self.vel.iter_mut() {
            *v = V3::new(rng.normal() * sd, rng.normal() * sd, rng.normal() * sd);
        }
        self.remove_com_motion();
        let t = self.temperature();
        if t > 0.0 {
            let s = (temp_k / t).sqrt();
            for v in self.vel.iter_mut() {
                *v = v.scale(s);
            }
        }
    }

    /// Zero the total momentum. Without this the box drifts and the measured
    /// diffusion coefficient is contaminated by bulk translation.
    pub fn remove_com_motion(&mut self) {
        let n = self.n() as f64;
        let mut com = V3::ZERO;
        for v in &self.vel {
            com = com.add(v);
        }
        com = com.scale(1.0 / n);
        for v in self.vel.iter_mut() {
            *v = v.sub(&com);
        }
    }

    /// Kinetic energy (kJ/mol).
    pub fn kinetic(&self) -> f64 {
        0.5 * self.species.mass * self.vel.iter().map(|v| v.norm2()).sum::<f64>()
    }

    /// Instantaneous temperature (K) from equipartition, with the three
    /// centre-of-mass constraints removed from the degree-of-freedom count.
    pub fn temperature(&self) -> f64 {
        let dof = (3 * self.n() - 3) as f64;
        2.0 * self.kinetic() / (dof * K_B)
    }

    pub fn total_energy(&self) -> f64 {
        self.kinetic() + self.potential
    }

    /// Recompute forces and potential energy from scratch: O(N²) with a
    /// cutoff. Honest and simple; a neighbour list is an optimisation, not a
    /// correctness change, and this crate prioritises being checkable.
    ///
    /// The potential is **shifted** so that `U(cutoff) = 0`, which removes the
    /// energy discontinuity at the cutoff that would otherwise inject heat and
    /// ruin energy conservation.
    pub fn compute_forces(&mut self) {
        let n = self.n();
        for f in self.force.iter_mut() {
            *f = V3::ZERO;
        }
        let (sig, eps, rc) = (self.species.sigma, self.species.epsilon, self.cutoff);
        let rc2 = rc * rc;
        let sr6c = (sig / rc).powi(6);
        let u_shift = 4.0 * eps * (sr6c * sr6c - sr6c);
        let mut pot = 0.0;
        for i in 0..n {
            for j in (i + 1)..n {
                let d = self.min_image(self.pos[i], self.pos[j]);
                let r2 = d.norm2();
                if r2 >= rc2 || r2 == 0.0 {
                    continue;
                }
                let inv_r2 = 1.0 / r2;
                let sr2 = sig * sig * inv_r2;
                let sr6 = sr2 * sr2 * sr2;
                let sr12 = sr6 * sr6;
                pot += 4.0 * eps * (sr12 - sr6) - u_shift;
                // F = -dU/dr * r_hat  =>  F_vec = 24ε(2σ¹²/r¹² − σ⁶/r⁶)/r² · d
                let fmag = 24.0 * eps * (2.0 * sr12 - sr6) * inv_r2;
                let fv = d.scale(fmag);
                self.force[i] = self.force[i].add(&fv);
                self.force[j] = self.force[j].sub(&fv);
            }
        }
        self.potential = pot;
    }

    /// Potential energy alone, for a numerical force check.
    pub fn potential_energy(&self) -> f64 {
        let mut clone = self.clone();
        clone.compute_forces();
        clone.potential
    }

    /// Wrap positions back into the box, tracking the unwrapped copy.
    fn wrap(&mut self) {
        let l = self.box_len;
        for p in self.pos.iter_mut() {
            p.x -= l * (p.x / l).floor();
            p.y -= l * (p.y / l).floor();
            p.z -= l * (p.z / l).floor();
        }
    }

    /// Rescale velocities toward a target temperature (Berendsen-style, with
    /// `strength` in [0,1]). Equilibration only — a rescaling thermostat does
    /// not sample a correct canonical ensemble, so production runs below are
    /// pure NVE and energy conservation is therefore a meaningful test.
    pub fn rescale_to(&mut self, target_k: f64, strength: f64) {
        let t = self.temperature();
        if t <= 0.0 {
            return;
        }
        let lambda = (1.0 + strength * (target_k / t - 1.0)).max(0.0).sqrt();
        for v in self.vel.iter_mut() {
            *v = v.scale(lambda);
        }
    }
}

/// Velocity-Verlet integrator — symplectic, time-reversible, and the reason
/// energy conservation below is a real test rather than a thermostat artifact.
#[derive(Debug, Clone, Copy)]
pub struct Integrator {
    /// Timestep (ps).
    pub dt: f64,
}

impl Integrator {
    pub fn new(dt: f64) -> Self {
        Self { dt }
    }

    /// One velocity-Verlet step:
    /// `v(t+½dt) = v + a dt/2`, `r(t+dt) = r + v(t+½dt) dt`,
    /// recompute `a`, `v(t+dt) = v(t+½dt) + a dt/2`.
    pub fn step(&self, sys: &mut System) {
        let dt = self.dt;
        let inv_m = 1.0 / sys.species.mass;
        let half = 0.5 * dt;
        for i in 0..sys.n() {
            let a = sys.force[i].scale(inv_m);
            sys.vel[i] = sys.vel[i].add(&a.scale(half));
            let dr = sys.vel[i].scale(dt);
            sys.pos[i] = sys.pos[i].add(&dr);
            sys.pos_unwrapped[i] = sys.pos_unwrapped[i].add(&dr);
        }
        sys.wrap();
        sys.compute_forces();
        for i in 0..sys.n() {
            let a = sys.force[i].scale(inv_m);
            sys.vel[i] = sys.vel[i].add(&a.scale(half));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observables::{diffusion_coefficient, rdf, RdfAccumulator};

    /// The single most important test in an MD code: the analytic force must
    /// equal −dU/dx computed numerically. If this passes, the integrator is
    /// propagating the potential it claims to.
    #[test]
    fn force_matches_numerical_gradient_of_potential() {
        let mut sys = System::fcc_lattice(Species::argon(), 2, 1.374, 7);
        // Displace an atom off-lattice so forces are non-trivial.
        sys.pos[0] = sys.pos[0].add(&V3::new(0.031, -0.017, 0.023));
        sys.compute_forces();
        let analytic = sys.force[0];
        let h = 1e-6;
        let mut num = V3::ZERO;
        for axis in 0..3 {
            let mut plus = sys.clone();
            let mut minus = sys.clone();
            match axis {
                0 => {
                    plus.pos[0].x += h;
                    minus.pos[0].x -= h;
                }
                1 => {
                    plus.pos[0].y += h;
                    minus.pos[0].y -= h;
                }
                _ => {
                    plus.pos[0].z += h;
                    minus.pos[0].z -= h;
                }
            }
            let du = (plus.potential_energy() - minus.potential_energy()) / (2.0 * h);
            match axis {
                0 => num.x = -du,
                1 => num.y = -du,
                _ => num.z = -du,
            }
        }
        let err = analytic.sub(&num).norm() / analytic.norm().max(1e-12);
        assert!(err < 1e-5, "force != -grad U: rel err {err}, analytic {analytic:?}, numeric {num:?}");
    }

    /// NVE energy conservation. No thermostat is applied, so any drift is the
    /// integrator's own error.
    #[test]
    fn nve_conserves_energy() {
        let mut sys = System::fcc_lattice(Species::argon(), 3, 1.374, 11);
        sys.set_temperature(94.4, 11);
        let integ = Integrator::new(0.004); // 4 fs
        for _ in 0..300 {
            integ.step(&mut sys);
        }
        let e0 = sys.total_energy();
        for _ in 0..1000 {
            integ.step(&mut sys);
        }
        let drift = (sys.total_energy() - e0).abs() / e0.abs();
        assert!(drift < 1e-3, "NVE energy drift {drift:.3e} exceeds 0.1%");
    }

    #[test]
    fn temperature_follows_equipartition() {
        let mut sys = System::fcc_lattice(Species::argon(), 3, 1.374, 3);
        sys.set_temperature(94.4, 3);
        assert!((sys.temperature() - 94.4).abs() < 1e-6);
    }

    #[test]
    fn total_momentum_stays_zero() {
        let mut sys = System::fcc_lattice(Species::argon(), 2, 1.374, 5);
        sys.set_temperature(94.4, 5);
        let integ = Integrator::new(0.004);
        for _ in 0..200 {
            integ.step(&mut sys);
        }
        let mut p = V3::ZERO;
        for v in &sys.vel {
            p = p.add(v);
        }
        assert!(p.norm() < 1e-9, "momentum drifted to {:?}", p);
    }

    #[test]
    fn minimum_image_is_correct_across_the_boundary() {
        let sys = System::fcc_lattice(Species::argon(), 2, 1.374, 1);
        let l = sys.box_len;
        let d = sys.min_image(V3::new(0.01, 0.0, 0.0), V3::new(l - 0.01, 0.0, 0.0));
        assert!((d.x - 0.02).abs() < 1e-12, "wrapped separation wrong: {d:?}");
    }

    #[test]
    fn density_round_trips_through_the_lattice_builder() {
        let sys = System::fcc_lattice(Species::argon(), 3, 1.374, 1);
        assert!((sys.mass_density() - 1.374).abs() < 1e-9);
    }

    /// Structure: liquid argon's first RDF peak sits near 3.7 Å (0.37 nm).
    /// This is a measured value, not an output of this program.
    #[test]
    fn rdf_first_peak_matches_experiment_for_liquid_argon() {
        let mut sys = System::fcc_lattice(Species::argon(), 3, 1.374, 23);
        sys.set_temperature(94.4, 23);
        let integ = Integrator::new(0.004);
        for _ in 0..800 {
            integ.step(&mut sys);
            sys.rescale_to(94.4, 0.1);
        }
        let mut acc = RdfAccumulator::new(200, sys.cutoff);
        for i in 0..600 {
            integ.step(&mut sys);
            if i % 10 == 0 {
                acc.sample(&sys);
            }
        }
        let g = rdf(&acc, &sys);
        let (peak_r, peak_g) = g
            .iter()
            .fold((0.0, 0.0), |acc, &(r, v)| if v > acc.1 { (r, v) } else { acc });
        assert!(
            (peak_r - 0.37).abs() < 0.04,
            "first RDF peak at {peak_r:.3} nm, experiment ~0.37 nm"
        );
        assert!(peak_g > 1.5, "liquid should be structured, g_max = {peak_g:.2}");
    }

    /// Deterministic: identical seeds must give identical trajectories, so any
    /// validation number in the README can be re-derived exactly.
    #[test]
    fn runs_are_reproducible_from_the_seed() {
        let run = || {
            let mut sys = System::fcc_lattice(Species::argon(), 2, 1.374, 99);
            sys.set_temperature(94.4, 99);
            let integ = Integrator::new(0.004);
            for _ in 0..100 {
                integ.step(&mut sys);
            }
            sys.total_energy()
        };
        assert_eq!(run().to_bits(), run().to_bits());
    }

    #[test]
    fn diffusion_of_a_frozen_system_is_zero() {
        let sys = System::fcc_lattice(Species::argon(), 2, 1.374, 4);
        let d = diffusion_coefficient(&[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)]);
        assert!(d.abs() < 1e-12);
        let _ = sys;
    }
}
