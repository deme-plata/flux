//! Isolated molecules: multiple species, harmonic bonds, energy minimisation.
//!
//! The [`crate::System`] type models a periodic box of one species — right for
//! liquid argon, wrong for a molecule. This module adds what a molecular
//! program actually needs:
//!
//! * **per-atom** Lennard-Jones parameters, combined by the Lorentz-Berthelot
//!   rules (`σ_ij = (σ_i+σ_j)/2`, `ε_ij = √(ε_i ε_j)`),
//! * **harmonic bonds** `U = ½k(r−r₀)²`,
//! * **1-2 exclusion** — bonded pairs do not also interact through LJ, or the
//!   repulsive core would fight the bond and no geometry would ever converge,
//! * **steepest-descent minimisation** with an adaptive step, which is what you
//!   want to ask "did this molecular program produce a sensible structure?"
//!
//! No periodic boundaries here: an isolated molecule in vacuum. That is the
//! honest model for a synthesis program, and it keeps the energy checkable.

use crate::{V3, K_B};

/// Per-atom force-field parameters. Values are OPLS-style united-atom
/// approximations in this crate's units (nm, kJ/mol, amu) — good enough to
/// relax a geometry, not good enough to publish a binding affinity, and this
/// crate says so rather than implying otherwise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AtomType {
    pub symbol: &'static str,
    /// LJ collision diameter (nm).
    pub sigma: f64,
    /// LJ well depth (kJ/mol).
    pub epsilon: f64,
    /// Mass (amu).
    pub mass: f64,
}

impl AtomType {
    pub const fn new(symbol: &'static str, sigma: f64, epsilon: f64, mass: f64) -> Self {
        Self { symbol, sigma, epsilon, mass }
    }

    /// Look up a common organic element by symbol. Returns `None` for elements
    /// this force field has no parameters for — an explicit gap rather than a
    /// silent default, because a fabricated parameter is worse than a refusal.
    pub fn from_symbol(sym: &str) -> Option<Self> {
        Some(match sym {
            "H" => Self::new("H", 0.2500, 0.1255, 1.008),
            "C" => Self::new("C", 0.3500, 0.2761, 12.011),
            "N" => Self::new("N", 0.3250, 0.7113, 14.007),
            "O" => Self::new("O", 0.2960, 0.8786, 15.999),
            "F" => Self::new("F", 0.2940, 0.2552, 18.998),
            "P" => Self::new("P", 0.3740, 0.8368, 30.974),
            "S" => Self::new("S", 0.3550, 1.0460, 32.065),
            "Cl" => Self::new("Cl", 0.3400, 1.2552, 35.453),
            "Ar" => Self::new("Ar", 0.3405, 0.9961, 39.948),
            _ => return None,
        })
    }
}

/// A harmonic bond: `U = ½ k (r − r₀)²`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HarmonicBond {
    pub i: usize,
    pub j: usize,
    /// Equilibrium length (nm).
    pub r0: f64,
    /// Force constant (kJ/mol/nm²).
    pub k: f64,
}

/// Default bond force constant (kJ/mol/nm²), typical of a single bond between
/// second-row elements.
pub const DEFAULT_BOND_K: f64 = 250_000.0;

/// A harmonic angle `i–j–k` about the central atom `j`: `U = ½ k (θ − θ₀)²`.
///
/// Angles are not decoration. Atoms two bonds apart (1-3 pairs) sit well inside
/// each other's Lennard-Jones core — in methane the H···H separation is 0.178 nm
/// against an H σ of 0.25 nm — so if they are left to interact through LJ the
/// repulsion fights the bonds and no geometry converges. Every real force field
/// therefore *excludes* 1-3 pairs from LJ and restrains them with this term
/// instead. Omitting it is a classic way to get bond lengths that are quietly
/// 10% too long.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HarmonicAngle {
    pub i: usize,
    pub j: usize,
    pub k: usize,
    /// Equilibrium angle (radians).
    pub theta0: f64,
    /// Force constant (kJ/mol/rad²).
    pub k_theta: f64,
}

/// Default angle force constant (kJ/mol/rad²).
pub const DEFAULT_ANGLE_K: f64 = 400.0;

