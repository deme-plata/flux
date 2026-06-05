//! bossfight.rs — the live, real-time **single-boss** arena: wires the prototype-3 boss brains
//! ([`crate::bosses`]) into the Witcher-3 combat kit so you can actually FIGHT Prism Czar, Sediment,
//! and Nullglyph in the terminal.
//!
//! It reuses the stable primitives from [`crate::rt`] ([`RtPlayer`], [`Dir`], [`Sign`]) + the
//! [`crate::World`] so the *feel* (dodge i-frames, light chains, heavy stagger, parry→riposte,
//! Signs) is identical — but **player damage is routed through the boss brain**, which is where each
//! boss's signature trap lives:
//!   • **Prism Czar** — casting its *attuned* Sign heals it (read the HUD, drop that Sign).
//!   • **Sediment** — armored; your blows only land while it's **staggered** AND you're on its
//!     **rotating core face** (heavy / Aard / a parry open the window).
//!   • **Nullglyph** — drains your Sigil; at 0 its next strike **executes** you unless you **Aard**.
//!
//! Kept in its own module (not edited into `rt.rs`) so it composes without colliding with the
//! swarm's live edits there. std-only / FLUXFOOD; deterministic; tested.

use crate::bosses::{Archetype, Audithollow, Caesura, MoveTag, Nullglyph, PackOfNine, PrismCzar, Rotmaw, Sediment};
use crate::dialog::{Bark, Director};
use crate::gore::{self, BloodField};
use crate::rt::{Action, Dir, RtPlayer, Sign};
use crate::{Terrain, V3, World, STEPS8};

/// Forward-arc test (mirrors rt's private one): `t` within Chebyshev `reach` and in the half-plane
/// the player faces.
fn in_arc(p: V3, face: Dir, t: V3, reach: i32) -> bool {
    if p.plane_cheby(t) > reach || p.z != t.z {
        return false;
    }
    (t.x - p.x) * face.dx + (t.y - p.y) * face.dy >= 1
}

/// The three bosses with live brains (the rest are specced in the design doc).
#[derive(Debug, Clone)]
pub enum Brain {
    Audit(Audithollow),
    Prism(PrismCzar),
    Pack(PackOfNine),
    Sediment(Sediment),
    Rotmaw(Rotmaw),
    Null(Nullglyph),
    Caesura(Caesura),
}
impl Brain {
    pub fn archetype(&self) -> Archetype {
        match self {
            Brain::Audit(_) => Archetype::Audithollow,
            Brain::Pack(_) => Archetype::PackOfNine,
            Brain::Rotmaw(_) => Archetype::Rotmaw,
            Brain::Prism(_) => Archetype::PrismCzar,
            Brain::Sediment(_) => Archetype::Sediment,
            Brain::Null(_) => Archetype::Nullglyph,
            Brain::Caesura(_) => Archetype::Caesura,
        }
    }
    pub fn hp(&self) -> i32 {
        match self {
            Brain::Audit(b) => b.hp,
            Brain::Pack(p) => p.total_hp(),
            Brain::Rotmaw(r) => r.hp,
            Brain::Prism(b) => b.hp,
            Brain::Sediment(b) => b.hp,
            Brain::Null(b) => b.hp,
            Brain::Caesura(b) => b.hp,
        }
    }
    pub fn max_hp(&self) -> i32 {
        match self {
            Brain::Audit(b) => b.max_hp,
            Brain::Pack(p) => p.max_total(),
            Brain::Rotmaw(r) => r.max_hp,
            Brain::Prism(b) => b.max_hp,
            Brain::Sediment(b) => b.max_hp,
            Brain::Null(b) => 90,
            Brain::Caesura(b) => b.max_hp,
        }
    }
    pub fn alive(&self) -> bool {
        match self {
            // Cæsura at 0 HP may still rewind back — its own rule decides "truly dead".
            Brain::Caesura(c) => c.alive(),
            _ => self.hp() > 0,
        }
    }
}

/// The boss body's offensive state machine (telegraphed, like the rank-and-file foes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BPhase {
    Stalk,
    Windup { left: u32, reach: i32, dmg: i32 },
    Recover { left: u32 },
    Stagger { left: u32 }, // stunned — also the damage window vs Sediment
}

#[derive(Debug, Clone)]
pub struct BossFight {
    pub world: World,
    pub player: RtPlayer,
    pub brain: Brain,
    pub boss: V3,
    phase: BPhase,
    move_cd: u32,
    slow: u32,
    pub blood: BloodField,
    dir: Director,
    pub feed: Vec<String>,
    pub tick: u64,
    pub won: bool,
    pub dead: bool,
    /// Rotmaw: plague stacks on the player.
    pub player_rot: u32,
    pub rot_cleanse_ticks: u32,
    pub hazard_anchor: V3,
    last_player_tag: Option<MoveTag>,
}

