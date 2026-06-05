//! The garage: the car, its parts, wear, and the operations a race engineer
//! performs — fix the tyres, perfect every component, shave weight, and tune
//! each part toward (but never past) its regulated ceiling.

use crate::regs::{Regulations, Violation};
use serde::{Deserialize, Serialize};

/// Every tunable / wearable part of the car. Weights below sum to 1.0 so a
/// fully-perfected, fully-fresh car scores a clean 1.0 performance index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PartKind {
    Tyres,
    Ice,    // internal combustion engine
    Mguk,   // electrical deployment
    Battery,
    FrontWing,
    RearWing,
    Floor,
    Brakes,
    Gearbox,
    Suspension,
}

impl PartKind {
    pub const ALL: [PartKind; 10] = [
        PartKind::Tyres,
        PartKind::Ice,
        PartKind::Mguk,
        PartKind::Battery,
        PartKind::FrontWing,
        PartKind::RearWing,
        PartKind::Floor,
        PartKind::Brakes,
        PartKind::Gearbox,
        PartKind::Suspension,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            PartKind::Tyres => "Tyres",
            PartKind::Ice => "ICE (engine)",
            PartKind::Mguk => "MGU-K",
            PartKind::Battery => "Battery",
            PartKind::FrontWing => "Front Wing",
            PartKind::RearWing => "Rear Wing",
            PartKind::Floor => "Floor",
            PartKind::Brakes => "Brakes",
            PartKind::Gearbox => "Gearbox",
            PartKind::Suspension => "Suspension",
        }
    }

    /// How much this part contributes to overall pace. Sums to 1.0 across ALL.
    pub fn perf_weight(&self) -> f64 {
        match self {
            PartKind::Tyres => 0.22,
            PartKind::Ice => 0.18,
            PartKind::Mguk => 0.15,
            PartKind::Battery => 0.08,
            PartKind::FrontWing => 0.09,
            PartKind::RearWing => 0.09,
            PartKind::Floor => 0.08,
            PartKind::Brakes => 0.05,
            PartKind::Gearbox => 0.03,
            PartKind::Suspension => 0.03,
        }
    }

    /// The legal ceiling for this part's engineering `value`, and its unit.
    /// Reaching this exactly = "perfect"; exceeding it = disqualified.
    pub fn legal_max(&self, regs: &Regulations) -> (f64, &'static str) {
        match self {
            PartKind::Mguk => (regs.max_mguk_kw, "kW"),
            PartKind::Ice => (regs.max_ice_kw, "kW"),
            PartKind::Battery => (regs.max_battery_mj, "MJ"),
            // Aero / mechanical parts are scored on a normalised 0–100 setup index.
            _ => (100.0, "idx"),
        }
    }
}

/// One physical part on the car.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub kind: PartKind,
    /// 0–100. Wear/freshness. 100 = box-fresh, 0 = scrap.
    pub condition: f64,
    /// The engineering setting (kW / MJ / setup index). Higher = faster.
    /// Legal up to `kind.legal_max`; beyond that the car fails scrutineering.
    pub value: f64,
}

impl Part {
    /// A fresh-from-the-truck baseline: full condition, conservative legal value.
    fn baseline(kind: PartKind, regs: &Regulations) -> Self {
        let (max, _unit) = kind.legal_max(regs);
        // Start at ~70% of the legal ceiling — there's real pace to unlock.
        Part { kind, condition: 100.0, value: max * 0.70 }
    }

    /// Per-part pace contribution in 0..=~1 (slightly above 1 if illegally over-tuned).
    fn pace(&self, regs: &Regulations) -> f64 {
        let (max, _) = self.kind.legal_max(regs);
        let tune = if max > 0.0 { self.value / max } else { 0.0 };
        (self.condition / 100.0) * tune
    }
}

/// Tyre compounds for 2026. Softer = faster but wears quicker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Compound {
    Soft,
    Medium,
    Hard,
    Intermediate,
    Wet,
}

impl Compound {
    pub fn label(&self) -> &'static str {
        match self {
            Compound::Soft => "Soft (C5)",
            Compound::Medium => "Medium (C3)",
            Compound::Hard => "Hard (C1)",
            Compound::Intermediate => "Intermediate",
            Compound::Wet => "Wet",
        }
    }
    /// Dry-pace multiplier applied to tyre value when fresh.
    pub fn grip(&self) -> f64 {
        match self {
            Compound::Soft => 1.00,
            Compound::Medium => 0.96,
            Compound::Hard => 0.92,
            Compound::Intermediate => 0.80,
            Compound::Wet => 0.70,
        }
    }
}