/// Ideal tetrahedral angle (radians) — 109.47°.
pub const TETRAHEDRAL: f64 = 1.910_633_236_249_019;

/// An isolated molecule: atoms, bonds, and the machinery to relax it.
#[derive(Debug, Clone, Default)]
pub struct Molecule {
    pub types: Vec<AtomType>,
    pub pos: Vec<V3>,
    pub vel: Vec<V3>,
    pub force: Vec<V3>,
    pub bonds: Vec<HarmonicBond>,
    pub angles: Vec<HarmonicAngle>,
    pub potential: f64,
}

impl Molecule {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an atom at a position (nm); returns its index.
    pub fn add_atom(&mut self, t: AtomType, pos: V3) -> usize {
        self.types.push(t);
        self.pos.push(pos);
        self.vel.push(V3::ZERO);
        self.force.push(V3::ZERO);
        self.types.len() - 1
    }

    /// Bond two atoms. Returns `false` if either index is out of range.
    pub fn add_bond(&mut self, i: usize, j: usize, r0: f64, k: f64) -> bool {
        if i >= self.n() || j >= self.n() || i == j {
            return false;
        }
        self.bonds.push(HarmonicBond { i, j, r0, k });
        true
    }

    pub fn n(&self) -> usize {
        self.types.len()
    }

    /// Add a harmonic angle `i–j–k` centred on `j`.
    pub fn add_angle(&mut self, i: usize, j: usize, k: usize, theta0: f64, k_theta: f64) -> bool {
        if i.max(j).max(k) >= self.n() || i == j || j == k || i == k {
            return false;
        }
        self.angles.push(HarmonicAngle { i, j, k, theta0, k_theta });
        true
    }

    /// Generate every 1-3 angle implied by the bond topology, all at `theta0`.
    /// Convenience for programs that specify bonds but not angles.
    pub fn generate_angles(&mut self, theta0: f64, k_theta: f64) {
        let n = self.n();
        let mut neighbours: Vec<Vec<usize>> = vec![Vec::new(); n];
        for b in &self.bonds {
            neighbours[b.i].push(b.j);
            neighbours[b.j].push(b.i);
        }
        let mut new_angles = Vec::new();
        for (j, nb) in neighbours.iter().enumerate() {
            for a in 0..nb.len() {
                for c in (a + 1)..nb.len() {
                    new_angles.push(HarmonicAngle {
                        i: nb[a],
                        j,
                        k: nb[c],
                        theta0,
                        k_theta,
                    });
                }
            }
        }
        self.angles.extend(new_angles);
    }

    /// Is this pair directly bonded (1-2, excluded from LJ)?
    fn bonded(&self, i: usize, j: usize) -> bool {
        self.bonds
            .iter()
            .any(|b| (b.i == i && b.j == j) || (b.i == j && b.j == i))
    }

    /// Is this pair 1-3 (the two ends of an angle)? Also excluded from LJ.
    fn angle_pair(&self, i: usize, j: usize) -> bool {
        self.angles
            .iter()
            .any(|a| (a.i == i && a.k == j) || (a.i == j && a.k == i))
    }

    /// Angle `i–j–k` in radians.
    pub fn angle(&self, i: usize, j: usize, k: usize) -> f64 {
        let a = self.pos[i].sub(&self.pos[j]);
        let b = self.pos[k].sub(&self.pos[j]);
        let cos = (a.x * b.x + a.y * b.y + a.z * b.z) / (a.norm() * b.norm());
        cos.clamp(-1.0, 1.0).acos()
    }

    /// Distance between two atoms (nm).
    pub fn distance(&self, i: usize, j: usize) -> f64 {
        self.pos[i].sub(&self.pos[j]).norm()
    }