impl BossFight {
    pub fn new(which: Archetype) -> BossFight {
        let world = World::arena();
        let brain = match which {
            Archetype::Audithollow => Brain::Audit(Audithollow::new(80)),
            Archetype::PrismCzar => Brain::Prism(PrismCzar::new(120)),
            Archetype::PackOfNine => Brain::Pack(PackOfNine::new()),
            Archetype::Sediment => Brain::Sediment(Sediment::new(160)),
            Archetype::Rotmaw => Brain::Rotmaw(Rotmaw::new(110)),
            Archetype::Nullglyph => Brain::Null(Nullglyph::new(90)),
            Archetype::Caesura => Brain::Caesura(Caesura::new(150)),
            _ => Brain::Prism(PrismCzar::new(120)),
        };
        let hazard_anchor = V3::new(5, 5, 0);
        let mut world = world;
        if matches!(which, Archetype::Rotmaw) {
            let i = world.idx(hazard_anchor).unwrap();
            world.tiles[i] = Terrain::Hazard;
        }
        let intro = match which {
            Archetype::PackOfNine => "Pack of Nine — kill LEAVES first; wrong order ⇒ Alpha rez.",
            Archetype::Rotmaw => "Rotmaw — rot stacks in melee; cleanse in the hazard (~).",
            Archetype::Audithollow => "Audithollow mirrors your last 3 distinct moves — stay varied.",
            Archetype::PrismCzar => "Prism Czar attunes — DON'T feed it its lit Sign.",
            Archetype::Sediment => "Sediment is armored — STAGGER it, then strike its lit core face.",
            Archetype::Nullglyph => "Nullglyph drains your Sigil — win with STEEL, save one Aard.",
            Archetype::Caesura => "Cæsura rewinds your wounds — PARRY each window to ANCHOR them.",
            _ => "A Crown of Ash stirs.",
        };
        let mut bf = BossFight {
            world,
            player: RtPlayer::new(V3::new(2, 6, 0)),
            brain,
            boss: V3::new(6, 4, 0),
            phase: BPhase::Stalk,
            move_cd: 0,
            slow: 0,
            blood: BloodField::new(),
            dir: Director::new(),
            feed: vec![intro.into()],
            tick: 0,
            won: false,
            dead: false,
            player_rot: 0,
            rot_cleanse_ticks: 0,
            hazard_anchor: V3::new(5, 5, 0),
            last_player_tag: None,
        };
        let arch = bf.brain.archetype();
        if let Some(bark) = bf.dir.intro(arch, 0) {
            bf.feed.push(bark);
        }
        bf
    }

    fn say(&mut self, s: impl Into<String>) {
        self.feed.push(s.into());
        let n = self.feed.len();
        if n > 64 {
            self.feed.drain(0..n - 64);
        }
    }

    /// The side the player is standing on, relative to the boss (drives Sediment's core check).
    fn from_dir(&self) -> Dir {
        Dir::new(self.player.pos.x - self.boss.x, self.player.pos.y - self.boss.y)
    }

    // ── damage router: where each boss's trap lives ──────────────────
    /// Apply `dmg` to the boss. `sign` = the Sign used (None = steel). Returns damage actually dealt.
    fn damage_boss(&mut self, dmg: i32, sign: Option<Sign>) -> i32 {
        let from = self.from_dir();
        let arch = self.brain.archetype();
        let mut dealt = 0;
        let mut feeds = false; // the player fed the boss's signature trap → it gloats
        let mut msg: Option<String> = None;
        match &mut self.brain {
            Brain::Audit(a) => {
                a.strike(dmg);
                dealt = dmg;
                msg = Some(format!("steel etches the Ledger-Wraith for {dmg}"));
            }
            Brain::Prism(c) => {
                if let Some(s) = sign {
                    let d = c.cast_at(s, dmg);
                    if d > 0 {
                        feeds = true;
                        msg = Some(format!("the Czar DRINKS your {} (+{d} hp) — it's attuned!", s.name()));
                    } else {
                        dealt = -d;
                        msg = Some(format!("{} bites the Czar for {}", s.name(), -d));
                    }
                } else {
                    c.strike(dmg);
                    dealt = dmg;
                }
            }
            Brain::Sediment(s) => {
                dealt = s.hit(from, dmg);
                if dealt == 0 {
                    if s.stagger == 0 {
                        feeds = true;
                        msg = Some("CLANG — armored. Stagger it first (heavy/Aard/parry).".into());
                    } else {
                        msg = Some("you strike the wrong seam — get to the lit core face.".into());
                    }
                } else {
                    msg = Some(format!("the core cracks for {dealt}!"));
                }
            }
            Brain::Null(n) => {
                let mut interrupted = false;
                if let Some(sg) = sign {
                    n.on_player_sign();
                    feeds = true; // any Sign feeds the drain…
                    if sg == Sign::Aard {
                        n.interrupt_with_aard();
                        interrupted = true;
                        feeds = false; // …except a decisive Aard, which is the correct play
                    }
                }
                n.strike(dmg);
                dealt = dmg;
                msg = Some(if interrupted {
                    "Aard slams Nullglyph — execute interrupted, drain reset.".into()
                } else {
                    format!("steel bites Nullglyph for {dmg}")
                });
            }
            Brain::Pack(p) => {
                let (d, rez) = p.strike(dmg, None);
                dealt = d;
                if rez {
                    feeds = true;
                    msg = Some(format!(
                        "WRONG KILL ORDER — hound {} REZZES! enrage {}",
                        p.rez_target, p.enrage
                    ));
                } else if d > 0 {
                    msg = Some(format!(
                        "pack hound takes {d} (total {} / {})",
                        p.total_hp(),
                        p.max_total()
                    ));
                } else {
                    feeds = true;
                    msg = Some(
                        "your blow hits a sealed hound — kill the lit LEAVES first".into(),
                    );
                }
            }
            Brain::Rotmaw(r) => {
                r.strike(dmg);
                dealt = dmg;
                msg = Some(format!("Rotmaw festers for {dmg} — rot stacks in melee"));
            }
            Brain::Caesura(c) => {
                // steel + Signs both wound it; the catch is temporal (it rewinds unless anchored)
                c.strike(dmg);
                dealt = dmg;
                msg = Some(format!("you score Cæsura for {dmg} — ANCHOR it (parry) before it rewinds"));
            }
        }
        // brain borrow released — now touch feed / blood / dialog
        if let Some(m) = msg {
            self.say(m);
        }
        if dealt > 0 {
            self.blood.spill(self.boss, dealt);
        }
        if feeds && self.tick % 3 == 0 {
            let bark = self.dir.event(arch, Bark::FeedsTrap, self.tick);
            self.say(bark);
        }
        dealt
    }

