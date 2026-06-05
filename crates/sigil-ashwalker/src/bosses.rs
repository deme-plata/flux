//! bosses.rs — Prototype 3: **The Ten Crowns of Ash**.
//!
//! A roster of *mechanically distinct* bosses (see `BOSSES_PROTOTYPE_3.md`). Each [`Archetype`] is a
//! different **solve**, not a stat-stick: no single tactic beats two of them, and the capstone needs
//! every prior solve at once. The point is that the live fight-state a player (or a 1M-context agent)
//! must track is large — *that* is what "requires a lot of context to conquer" means.
//!
//! This module gives:
//! - the [`Archetype`] enum + metadata + deterministic [`Archetype::from_root`] selection (so a
//!   `merkle::MerkleBoss` *is* one of the ten, committed to your decision ledger);
//! - three fully-simulated **reference brains** with their signature mechanic under test —
//!   [`PrismCzar`] (Sign-attunement reflection), [`Sediment`] (rotating staggered core), and
//!   [`Nullglyph`] (Sigil-drain execute). The remaining seven are specified in the design doc and
//!   land as sibling lanes against this same interface.
//!
//! std-only / FLUXFOOD. Depends only on the stable lib types ([`crate::rt::Sign`], [`crate::rt::Dir`]).

use crate::rt::{Dir, Sign};

// ───────────────────────────── the roster ─────────────────────────────

/// The ten unique bosses. Order is the intended progression (1 → 10 capstone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archetype {
    Audithollow, // 1 · mirrors your last moves
    PrismCzar,   // 2 · Sign-attunement reflection
    PackOfNine,  // 3 · rez-tree kill-order
    Sediment,    // 4 · rotating staggered core
    Reflection,  // 5 · a clone of you
    Rotmaw,      // 6 · cleanse-in-the-hazard
    Caesura,     // 7 · rewind-unless-anchored
    Nullglyph,   // 8 · Sigil-drain execute
    Council,     // 9 · votes its next mechanic
    UnmadeKing,  // 10 · capstone medley
}

impl Archetype {
    pub fn all() -> [Archetype; 10] {
        [
            Archetype::Audithollow,
            Archetype::PrismCzar,
            Archetype::PackOfNine,
            Archetype::Sediment,
            Archetype::Reflection,
            Archetype::Rotmaw,
            Archetype::Caesura,
            Archetype::Nullglyph,
            Archetype::Council,
            Archetype::UnmadeKing,
        ]
    }