    /// Recompute forces and potential energy. Non-periodic, all pairs.
    pub fn compute_forces(&mut self) {
        for f in self.force.iter_mut() {
            *f = V3::ZERO;
        }
        let mut pot = 0.0;

        // --- non-bonded Lennard-Jones, Lorentz-Berthelot mixing -------------
        for i in 0..self.n() {
            for j in (i + 1)..self.n() {
                if self.bonded(i, j) || self.angle_pair(i, j) {
                    continue; // 1-2 and 1-3 exclusions
                }
                let sigma = 0.5 * (self.types[i].sigma + self.types[j].sigma);
                let eps = (self.types[i].epsilon * self.types[j].epsilon).sqrt();
                let d = self.pos[i].sub(&self.pos[j]);
                let r2 = d.norm2();
                if r2 < 1e-12 {
                    continue;
                }
                let inv_r2 = 1.0 / r2;
                let sr2 = sigma * sigma * inv_r2;
                let sr6 = sr2 * sr2 * sr2;
                let sr12 = sr6 * sr6;
                pot += 4.0 * eps * (sr12 - sr6);
                let fmag = 24.0 * eps * (2.0 * sr12 - sr6) * inv_r2;
                let fv = d.scale(fmag);
                self.force[i] = self.force[i].add(&fv);
                self.force[j] = self.force[j].sub(&fv);
            }
        }

        // --- harmonic bonds -------------------------------------------------
        for b in &self.bonds {
            let d = self.pos[b.i].sub(&self.pos[b.j]);
            let r = d.norm();
            if r < 1e-12 {
                continue;
            }
            let dr = r - b.r0;
            pot += 0.5 * b.k * dr * dr;
            // F_i = -dU/dr_i = -k (r - r0) * d/r
            let fv = d.scale(-b.k * dr / r);
            self.force[b.i] = self.force[b.i].add(&fv);
            self.force[b.j] = self.force[b.j].sub(&fv);
        }

        // --- harmonic angles -------------------------------------------------
        // U = ½k(θ−θ₀)² with θ the i–j–k angle. Using
        //   ∂θ/∂r_i = −(1/sinθ) ∂cosθ/∂r_i,
        //   ∂cosθ/∂r_i = r_jk/(|r_ji||r_jk|) − cosθ · r_ji/|r_ji|²,
        // and F = −∂U/∂r, the force on the outer atoms is
        //   F_i = (k(θ−θ₀)/sinθ) · ∂cosθ/∂r_i,
        // with F_j fixed by Newton's third law so the triple exerts no net force.
        for a in &self.angles {
            let rji = self.pos[a.i].sub(&self.pos[a.j]);
            let rjk = self.pos[a.k].sub(&self.pos[a.j]);
            let (n1, n2) = (rji.norm(), rjk.norm());
            if n1 < 1e-12 || n2 < 1e-12 {
                continue;
            }
            let dot = rji.x * rjk.x + rji.y * rjk.y + rji.z * rjk.z;
            let cos = (dot / (n1 * n2)).clamp(-1.0, 1.0);
            let theta = cos.acos();
            let sin = (1.0 - cos * cos).sqrt();
            let dtheta = theta - a.theta0;
            pot += 0.5 * a.k_theta * dtheta * dtheta;
            if sin < 1e-8 {
                continue; // linear: the angle force is undefined and vanishing
            }
            let pref = a.k_theta * dtheta / sin;
            // ∂cosθ/∂r_i and ∂cosθ/∂r_k
            let dcos_di = rjk.scale(1.0 / (n1 * n2)).sub(&rji.scale(cos / (n1 * n1)));
            let dcos_dk = rji.scale(1.0 / (n1 * n2)).sub(&rjk.scale(cos / (n2 * n2)));
            let fi = dcos_di.scale(pref);
            let fk = dcos_dk.scale(pref);
            self.force[a.i] = self.force[a.i].add(&fi);
            self.force[a.k] = self.force[a.k].add(&fk);
            self.force[a.j] = self.force[a.j].sub(&fi).sub(&fk);
        }

        self.potential = pot;
    }

    /// Potential energy of the current geometry (kJ/mol), recomputed.
    pub fn potential_energy(&self) -> f64 {
        let mut c = self.clone();
        c.compute_forces();
        c.potential
    }

    /// Largest force magnitude (kJ/mol/nm) — the convergence criterion.
    pub fn max_force(&self) -> f64 {
        self.force.iter().map(|f| f.norm()).fold(0.0, f64::max)
    }

