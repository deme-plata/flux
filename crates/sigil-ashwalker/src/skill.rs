//! skill.rs — attributes (agility/dexterity/strength), combo **action-timers**, archery **aim**
//! (calibration + aim-while-moving on foot or horseback), and a **battle-tested learning** model.
//!
//! Nothing here is a fixed number: an actor's combo window, dodge reaction and aim error all derive
//! from a simulated [`Body`](crate::body::Body) PLUS earned [`Skills`] mastery. Mastery rises from
//! actual fights via [`Skills::learn`] — fast early, asymptotic — so an AI (or you) gets measurably
//! **better and better the more it battles**. The archer literally **calibrates** (converges its aim
//! error toward a floor as it holds/practises), and aiming while MOVING — worst on a galloping mount —
//! costs accuracy until training buys it back.

use crate::body::Body;

const ARROW_SPEED_TILES: f32 = 9.0;

// ───────────────────────── attributes (from the body) ─────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Attributes { pub agility: i32, pub dexterity: i32, pub strength: i32, pub reflex_ms: u32 }
impl Attributes {
    pub fn from_body(b: &Body) -> Attributes {
        // faster reflex → more agile; grip+arms → dexterity; arms+core → strength
        let agility = (300i32.saturating_sub(b.base_reflex_ms as i32) / 12).max(1) + (b.legs.max_force * 6.0) as i32;
        let dexterity = ((b.grip.max_force + b.arms.max_force) * 9.0) as i32;
        let strength = ((b.arms.max_force + b.core.max_force) * 10.0) as i32;
        Attributes { agility, dexterity, strength, reflex_ms: b.reaction_ms() }
    }
}

// ───────────────────────── learned mastery ─────────────────────────

/// 0.0 (green) → ~1.0 (master) per discipline. Earned in battle.
#[derive(Debug, Clone, Copy)]
pub struct Skills { pub aim: f32, pub combo: f32, pub dodge: f32, pub battles: u32 }
impl Skills {
    pub fn green() -> Skills { Skills { aim: 0.08, combo: 0.10, dodge: 0.10, battles: 0 } }

    /// Learn from ONE battle. Each skill closes a fraction of its gap-to-mastery, weighted by how
    /// well it performed — fast early, diminishing as you near mastery (the asymptote).
    pub fn learn(&mut self, r: &BattleReport) {
        let rate = 0.30; // learns fast
        let close = |m: &mut f32, perf: f32| { *m = (*m + (1.0 - *m) * rate * perf.clamp(0.0, 1.0)).min(0.99); };
        close(&mut self.aim, r.hit_rate());
        close(&mut self.combo, r.combo_rate());
        close(&mut self.dodge, r.dodge_rate());
        self.battles += 1;
    }
}

/// What happened in a fight — the training signal.
#[derive(Debug, Clone, Copy, Default)]
pub struct BattleReport {
    pub shots_fired: u32, pub shots_hit: u32,
    pub combos_tried: u32, pub combos_landed: u32,
    pub dodges_tried: u32, pub dodges_clean: u32,
}
impl BattleReport {
    fn ratio(n: u32, d: u32) -> f32 { if d == 0 { 0.5 } else { n as f32 / d as f32 } }
    pub fn hit_rate(&self) -> f32 { Self::ratio(self.shots_hit, self.shots_fired) }
    pub fn combo_rate(&self) -> f32 { Self::ratio(self.combos_landed, self.combos_tried) }
    pub fn dodge_rate(&self) -> f32 { Self::ratio(self.dodges_clean, self.dodges_tried) }
}

// ───────────────────────── combo action-timers (measure agility) ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timing { Perfect, Good, Late, Whiff }
impl Timing { pub fn damage_mult(self) -> f32 { match self { Timing::Perfect=>1.5, Timing::Good=>1.2, Timing::Late=>0.9, Timing::Whiff=>0.0 } } }

/// The window to chain the 2nd MCP tool of a combo. Wider/more-forgiving with agility + combo mastery
/// — so HOW FAST you chain measures (and trains) agility.
#[derive(Debug, Clone, Copy)]
pub struct ComboTimer { pub window_ms: u32 }
impl ComboTimer {
    pub fn new(attr: &Attributes, skills: &Skills) -> ComboTimer {
        ComboTimer { window_ms: (260 + attr.agility * 18 + (skills.combo * 220.0) as i32).max(120) as u32 }
    }
    /// Judge the chain: chained well inside the window = Perfect, within = Good, sluggish = Late, missed = Whiff.
    pub fn judge(&self, elapsed_ms: u32) -> Timing {
        if elapsed_ms <= self.window_ms / 3 { Timing::Perfect }
        else if elapsed_ms <= self.window_ms { Timing::Good }
        else if elapsed_ms <= self.window_ms * 2 { Timing::Late }
        else { Timing::Whiff }
    }
}

// ───────────────────────── archery aim ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shot { Bullseye, Hit, Graze, Miss }

/// An aiming solution. `error_deg` is the current dispersion; the AI **calibrates** it down.
#[derive(Debug, Clone, Copy)]
pub struct Aim { pub error_deg: f32, floor_deg: f32 }
impl Aim {
    /// Fresh aim: dispersion from dexterity + aim mastery; floor is the best it can calibrate to.
    pub fn new(attr: &Attributes, skills: &Skills) -> Aim {
        let base = (13.0 - attr.dexterity as f32 * 0.5).max(3.0) * (1.0 - 0.6 * skills.aim);
        let floor = (base * 0.35).max(1.0);
        Aim { error_deg: base, floor_deg: floor }
    }
    /// The AI calibrates by holding/practising: each step closes 55% of the gap to the floor.
    /// Returns the new error (converges → "calibrated").
    pub fn calibrate_step(&mut self) -> f32 { self.error_deg = self.floor_deg + (self.error_deg - self.floor_deg) * 0.45; self.error_deg }
    pub fn calibrate(&mut self, steps: u32) { for _ in 0..steps { self.calibrate_step(); } }
    pub fn calibrated(&self) -> bool { self.error_deg <= self.floor_deg * 1.15 }

