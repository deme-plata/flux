//! body.rs — a simulated character BODY: muscle groups, fatigue, stamina, reflexes, balance.
//!
//! The hero isn't a stat block — it's a body. Muscle groups produce force that **fatigues** with use
//! and **recovers** with rest; a stamina pool gates exertion; a **reflex** model means you can only
//! dodge a telegraphed strike if your reaction time beats its wind-up (and fatigue slows your
//! reactions); **balance** decays under hits and causes **stagger**. Strike power, dodge success and
//! stagger all fall out of the sim rather than a fixed number — and it ticks in real time, so it
//! drops straight into the live combat loop (rt.rs). Derived from the hero's Origin + Traits.
//!
//! Pure, std-only, deterministic — fully unit-testable.

use crate::traits::{CharSheet, Origin, Trait};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuscleGroup { Legs, Core, Arms, Grip }

/// One muscle group. `fatigue` 0.0 (fresh) → 1.0 (spent); effective force drops with fatigue.
#[derive(Debug, Clone, Copy)]
pub struct Muscle { pub group: MuscleGroup, pub max_force: f32, pub fatigue: f32 }
impl Muscle {
    pub fn new(group: MuscleGroup, max_force: f32) -> Self { Self { group, max_force, fatigue: 0.0 } }
    /// force available right now at a given activation (0..1) — fatigue saps up to 70%.
    pub fn force(&self, activation: f32) -> f32 { self.max_force * (1.0 - 0.7 * self.fatigue) * activation.clamp(0.0, 1.0) }
    /// contract at `intensity` → accrues fatigue.
    fn contract(&mut self, intensity: f32) { self.fatigue = (self.fatigue + 0.09 * intensity.clamp(0.0, 1.5)).min(1.0); }
    /// recover over `dt_ms` (slow, exponential-ish).
    fn recover(&mut self, dt_ms: u32) { self.fatigue = (self.fatigue - (dt_ms as f32 / 1000.0) * 0.10).max(0.0); }
}

/// An incoming telegraphed attack: it lands in `telegraph_ms`; `severity` 0..1 scales the cost/impact.
#[derive(Debug, Clone, Copy)]
pub struct Stimulus { pub telegraph_ms: u32, pub severity: f32 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dodge { Clean, Partial, Hit }

/// The full body. `base_reflex_ms` is the floor reaction time when fresh.
#[derive(Debug, Clone)]
pub struct Body {
    pub legs: Muscle, pub core: Muscle, pub arms: Muscle, pub grip: Muscle,
    pub stamina: f32, pub max_stamina: f32,
    pub balance: f32,          // 1.0 = steady, ≤0.3 = staggered
    pub base_reflex_ms: u32,
}
impl Body {
    /// A default trained body.
    pub fn new() -> Self {
        Body {
            legs: Muscle::new(MuscleGroup::Legs, 1.0),
            core: Muscle::new(MuscleGroup::Core, 0.9),
            arms: Muscle::new(MuscleGroup::Arms, 0.85),
            grip: Muscle::new(MuscleGroup::Grip, 0.55),
            stamina: 100.0, max_stamina: 100.0, balance: 1.0, base_reflex_ms: 240,
        }
    }

    /// Build the body from a hero sheet — Origin sets the physique, Traits tune reflex/stamina.
    pub fn from_sheet(s: &CharSheet) -> Body {
        let mut b = Body::new();
        match s.origin {
            Origin::Ashborn        => { b.arms.max_force += 0.25; b.core.max_force += 0.20; b.base_reflex_ms += 30; } // strong, a touch slower
            Origin::CompilerMonk   => { b.base_reflex_ms = 190; b.max_stamina += 10.0; }                              // sharp reflexes
            Origin::ExiledValidator=> { b.max_stamina += 25.0; b.legs.max_force += 0.15; }                            // endurance, footwork
            Origin::VoidpactBroker => { b.base_reflex_ms -= 10; b.grip.max_force += 0.2; }
        }
        for t in &s.traits {
            match t {
                Trait::ZeroCopy => b.base_reflex_ms = b.base_reflex_ms.saturating_sub(40), // moves without cost → faster
                Trait::AshLunged => b.max_stamina += 20.0,
                Trait::QuorumKin => { b.core.max_force += 0.15; }
                Trait::Forkbound => b.base_reflex_ms = b.base_reflex_ms.saturating_sub(15),
                _ => {}
            }
        }
        b.stamina = b.max_stamina;
        b
    }

    fn muscle_mut(&mut self, g: MuscleGroup) -> &mut Muscle {
        match g { MuscleGroup::Legs=>&mut self.legs, MuscleGroup::Core=>&mut self.core, MuscleGroup::Arms=>&mut self.arms, MuscleGroup::Grip=>&mut self.grip }
    }

    /// Overall fatigue (mean across groups) — drives reaction-time penalty.
    pub fn fatigue(&self) -> f32 { (self.legs.fatigue + self.core.fatigue + self.arms.fatigue + self.grip.fatigue) / 4.0 }

    /// Current reaction time (ms): floor + fatigue penalty (up to +180ms) + low-stamina penalty.
    pub fn reaction_ms(&self) -> u32 {
        let stam_pen = if self.stamina < self.max_stamina * 0.3 { 60.0 } else { 0.0 };
        self.base_reflex_ms + (self.fatigue() * 180.0) as u32 + stam_pen as u32
    }