    /// Steepest-descent energy minimisation with an adaptive step size.
    ///
    /// Returns `(steps_taken, final_energy, final_max_force)`. Stops early when
    /// `max_force < f_tol`. The step grows on success and is halved whenever a
    /// move *raises* the energy, which is what keeps stiff bonds from exploding.
    pub fn minimize(&mut self, max_steps: usize, f_tol: f64) -> (usize, f64, f64) {
        self.compute_forces();
        let mut step = 1e-5; // nm per (kJ/mol/nm)
        let mut energy = self.potential;
        for s in 0..max_steps {
            let fmax = self.max_force();
            if fmax < f_tol {
                return (s, energy, fmax);
            }
            let saved_pos = self.pos.clone();
            // Move along the force, capped so no atom jumps more than 0.01 nm.
            let scale = (0.01 / (step * fmax)).min(1.0);
            for i in 0..self.n() {
                self.pos[i] = self.pos[i].add(&self.force[i].scale(step * scale));
            }
            self.compute_forces();
            if self.potential < energy {
                energy = self.potential;
                step *= 1.2;
            } else {
                self.pos = saved_pos;
                self.compute_forces();
                step *= 0.5;
                if step < 1e-12 {
                    return (s, energy, self.max_force());
                }
            }
        }
        (max_steps, self.potential, self.max_force())
    }

    /// Kinetic energy (kJ/mol).
    pub fn kinetic(&self) -> f64 {
        0.5 * self
            .vel
            .iter()
            .zip(&self.types)
            .map(|(v, t)| t.mass * v.norm2())
            .sum::<f64>()
    }

    /// Instantaneous temperature (K), with 3 COM degrees of freedom removed.
    pub fn temperature(&self) -> f64 {
        let dof = (3 * self.n()).saturating_sub(3) as f64;
        if dof <= 0.0 {
            return 0.0;
        }
        2.0 * self.kinetic() / (dof * K_B)
    }