    pub fn name(self) -> &'static str {
        match self {
            Archetype::Audithollow => "Audithollow, the Ledger-Wraith",
            Archetype::PrismCzar => "Prism Czar, the Glass Tyrant",
            Archetype::PackOfNine => "The Pack of Nine",
            Archetype::Sediment => "Sediment, the Ash-Colossus",
            Archetype::Reflection => "Your Reflection",
            Archetype::Rotmaw => "Rotmaw, the Plague-Cantor",
            Archetype::Caesura => "Cæsura, the Time-Eater",
            Archetype::Nullglyph => "Nullglyph, the Sigil-Devourer",
            Archetype::Council => "The Council of Nine Masks",
            Archetype::UnmadeKing => "The Unmade King",
        }
    }

    pub fn glyph(self) -> char {
        match self {
            Archetype::Audithollow => 'A',
            Archetype::PrismCzar => 'P',
            Archetype::PackOfNine => 'N',
            Archetype::Sediment => 'S',
            Archetype::Reflection => '@',
            Archetype::Rotmaw => 'R',
            Archetype::Caesura => 'C',
            Archetype::Nullglyph => '0',
            Archetype::Council => 'M',
            Archetype::UnmadeKing => 'K',
        }
    }

    /// One-line trap — the thing that makes the boss unique.
    pub fn trap(self) -> &'static str {
        match self {
            Archetype::Audithollow => "mirrors your last 3 distinct moves back at you",
            Archetype::PrismCzar => "attunes to a Sign each phase — that Sign heals it",
            Archetype::PackOfNine => "adds rez unless killed in dependency order",
            Archetype::Sediment => "immune except a rotating core, only while staggered",
            Archetype::Reflection => "a clone with your loadout and cooldowns",
            Archetype::Rotmaw => "stacking poison cleansed only inside its hazard",
            Archetype::Caesura => "rewinds your damage unless you parry-anchor",
            Archetype::Nullglyph => "drains Sigil; at zero it executes you",
            Archetype::Council => "votes its next mechanic; Axii biases the vote",
            Archetype::UnmadeKing => "four phases, each is a prior boss's mechanic",
        }
    }

    /// Minimum gear score (see `item.rs`) below which the fight is meant to be unfair.
    pub fn gear_req(self) -> i32 {
        match self {
            Archetype::Audithollow => 40,
            Archetype::PrismCzar => 90,
            Archetype::PackOfNine => 70,
            Archetype::Sediment => 140,
            Archetype::Reflection => 0, // scales to YOUR gear — never under/over
            Archetype::Rotmaw => 110,
            Archetype::Caesura => 130,
            Archetype::Nullglyph => 100,
            Archetype::Council => 120,
            Archetype::UnmadeKing => 220,
        }
    }

    /// A flavour metric: how much live state the boss forces you to track (1=reflex, 10=exam).
    /// Surfaced to telemetry as a proxy for "how much context to conquer".
    pub fn context_load(self) -> u8 {
        match self {
            Archetype::Audithollow => 6,
            Archetype::PrismCzar => 7,
            Archetype::PackOfNine => 8,
            Archetype::Sediment => 7,
            Archetype::Reflection => 9,
            Archetype::Rotmaw => 6,
            Archetype::Caesura => 8,
            Archetype::Nullglyph => 5,
            Archetype::Council => 8,
            Archetype::UnmadeKing => 10,
        }
    }

    /// Deterministically pick an archetype from a Merkle root (so a `MerkleBoss` *is* one of these).
    /// The capstone is reserved — it never rolls randomly; it is the hand-placed final fight.
    pub fn from_root(root: u64) -> Archetype {
        let nine = &Archetype::all()[..9];
        nine[(root % nine.len() as u64) as usize]
    }
}


// ───────────────────────── reference brain: Audithollow (#1) ─────────────────────────

/// The Ledger-Wraith: records your last 3 **distinct** move tags and replays them as telegraphed
/// counter-pressure when the buffer is full. Repeating the same move sharpens its read (spam counter).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveTag {
    Move,
    Dodge,
    Light,
    Heavy,
    Parry,
    Igni,
    Quen,
    Aard,
    Yrden,
    Axii,
}

#[derive(Debug, Clone)]
pub struct Audithollow {
    pub hp: i32,
    pub max_hp: i32,
    pub buffer: [Option<MoveTag>; 3],
    pub replay_cd: u32,
    pub spam_tag: Option<MoveTag>,
    pub spam_count: u32,
}
impl Audithollow {
    pub fn new(max_hp: i32) -> Self {
        Self {
            hp: max_hp,
            max_hp,
            buffer: [None; 3],
            replay_cd: 0,
            spam_tag: None,
            spam_count: 0,
        }
    }
    pub fn record(&mut self, tag: MoveTag) {
        if self.spam_tag == Some(tag) {
            self.spam_count += 1;
        } else {
            self.spam_tag = Some(tag);
            self.spam_count = 1;
        }
        if let Some(i) = self.buffer.iter().position(|&s| s == Some(tag)) {
            // refresh slot order
            for j in i..2 {
                self.buffer[j] = self.buffer[j + 1];
            }
            self.buffer[2] = Some(tag);
        } else {
            self.buffer[0] = self.buffer[1];
            self.buffer[1] = self.buffer[2];
            self.buffer[2] = Some(tag);
        }
    }
    pub fn buffer_full(&self) -> bool {
        self.buffer.iter().all(|s| s.is_some())
    }
    pub fn replay_damage(&self) -> i32 {
        8 + self.spam_count as i32 * 2
    }
    pub fn tick(&mut self) {
        if self.replay_cd > 0 {
            self.replay_cd -= 1;
        }
    }
    pub fn arm_replay(&mut self) {
        self.replay_cd = 18;
    }
    pub fn strike(&mut self, dmg: i32) {
        self.hp = (self.hp - dmg).max(0);
    }
    pub fn alive(&self) -> bool {
        self.hp > 0
    }
}