    /// Exert a group at `intensity`, spending stamina + fatiguing it. Returns the force produced.
    pub fn exert(&mut self, g: MuscleGroup, intensity: f32) -> f32 {
        let cost = 8.0 * intensity.clamp(0.0, 1.5);
        let scale = (self.stamina / self.max_stamina).clamp(0.25, 1.0); // gassed = weaker
        let force = self.muscle_mut(g).force(intensity) * scale;
        self.stamina = (self.stamina - cost).max(0.0);
        self.muscle_mut(g).contract(intensity);
        force
    }

    /// A melee strike: arms + core drive it. Force feeds the rt damage. Fatigues + costs stamina.
    pub fn strike(&mut self) -> f32 {
        let a = self.exert(MuscleGroup::Arms, 1.0);
        let c = self.exert(MuscleGroup::Core, 0.7);
        let g = self.exert(MuscleGroup::Grip, 0.5);
        a * 0.6 + c * 0.3 + g * 0.1
    }

    /// Attempt a reflexive dodge of a telegraphed strike. Clean if you react in time AND have the
    /// leg stamina; Partial if marginal; Hit if you're too slow/gassed. Costs stamina + a little balance.
    pub fn dodge(&mut self, s: &Stimulus) -> Dodge {
        let rt = self.reaction_ms();
        // legs power the roll
        let _ = self.exert(MuscleGroup::Legs, 0.8);
        let outcome = if rt <= s.telegraph_ms && self.stamina > 5.0 {
            Dodge::Clean
        } else if (rt as f32) <= s.telegraph_ms as f32 * 1.3 {
            Dodge::Partial
        } else {
            Dodge::Hit
        };
        // a Hit rocks your balance; a clean dodge barely touches it
        let knock = match outcome { Dodge::Hit => 0.45 * s.severity, Dodge::Partial => 0.2 * s.severity, Dodge::Clean => 0.05 };
        self.balance = (self.balance - knock).max(0.0);
        outcome
    }

    /// Balance ≤ 0.3 → staggered (can't act cleanly).
    pub fn staggered(&self) -> bool { self.balance <= 0.3 }

    /// Advance the sim by `dt_ms`: recover muscles + stamina + balance.
    pub fn tick(&mut self, dt_ms: u32) {
        for g in [MuscleGroup::Legs, MuscleGroup::Core, MuscleGroup::Arms, MuscleGroup::Grip] { self.muscle_mut(g).recover(dt_ms); }
        self.stamina = (self.stamina + (dt_ms as f32 / 1000.0) * 22.0).min(self.max_stamina);
        self.balance = (self.balance + (dt_ms as f32 / 1000.0) * 0.5).min(1.0);
    }
}
impl Default for Body { fn default() -> Self { Body::new() } }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::CharSheet;

    #[test]
    fn fatigue_saps_strike_force() {
        let mut b = Body::new();
        let fresh = b.strike();
        for _ in 0..12 { b.strike(); } // hammer away
        let tired = b.strike();
        assert!(tired < fresh * 0.85, "a gassed, fatigued body hits softer ({tired} vs {fresh})");
        assert!(b.fatigue() > 0.2 && b.stamina < b.max_stamina);
    }

    #[test]
    fn reactions_slow_with_fatigue() {
        let mut b = Body::new();
        let fresh_rt = b.reaction_ms();
        for _ in 0..14 { b.strike(); b.dodge(&Stimulus { telegraph_ms: 0, severity: 0.0 }); }
        assert!(b.reaction_ms() > fresh_rt, "tired → slower reactions ({} > {fresh_rt})", b.reaction_ms());
    }

    #[test]
    fn you_dodge_what_you_can_react_to() {
        let mut b = Body::new();
        // a slow, well-telegraphed swing (500ms) vs a fresh ~240ms reaction → clean
        assert_eq!(b.dodge(&Stimulus { telegraph_ms: 500, severity: 0.6 }), Dodge::Clean);
        // a fast jab (120ms) you can't react to → Hit
        let mut b2 = Body::new();
        assert_eq!(b2.dodge(&Stimulus { telegraph_ms: 120, severity: 0.6 }), Dodge::Hit);
    }

    #[test]
    fn hits_break_balance_into_stagger() {
        let mut b = Body::new();
        for _ in 0..4 { b.dodge(&Stimulus { telegraph_ms: 80, severity: 1.0 }); } // repeatedly clobbered
        assert!(b.staggered(), "repeated heavy hits stagger you (balance {})", b.balance);
    }

    #[test]
    fn rest_recovers_the_body() {
        let mut b = Body::new();
        for _ in 0..10 { b.strike(); }
        let (f0, s0) = (b.fatigue(), b.stamina);
        b.tick(3000); // rest 3s
        assert!(b.fatigue() < f0 && b.stamina > s0, "rest recovers fatigue + stamina");
    }

    #[test]
    fn origin_and_traits_shape_the_body() {
        // a Compiler-Monk (sharp reflexes) reacts faster than an Ashborn (strong but slower)
        let monk = Body::from_sheet(&CharSheet::from_seed("m", 0)); // origin from seed; just check the mapping runs
        assert!(monk.base_reflex_ms > 0 && monk.max_stamina > 0.0);
        // explicit: build the two origins directly
        let mut s = CharSheet::create("anyone");
        s.origin = Origin::CompilerMonk; s.traits = vec![Trait::ZeroCopy];
        let fast = Body::from_sheet(&s);
        s.origin = Origin::Ashborn; s.traits = vec![];
        let strong = Body::from_sheet(&s);
        assert!(fast.base_reflex_ms < strong.base_reflex_ms, "monk+zerocopy reacts faster than ashborn");
        assert!(strong.arms.max_force > fast.arms.max_force, "ashborn hits harder");
    }
}