    /// Open the stagger window (heavy / Aard / a parry). Syncs Sediment's core window.
    fn stagger_boss(&mut self, ticks: u32) {
        self.phase = BPhase::Stagger { left: ticks };
        if let Brain::Sediment(s) = &mut self.brain {
            s.open(ticks);
        }
    }

    // ── player input (mirrors rt.rs, single-boss) ────────────────────
    fn action_to_tag(a: Action) -> MoveTag {
        match a {
            Action::Move(_) => MoveTag::Move,
            Action::Dodge(_) => MoveTag::Dodge,
            Action::Light => MoveTag::Light,
            Action::Heavy => MoveTag::Heavy,
            Action::Parry => MoveTag::Parry,
            Action::Cast(s) => match s {
                crate::rt::Sign::Igni => MoveTag::Igni,
                crate::rt::Sign::Quen => MoveTag::Quen,
                crate::rt::Sign::Aard => MoveTag::Aard,
                crate::rt::Sign::Yrden => MoveTag::Yrden,
                crate::rt::Sign::Axii => MoveTag::Axii,
            },
        }
    }

    pub fn input(&mut self, a: Action) {
        let tag = Self::action_to_tag(a);
        self.last_player_tag = Some(tag);
        if let Brain::Audit(ref mut a) = self.brain {
            a.record(tag);
        }
        if let Brain::Pack(ref mut p) = self.brain {
            if matches!(a, Action::Cast(crate::rt::Sign::Axii)) {
                if p.hound_hp[0] <= p.max_hound[0] / 3 {
                    p.flip_alpha();
                    self.say("Axii flips the Alpha — rezzes now fight FOR you!");
                }
            }
        }
        match a {
            Action::Move(d) => self.do_move(d),
            Action::Dodge(d) => self.do_dodge(d),
            Action::Light => self.do_light(),
            Action::Heavy => self.do_heavy(),
            Action::Parry => self.do_parry(),
            Action::Cast(s) => self.do_cast(s),
        }
    }

    fn do_move(&mut self, d: Dir) {
        self.player.face = d;
        if self.player.busy > 0 || self.player.move_cd > 0 {
            return;
        }
        let mut dest = self.player.pos.add(V3::new(d.dx, d.dy, 0));
        if self.world.at(self.player.pos) == Terrain::Ramp || self.world.at(dest) == Terrain::Ramp {
            let up = dest.add(V3::new(0, 0, 1));
            if self.world.walkable(up) {
                dest = up;
            }
        }
        if !self.world.walkable(dest) || dest == self.boss {
            return;
        }
        self.player.pos = dest;
        self.player.move_cd = 2;
        if self.world.at(dest) == Terrain::Hazard {
            self.player.hp -= 4;
            self.say("ash-lava sears you (-4)");
        }
    }

    fn do_dodge(&mut self, d: Dir) {
        if self.player.stamina < 25 {
            return;
        }
        self.player.face = d;
        self.player.stamina -= 25;
        self.player.iframes = 6;
        self.player.busy = 2;
        for _ in 0..2 {
            let step = self.player.pos.add(V3::new(d.dx, d.dy, 0));
            if self.world.walkable(step) && step != self.boss {
                self.player.pos = step;
            } else {
                break;
            }
        }
    }

    fn boss_in_arc(&self, reach: i32) -> bool {
        in_arc(self.player.pos, self.player.face, self.boss, reach)
    }

    fn do_light(&mut self) {
        if self.player.busy > 0 || self.player.light_cd > 0 || self.player.stamina < 6 {
            return;
        }
        self.player.stamina -= 6;
        self.player.light_cd = 3;
        self.player.busy = 1;
        self.player.combo = (self.player.combo % 3) + 1;
        self.player.combo_decay = 18;
        let base = 7 + self.player.combo as i32 * 2;
        let crit = self.player.riposte;
        self.player.riposte = false;
        let dmg = if crit { base * 2 + 6 } else { base };
        if self.boss_in_arc(1) {
            self.damage_boss(dmg, None);
        }
    }

    fn do_heavy(&mut self) {
        if self.player.busy > 0 || self.player.stamina < 30 {
            return;
        }
        self.player.stamina -= 30;
        self.player.busy = 5;
        self.player.combo = 0;
        if self.boss_in_arc(2) {
            self.stagger_boss(6); // the heavy opens the window…
            self.damage_boss(26, None); // …and lands inside it
        }
    }

    fn do_parry(&mut self) {
        if self.player.busy > 0 || self.player.stamina < 10 {
            return;
        }
        self.player.stamina -= 10;
        self.player.guard = 6;
    }