/// The whole car sitting in the garage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Car {
    pub parts: Vec<Part>,
    pub compound: Compound,
    /// Total weight including driver, kg. Below the regulated minimum = illegal.
    pub weight_kg: f64,
    pub sustainable_fuel: bool,
    pub has_mgu_h: bool,
    pub aero_modes: u8,
}

impl Car {
    /// A legal, slightly-underdeveloped 2026 car ready to be perfected.
    pub fn new_2026(regs: &Regulations) -> Self {
        Car {
            parts: PartKind::ALL.iter().map(|k| Part::baseline(*k, regs)).collect(),
            compound: Compound::Medium,
            weight_kg: 798.0, // heavy and safe; legal but slow
            sustainable_fuel: true,
            has_mgu_h: false,
            aero_modes: 2,
        }
    }

    fn part_mut(&mut self, kind: PartKind) -> &mut Part {
        self.parts.iter_mut().find(|p| p.kind == kind).expect("all parts present")
    }

    pub fn part(&self, kind: PartKind) -> &Part {
        self.parts.iter().find(|p| p.kind == kind).expect("all parts present")
    }

    /// Fit a fresh set of the given compound (condition back to 100).
    pub fn fix_tyres(&mut self, compound: Compound) {
        self.compound = compound;
        let p = self.part_mut(PartKind::Tyres);
        p.condition = 100.0;
        // tyre "value" tracks compound grip on the 0–100 index.
        p.value = 100.0 * compound.grip();
    }

    /// Perfect one part: full condition + tuned exactly to the legal ceiling.
    /// This is the fast-AND-legal sweet spot.
    pub fn perfect_part(&mut self, kind: PartKind, regs: &Regulations) {
        let (max, _) = kind.legal_max(regs);
        let p = self.part_mut(kind);
        p.condition = 100.0;
        p.value = max;
        if kind == PartKind::Tyres {
            // perfecting tyres = fresh softs
            self.compound = Compound::Soft;
        }
    }

    /// Bring the entire car to a flawless, fully-legal 2026 spec.
    pub fn perfect_all(&mut self, regs: &Regulations) {
        for k in PartKind::ALL {
            self.perfect_part(k, regs);
        }
        self.weight_kg = regs.min_weight_kg; // shave to the legal floor
        self.has_mgu_h = false;
        self.sustainable_fuel = true;
        self.aero_modes = regs.max_aero_modes;
        self.compound = Compound::Soft;
    }

    /// Risky move: over-tune a part past its legal ceiling for raw speed.
    /// Returns the new value. The car will fail scrutineering until reverted.
    pub fn overtune(&mut self, kind: PartKind, percent_over: f64, regs: &Regulations) -> f64 {
        let (max, _) = kind.legal_max(regs);
        let p = self.part_mut(kind);
        p.condition = 100.0;
        p.value = max * (1.0 + percent_over.max(0.0) / 100.0);
        p.value
    }

    /// Simulate tyre + component wear over a number of racing laps.
    pub fn apply_wear(&mut self, laps: u32) {
        let laps = laps as f64;
        let tyre_rate = match self.compound {
            Compound::Soft => 1.7,
            Compound::Medium => 1.1,
            Compound::Hard => 0.7,
            Compound::Intermediate => 1.3,
            Compound::Wet => 0.9,
        };
        for p in &mut self.parts {
            let rate = if p.kind == PartKind::Tyres { tyre_rate } else { 0.10 };
            p.condition = (p.condition - rate * laps).max(0.0);
        }
    }

    /// Overall pace index in 0..~1 (perfect legal car = 1.0; over-tuned > 1.0).
    pub fn performance(&self, regs: &Regulations) -> f64 {
        let mut score: f64 = 0.0;
        for p in &self.parts {
            score += p.kind.perf_weight() * p.pace(regs);
        }
        // Weight bonus: every kg below the *starting* heavy spec (798) helps,
        // capped so that hitting the 768 floor gives the full legal bonus.
        let weight_bonus = ((798.0 - self.weight_kg) / (798.0 - regs.min_weight_kg))
            .clamp(-0.2, 0.15)
            * 0.06;
        (score + weight_bonus).max(0.0)
    }

