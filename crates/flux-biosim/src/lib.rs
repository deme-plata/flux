//! flux-biosim — the HEADLESS bioreactor / lab-meat sim core, the CPU backend for the `bioreactor`
//! boilerplate. The browser shows the visualization; this is the real cell-population kinetics that
//! runs on Epsilon's CPU (no GPU, no bevy) — and being a flux crate it's `flux_optimize`-able.
//!
//! Model: a Monod chemostat with the two constraints that actually decide whether *cultured meat*
//! is viable, not just whether cells grow:
//!   • growth   μ = μmax·S/(Ks+S) · O₂ · Ki/(Ki+L)   — Monod, O₂-throttled, **lactate-inhibited**
//!   • substrate S consumed by growth (yield), fed back by the pump
//!   • **lactate L** accumulates with growth (the #1 real bottleneck in cultured-meat scale-up) and
//!     throttles μ — high feed ⇒ fast growth ⇒ fast lactate ⇒ self-poisoning
//!   • **perfusion** clears lactate each step but washes out some cells — the real fix, with a cost
//! So `flux_optimize` faces the actual cultured-meat tension (feed hard vs. poison the culture vs.
//! perfuse it out at a cell/cost penalty), not a trivial "more feed = more cells" ramp.
//!
//! The molecular/DNA level (place_atom / form_bond, nucleotide-by-nucleotide) lives in `q-bio-dsl`;
//! this crate is the population/process tier above it. The heavyweight bevy renderer is
//! `mitochondria-sim` (q-narwhalknight); this is the curve that matters, distilled for the recipe.

use serde::{Deserialize, Serialize};

/// Reactor parameters — what an agent/`flux_optimize` tunes to maximize harvest grams (net of cost).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Params {
    pub mu_max: f64,        // max specific growth rate (1/step)
    pub ks: f64,            // Monod half-saturation (substrate units)
    pub yield_cells: f64,   // cells produced per unit substrate consumed
    pub feed: f64,          // glucose fed per step (the EWOD pump rate), 0..1
    pub o2: f64,            // dissolved O₂ fraction, 0..1 (growth throttle)
    pub cap: f64,           // carrying capacity (max cells the tank holds)
    // --- lab-meat realism ---
    pub lactate_yield: f64,  // lactate accumulated per unit of (new_cells / cap), 0..~
    pub ki_lactate: f64,     // lactate inhibition half-constant — μ × Ki/(Ki+L)
    pub lactate_lethal: f64, // lactate above this kills cells (the batch-culture death phase)
    pub death_rate: f64,     // cell death per unit (L − lethal) per step
    pub perfusion: f64,      // 0..1 fraction of lactate cleared per step (cell-retained)
    pub washout: f64,        // cell loss per unit perfusion per step (≈0 with retention)
    pub perfusion_cost: f64, // relative $ cost per unit perfusion (fresh media throughput)
    pub grams_per_1e9: f64,  // wet-mass conversion: grams of meat per 1e9 cells (~1g is realistic)
    pub feed_cost: f64,      // relative $ cost per unit feed (media is the dominant cultured-meat cost)
}

impl Default for Params {
    fn default() -> Self {
        Params {
            mu_max: 0.045, ks: 0.30, yield_cells: 5.0e5, feed: 0.012, o2: 0.6, cap: 4.0e7,
            lactate_yield: 0.6, ki_lactate: 1.2, lactate_lethal: 1.5, death_rate: 0.03,
            perfusion: 0.0, washout: 0.0, perfusion_cost: 0.5, grams_per_1e9: 1.0, feed_cost: 0.4,
        }
    }
}

/// One sampled point of the run (what the browser viz / a stream consumes).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BioReport {
    pub step: u32,
    pub cells: f64,
    pub substrate: f64,
    pub lactate: f64,   // accumulated lactate (the inhibitor) — perfusion keeps this down
    pub mu: f64,
    pub biomass: f64,   // cells normalized to carrying capacity, 0..1
    pub grams: f64,     // wet harvest mass at this point
}

