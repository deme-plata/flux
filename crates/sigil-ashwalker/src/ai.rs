use crate::body::{Body, Dodge, Stimulus};
use crate::skill::{Aim, Attributes, BattleReport, ComboTimer, Shot, Skills, Timing};
use crate::Mcp;

pub enum Action {
    Approach,
    Retreat,
    Dodge,
    MeleeStrike,
    CastCombo(Mcp, Mcp),
    Shoot,
}

pub struct Perception {
    pub dist: f32,
    pub player_telegraph_ms: Option<u32>,
    pub player_low_hp: bool,
    pub mounted: bool,
    pub shooter_speed: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TacticalMood {
    Dodge,
    Reset,
    Burst,
    Control,
    Combo,
    Close,
    Press,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TacticalSnapshot {
    pub tick: u32,
    pub dist: f32,
    pub player_hp: i32,
    pub player_max_hp: i32,
    pub sigil: i32,
    pub foe_count: usize,
    pub boss_hp: i32,
    pub boss_max_hp: i32,
    pub incoming_telegraph_ms: Option<u32>,
    pub combo_ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TacticalTick {
    pub tick: u32,
    pub mood: TacticalMood,
    pub urgency: u8,
    pub line: String,
}

impl TacticalSnapshot {
    fn player_hp_pct(&self) -> u8 {
        pct(self.player_hp, self.player_max_hp)
    }

    fn boss_hp_pct(&self) -> u8 {
        pct(self.boss_hp, self.boss_max_hp)
    }
}

pub fn tactical_tick(s: &TacticalSnapshot) -> TacticalTick {
    let player_hp = s.player_hp_pct();
    let boss_hp = s.boss_hp_pct();
    let incoming = s.incoming_telegraph_ms.unwrap_or(u32::MAX);

    let (mood, label, urgency, hint) = if incoming <= 420 {
        (TacticalMood::Dodge, "DODGE", 95, "telegraph is live")
    } else if player_hp <= 25 && s.foe_count > 0 {
        (TacticalMood::Reset, "RESET", 88, "low HP: kite and rebuild shield")
    } else if boss_hp <= 20 && s.combo_ready && s.sigil >= 12 {
        (TacticalMood::Burst, "BURST", 82, "boss is cracked: spend the combo")
    } else if s.foe_count >= 3 {
        (TacticalMood::Control, "CONTROL", 72, "thin the pack before committing")
    } else if s.combo_ready && s.sigil >= 10 && s.dist <= 2.0 {
        (TacticalMood::Combo, "COMBO", 66, "close range combo window")
    } else if s.dist > 3.0 {
        (TacticalMood::Close, "CLOSE", 48, "close distance without spending sigil")
    } else {
        (TacticalMood::Press, "PRESS", 40, "keep pressure and watch stamina")
    };

    TacticalTick {
        tick: s.tick,
        mood,
        urgency,
        line: format!(
            "AI[{tick:04}] {label:<7} u={urgency:02} hp={player_hp:03}% boss={boss_hp:03}% foes={foes} sigil={sigil} dist={dist:.1} - {hint}",
            tick = s.tick,
            foes = s.foe_count,
            sigil = s.sigil,
            dist = s.dist,
        ),
    }
}

fn pct(value: i32, max: i32) -> u8 {
    if max <= 0 {
        return 0;
    }
    ((value.clamp(0, max) * 100) / max) as u8
}

pub struct AiBrain {
    body: Body,
    skills: Skills,
    aim: Aim,
    tally: BattleReport,
}

impl AiBrain {
    pub fn new() -> Self {
        let body = Body::new();
        let attrs = Attributes::from_body(&body);
        let skills = Skills::green();
        let aim = Aim::new(&attrs, &skills);
        let tally = BattleReport::default();
        Self {
            body,
            skills,
            aim,
            tally,
        }
    }

    pub fn decide(&self, p: &Perception) -> Action {
        // 1. Dodge if reactable incoming telegraph
        if let Some(telegraph) = p.player_telegraph_ms {
            if telegraph >= self.body.reaction_ms() {
                return Action::Dodge;
            }
        }

        let low_stamina = self.body.stamina < 5.0;
        let high_fatigue = self.body.fatigue() > 0.8;
        let staggered = self.body.staggered();

        // 2. Retreat when in bad shape
        if low_stamina || high_fatigue || staggered {
            return Action::Retreat;
        }

        // 3. Range‑based decisions
        if p.dist > 1.5 {
            if self.aim.calibrated() {
                Action::Shoot
            } else {
                Action::Approach
            }
        } else {
            // Close range
            if self.body.stamina >= 10.0 {
                // Alternate between melee and cast based on battle count
                if self.skills.battles % 2 == 0 {
                    Action::MeleeStrike
                } else {
                    // hard‑coded combo pair
                    Action::CastCombo(Mcp::FluxCombo, Mcp::DexSwap)
                }
            } else {
                Action::Retreat
            }
        }
    }

    pub fn act(&mut self, a: &Action, p: &Perception) {
        match a {
            Action::Approach | Action::Retreat => { /* movement only */ }
            Action::Dodge => {
                if let Some(telegraph) = p.player_telegraph_ms {
                    let stimulus = Stimulus {
                        telegraph_ms: telegraph,
                        severity: 1.0,
                    };
                    let result = self.body.dodge(&stimulus);
                    self.tally.dodges_tried += 1;
                    if let Dodge::Clean = result {
                        self.tally.dodges_clean += 1;
                    }
                }
            }
            Action::MeleeStrike => {
                let dmg = self.body.strike();
                self.tally.combos_tried += 1;
                if dmg > 0.0 {
                    self.tally.combos_landed += 1;
                }
            }
            Action::Shoot => {
                // reasonable defaults for target properties not carried by Perception
                let target_radius = 0.5;
                let target_speed = 0.0;
                let (_dmg_mult, shot) = self.aim.shoot(
                    p.dist,
                    target_radius,
                    target_speed,
                    p.shooter_speed,
                    p.mounted,
                    &self.skills,
                );
                self.tally.shots_fired += 1;
                if matches!(shot, Shot::Bullseye | Shot::Hit) {
                    self.tally.shots_hit += 1;
                }
            }
            Action::CastCombo(m1, m2) => {
                // simulate combo execution with a deterministic timing
                let attrs = Attributes::from_body(&self.body);
                let timer = ComboTimer::new(&attrs, &self.skills);
                let ideal = timer.window_ms as f32 / 2.0;
                // better combo skill → smaller deviation
                let noise = self.body.reaction_ms() as f32 * 0.2 * (1.0 - self.skills.combo);
                let elapsed = (ideal + noise) as u32;
                let timing = timer.judge(elapsed);
                self.tally.combos_tried += 1;
                if matches!(timing, Timing::Perfect | Timing::Good) {
                    self.tally.combos_landed += 1;
                }
                // the pair of Mcp values is used by the engine for the actual spell
                let _ = (m1, m2);
            }
        }
    }

    pub fn end_bout(&mut self) {
        self.skills.learn(&self.tally);
        self.tally = BattleReport::default();
    }

    pub fn battles(&self) -> u32 {
        self.skills.battles
    }

    pub fn mastery(&self) -> f32 {
        (self.skills.aim + self.skills.combo + self.skills.dodge) / 3.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calibrated_brain() -> AiBrain {
        let mut brain = AiBrain::new();
        // run enough calibration steps so aim reports calibrated
        while !brain.aim.calibrated() {
            brain.aim.calibrate(1);
        }
        brain
    }

    #[test]
    fn decides_shoot_at_long_range() {
        let brain = calibrated_brain();
        let p = Perception {
            dist: 5.0,
            player_telegraph_ms: None,
            player_low_hp: false,
            mounted: false,
            shooter_speed: 0.0,
        };
        assert!(matches!(brain.decide(&p), Action::Shoot));
    }

    #[test]
    fn decides_melee_or_cast_when_adjacent() {
        let brain = calibrated_brain();
        let p = Perception {
            dist: 1.0,
            player_telegraph_ms: None,
            player_low_hp: false,
            mounted: false,
            shooter_speed: 0.0,
        };
        let action = brain.decide(&p);
        // first battle should yield MeleeStrike (battles 0 is even)
        assert!(matches!(action, Action::MeleeStrike));
    }

    #[test]
    fn decides_dodge_against_reactable_telegraph() {
        let brain = AiBrain::new(); // reaction time is some default
        let reaction = brain.body.reaction_ms();
        let p = Perception {
            dist: 3.0,
            player_telegraph_ms: Some(reaction + 50), // comfortably reactable
            player_low_hp: false,
            mounted: false,
            shooter_speed: 0.0,
        };
        assert!(matches!(brain.decide(&p), Action::Dodge));
    }

    #[test]
    fn end_bout_raises_mastery() {
        let mut brain = calibrated_brain();
        let before = brain.mastery();

        // simulate a bout where we land some actions
        let p = Perception {
            dist: 5.0,
            player_telegraph_ms: None,
            player_low_hp: false,
            mounted: false,
            shooter_speed: 0.0,
        };
        // shoot a few times
        for _ in 0..3 {
            brain.act(&Action::Shoot, &p);
        }
        brain.end_bout();

        assert!(brain.mastery() > before);
    }

    #[test]
    fn mastery_grows_with_diminishing_returns() {
        let mut brain = calibrated_brain();
        let mut prev = 0.0;
        let p = Perception {
            dist: 5.0,
            player_telegraph_ms: None,
            player_low_hp: false,
            mounted: false,
            shooter_speed: 0.0,
        };
        let mut increments = Vec::new();

        for _ in 0..5 {
            // decent bout
            for _ in 0..5 {
                brain.act(&Action::Shoot, &p);
            }
            brain.end_bout();
            let current = brain.mastery();
            if prev > 0.0 {
                // record the per-bout gain (every iteration after the first, once prev is set)
                increments.push(current - prev);
            }
            prev = current;
        }

        // check that later increments are smaller than the first (diminishing)
        assert!(increments.len() >= 3);
        assert!(increments[0] > increments[increments.len() - 1]);
    }

    fn snapshot() -> TacticalSnapshot {
        TacticalSnapshot {
            tick: 42,
            dist: 1.5,
            player_hp: 50,
            player_max_hp: 100,
            sigil: 18,
            foe_count: 1,
            boss_hp: 80,
            boss_max_hp: 100,
            incoming_telegraph_ms: None,
            combo_ready: true,
        }
    }

    #[test]
    fn tactical_tick_is_deterministic_for_same_snapshot() {
        let s = snapshot();
        assert_eq!(tactical_tick(&s), tactical_tick(&s));
    }

    #[test]
    fn tactical_tick_prioritizes_dodge_over_burst() {
        let mut s = snapshot();
        s.boss_hp = 8;
        s.incoming_telegraph_ms = Some(300);

        let tick = tactical_tick(&s);

        assert_eq!(tick.mood, TacticalMood::Dodge);
        assert!(tick.line.contains("DODGE"));
    }

    #[test]
    fn tactical_tick_calls_burst_for_cracked_boss() {
        let mut s = snapshot();
        s.boss_hp = 15;

        let tick = tactical_tick(&s);

        assert_eq!(tick.mood, TacticalMood::Burst);
        assert!(tick.line.contains("boss=015%"));
    }

    #[test]
    fn tactical_tick_line_is_spectator_ready() {
        let tick = tactical_tick(&snapshot());

        assert!(tick.line.starts_with("AI[0042]"));
        assert!(tick.line.contains("foes=1"));
        assert!(tick.line.contains("sigil=18"));
    }
}