    fn do_cast(&mut self, sign: Sign) {
        if self.player.busy > 0 || self.player.sigil < sign.cost() {
            return;
        }
        self.player.sigil -= sign.cost();
        self.player.signs_cast += 1;
        self.player.sign_kinds.insert(sign.name());
        match sign {
            Sign::Igni => {
                if self.boss_in_arc(3) {
                    self.damage_boss(16, Some(Sign::Igni));
                }
            }
            Sign::Quen => {
                self.player.shield += 22;
                self.say("Quen — +22 ward");
            }
            Sign::Aard => {
                // Aard staggers + (vs Nullglyph) interrupts the execute
                self.stagger_boss(5);
                if self.boss_in_arc(2) {
                    self.damage_boss(6, Some(Sign::Aard));
                } else if let Brain::Null(n) = &mut self.brain {
                    n.interrupt_with_aard();
                    self.say("Aard interrupts Nullglyph's execute.");
                }
            }
            Sign::Yrden => {
                self.slow = 60; // slow the boss
                self.say("Yrden — the boss is slowed");
            }
            Sign::Axii => {
                // no adds in a duel — Axii instead briefly dazes (a soft stagger)
                self.stagger_boss(3);
                self.say("Axii rattles the boss for a moment");
            }
        }
    }

    // ── the world tick ───────────────────────────────────────────────
    pub fn tick(&mut self) {
        self.tick += 1;
        let p = &mut self.player;
        for t in [&mut p.iframes, &mut p.busy, &mut p.guard, &mut p.move_cd, &mut p.light_cd, &mut p.combo_decay] {
            if *t > 0 {
                *t -= 1;
            }
        }
        if p.combo_decay == 0 {
            p.combo = 0;
        }
        if self.slow > 0 {
            self.slow -= 1;
        }
        let audit_replay = if let Brain::Audit(a) = &mut self.brain {
            a.tick();
            if a.buffer_full() && a.replay_cd == 0 && self.phase == BPhase::Stalk {
                a.arm_replay();
                let dmg = a.replay_damage();
                self.phase = BPhase::Windup { left: 4, reach: 2, dmg };
                Some(format!("⚠ AUDITHOLLOW REPLAYS your buffer {:?} — incoming!", a.buffer))
            } else {
                None
            }
        } else {
            None
        };
        if let Some(m) = audit_replay {
            self.say(m);
        }
        if let Brain::Pack(p) = &mut self.brain {
            p.tick();
        }
        if let Brain::Rotmaw(_) = &self.brain {
            let near = self.boss.reach(self.player.pos) <= 1;
            let on_hazard = self.player.pos == self.hazard_anchor;
            if near && self.tick % 20 == 0 {
                self.player_rot = (self.player_rot + 1).min(15);
                if self.player_rot > 0 {
                    self.say(format!("ROT +1 (now {}) — dive hazard to cleanse", self.player_rot));
                }
            }
            if on_hazard {
                self.rot_cleanse_ticks += 1;
                self.player.hp = (self.player.hp - 4).max(1);
                if self.rot_cleanse_ticks >= 40 {
                    self.player_rot = self.player_rot.saturating_sub(5);
                    self.rot_cleanse_ticks = 0;
                    self.say("hazard cleanse — rot -5 (cost 4 hp/tick while bathing)");
                }
            } else {
                self.rot_cleanse_ticks = 0;
            }
            if self.player_rot >= 10 && self.tick % 10 == 0 {
                self.player.hp -= 6;
                self.say("ROT ≥10 — plague ticks for 6");
            }
        }

        if let Brain::Sediment(s) = &mut self.brain {
            s.tick(); // decay stagger + rotate core across phases
        }

        // Nullglyph drains while the player is in range
        let in_range = self.boss.reach(self.player.pos) <= 2;
        if let Brain::Null(n) = &mut self.brain {
            if in_range {
                n.tick_in_range(&mut self.player.sigil);
            }
        }

        // Cæsura: the rewind clock. If a window closes unanchored, the boss heals back its wounds.
        let caesura_rewind = if let Brain::Caesura(c) = &mut self.brain {
            let rewound = c.tick();
            if rewound {
                Some(format!(
                    "⏪ CÆSURA REWIND — HP back to {} (you failed to anchor)",
                    c.checkpoint
                ))
            } else {
                None
            }
        } else {
            None
        };
        if let Some(m) = caesura_rewind {
            self.say(m);
            let bark = self.dir.event(Archetype::Caesura, Bark::FeedsTrap, self.tick);
            self.say(bark);
        }

        // boss offense state machine
        let bpos = self.boss;
        let ppos = self.player.pos;
        let reach_now = bpos.reach(ppos);
        match self.phase {
            BPhase::Stagger { left } => {
                self.phase = if left <= 1 { BPhase::Stalk } else { BPhase::Stagger { left: left - 1 } };
            }
            BPhase::Recover { left } => {
                self.phase = if left <= 1 { BPhase::Stalk } else { BPhase::Recover { left: left - 1 } };
            }
            BPhase::Windup { left, reach, dmg } => {
                if left <= 1 {
                    self.resolve_boss_strike(reach, dmg);
                    self.phase = BPhase::Recover { left: 6 };
                } else {
                    self.phase = BPhase::Windup { left: left - 1, reach, dmg };
                }
            }
            BPhase::Stalk => {
                // Sediment doesn't attack while staggered/open (its stagger == your window)
                let stunned = matches!(&self.brain, Brain::Sediment(s) if s.stagger > 0);
                if !stunned && reach_now <= 2 {
                    let (reach, dmg, tell) = self.boss_attack_profile();
                    self.phase = BPhase::Windup { left: tell, reach, dmg };
                    self.say(format!("⚠ {} rears to strike!", self.brain.archetype().name()));
                } else if self.move_cd > 0 {
                    self.move_cd -= 1;
                } else if reach_now > 1 {
                    // chase
                    let best = STEPS8
                        .iter()
                        .map(|&(dx, dy)| bpos.add(V3::new(dx, dy, 0)))
                        .filter(|&q| self.world.walkable(q) && q != ppos)
                        .min_by_key(|&q| q.reach(ppos));
                    if let Some(q) = best {
                        self.boss = q;
                    }
                    self.move_cd = if self.slow > 0 { 3 } else { 1 };
                }
            }
        }

        // regen
        self.player.stamina = (self.player.stamina + 3).min(self.player.max_stamina);
        if self.tick % 3 == 0 {
            self.player.sigil = (self.player.sigil + 1).min(self.player.max_sigil);
        }

        // blood ages each tick
        self.blood.tick();

        // dialog: phase-shift + low-HP barks
        let arch = self.brain.archetype();
        let frac = (self.brain.hp() as f64 / self.brain.max_hp().max(1) as f64).clamp(0.0, 1.0);
        let phase = ((1.0 - frac) * 4.0) as usize;
        if self.brain.alive() && self.player.alive() {
            if let Some(bark) = self.dir.on_state(arch, phase, frac, self.tick) {
                self.say(bark);
            }
        }

        // win / loss — gore + a final bark
        if !self.brain.alive() && !self.won {
            self.won = true;
            let line = self.blood.gib(self.boss);
            self.say(format!("✦ {} falls — {line}", arch.name()));
            let bark = self.dir.event(arch, Bark::Dies, self.tick);
            self.say(bark);
        }
        if !self.player.alive() && !self.dead {
            self.dead = true;
            self.blood.gib(self.player.pos);
            let bark = self.dir.event(arch, Bark::KillsPlayer, self.tick);
            self.say(bark);
        }
    }

