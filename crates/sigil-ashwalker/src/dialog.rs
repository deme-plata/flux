//! dialog.rs — boss barks. Grim, terse, Souls/Witcher-flavoured one-liners surfaced into the combat
//! feed on key beats (intro, phase shift, low HP, when you feed the boss's trap, when you parry it,
//! and on either death).
//!
//! **Authored by DeepSeek-V4** (`deepseek-v4-flash`) at build-authoring time, then **baked here as
//! static data** — so the game itself stays std-only / zero-dep / offline at runtime (no API call in
//! the loop). This is the flux-moe pattern: an LLM writes the content; we verify it and ship it cold.
//! Regenerate by re-running the authoring prompt and replacing the match arms below.

use crate::bosses::Archetype;

/// The combat beats that trigger a bark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bark {
    Intro,
    PhaseShift,
    LowHp,
    /// The boss gloats at the player's *signature mistake* (Czar fed its attuned Sign, Sediment
    /// hit while armored, Nullglyph panic-cast into the drain).
    FeedsTrap,
    GotParried,
    KillsPlayer,
    Dies,
}

/// The line pool for a (boss, beat). Empty slice ⇒ that boss has no line for the beat.
fn pool(a: Archetype, b: Bark) -> &'static [&'static str] {
    match (a, b) {
        (Archetype::Audithollow, Bark::Intro) => &["The ledger opens.", "I have already read your next mistake."],
        (Archetype::Audithollow, Bark::FeedsTrap) => &["Again? I wrote that move down.", "Variety is the only ward I cannot mirror."],
        (Archetype::Audithollow, Bark::GotParried) => &["A new line in the ledger.", "Unexpected — refreshing."],
        (Archetype::PackOfNine, Bark::Intro) => &["Nine throats, one Alpha.", "Kill the leaves — or the tree grows back."],
        (Archetype::PackOfNine, Bark::FeedsTrap) => &["The Alpha HOWLS — your kill-order was wrong.", "I raise what you slew."],
        (Archetype::Rotmaw, Bark::Intro) => &["Breathe deep — the rot loves lungs.", "The cure is in my ash."],
        (Archetype::Rotmaw, Bark::FeedsTrap) => &["Rot stacks — dive the hazard or die.", "You fear the fire you need."],
        (Archetype::Caesura, Bark::Intro) => &["Time is mine to unwrite.", "Parry, or watch your work vanish."],
        (Archetype::Caesura, Bark::FeedsTrap) => &["REWIND — did you forget to anchor?", "Your progress was never real."],
        (Archetype::Caesura, Bark::GotParried) => &["Anchored. The wound stays.", "Clever — for a mortal."],
        (Archetype::Audithollow, Bark::Dies) => &["The wraith closes its book.", "Your variety outlived my mirror."],
        (Archetype::PrismCzar, Bark::Intro) => &["The glass tyrant shatters silence.", "Come, break yourself upon my facets."],
        (Archetype::PrismCzar, Bark::PhaseShift) => &["A new sign resonates.", "Your ignorance is my sustenance."],
        (Archetype::PrismCzar, Bark::LowHp) => &["Cracks run deep, yet I endure.", "Your feeble light still feeds me."],
        (Archetype::PrismCzar, Bark::FeedsTrap) => &["Yes! That was the key.", "You gift me strength with every cast."],
        (Archetype::PrismCzar, Bark::GotParried) => &["You learn. Too late.", "Glass reflects even the sharpest wit."],
        (Archetype::PrismCzar, Bark::KillsPlayer) => &["Shatter into nothing.", "Your soul joins my broken kingdom."],
        (Archetype::PrismCzar, Bark::Dies) => &["The prism falls... but shards remain.", "Even in death, I reflect your folly."],
        (Archetype::Sediment, Bark::Intro) => &["The ash-colossus rises from ruin.", "Your puny steel will only wake the stone."],
        (Archetype::Sediment, Bark::PhaseShift) => &["The core turns. Seek the crack.", "Your blows are but whispers."],
        (Archetype::Sediment, Bark::LowHp) => &["Stone shatters. Ash scatters.", "One more strike? You barely chip me."],
        (Archetype::Sediment, Bark::FeedsTrap) => &["Ha! Hammer the mountain.", "My armor laughs at your impotence."],
        (Archetype::Sediment, Bark::GotParried) => &["Staggered? Then you see the flaw.", "But can you reach it in time?"],
        (Archetype::Sediment, Bark::KillsPlayer) => &["Crushed into dust.", "Join the sediment of ages."],
        (Archetype::Sediment, Bark::Dies) => &["The colossus falls, a mountain mourns.", "Now you carry its weight."],
        (Archetype::Nullglyph, Bark::Intro) => &["The sigil-devourer hungers.", "Cast your magic into oblivion."],
        (Archetype::Nullglyph, Bark::PhaseShift) => &["Your sigil wanes. I grow.", "Feed me more, foolish mage."],
        (Archetype::Nullglyph, Bark::LowHp) => &["Fading, but still gluttonous.", "One last feast before the void."],
        (Archetype::Nullglyph, Bark::FeedsTrap) => &["Mm, that taste of panic.", "You hand me your own death."],
        (Archetype::Nullglyph, Bark::GotParried) => &["You deny me? Clever.", "But my appetite is endless."],
        (Archetype::Nullglyph, Bark::KillsPlayer) => &["Empty. Now you are nothing.", "Join the sigils I have consumed."],
        (Archetype::Nullglyph, Bark::Dies) => &["The devourer chokes... on itself.", "Its own hunger was its end."],
        // NB: Caesura Intro/FeedsTrap/GotParried live above (grok-viktor's); these 4 complete the set.
        (Archetype::Caesura, Bark::PhaseShift) => &["The hourglass turns.", "Your deeds unravel like thread."],
        (Archetype::Caesura, Bark::LowHp) => &["Even your defiance grows brittle.", "All moments bend to the end."],
        (Archetype::Caesura, Bark::KillsPlayer) => &["Your name was never written.", "Fall into the gap between ticks."],
        (Archetype::Caesura, Bark::Dies) => &["The clock cracks.", "Even I am not spared the final chime."],
        _ => &[],
    }
}