    /// Run scrutineering. Empty vec = the car is race-legal.
    pub fn scrutineer(&self, regs: &Regulations) -> Vec<Violation> {
        let mut v = Vec::new();
        if self.weight_kg < regs.min_weight_kg - 1e-6 {
            v.push(Violation {
                rule: "Minimum weight".into(),
                limit: format!("{:.0} kg", regs.min_weight_kg),
                actual: format!("{:.1} kg", self.weight_kg),
            });
        }
        for p in &self.parts {
            let (max, unit) = p.kind.legal_max(regs);
            if p.value > max + 1e-6 {
                v.push(Violation {
                    rule: format!("{} over limit", p.kind.label()),
                    limit: format!("{:.1} {}", max, unit),
                    actual: format!("{:.1} {}", p.value, unit),
                });
            }
        }
        if self.has_mgu_h && regs.mgu_h_banned {
            v.push(Violation {
                rule: "MGU-H fitted".into(),
                limit: "banned in 2026".into(),
                actual: "present".into(),
            });
        }
        if self.aero_modes > regs.max_aero_modes {
            v.push(Violation {
                rule: "Active-aero modes".into(),
                limit: format!("{}", regs.max_aero_modes),
                actual: format!("{}", self.aero_modes),
            });
        }
        if regs.sustainable_fuel_required && !self.sustainable_fuel {
            v.push(Violation {
                rule: "Sustainable fuel".into(),
                limit: "100% required".into(),
                actual: "non-compliant".into(),
            });
        }
        v
    }

    pub fn is_legal(&self, regs: &Regulations) -> bool {
        self.scrutineer(regs).is_empty()
    }

    /// 0–100 readiness: how close the car is to a perfect, legal build.
    pub fn readiness(&self, regs: &Regulations) -> f64 {
        let perf = self.performance(regs).clamp(0.0, 1.0);
        let legal = if self.is_legal(regs) { 1.0 } else { 0.6 };
        (perf * legal * 100.0).clamp(0.0, 100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regs() -> Regulations {
        Regulations::y2026()
    }

    #[test]
    fn fresh_car_is_legal_but_not_perfect() {
        let car = Car::new_2026(&regs());
        assert!(car.is_legal(&regs()), "baseline car must pass scrutineering");
        assert!(car.performance(&regs()) < 0.95, "baseline should leave pace on the table");
    }

    #[test]
    fn perfecting_everything_is_fast_and_legal() {
        let r = regs();
        let mut car = Car::new_2026(&r);
        let before = car.performance(&r);
        car.perfect_all(&r);
        let after = car.performance(&r);
        assert!(after > before, "perfecting must improve pace");
        assert!(after >= 1.0, "perfect legal car scores >= 1.0, got {after}");
        assert!(car.is_legal(&r), "a perfected car is still legal");
        assert_eq!(car.weight_kg, r.min_weight_kg);
    }

    #[test]
    fn overtuning_mguk_is_faster_but_illegal() {
        let r = regs();
        let mut car = Car::new_2026(&r);
        car.perfect_all(&r);
        let legal_perf = car.performance(&r);
        let v = car.overtune(PartKind::Mguk, 20.0, &r);
        assert!(v > r.max_mguk_kw, "overtune must exceed the 350kW cap");
        assert!(car.performance(&r) > legal_perf, "cheating is faster…");
        assert!(!car.is_legal(&r), "…but illegal");
        let viol = car.scrutineer(&r);
        assert!(viol.iter().any(|x| x.rule.contains("MGU-K")));
    }

    #[test]
    fn fix_tyres_restores_condition() {
        let r = regs();
        let mut car = Car::new_2026(&r);
        car.apply_wear(45);
        assert!(car.part(PartKind::Tyres).condition < 60.0,
            "45 laps on mediums should drop tyres below 60%, got {}",
            car.part(PartKind::Tyres).condition);
        car.fix_tyres(Compound::Soft);
        assert_eq!(car.part(PartKind::Tyres).condition, 100.0);
    }

    #[test]
    fn mguk_respects_350kw_ceiling_when_perfected() {
        let r = regs();
        let mut car = Car::new_2026(&r);
        car.perfect_part(PartKind::Mguk, &r);
        assert_eq!(car.part(PartKind::Mguk).value, 350.0);
        assert!(car.is_legal(&r));
    }

    #[test]
    fn underweight_car_fails() {
        let r = regs();
        let mut car = Car::new_2026(&r);
        car.weight_kg = 760.0;
        assert!(!car.is_legal(&r));
        assert!(car.scrutineer(&r).iter().any(|v| v.rule.contains("weight")));
    }
}