    fn boss_attack_profile(&self) -> (i32, i32, u32) {
        // (reach, dmg, windup-ticks) — telegraph length scales with the hit
        match self.brain {
            Brain::Audit(_) => (1, 8, 5),
            Brain::Pack(_) => (2, 10, 5),
            Brain::Rotmaw(_) => (2, 12, 7),
            Brain::Prism(_) => (2, 12, 6),
            Brain::Sediment(_) => (2, 18, 8),
            Brain::Null(_) => (1, 10, 5),
            Brain::Caesura(_) => (2, 14, 6),
        }
    }

    fn resolve_boss_strike(&mut self, reach: i32, dmg: i32) {
        if self.boss.reach(self.player.pos) > reach {
            self.say(format!("{} swings wide — you slipped it", self.brain.archetype().name()));
            return;
        }
        if self.player.iframes > 0 {
            self.player.perfect_dodges += 1;
            self.say("✸ you DODGE the blow!");
            return;
        }
        // parry: guarding + facing the boss → negate + stagger + riposte
        let facing_it = {
            let v = V3::new(self.boss.x - self.player.pos.x, self.boss.y - self.player.pos.y, 0);
            v.x * self.player.face.dx + v.y * self.player.face.dy >= 0
        };
        if self.player.guard > 0 && facing_it {
            self.player.parries += 1;
            self.player.riposte = true;
            self.player.guard = 0;
            let arch = self.brain.archetype();
            self.stagger_boss(8);
            if let Brain::Caesura(c) = &mut self.brain {
                c.anchor();
                self.say("⚓ ANCHOR — Cæsura cannot rewind this window's damage");
            }
            self.say("⚔ PARRY! the boss reels — riposte + window open");
            // Cæsura: a parry ANCHORS this window — the damage you've dealt becomes permanent.
            let mut anchored = false;
            if let Brain::Caesura(c) = &mut self.brain {
                c.anchor();
                anchored = true;
            }
            if anchored {
                self.say("⚓ ANCHORED — Cæsura cannot rewind this window's wounds");
            }
            let bark = self.dir.event(arch, Bark::GotParried, self.tick);
            self.say(bark);
            return;
        }
        // Nullglyph's execute: lethal at 0 Sigil unless interrupted
        if let Brain::Null(n) = &self.brain {
            if n.resolve_strike() {
                self.player.hp = 0;
                self.blood.gib(self.player.pos);
                self.say("☠ NULLGLYPH EXECUTES YOU — Sigil hit zero. (Aard at the threshold next time.)");
                return;
            }
        }
        let absorbed = dmg.min(self.player.shield);
        self.player.shield -= absorbed;
        let through = dmg - absorbed;
        self.player.hp -= through;
        if through > 0 {
            self.blood.spill(self.player.pos, through);
        }
        self.say(format!(
            "the {} hits you for {through}{}",
            self.brain.archetype().name(),
            if absorbed > 0 { format!(" ({absorbed} warded)") } else { String::new() }
        ));
    }