// ───────────────────────── reference brain: Prism Czar (#2) ─────────────────────────

/// The Glass Tyrant: each phase it **attunes** to a Sign. Casting the attuned Sign *heals* it (and
/// is reflected); any other Sign / steel damages it. Attunement rotates as it loses HP — so the
/// player's optimal Sign rotation **inverts** every phase.
#[derive(Debug, Clone)]
pub struct PrismCzar {
    pub hp: i32,
    pub max_hp: i32,
    pub attuned: Sign,
}
impl PrismCzar {
    pub fn new(max_hp: i32) -> PrismCzar {
        PrismCzar { hp: max_hp, max_hp, attuned: Sign::Igni }
    }
    /// Which 25%-HP phase we're in (0..=3) — drives attunement.
    pub fn phase(&self) -> usize {
        let frac = self.hp.max(0) as f64 / self.max_hp as f64;
        match frac {
            f if f > 0.75 => 0,
            f if f > 0.50 => 1,
            f if f > 0.25 => 2,
            _ => 3,
        }
    }
    fn attune(&mut self) {
        self.attuned = [Sign::Igni, Sign::Quen, Sign::Aard, Sign::Yrden][self.phase()];
    }
    /// Player casts `sign` at it for `dmg`. Returns the *net* HP delta applied (negative = damage,
    /// positive = the Czar healed because you fed its attunement).
    pub fn cast_at(&mut self, sign: Sign, dmg: i32) -> i32 {
        self.attune();
        let delta = if sign == self.attuned {
            (dmg / 2).max(4) // reflected back as healing
        } else {
            -dmg
        };
        self.hp = (self.hp + delta).clamp(0, self.max_hp);
        self.attune(); // re-attune in case the hit crossed a phase line
        delta
    }
    /// Steel (non-Sign) damage always lands.
    pub fn strike(&mut self, dmg: i32) {
        self.hp = (self.hp - dmg).max(0);
        self.attune();
    }
    pub fn alive(&self) -> bool {
        self.hp > 0
    }
}

// ───────────────────────── reference brain: Sediment (#4) ─────────────────────────

/// The Ash-Colossus: armored. Direct hits ping off. It opens only while **staggered**, and even then
/// only on its **lit core face**, which rotates N→E→S→W each phase. You must be on the right side
/// *and* have a stagger up.
#[derive(Debug, Clone)]
pub struct Sediment {
    pub hp: i32,
    pub max_hp: i32,
    pub core: Dir,      // the currently-lit face
    pub stagger: u32,   // ticks of open window remaining
    last_phase: usize,
}
impl Sediment {
    pub fn new(max_hp: i32) -> Sediment {
        Sediment { hp: max_hp, max_hp, core: Dir::new(0, -1), stagger: 0, last_phase: 0 }
    }
    fn phase(&self) -> usize {
        let frac = self.hp.max(0) as f64 / self.max_hp as f64;
        ((1.0 - frac) * 4.0) as usize
    }
    /// Rotate the lit core face when we cross a phase boundary. Call once per tick.
    pub fn tick(&mut self) {
        if self.stagger > 0 {
            self.stagger -= 1;
        }
        let p = self.phase();
        if p != self.last_phase {
            self.last_phase = p;
            // N → E → S → W
            self.core = match (self.core.dx, self.core.dy) {
                (0, -1) => Dir::new(1, 0),
                (1, 0) => Dir::new(0, 1),
                (0, 1) => Dir::new(-1, 0),
                _ => Dir::new(0, -1),
            };
        }
    }
    /// Open the stagger window (heavy / Aard / parry-riposte do this in the live loop).
    pub fn open(&mut self, ticks: u32) {
        self.stagger = ticks;
    }
    /// Attack from direction `from` (the side you're standing on) for `dmg`. Lands ONLY if staggered
    /// AND you're on the lit core face. Returns the damage actually dealt.
    pub fn hit(&mut self, from: Dir, dmg: i32) -> i32 {
        if self.stagger == 0 {
            return 0; // armored
        }
        if from != self.core {
            return 0; // wrong seam
        }
        self.hp = (self.hp - dmg).max(0);
        dmg
    }
    pub fn alive(&self) -> bool {
        self.hp > 0
    }
}