    /// Lead a moving target: aim ahead by (target_speed × flight_time).
    pub fn lead_tiles(&self, dist: f32, target_speed: f32) -> f32 { target_speed * (dist / ARROW_SPEED_TILES) }

    /// Take a shot. Aiming while MOVING adds dispersion — far worse mounted at speed — but skill +
    /// calibration buy it back. `quality` compares total error to the target's angular size.
    pub fn shoot(&self, dist: f32, target_radius: f32, target_speed: f32, shooter_speed: f32, mounted: bool, skills: &Skills) -> (f32, Shot) {
        let platform = if mounted { 2.6 } else { 1.5 };
        let move_pen = shooter_speed * platform * (1.0 - 0.5 * skills.aim); // training steadies the moving shot
        let total_err = self.error_deg + move_pen;
        // target angular size (deg); closer/bigger = easier
        let ang = (target_radius / dist.max(0.5)).atan().to_degrees().max(0.5);
        let q = if total_err <= ang * 0.5 { Shot::Bullseye }
                else if total_err <= ang { Shot::Hit }
                else if total_err <= ang * 1.8 { Shot::Graze }
                else { Shot::Miss };
        (total_err, q)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::Body;

    fn attrs() -> Attributes { Attributes::from_body(&Body::new()) }

    #[test]
    fn faster_chain_scores_better_timing() {
        let t = ComboTimer::new(&attrs(), &Skills::green());
        assert_eq!(t.judge(t.window_ms / 4), Timing::Perfect);
        assert_eq!(t.judge(t.window_ms), Timing::Good);
        assert_eq!(t.judge(t.window_ms * 3), Timing::Whiff);
        assert!(Timing::Perfect.damage_mult() > Timing::Good.damage_mult());
    }

    #[test]
    fn more_combo_mastery_widens_the_window() {
        let a = attrs();
        let green = ComboTimer::new(&a, &Skills::green());
        let mut master = Skills::green(); master.combo = 0.9;
        let trained = ComboTimer::new(&a, &master);
        assert!(trained.window_ms > green.window_ms, "mastery makes combos more forgiving");
    }

    #[test]
    fn archer_calibrates_error_down_to_a_floor() {
        let mut aim = Aim::new(&attrs(), &Skills::green());
        let start = aim.error_deg;
        aim.calibrate(6);
        assert!(aim.error_deg < start, "calibration reduces dispersion");
        assert!(aim.calibrated(), "after holding, it converges to its floor");
    }

    #[test]
    fn aiming_while_moving_is_harder_mounted_worst() {
        let a = attrs(); let s = Skills::green();
        let aim = Aim::new(&a, &s);
        let (still, _) = aim.shoot(6.0, 0.5, 0.0, 0.0, false, &s);
        let (ground_move, _) = aim.shoot(6.0, 0.5, 0.0, 2.0, false, &s);
        let (horse_move, _) = aim.shoot(6.0, 0.5, 0.0, 2.0, true, &s);
        assert!(ground_move > still, "moving on foot disperses the shot");
        assert!(horse_move > ground_move, "a galloping mount is the hardest platform");
    }

    #[test]
    fn lead_increases_with_target_speed_and_range() {
        let aim = Aim::new(&attrs(), &Skills::green());
        assert!(aim.lead_tiles(9.0, 2.0) > aim.lead_tiles(9.0, 0.0));
        assert!(aim.lead_tiles(18.0, 2.0) > aim.lead_tiles(9.0, 2.0));
    }

    #[test]
    fn ai_gets_better_and_better_with_battles() {
        // a learner that fights well should see aim error drop + combo window grow over battles
        let a = attrs();
        let mut sk = Skills::green();
        let err0 = Aim::new(&a, &sk).error_deg;
        let win0 = ComboTimer::new(&a, &sk).window_ms;
        let good_fight = BattleReport { shots_fired: 10, shots_hit: 7, combos_tried: 6, combos_landed: 5, dodges_tried: 8, dodges_clean: 6 };
        for _ in 0..8 { sk.learn(&good_fight); }
        let err1 = Aim::new(&a, &sk).error_deg;
        let win1 = ComboTimer::new(&a, &sk).window_ms;
        assert!(sk.battles == 8);
        assert!(err1 < err0, "battle-tested → tighter aim ({err1} < {err0})");
        assert!(win1 > win0, "battle-tested → more forgiving combos ({win1} > {win0})");
        assert!(sk.aim > 0.5 && sk.aim < 0.99, "fast early, asymptotic — never hits 1.0");
    }

    #[test]
    fn learning_is_diminishing_not_runaway() {
        let mut sk = Skills::green();
        let perfect = BattleReport { shots_fired: 10, shots_hit: 10, combos_tried: 5, combos_landed: 5, dodges_tried: 5, dodges_clean: 5 };
        let first = { let before = sk.aim; sk.learn(&perfect); sk.aim - before };
        let mut sk2 = sk; for _ in 0..20 { sk2.learn(&perfect); }
        let later = { let before = sk2.aim; sk2.learn(&perfect); sk2.aim - before };
        assert!(later < first, "later gains are smaller — diminishing returns");
    }
}