    // ── HUD: the boss-specific status line (tells you how to win) ─────
    pub fn status_line(&self) -> String {
        match &self.brain {
            Brain::Audit(a) => format!(
                "AUDITHOLLOW · buffer {:?} · spam {:?}×{}  · {}",
                a.buffer,
                a.spam_tag,
                a.spam_count,
                if a.buffer_full() { "REPLAY ARMED" } else { "recording moves" }
            ),
            Brain::Prism(c) => format!(
                "PRISM CZAR · ATTUNED: {} ← don't cast it (it heals)  · phase {}",
                c.attuned.name(),
                c.phase() + 1
            ),
            Brain::Pack(p) => format!(
                "PACK · total HP {} · enrage {} · next target {:?} · {}",
                p.total_hp(),
                p.enrage,
                p.valid_target(),
                if p.alpha_axii { "ALPHA FLIPPED (Axii)" } else { "kill leaves→mids→Alpha" }
            ),
            Brain::Sediment(s) => format!(
                "SEDIMENT · CORE FACE {}  · {}",
                s.core.arrow(),
                if s.stagger > 0 {
                    format!("STAGGERED {} — strike the core NOW", s.stagger)
                } else {
                    "ARMORED — heavy/Aard/parry to open it".into()
                }
            ),
            Brain::Rotmaw(r) => format!(
                "ROTMAW · ROT stacks {}  · cleanse in hazard @({}, {})  · boss HP {}/{}",
                self.player_rot,
                self.hazard_anchor.x,
                self.hazard_anchor.y,
                r.hp,
                r.max_hp
            ),
            Brain::Null(n) => format!(
                "NULLGLYPH · SIGIL DRAIN {}/t  · {}",
                n.drain,
                if n.execute_armed { "⚠ EXECUTE ARMED — Aard NOW or die".into() } else { "win with steel, bank Sigil".to_string() }
            ),
            Brain::Caesura(c) => format!(
                "CÆSURA · window {} ticks · checkpoint {} · {}",
                c.window,
                c.checkpoint,
                if c.anchored { "ANCHORED this window" } else { "PARRY to anchor before rewind" }
            ),
        }
    }

    /// Compact boss-fight snapshot for agents / spectator.
    /// Deterministic teach/autoplay driver — shows each boss solve (AW-C02).
    pub fn teach_action(&mut self) -> Option<crate::rt::Action> {
        use crate::rt::{Action, Dir, Sign};
        let arch = self.brain.archetype();
        // face boss
        let dx = (self.boss.x - self.player.pos.x).signum();
        let dy = (self.boss.y - self.player.pos.y).signum();
        if dx != 0 || dy != 0 {
            self.player.face = Dir::new(dx, dy);
        }
        if self.player.busy > 0 || self.player.move_cd > 0 {
            return None;
        }
        let near = self.boss.reach(self.player.pos) <= 2;
        if !near {
            return Some(Action::Move(Dir::new(dx.max(-1).min(1), dy.max(-1).min(1))));
        }
        match arch {
            Archetype::PrismCzar => {
                if let Brain::Prism(c) = &self.brain {
                    if self.player.sigil >= Sign::Aard.cost() && self.tick % 30 == 0 {
                        return Some(Action::Cast(Sign::Aard));
                    }
                    if self.player.stamina >= 30 {
                        return Some(Action::Heavy);
                    }
                }
                Some(Action::Light)
            }
            Archetype::Sediment => {
                if let Brain::Sediment(s) = &self.brain {
                    if s.stagger > 0 && self.player.stamina >= 6 {
                        return Some(Action::Light);
                    }
                }
                if self.player.stamina >= 30 {
                    return Some(Action::Heavy);
                }
                Some(Action::Light)
            }
            Archetype::Nullglyph => {
                if let Brain::Null(n) = &self.brain {
                    if n.execute_armed && self.player.sigil >= Sign::Aard.cost() {
                        return Some(Action::Cast(Sign::Aard));
                    }
                }
                if self.player.stamina >= 30 {
                    return Some(Action::Heavy);
                }
                Some(Action::Light)
            }
            Archetype::Audithollow => {
                if matches!(self.phase, BPhase::Windup { .. }) && self.player.stamina >= 25 {
                    return Some(Action::Dodge(self.player.face));
                }
                if self.player.guard == 0 && self.player.stamina >= 10 && self.tick % 40 == 0 {
                    return Some(Action::Parry);
                }
                let slot = (self.tick / 6) % 4;
                match slot {
                    0 => Some(Action::Light),
                    1 => Some(Action::Dodge(self.player.face)),
                    2 => Some(Action::Cast(Sign::Igni)),
                    _ => {
                        if self.player.stamina >= 30 {
                            Some(Action::Heavy)
                        } else {
                            Some(Action::Light)
                        }
                    }
                }
            }
            Archetype::PackOfNine => {
                if matches!(self.phase, BPhase::Windup { .. }) && self.player.stamina >= 25 {
                    return Some(Action::Dodge(self.player.face));
                }
                if self.player.hp < 55 && self.player.sigil >= Sign::Quen.cost() {
                    return Some(Action::Cast(Sign::Quen));
                }
                if self.player.stamina >= 30 && self.tick % 6 == 0 {
                    Some(Action::Heavy)
                } else if self.player.stamina >= 6 {
                    Some(Action::Light)
                } else {
                    None
                }
            }
            Archetype::Rotmaw => {
                if self.player.pos != self.hazard_anchor && self.player_rot >= 4 {
                    let hx = (self.hazard_anchor.x - self.player.pos.x).signum();
                    let hy = (self.hazard_anchor.y - self.player.pos.y).signum();
                    return Some(Action::Move(Dir::new(hx, hy)));
                }
                if self.player.stamina >= 30 {
                    Some(Action::Heavy)
                } else {
                    Some(Action::Light)
                }
            }
            Archetype::Caesura => {
                if let Brain::Caesura(c) = &self.brain {
                    if !c.anchored && c.hp < c.checkpoint && self.player.stamina >= 10 {
                        return Some(Action::Parry);
                    }
                }
                if self.player.stamina >= 30 {
                    Some(Action::Heavy)
                } else {
                    Some(Action::Light)
                }
            }
            _ => Some(Action::Light),
        }
    }