// ───────────────────────── reference brain: Nullglyph (#8) ─────────────────────────

/// The Sigil-Devourer: inverts the kit. It drains your Sigil each tick in range; casting a Sign
/// *feeds* its drain. At Sigil 0 its next strike **executes** you — unless you spend your one
/// decisive cast (Aard interrupts the execute) at the threshold.
#[derive(Debug, Clone)]
pub struct Nullglyph {
    pub hp: i32,
    pub drain: i32,     // sigil drained per tick while in range
    pub execute_armed: bool,
}
impl Nullglyph {
    pub fn new(hp: i32) -> Nullglyph {
        Nullglyph { hp, drain: 3, execute_armed: false }
    }
    /// One tick with the player in range. Mutates `sigil`; arms the execute when it bottoms out.
    pub fn tick_in_range(&mut self, sigil: &mut i32) {
        *sigil = (*sigil - self.drain).max(0);
        if *sigil == 0 {
            self.execute_armed = true;
        }
    }
    /// Casting a Sign while it's up worsens the drain (it wants you to panic-cast).
    pub fn on_player_sign(&mut self) {
        self.drain += 10;
    }
    /// The one trick: an Aard at the threshold interrupts the execute and resets the drain.
    pub fn interrupt_with_aard(&mut self) {
        self.execute_armed = false;
        self.drain = 3;
    }
    /// Resolve the boss's strike. Returns true if it was a lethal execute.
    pub fn resolve_strike(&self) -> bool {
        self.execute_armed
    }
    pub fn strike(&mut self, dmg: i32) {
        self.hp = (self.hp - dmg).max(0);
    }
    pub fn alive(&self) -> bool {
        self.hp > 0
    }
}


// ───────────────────────── reference brain: Pack of Nine (#3) ─────────────────────────