    /// One velocity-Verlet step of `dt` ps. Masses are per-atom here, unlike
    /// the single-species [`crate::Integrator`].
    pub fn md_step(&mut self, dt: f64) {
        let half = 0.5 * dt;
        for i in 0..self.n() {
            let a = self.force[i].scale(1.0 / self.types[i].mass);
            self.vel[i] = self.vel[i].add(&a.scale(half));
            self.pos[i] = self.pos[i].add(&self.vel[i].scale(dt));
        }
        self.compute_forces();
        for i in 0..self.n() {
            let a = self.force[i].scale(1.0 / self.types[i].mass);
            self.vel[i] = self.vel[i].add(&a.scale(half));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn water_distorted() -> Molecule {
        let mut m = Molecule::new();
        let o = AtomType::from_symbol("O").unwrap();
        let h = AtomType::from_symbol("H").unwrap();
        m.add_atom(o, V3::new(0.0, 0.0, 0.0));
        // Deliberately wrong O-H distances (0.13 and 0.07 nm vs 0.096 target).
        m.add_atom(h, V3::new(0.13, 0.0, 0.0));
        m.add_atom(h, V3::new(-0.0176, 0.0679, 0.0));
        m.add_bond(0, 1, 0.096, 462_750.0);
        m.add_bond(0, 2, 0.096, 462_750.0);
        m
    }

    /// The molecular twin of the argon gradient test: analytic force must equal
    /// −dU/dx numerically, now with bonds and mixed species in play.
    #[test]
    fn force_matches_numerical_gradient_with_bonds_and_mixed_species() {
        let mut m = water_distorted();
        m.compute_forces();
        let analytic = m.force[1];
        let h = 1e-7;
        let mut num = V3::ZERO;
        for axis in 0..3 {
            let (mut p, mut q) = (m.clone(), m.clone());
            match axis {
                0 => {
                    p.pos[1].x += h;
                    q.pos[1].x -= h;
                }
                1 => {
                    p.pos[1].y += h;
                    q.pos[1].y -= h;
                }
                _ => {
                    p.pos[1].z += h;
                    q.pos[1].z -= h;
                }
            }
            let du = (p.potential_energy() - q.potential_energy()) / (2.0 * h);
            match axis {
                0 => num.x = -du,
                1 => num.y = -du,
                _ => num.z = -du,
            }
        }
        let err = analytic.sub(&num).norm() / analytic.norm().max(1e-9);
        assert!(err < 1e-4, "rel err {err}: analytic {analytic:?} numeric {num:?}");
    }

    /// Minimisation must drive distorted bonds to their equilibrium length.
    /// The target 0.096 nm is the experimental O-H bond length that q-bio-dsl
    /// already carries in `types.rs` — so the DSL's own reference data grades
    /// the physics.
    #[test]
    fn minimisation_recovers_experimental_oh_bond_length() {
        let mut m = water_distorted();
        let (_, _, fmax) = m.minimize(20_000, 10.0);
        let d1 = m.distance(0, 1);
        let d2 = m.distance(0, 2);
        assert!(fmax < 10.0, "did not converge, fmax = {fmax}");
        assert!((d1 - 0.096).abs() < 0.002, "O-H(1) = {d1:.4} nm, expected 0.096");
        assert!((d2 - 0.096).abs() < 0.002, "O-H(2) = {d2:.4} nm, expected 0.096");
    }

    #[test]
    fn minimisation_lowers_the_energy_monotonically_overall() {
        let mut m = water_distorted();
        let before = m.potential_energy();
        let (_, after, _) = m.minimize(20_000, 10.0);
        assert!(after < before, "energy rose: {before} -> {after}");
    }

    /// A methane-like carbon centre: four C-H bonds relax to 0.109 nm, the
    /// experimental value carried by the DSL.
    #[test]
    fn methane_ch_bonds_relax_to_experiment() {
        let mut m = Molecule::new();
        let c = AtomType::from_symbol("C").unwrap();
        let h = AtomType::from_symbol("H").unwrap();
        m.add_atom(c, V3::new(0.0, 0.0, 0.0));
        let t = 0.075; // start well off the equilibrium 0.109
        for (dx, dy, dz) in [(1.0, 1.0, 1.0), (-1.0, -1.0, 1.0), (-1.0, 1.0, -1.0), (1.0, -1.0, -1.0)] {
            let idx = m.add_atom(h, V3::new(dx * t, dy * t, dz * t));
            m.add_bond(0, idx, 0.109, 284_512.0);
        }
        m.generate_angles(TETRAHEDRAL, DEFAULT_ANGLE_K);
        assert_eq!(m.angles.len(), 6, "methane has six H-C-H angles");
        let (_, _, fmax) = m.minimize(50_000, 10.0);
        assert!(fmax < 10.0, "fmax {fmax}");
        for i in 1..=4 {
            let d = m.distance(0, i);
            assert!((d - 0.109).abs() < 0.003, "C-H #{i} = {d:.4} nm, expected 0.109");
        }
        // And the geometry should be tetrahedral, 109.47 degrees.
        let ang = m.angle(1, 0, 2).to_degrees();
        assert!((ang - 109.47).abs() < 2.0, "H-C-H = {ang:.2} deg, expected 109.47");
    }

    /// The angle force is the easiest term in a force field to get wrong by a
    /// sign or a factor; check it against the numerical gradient too.
    #[test]
    fn angle_force_matches_numerical_gradient() {
        let mut m = Molecule::new();
        let c = AtomType::from_symbol("C").unwrap();
        let h = AtomType::from_symbol("H").unwrap();
        m.add_atom(c, V3::new(0.0, 0.0, 0.0));
        m.add_atom(h, V3::new(0.109, 0.0, 0.0));
        m.add_atom(h, V3::new(0.02, 0.107, 0.013)); // deliberately off-angle
        m.add_bond(0, 1, 0.109, 284_512.0);
        m.add_bond(0, 2, 0.109, 284_512.0);
        m.generate_angles(TETRAHEDRAL, DEFAULT_ANGLE_K);
        m.compute_forces();
        for atom in 0..3 {
            let analytic = m.force[atom];
            let h_step = 1e-8;
            let mut num = V3::ZERO;
            for axis in 0..3 {
                let (mut p, mut q) = (m.clone(), m.clone());
                match axis {
                    0 => {
                        p.pos[atom].x += h_step;
                        q.pos[atom].x -= h_step;
                    }
                    1 => {
                        p.pos[atom].y += h_step;
                        q.pos[atom].y -= h_step;
                    }
                    _ => {
                        p.pos[atom].z += h_step;
                        q.pos[atom].z -= h_step;
                    }
                }
                let du = (p.potential_energy() - q.potential_energy()) / (2.0 * h_step);
                match axis {
                    0 => num.x = -du,
                    1 => num.y = -du,
                    _ => num.z = -du,
                }
            }
            let err = analytic.sub(&num).norm() / analytic.norm().max(1.0);
            assert!(err < 1e-3, "atom {atom}: analytic {analytic:?} numeric {num:?}");
        }
    }

    /// A three-atom molecule must exert no net force on itself, or it will
    /// spontaneously accelerate — the signature of a botched angle term.
    #[test]
    fn angle_term_conserves_total_momentum() {
        let mut m = Molecule::new();
        let o = AtomType::from_symbol("O").unwrap();
        let h = AtomType::from_symbol("H").unwrap();
        m.add_atom(o, V3::new(0.0, 0.0, 0.0));
        m.add_atom(h, V3::new(0.11, 0.0, 0.0));
        m.add_atom(h, V3::new(-0.03, 0.09, 0.01));
        m.add_bond(0, 1, 0.096, 462_750.0);
        m.add_bond(0, 2, 0.096, 462_750.0);
        m.generate_angles(1.824, 383.0); // 104.5 deg, water
        m.compute_forces();
        let net = m.force.iter().fold(V3::ZERO, |a, f| a.add(f));
        assert!(net.norm() < 1e-9, "net force {net:?} is not zero");
    }

    /// Water's experimental H-O-H angle is 104.5 degrees.
    #[test]
    fn water_angle_relaxes_to_experiment() {
        let mut m = water_distorted();
        m.generate_angles(104.5_f64.to_radians(), 383.0);
        m.minimize(50_000, 10.0);
        let ang = m.angle(1, 0, 2).to_degrees();
        assert!((ang - 104.5).abs() < 1.5, "H-O-H = {ang:.2} deg, expected 104.5");
    }

    #[test]
    fn lorentz_berthelot_mixing_is_symmetric() {
        let mut a = Molecule::new();
        a.add_atom(AtomType::from_symbol("C").unwrap(), V3::new(0.0, 0.0, 0.0));
        a.add_atom(AtomType::from_symbol("O").unwrap(), V3::new(0.4, 0.0, 0.0));
        let mut b = Molecule::new();
        b.add_atom(AtomType::from_symbol("O").unwrap(), V3::new(0.0, 0.0, 0.0));
        b.add_atom(AtomType::from_symbol("C").unwrap(), V3::new(0.4, 0.0, 0.0));
        assert!((a.potential_energy() - b.potential_energy()).abs() < 1e-12);
    }

    #[test]
    fn bonded_pairs_are_excluded_from_lennard_jones() {
        // Two atoms far inside each other's LJ core: with a bond (and thus
        // exclusion) the energy is purely harmonic and finite.
        let mut m = Molecule::new();
        let h = AtomType::from_symbol("H").unwrap();
        m.add_atom(h, V3::new(0.0, 0.0, 0.0));
        m.add_atom(h, V3::new(0.074, 0.0, 0.0));
        m.add_bond(0, 1, 0.074, 300_000.0);
        let e = m.potential_energy();
        assert!(e.abs() < 1e-6, "bonded pair should sit at zero energy, got {e}");
    }

    #[test]
    fn unknown_elements_are_refused_rather_than_defaulted() {
        assert!(AtomType::from_symbol("Uup").is_none());
        assert!(AtomType::from_symbol("C").is_some());
    }

    #[test]
    fn md_conserves_energy_for_a_molecule() {
        let mut m = water_distorted();
        m.minimize(5_000, 10.0);
        m.compute_forces();
        // Give it a small kick and integrate; stiff bonds need a small dt.
        m.vel[1] = V3::new(0.1, 0.0, 0.0);
        let dt = 0.0002; // 0.2 fs
        m.compute_forces();
        let e0 = m.kinetic() + m.potential;
        for _ in 0..2_000 {
            m.md_step(dt);
        }
        let e1 = m.kinetic() + m.potential;
        let drift = (e1 - e0).abs() / e0.abs().max(1e-9);
        assert!(drift < 1e-3, "molecular NVE drift {drift:.3e}");
    }
}