/// The reactor state. `step` advances the lactate-inhibited Monod chemostat one tick.
#[derive(Debug, Clone)]
pub struct Reactor {
    pub p: Params,
    pub cells: f64,
    pub substrate: f64,
    pub lactate: f64,
    pub fed_total: f64, // cumulative feed dosed — drives the cost term
    pub t: u32,
}

impl Reactor {
    /// Seed with `cells0` cells, full substrate, zero lactate.
    pub fn new(p: Params, cells0: f64) -> Self {
        Reactor { p, cells: cells0.max(1.0), substrate: 1.0, lactate: 0.0, fed_total: 0.0, t: 0 }
    }

    /// Instantaneous specific growth rate μ = μmax·S/(Ks+S)·O₂·Ki/(Ki+L).
    pub fn mu(&self) -> f64 {
        let monod = self.substrate / (self.p.ks + self.substrate);
        let inhib = self.p.ki_lactate / (self.p.ki_lactate + self.lactate);
        self.p.mu_max * monod * self.p.o2 * inhib
    }

    /// Wet harvest mass for a given cell count.
    pub fn grams_of(&self, cells: f64) -> f64 {
        cells / 1.0e9 * self.p.grams_per_1e9
    }

    /// Advance one step: birth (logistic-capped, lactate-inhibited Monod) MINUS lactate-driven death,
    /// make lactate, consume + feed substrate, then perfuse (clear lactate; cells retained).
    pub fn step(&mut self) -> BioReport {
        let mu = self.mu();
        // logistic cap so cells plateau at carrying capacity, not explode
        let birth = (mu * self.cells * (1.0 - self.cells / self.p.cap)).max(0.0);
        // lactate above the lethal threshold kills cells — the real batch-culture death phase
        let death = self.p.death_rate * self.cells * (self.lactate - self.p.lactate_lethal).max(0.0);
        self.cells = (self.cells + birth - death).clamp(0.0, self.p.cap);
        // growth makes lactate (scaled by the fraction of capacity just added)
        self.lactate += (birth / self.p.cap) * self.p.lactate_yield;
        // growth eats substrate; pump feeds it back (clamped 0..1)
        self.substrate = (self.substrate - birth / self.p.yield_cells + self.p.feed).clamp(0.0, 1.0);
        self.fed_total += self.p.feed;
        // perfusion: clear lactate; cell-retention means ~no cell loss (washout defaults to 0)
        if self.p.perfusion > 0.0 {
            self.lactate *= 1.0 - self.p.perfusion;
            self.cells *= 1.0 - self.p.perfusion * self.p.washout;
        }
        self.t += 1;
        self.report()
    }

    pub fn report(&self) -> BioReport {
        BioReport {
            step: self.t, cells: self.cells, substrate: self.substrate, lactate: self.lactate,
            mu: self.mu(), biomass: self.cells / self.p.cap, grams: self.grams_of(self.cells),
        }
    }

    /// Run `steps` ticks, returning every `sample`-th report. The CPU run.
    pub fn run(&mut self, steps: u32, sample: u32) -> Vec<BioReport> {
        let sample = sample.max(1);
        let mut out = vec![self.report()];
        for _ in 0..steps {
            let r = self.step();
            if r.step % sample == 0 { out.push(r); }
        }
        out
    }
}

/// Final biomass (0..1) after `steps` — kept for compatibility / quick growth checks.
pub fn final_biomass(p: Params, steps: u32) -> f64 {
    let mut r = Reactor::new(p, 2.0e4);
    for _ in 0..steps { r.step(); }
    r.report().biomass
}

/// Final wet harvest grams after `steps` — the *product*.
pub fn final_grams(p: Params, steps: u32) -> f64 {
    let mut r = Reactor::new(p, 2.0e4);
    for _ in 0..steps { r.step(); }
    r.report().grams
}