/// Pick a line deterministically from `seed` (e.g. the tick). Empty string if no pool.
pub fn line(a: Archetype, b: Bark, seed: u64) -> &'static str {
    let p = pool(a, b);
    if p.is_empty() {
        ""
    } else {
        p[(seed as usize) % p.len()]
    }
}

/// Tracks which once-only barks have fired so the boss doesn't repeat itself every tick.
#[derive(Debug, Clone)]
pub struct Director {
    intro_done: bool,
    low_said: bool,
    last_phase: usize,
}
impl Director {
    pub fn new() -> Director {
        Director { intro_done: false, low_said: false, last_phase: 0 }
    }
    /// Fire the intro once (call at fight start).
    pub fn intro(&mut self, a: Archetype, seed: u64) -> Option<String> {
        if self.intro_done {
            return None;
        }
        self.intro_done = true;
        Some(format!("{}: \"{}\"", a.name(), line(a, Bark::Intro, seed)))
    }
    /// Surface a PhaseShift bark when `phase` advances, and a one-time LowHp bark under 25%.
    pub fn on_state(&mut self, a: Archetype, phase: usize, hp_frac: f64, seed: u64) -> Option<String> {
        if phase > self.last_phase {
            self.last_phase = phase;
            return Some(format!("{}: \"{}\"", a.name(), line(a, Bark::PhaseShift, seed)));
        }
        if !self.low_said && hp_frac > 0.0 && hp_frac < 0.25 {
            self.low_said = true;
            return Some(format!("{}: \"{}\"", a.name(), line(a, Bark::LowHp, seed)));
        }
        None
    }
    /// An event bark (FeedsTrap / GotParried / KillsPlayer / Dies) — caller decides when.
    pub fn event(&self, a: Archetype, b: Bark, seed: u64) -> String {
        format!("{}: \"{}\"", a.name(), line(a, b, seed))
    }
}
impl Default for Director {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_live_boss_has_all_seven_beats() {
        let beats = [Bark::Intro, Bark::PhaseShift, Bark::LowHp, Bark::FeedsTrap, Bark::GotParried, Bark::KillsPlayer, Bark::Dies];
        for a in [Archetype::PrismCzar, Archetype::Sediment, Archetype::Nullglyph] {
            for b in beats {
                assert!(!pool(a, b).is_empty(), "{:?} missing a {:?} line", a, b);
                assert!(!line(a, b, 0).is_empty());
            }
        }
    }

    #[test]
    fn director_fires_intro_once_then_phase_and_low() {
        let mut d = Director::new();
        assert!(d.intro(Archetype::PrismCzar, 0).is_some());
        assert!(d.intro(Archetype::PrismCzar, 0).is_none(), "intro only once");
        // phase advance fires a bark
        assert!(d.on_state(Archetype::PrismCzar, 1, 0.6, 0).is_some());
        // low hp fires once
        let l = d.on_state(Archetype::PrismCzar, 1, 0.2, 0);
        assert!(l.is_some());
        assert!(d.on_state(Archetype::PrismCzar, 1, 0.2, 0).is_none(), "low hp only once");
    }

    #[test]
    fn lines_are_quotable_and_bounded() {
        for a in [Archetype::PrismCzar, Archetype::Sediment, Archetype::Nullglyph] {
            for b in [Bark::Intro, Bark::Dies] {
                for s in 0..4u64 {
                    let l = line(a, b, s);
                    assert!(l.len() <= 90, "bark stays terminal-sized");
                    assert!(!l.contains('"'), "no nested quotes");
                }
            }
        }
    }
}