/// Distributed rez-tree: kill leaves → mids → Alpha. Wrong order ⇒ Alpha rez (in bossfight feed).
#[derive(Debug, Clone)]
pub struct PackOfNine {
    pub hound_hp: [i32; 9],
    pub max_hound: [i32; 9],
    pub rez_timer: u32,
    pub rez_target: usize,
    pub alpha_axii: bool,
    pub enrage: u32,
}
impl PackOfNine {
    pub fn new() -> Self {
        let mut max = [8; 9];
        max[0] = 36;
        let mut hp = max;
        Self { hound_hp: hp, max_hound: max, rez_timer: 0, rez_target: 0, alpha_axii: false, enrage: 0 }
    }
    /// Which hound index may take damage now (topological leaves-first).
    pub fn valid_target(&self) -> Option<usize> {
        let leaf_ok = |i: usize| self.hound_hp[i] > 0;
        for i in [4usize, 5, 6, 7, 8] {
            if leaf_ok(i) {
                return Some(i);
            }
        }
        if self.hound_hp[4] <= 0 && self.hound_hp[5] <= 0 && leaf_ok(1) {
            return Some(1);
        }
        if self.hound_hp[6] <= 0 && self.hound_hp[7] <= 0 && leaf_ok(2) {
            return Some(2);
        }
        if self.hound_hp[8] <= 0 && leaf_ok(3) {
            return Some(3);
        }
        if self.hound_hp[1] <= 0 && self.hound_hp[2] <= 0 && self.hound_hp[3] <= 0 && leaf_ok(0) {
            return Some(0);
        }
        None
    }
    pub fn total_hp(&self) -> i32 {
        self.hound_hp.iter().sum()
    }
    pub fn max_total(&self) -> i32 {
        self.max_hound.iter().sum()
    }
    /// Strike the next valid hound. Returns (damage dealt, illegal_kill_triggered_rez).
    pub fn strike(&mut self, dmg: i32, forced_target: Option<usize>) -> (i32, bool) {
        let valid = self.valid_target();
        let tgt = match (forced_target, valid) {
            (Some(f), Some(v)) if f == v => Some(f),
            (None, v) => v,
            _ => return (0, false),
        };
        let Some(i) = tgt else {
            return (0, false);
        };
        if self.hound_hp[i] <= 0 {
            return (0, false);
        }
        let dealt = dmg.min(self.hound_hp[i]);
        self.hound_hp[i] -= dealt;
        let mut rez = false;
        if self.hound_hp[i] <= 0 && i != 0 {
            let parent = match i {
                4 | 5 => 1,
                6 | 7 => 2,
                8 => 3,
                1 | 2 | 3 => 0,
                _ => 0,
            };
            let leaves_remain = match i {
                1 => self.hound_hp[4] > 0 || self.hound_hp[5] > 0,
                2 => self.hound_hp[6] > 0 || self.hound_hp[7] > 0,
                3 => self.hound_hp[8] > 0,
                _ => false,
            };
            // Rez only when a MID is killed before its leaves (wrong order). Leaves never rez; Alpha is last.
            let should_rez = match i {
                1 | 2 | 3 => leaves_remain,
                _ => false,
            };
            if should_rez && !self.alpha_axii {
                self.hound_hp[i] = self.max_hound[i];
                self.rez_timer = 80;
                self.rez_target = i;
                self.enrage += 1;
                rez = true;
            }
        }
        (dealt, rez)
    }
    pub fn tick(&mut self) {
        if self.rez_timer > 0 {
            self.rez_timer -= 1;
        }
    }
    pub fn flip_alpha(&mut self) {
        self.alpha_axii = true;
    }
    pub fn alive(&self) -> bool {
        self.total_hp() > 0
    }
}

// ───────────────────────── reference brain: Rotmaw (#6) ─────────────────────────

/// Plague stacks outside; cleanse in hazard (~). Hazard interaction lives in bossfight.
#[derive(Debug, Clone)]
pub struct Rotmaw {
    pub hp: i32,
    pub max_hp: i32,
}
impl Rotmaw {
    pub fn new(max_hp: i32) -> Self {
        Self { hp: max_hp, max_hp }
    }
    pub fn strike(&mut self, dmg: i32) {
        self.hp = (self.hp - dmg).max(0);
    }
    pub fn alive(&self) -> bool {
        self.hp > 0
    }
}

// ───────────────────────── reference brain: Caesura (#7) ─────────────────────────

/// The Time-Eater: damage is **provisional**. Every [`Caesura::REWIND_PERIOD`] ticks it REWINDS its
/// HP back to a checkpoint — undoing everything you did that window — UNLESS you **anchored** it by
/// landing a parry. Anchoring commits the damage (moves the checkpoint to the current HP). So the
/// fight is burst → parry-to-commit → burst → … A greedy all-damage line evaporates at the rewind.
#[derive(Debug, Clone)]
pub struct Caesura {
    pub hp: i32,
    pub max_hp: i32,
    pub checkpoint: i32,
    pub window: u32,
    pub anchored: bool,
}
impl Caesura {
    pub const REWIND_PERIOD: u32 = 140; // ~7s at 20 ticks/s