    pub fn teach_lesson(arch: Archetype) -> &'static str {
        match arch {
            Archetype::PrismCzar => "LESSON: steel + off-attune Signs hurt — NEVER cast its ATTUNED Sign (it heals).",
            Archetype::Sediment => "LESSON: heavy/Aard/parry STAGGERS — then strike the lit CORE face.",
            Archetype::Nullglyph => "LESSON: win with STEEL; Signs feed drain — save Aard for EXECUTE.",
            Archetype::Audithollow => "LESSON: vary moves (3 distinct) — dodge REPLAY windup, parry to survive.",
            Archetype::PackOfNine => "LESSON: kill LEAVES → mids → Alpha; wrong mid-before-leaves = REZ.",
            Archetype::Rotmaw => "LESSON: rot stacks in melee — bath in HAZARD (~) to cleanse, then burst.",
            Archetype::Caesura => "LESSON: burst damage then PARRY to ANCHOR — or rewind undoes it.",
            _ => "LESSON: read TRAP line and answer it.",
        }
    }

    
        pub fn boss_snapshot(&self) -> String {
        format!(
            "tick={} boss@({},{},{}) player@({},{},{}) hp={}/{} rot={} phase={:?} trap={}",
            self.tick,
            self.boss.x,
            self.boss.y,
            self.boss.z,
            self.player.pos.x,
            self.player.pos.y,
            self.player.pos.z,
            self.player.hp,
            self.player.max_hp,
            self.player_rot,
            self.phase,
            self.brain.archetype().trap(),
        )
    }
}

// ───────────────────────────── renderer ─────────────────────────────

fn bar(cur: i32, max: i32, width: usize, full: char) -> String {
    let max = max.max(1);
    let n = (((cur.max(0) as f64 / max as f64) * width as f64).round() as usize).min(width);
    (0..width).map(|i| if i < n { full } else { '·' }).collect()
}

