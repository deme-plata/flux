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
    use crate::body::{Body, Stimulus};
    use crate::skill::{Aim, BattleReport};

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
}