    pub fn new(max_hp: i32) -> Caesura {
        Caesura { hp: max_hp, max_hp, checkpoint: max_hp, window: Self::REWIND_PERIOD, anchored: false }
    }
    /// Steel and Signs both wound it normally — the trick is temporal, not elemental.
    pub fn strike(&mut self, dmg: i32) {
        self.hp = (self.hp - dmg).max(0);
    }
    /// A landed parry anchors the current window: the damage so far becomes permanent.
    pub fn anchor(&mut self) {
        self.anchored = true;
    }
    /// Advance one tick. Returns true on the tick a REWIND actually fires (for the feed/bark).
    pub fn tick(&mut self) -> bool {
        if self.window > 0 {
            self.window -= 1;
        }
        if self.window == 0 {
            let rewound = !self.anchored && self.hp < self.checkpoint;
            if self.anchored {
                self.checkpoint = self.hp; // commit the window's damage
            } else {
                self.hp = self.checkpoint; // undo it
            }
            self.anchored = false;
            self.window = Self::REWIND_PERIOD;
            return rewound;
        }
        false
    }
    /// Temporal truth: at 0 HP it is only truly dead if the kill was **committed** (anchored, or the
    /// checkpoint already fell to 0). An un-anchored 0 is just a pending rewind — not a win.
    pub fn alive(&self) -> bool {
        self.hp > 0 || (!self.anchored && self.checkpoint > 0)
    }
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roster_is_ten_unique_glyphs_and_names() {
        let all = Archetype::all();
        assert_eq!(all.len(), 10);
        // names + traps are all distinct
        let mut names: Vec<&str> = all.iter().map(|a| a.name()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 10, "all boss names distinct");
        let mut traps: Vec<&str> = all.iter().map(|a| a.trap()).collect();
        traps.sort();
        traps.dedup();
        assert_eq!(traps.len(), 10, "all solves distinct");
    }

    #[test]
    fn capstone_is_reserved_from_random_roll() {
        // from_root should never hand back the capstone — it's hand-placed.
        for root in 0u64..512 {
            assert_ne!(Archetype::from_root(root), Archetype::UnmadeKing);
        }
        // and it is deterministic
        assert_eq!(Archetype::from_root(42), Archetype::from_root(42));
    }

    #[test]
    fn capstone_carries_the_heaviest_context_load() {
        let king = Archetype::UnmadeKing.context_load();
        for a in Archetype::all() {
            if a != Archetype::UnmadeKing {
                assert!(king >= a.context_load(), "the exam is the heaviest");
            }
        }
        assert_eq!(king, 10);
    }

    #[test]
    fn prism_czar_heals_on_attuned_sign_damages_otherwise() {
        let mut p = PrismCzar::new(200); // phase 0 → attuned Igni
        assert_eq!(p.attuned, Sign::Igni);
        let before = p.hp;
        // feeding the attuned Sign HEALS it (clamped at max, so drop it first)
        p.strike(20);
        let mid = p.hp;
        let d = p.cast_at(Sign::Igni, 30);
        assert!(d > 0, "attuned Sign healed the Czar");
        assert!(p.hp > mid);
        // an off-attuned Sign damages
        let d2 = p.cast_at(Sign::Aard, 30);
        assert!(d2 < 0 && p.hp < before);
    }

    #[test]
    fn prism_czar_attunement_rotates_by_phase() {
        let mut p = PrismCzar::new(100);
        assert_eq!(p.phase(), 0);
        p.strike(60); // down to 40% → phase 2
        p.cast_at(Sign::Igni, 0); // force re-attune
        assert_eq!(p.phase(), 2);
        assert_eq!(p.attuned, Sign::Aard, "phase 2 attunes Aard — Igni is now safe to cast");
    }