/// Isometric render of the single-boss arena + the boss-specific HUD.
pub fn boss_render(g: &BossFight) -> String {
    let (w, h, d) = (g.world.w, g.world.h, g.world.d);
    let sw = ((w + h) * 2 + 4) as usize;
    let sh = ((w + h) + d * 2 + 4) as usize;
    let mut buf = vec![vec![' '; sw]; sh];
    // color overlay: 0 none · 1 fresh blood · 4 heavy viscera · 2 player · 3 boss · 5 boss-telegraph
    let mut col = vec![vec![0u8; sw]; sh];
    let proj = |v: V3| -> (usize, usize) {
        let sx = ((v.x - v.y) * 2 + (h * 2)) as usize;
        let sy = ((v.x + v.y) - v.z * 2 + d * 2) as usize;
        (sx.min(sw - 1), sy.min(sh - 1))
    };
    let mut cells: Vec<V3> = Vec::new();
    for z in 0..d {
        for y in 0..h {
            for x in 0..w {
                cells.push(V3::new(x, y, z));
            }
        }
    }
    cells.sort_by_key(|v| (v.z, v.x + v.y));
    for v in &cells {
        let ch = match g.world.at(*v) {
            Terrain::Floor => '.',
            Terrain::Wall => '#',
            Terrain::Ramp => '/',
            Terrain::Hazard => '~',
            Terrain::Gate => '0',
        };
        let (sx, sy) = proj(*v);
        buf[sy][sx] = ch;
    }
    // blood layer — painted over terrain, under the fighters
    for s in g.blood.cells() {
        let (sx, sy) = proj(s.pos);
        buf[sy][sx] = s.ch;
        col[sy][sx] = if s.heavy { 4 } else { 1 };
    }
    // boss glyph — UPPERCASE while telegraphing
    let telegraph = matches!(g.phase, BPhase::Windup { .. });
    let bg = g.brain.archetype().glyph();
    let bg = if telegraph { bg.to_ascii_uppercase() } else { bg };
    let (bsx, bsy) = proj(g.boss);
    buf[bsy][bsx] = bg;
    col[bsy][bsx] = if telegraph { 5 } else { 3 };
    let pg = if g.player.iframes > 0 { '*' } else { '@' };
    let (psx, psy) = proj(g.player.pos);
    buf[psy][psx] = pg;
    col[psy][psx] = 2;

    let mut out = String::new();
    out.push_str(&format!(
        "  ASHWALKER · BOSS · tick {} · facing {}{}\n",
        g.tick,
        g.player.face.arrow(),
        if telegraph { "  ⚠ INCOMING" } else { "" }
    ));
    let ansi = |c: u8| -> &'static str {
        match c {
            1 => gore::RED,
            4 => gore::DARK_RED,
            2 => "\x1b[38;5;51m",  // player — cyan
            3 => "\x1b[38;5;201m", // boss — magenta
            5 => "\x1b[1;38;5;196m", // boss telegraph — bold red
            _ => "",
        }
    };
    for (ri, row) in buf.iter().enumerate() {
        let plain: String = row.iter().collect();
        let trimmed = plain.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        let keep = trimmed.chars().count();
        let mut line = String::new();
        let mut cur = 0u8;
        for (i, ch) in row.iter().enumerate().take(keep) {
            let c = col[ri][i];
            if c != cur {
                line.push_str(gore::RESET);
                if c != 0 {
                    line.push_str(ansi(c));
                }
                cur = c;
            }
            line.push(*ch);
        }
        if cur != 0 {
            line.push_str(gore::RESET);
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str(&format!("  {}BOSS{} [{}] {}/{}\n", gore::CRIMSON, gore::RESET, bar(g.brain.hp(), g.brain.max_hp(), 22, '#'), g.brain.hp().max(0), g.brain.max_hp()));
    out.push_str(&format!("  {}\n", g.status_line()));
    out.push_str(&format!(
        "  HP [{}]  STA [{}]  SIG [{}]  ward {}\n",
        bar(g.player.hp, g.player.max_hp, 14, '#'),
        bar(g.player.stamina, g.player.max_stamina, 12, '='),
        bar(g.player.sigil, g.player.max_sigil, 10, '*'),
        g.player.shield
    ));
    out
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn adj(g: &mut BossFight) {
        // stand the player just west of the boss, facing east
        g.player.pos = V3::new(g.boss.x - 1, g.boss.y, 0);
        g.player.face = Dir::new(1, 0);
        g.player.busy = 0;
        g.player.light_cd = 0;
    }

    #[test]
    fn prism_attuned_sign_heals_offsign_damages() {
        let mut g = BossFight::new(Archetype::PrismCzar);
        adj(&mut g);
        if let Brain::Prism(c) = &mut g.brain {
            c.hp = 100; // still phase 0 (>75% → attuned Igni), with room to heal
        }
        let hp0 = g.brain.hp();
        g.do_cast(Sign::Igni); // phase 0 attuned == Igni → heals
        assert!(g.brain.hp() >= hp0, "feeding the attuned Sign healed it");
        g.player.busy = 0;
        let hp1 = g.brain.hp();
        g.do_cast(Sign::Aard); // off-attune → damages (Aard also staggers)
        assert!(g.brain.hp() < hp1, "off-attune Sign damaged it");
    }

    #[test]
    fn sediment_needs_stagger_and_core_face() {
        let mut g = BossFight::new(Archetype::Sediment);
        // approach from the lit core face
        let core = if let Brain::Sediment(s) = &g.brain { s.core } else { Dir::S };
        g.player.pos = V3::new(g.boss.x + core.dx, g.boss.y + core.dy, 0);
        g.player.face = Dir::new(-core.dx, -core.dy); // face the boss
        g.player.busy = 0;
        g.player.light_cd = 0;
        let hp0 = g.brain.hp();
        g.do_light(); // not staggered → CLANG, no damage
        assert_eq!(g.brain.hp(), hp0, "armored without a stagger");
        // open the window, then a light on the core face lands
        g.stagger_boss(6);
        g.player.busy = 0;
        g.player.light_cd = 0;
        g.do_light();
        assert!(g.brain.hp() < hp0, "core cracked while staggered + on the lit face");
    }

    #[test]
    fn nullglyph_drains_and_aard_saves_you() {
        let mut g = BossFight::new(Archetype::Nullglyph);
        g.player.pos = V3::new(g.boss.x - 1, g.boss.y, 0);
        g.player.sigil = 6;
        for _ in 0..3 {
            g.tick(); // drains 3/t in range → hits 0, arms execute
        }
        let armed = matches!(&g.brain, Brain::Null(n) if n.execute_armed);
        assert!(armed, "execute armed at 0 Sigil");
        // the trick: Aard interrupts it (give sigil to afford the cast)
        g.player.sigil = 50;
        g.player.busy = 0;
        g.do_cast(Sign::Aard);
        let armed2 = matches!(&g.brain, Brain::Null(n) if n.execute_armed);
        assert!(!armed2, "Aard interrupted the execute");
    }

    #[test]
    fn prism_can_be_won_with_steel() {
        // Ignoring Signs entirely and just hitting it with light/heavy should kill the Czar.
        let mut g = BossFight::new(Archetype::PrismCzar);
        for _ in 0..400 {
            adj(&mut g);
            g.do_heavy();
            g.player.busy = 0;
            adj(&mut g);
            g.do_light();
            g.tick();
            if g.won {
                break;
            }
        }
        assert!(g.won, "the Czar dies to pure steel — Signs are a trap, not a requirement");
    }

    #[test]
    fn boss_telegraphs_before_it_hits() {
        let mut g = BossFight::new(Archetype::PrismCzar);
        g.player.pos = V3::new(g.boss.x - 1, g.boss.y, 0); // in reach
        let hp0 = g.player.hp;
        g.tick(); // should enter Windup, not hit yet
        assert!(matches!(g.phase, BPhase::Windup { .. }), "the boss tells first");
        assert_eq!(g.player.hp, hp0, "no damage during the tell");
    }

    #[test]
    fn render_carries_the_solve_hint() {
        let g = BossFight::new(Archetype::Sediment);
        let frame = boss_render(&g);
        assert!(frame.contains("BOSS"));
        assert!(frame.contains("CORE FACE"), "HUD tells you how to beat it");
    }
}