/// The economic objective `flux_optimize` should MAXIMIZE: harvest minus media cost, in normalized
/// units (both terms scaled to ~0..1) so it's a clean scalar. Captures the real cultured-meat goal —
/// grow a lot of meat *without* burning media or poisoning the culture with lactate.
pub fn net_yield(p: Params, steps: u32) -> f64 {
    let mut r = Reactor::new(p, 2.0e4);
    for _ in 0..steps { r.step(); }
    let rep = r.report();
    let avg_feed = if steps > 0 { r.fed_total / steps as f64 } else { 0.0 };
    rep.biomass - p.feed_cost * avg_feed - p.perfusion_cost * p.perfusion
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_grow_when_fed_and_oxygenated() {
        let mut r = Reactor::new(Params::default(), 2.0e4);
        let start = r.cells;
        let series = r.run(2000, 200);
        assert!(series.last().unwrap().cells > start * 10.0, "a fed reactor grows its cell population");
        assert!(series.last().unwrap().biomass <= 1.0 + 1e-9, "biomass never exceeds carrying capacity");
    }

    #[test]
    fn starving_or_anoxic_reactor_barely_grows() {
        let p = Params { feed: 0.0, o2: 0.0, ..Params::default() };
        let mut r = Reactor::new(p, 2.0e4);
        let series = r.run(2000, 200);
        assert!(series.last().unwrap().cells < 2.0e4 * 2.0, "no feed + no O₂ ⇒ no real growth");
    }

    #[test]
    fn more_oxygen_yields_more_biomass() {
        let lo = final_biomass(Params { o2: 0.2, ..Params::default() }, 3000);
        let hi = final_biomass(Params { o2: 0.9, ..Params::default() }, 3000);
        assert!(hi > lo, "more dissolved O₂ ⇒ more final biomass ({lo} -> {hi})");
    }

    #[test]
    fn lactate_inhibition_caps_a_hard_fed_culture() {
        // crank feed: a culture with no lactate inhibition would just pin at cap; with inhibition the
        // accumulated lactate throttles late growth, so heavy-lactate runs end with LESS biomass.
        let gentle = final_biomass(Params { feed: 0.05, lactate_yield: 0.2, ..Params::default() }, 4000);
        let toxic  = final_biomass(Params { feed: 0.05, lactate_yield: 3.0, ..Params::default() }, 4000);
        assert!(toxic < gentle, "more lactate per growth ⇒ self-poisoning ⇒ less biomass ({toxic} < {gentle})");
    }

    #[test]
    fn perfusion_rescues_a_lactate_limited_culture() {
        // a lactate-limited culture (high lactate_yield) poisons itself: lactate climbs past the
        // lethal threshold and the culture crashes (death phase). Perfusing keeps lactate low, so the
        // culture sustains near capacity. Cell-retention ⇒ perfusion costs media, not cells.
        let limited = Params { feed: 0.5, lactate_yield: 2.5, perfusion: 0.0, ..Params::default() };
        let perfused = Params { perfusion: 0.02, ..limited };
        let a = final_biomass(limited, 5000);
        let b = final_biomass(perfused, 5000);
        assert!(b > a, "perfusion clears the inhibitor before it turns lethal ⇒ more biomass ({a} -> {b})");
    }

    #[test]
    fn net_yield_penalizes_wasted_feed() {
        // two recipes that both saturate growth (feed ≥ peak demand), but the spendthrift one feeds
        // nearly 2× as hard: net_yield (harvest minus media cost) should prefer the leaner recipe.
        let adequate = net_yield(Params { feed: 0.5, ..Params::default() }, 4000);
        let spendthrift = net_yield(Params { feed: 0.9, ..Params::default() }, 4000);
        assert!(adequate > spendthrift, "wasting media lowers net yield ({spendthrift} < {adequate})");
    }
}