    #[test]
    fn sediment_is_immune_unless_staggered_and_on_the_core_face() {
        let mut s = Sediment::new(300);
        let core = s.core; // lit face (starts North)
        // not staggered → armored
        assert_eq!(s.hit(core, 50), 0);
        // staggered but wrong face → no seam
        s.open(4);
        let wrong = Dir::new(core.dx + 1, core.dy + 1);
        assert_eq!(s.hit(wrong, 50), 0);
        // staggered + correct face → lands
        s.open(4);
        assert_eq!(s.hit(core, 50), 50);
        assert!(s.hp < s.max_hp);
    }

    #[test]
    fn sediment_core_rotates_across_phases() {
        let mut s = Sediment::new(100);
        let start = s.core;
        s.hp = 40; // cross into a later phase
        s.tick();
        assert_ne!(s.core, start, "the lit core moved to a new face");
    }

    #[test]
    fn nullglyph_drains_sigil_and_arms_execute() {
        let mut n = Nullglyph::new(120);
        let mut sigil = 8;
        n.tick_in_range(&mut sigil); // 8 -> 5
        n.tick_in_range(&mut sigil); // 5 -> 2
        n.tick_in_range(&mut sigil); // 2 -> 0, armed
        assert_eq!(sigil, 0);
        assert!(n.execute_armed);
        assert!(n.resolve_strike(), "execute would land");
        // the one trick: Aard interrupts it
        n.interrupt_with_aard();
        assert!(!n.resolve_strike());
        assert_eq!(n.drain, 3, "drain reset");
    }

    #[test]
    fn pack_requires_leaf_order() {
        let mut p = PackOfNine::new();
        // Mid hounds are sealed until their leaves are gone.
        let (d, rez) = p.strike(20, Some(1));
        assert_eq!(d, 0);
        assert!(!rez);
        let (d, rez_leaf) = p.strike(20, Some(4));
        assert!(d > 0);
        assert!(!rez_leaf, "valid leaf kill must not rez while mid parent lives");
        // Leaves 4+5 cleared → mid 1 is the valid target; Alpha (0) still up ⇒ rez on kill.
        for i in 4..=8 {
            p.hound_hp[i] = 0;
        }
        assert_eq!(p.valid_target(), Some(1));
        let (d, rez) = p.strike(40, Some(1));
        assert!(d > 0);
        assert!(!rez, "mid kill after its leaves are cleared is legal");
        assert_eq!(p.hound_hp[1], 0);
        // clear other mids then Alpha
        p.hound_hp[2] = 0;
        p.hound_hp[3] = 0;
        assert_eq!(p.valid_target(), Some(0));
        let (d, _) = p.strike(50, Some(0));
        assert!(d > 0);
        assert_eq!(p.total_hp(), 0);
    }

    #[test]
    fn caesura_rewinds_without_anchor() {
        let mut c = Caesura::new(100);
        c.window = 1;
        c.strike(40);
        assert!(c.hp < 100);
        let rewound = c.tick();
        assert!(rewound);
        assert_eq!(c.hp, 100);
    }

    #[test]
    fn caesura_anchor_commits_damage() {
        let mut c = Caesura::new(100);
        c.window = 2;
        c.strike(50);
        c.anchor();
        let _ = c.tick();
        assert_eq!(c.hp, 50);
    }

    #[test]
    fn audithollow_records_three_distinct_moves_and_spam_counter() {
        let mut a = Audithollow::new(40);
        a.record(MoveTag::Light);
        a.record(MoveTag::Dodge);
        a.record(MoveTag::Igni);
        assert!(a.buffer_full());
        a.record(MoveTag::Light);
        a.record(MoveTag::Light);
        assert!(a.spam_count >= 2);
        assert!(a.replay_damage() > 8);
    }

    fn nullglyph_punishes_panic_casting() {
        let mut n = Nullglyph::new(120);
        let d0 = n.drain;
        n.on_player_sign();
        assert!(n.drain > d0, "casting a Sign fed its drain — don't panic-cast");
    }
}
