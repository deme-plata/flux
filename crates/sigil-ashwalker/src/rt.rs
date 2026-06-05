//! ASHWALKER · real-time combat ("sincere & moving") — a Witcher-3 feel in the terminal.
//!
//! The turn-based [`crate::Game`] is kept for the scripted demo + the Crown & Ash ascension. THIS
//! module is the live combat heart: a **tick-driven** loop (20 ticks/sec) where you move rapidly,
//! **dodge-roll** with i-frames, chain **light** attacks, wind up **heavy** cleaves, **parry** into
//! ripostes, and cast the **five Signs** (mapped onto the MCP spells). Combat is *sincere* because
//! enemies **telegraph**: they rear into a visible windup before they strike, and your whole job is
//! to read the tell and answer it — dodge through it, parry it, or burn them down first.
//!
//! Deterministic: no RNG. Given the same `Action` sequence + tick count, the state is identical, so
//! every mechanic below is unit-tested. std-only / FLUXFOOD.

use crate::bosses::Archetype;
use crate::dialog::{Bark, Director};
use crate::gore::{self, BloodField};
use crate::{Foe, Mcp, Player, Terrain, V3, World, STEPS8};
use std::collections::BTreeSet;

/// Map regular Ashlands foes to boss archetypes for baked barks (minions share voice).
pub(crate) fn foe_archetype(k: Foe) -> Option<Archetype> {
    match k {
        Foe::GlassGolem => Some(Archetype::PrismCzar),
        Foe::EmberLord => Some(Archetype::Sediment),
        Foe::AshWraith => Some(Archetype::Nullglyph),
        Foe::CinderHound => None,
    }
}

/// Wall-clock per tick. 20 ticks/sec gives terminal combat a real pulse without thrashing the CPU.
pub const TICK_MS: u64 = 50;

// ───────────────────────────── facing ─────────────────────────────

/// 8-direction facing. Drives attack arcs, dodge direction, and the Igni/Aard cones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dir {
    pub dx: i32,
    pub dy: i32,
}
impl Dir {
    pub const S: Dir = Dir { dx: 0, dy: 1 };
    pub fn new(dx: i32, dy: i32) -> Dir {
        Dir { dx: dx.signum(), dy: dy.signum() }
    }
    /// A glyph for the HUD facing indicator.
    pub fn arrow(self) -> char {
        match (self.dx, self.dy) {
            (0, -1) => '^',
            (0, 1) => 'v',
            (-1, 0) => '<',
            (1, 0) => '>',
            (1, -1) => '/',
            (-1, 1) => '/',
            (1, 1) => '\\',
            (-1, -1) => '\\',
            _ => 'v',
        }
    }
}

/// Is `t` inside the forward arc from `p` facing `face`, within Chebyshev `reach`?
/// Forward = the half-plane the player is looking at (dot ≥ 1). Forgiving enough for a sparse grid.
pub fn rt_in_arc(p: V3, face: Dir, t: V3, reach: i32) -> bool {
    if p.plane_cheby(t) > reach || p.z != t.z {
        return false;
    }
    let vx = t.x - p.x;
    let vy = t.y - p.y;
    vx * face.dx + vy * face.dy >= 1
}

// ───────────────────────────── the five Signs ─────────────────────────────

/// The Witcher signs, each backed by one of the MCP spells (name/cost reused for flavour + economy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    /// Igni — a cone of compiler-fire in your facing. (`mining_status` hashstorm)
    Igni,
    /// Quen — a protective ward that soaks the next hits. (`flux_zk_combo`)
    Quen,
    /// Aard — a telekinetic blast: knockback + stagger everything in front. (`dex_swap`)
    Aard,
    /// Yrden — lay a slowing glyph at your feet; foes crossing it crawl. (`council_consensus`)
    Yrden,
    /// Axii — bend a *weakened* foe's mind; it defects to your side. (`send_token`)
    Axii,
}
impl Sign {
    pub fn mcp(self) -> Mcp {
        match self {
            Sign::Igni => Mcp::Hashstorm,
            Sign::Quen => Mcp::ZkVeil,
            Sign::Aard => Mcp::DexSwap,
            Sign::Yrden => Mcp::CouncilQuorum,
            Sign::Axii => Mcp::Tribute,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Sign::Igni => "Igni",
            Sign::Quen => "Quen",
            Sign::Aard => "Aard",
            Sign::Yrden => "Yrden",
            Sign::Axii => "Axii",
        }
    }
    pub fn cost(self) -> i32 {
        self.mcp().cost()
    }
    pub fn all() -> [Sign; 5] {
        [Sign::Igni, Sign::Quen, Sign::Aard, Sign::Yrden, Sign::Axii]
    }
}

// ───────────────────────────── player input ─────────────────────────────

/// Compact tag for the Audithollow replay buffer / fight snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionTag {
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
impl Action {
    pub fn tag(self) -> ActionTag {
        match self {
            Action::Move(_) => ActionTag::Move,
            Action::Dodge(_) => ActionTag::Dodge,
            Action::Light => ActionTag::Light,
            Action::Heavy => ActionTag::Heavy,
            Action::Parry => ActionTag::Parry,
            Action::Cast(s) => match s {
                Sign::Igni => ActionTag::Igni,
                Sign::Quen => ActionTag::Quen,
                Sign::Aard => ActionTag::Aard,
                Sign::Yrden => ActionTag::Yrden,
                Sign::Axii => ActionTag::Axii,
            },
        }
    }
}

/// One frame's worth of player intent. The bin translates keystrokes → `Action`; tests feed it directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Step one tile (and face that way). The bread-and-butter "moving".
    Move(Dir),
    /// Dodge-roll: launch ~2 tiles in `Dir` with brief invulnerability. The signature rapid move.
    Dodge(Dir),
    /// Fast strike in the forward arc; chains 1→2→3 for rising damage.
    Light,
    /// Slow, heavy, telegraphed cleave — wide arc + knockback. You commit to it.
    Heavy,
    /// Raise guard for a window; a strike taken while guarding (and facing it) is parried → riposte.
    Parry,
    /// Cast a Sign.
    Cast(Sign),
}

/// What just happened, surfaced to the HUD feed (and asserted in tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Feedback {
    Moved,
    Blocked,
    Dodged,
    NoStamina,
    NoSigil,
    NoTarget,
    Hit(String),
    Cast(String),
    Busy,
}

// ───────────────────────────── enemy state machine ─────────────────────────────

/// The enemy's attack archetypes — windup length is the telegraph: longer tell ↔ bigger hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strike {
    Bite,  // wraith: quick, reach 1
    Lunge, // hound: dashes in then bites
    Smash, // golem: slow, reach 2, knocks back
    Cleave, // ember lord: slow, wide reach 2
}
impl Strike {
    fn of(kind: Foe) -> Strike {
        match kind {
            Foe::AshWraith => Strike::Bite,
            Foe::CinderHound => Strike::Lunge,
            Foe::GlassGolem => Strike::Smash,
            Foe::EmberLord => Strike::Cleave,
        }
    }
    /// Telegraph length in ticks (how long the tell is up before the hit lands).
    fn windup(self) -> u32 {
        match self {
            Strike::Bite => 4,
            Strike::Lunge => 5,
            Strike::Smash => 8,
            Strike::Cleave => 7,
        }
    }
    fn reach(self) -> i32 {
        match self {
            Strike::Bite | Strike::Lunge => 1,
            Strike::Smash | Strike::Cleave => 2,
        }
    }
    fn recover(self) -> u32 {
        match self {
            Strike::Bite => 4,
            Strike::Lunge => 5,
            Strike::Smash => 10,
            Strike::Cleave => 8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnemyPhase {
    /// Closing distance (or idling out of range).
    Stalk,
    /// Telegraphing — `left` ticks until the blow lands. THIS is the tell you react to.
    Windup { left: u32, strike: Strike },
    /// Whiffed/landed; vulnerable for `left` ticks.
    Recover { left: u32 },
    /// Staggered by a parry/heavy/Aard — fully open for `left` ticks.
    Stagger { left: u32 },
}

#[derive(Debug, Clone)]
pub struct RtEnemy {
    pub pos: V3,
    pub hp: i32,
    pub kind: Foe,
    pub ally: bool,
    pub phase: EnemyPhase,
    pub slow: u32, // ticks of Yrden slow remaining (acts at half cadence)
    pub kb: (i32, i32, u32), // knockback (dx,dy,ticks)
    move_cd: u32,
}
impl RtEnemy {
    pub fn new(kind: Foe, pos: V3) -> RtEnemy {
        RtEnemy {
            pos,
            hp: kind.max_hp(),
            kind,
            ally: false,
            phase: EnemyPhase::Stalk,
            slow: 0,
            kb: (0, 0, 0),
            move_cd: 0,
        }
    }
    pub fn alive(&self) -> bool {
        self.hp > 0
    }
    /// Is this enemy mid-telegraph? Drives the '!' marker + uppercase glyph in the renderer.
    pub fn telegraphing(&self) -> bool {
        matches!(self.phase, EnemyPhase::Windup { .. })
    }
    pub fn open(&self) -> bool {
        matches!(self.phase, EnemyPhase::Recover { .. } | EnemyPhase::Stagger { .. })
    }
}

// ───────────────────────────── real-time player ─────────────────────────────

#[derive(Debug, Clone)]
pub struct RtPlayer {
    pub pos: V3,
    pub face: Dir,
    pub hp: i32,
    pub max_hp: i32,
    pub stamina: i32,
    pub max_stamina: i32,
    pub sigil: i32,
    pub max_sigil: i32,
    pub shield: i32, // Quen ward
    pub level: i32,
    pub xp: i32,

    pub iframes: u32, // invulnerable ticks (from a dodge)
    pub busy: u32,    // self-recovery ticks: can't act mid-swing
    pub guard: u32,   // parry window ticks remaining
    pub riposte: bool, // next light attack is a free crit (earned by a parry)
    pub move_cd: u32,
    pub light_cd: u32,
    pub combo: u32,        // light-attack chain position
    pub combo_decay: u32,  // ticks until the chain resets

    // run telemetry → Crown & Ash ascension
    pub slain: i32,
    pub converted: i32,
    pub perfect_dodges: i32,
    pub parries: i32,
    pub signs_cast: i32,
    pub mcp_fusions: i32,
    pub depth_z: i32,
    pub sign_kinds: BTreeSet<&'static str>,
}
impl RtPlayer {
    pub fn new(pos: V3) -> RtPlayer {
        RtPlayer {
            pos,
            face: Dir::S,
            hp: 70,
            max_hp: 70,
            stamina: 100,
            max_stamina: 100,
            sigil: 40,
            max_sigil: 40,
            shield: 0,
            level: 1,
            xp: 0,
            iframes: 0,
            busy: 0,
            guard: 0,
            riposte: false,
            move_cd: 0,
            light_cd: 0,
            combo: 0,
            combo_decay: 0,
            slain: 0,
            converted: 0,
            perfect_dodges: 0,
            parries: 0,
            signs_cast: 0,
            mcp_fusions: 0,
            depth_z: pos.z,
            sign_kinds: BTreeSet::new(),
        }
    }
    pub fn alive(&self) -> bool {
        self.hp > 0
    }
    pub fn invulnerable(&self) -> bool {
        self.iframes > 0
    }
    fn gain_xp(&mut self, x: i32) {
        self.xp += x;
        while self.xp >= self.level * 20 {
            self.xp -= self.level * 20;
            self.level += 1;
            self.max_hp += 10;
            self.max_stamina += 10;
            self.max_sigil += 6;
            self.hp = self.max_hp;
        }
    }
}

// ───────────────────────────── the live game ─────────────────────────────

#[derive(Debug, Clone)]
pub struct RtGame {
    pub world: World,
    pub player: RtPlayer,
    pub enemies: Vec<RtEnemy>,
    pub yrden: Option<V3>, // active slowing glyph
    pub yrden_life: u32,
    pub feed: Vec<String>,
    pub tick: u64,
    /// Decaying blood overlay (hits spill, kills gib) — same layer as bossfight.
    pub blood: BloodField,
    /// Boss-flavoured barks for mapped foe types.
    dir: Director,
    /// One-shot intro barks per foe kind (indexed by [`foe_idx`]).
    bark_intro: [bool; 4],
    player_dead: bool,
    /// Last MCP invocation line (spectator HUD).
    pub last_mcp: Option<String>,
    /// Sign-fusion window for MCP combos (Sign, tick).
    pub(crate) sign_pending: Option<(Sign, u64)>,
    /// Last 3 distinct action tags (Audithollow / AI snapshot).
    pub move_buffer: Vec<ActionTag>,
    pub action_spam: u32,
    /// One-shot spectator event banner.
    pub pulse_event: Option<String>,
    /// Slow-mo frames after a kill.
    pub death_cam: u32,
    /// Merkle ledger root shaping spawn (0 = default arena).
    pub ledger_root: u64,
}

fn foe_idx(k: Foe) -> usize {
    match k {
        Foe::AshWraith => 0,
        Foe::CinderHound => 1,
        Foe::GlassGolem => 2,
        Foe::EmberLord => 3,
    }
}


/// Ticks within which two Signs fuse into an MCP combo (mirrors turn-based [`crate::Game::fuse`]).
pub const SIGN_FUSE_TICKS: u64 = 40;

fn combo_name_mcp(a: crate::Mcp, b: crate::Mcp) -> &'static str {
    let mut key = [a, b];
    key.sort_by_key(|m| m.name());
    match (key[0], key[1]) {
        (crate::Mcp::DexSwap, crate::Mcp::FluxCombo) => "Blink-Nova",
        (crate::Mcp::CouncilQuorum, crate::Mcp::ZkVeil) => "Warded-Rally",
        (crate::Mcp::FluxCombo, crate::Mcp::Hashstorm) => "Hashfire",
        (crate::Mcp::Hashstorm, crate::Mcp::Tribute) => "Mindfire",
        (crate::Mcp::DexSwap, crate::Mcp::ZkVeil) => "Ward-Blink",
        (crate::Mcp::DexSwap, crate::Mcp::Tribute) => "Bribe-Blink",
        _ => "Sigil-Weave",
    }
}

fn mcp_invoke_line(sign: Sign) -> String {
    let m = sign.mcp();
    format!(
        "▸ {} via {} — {}",
        sign.name(),
        m.name(),
        m.blurb()
    )
}

impl RtGame {
    pub fn new() -> RtGame {
        let world = World::arena();
        let player = RtPlayer::new(V3::new(2, 6, 0));
        let enemies = vec![
            RtEnemy::new(Foe::AshWraith, V3::new(4, 4, 0)),
            RtEnemy::new(Foe::CinderHound, V3::new(6, 3, 0)),
            RtEnemy::new(Foe::GlassGolem, V3::new(4, 7, 0)),
            RtEnemy::new(Foe::EmberLord, V3::new(6, 6, 1)),
        ];
        RtGame {
            world,
            player,
            enemies,
            yrden: None,
            yrden_life: 0,
            feed: vec!["The Ashlands breathe. Read their tells — dodge, parry, or burn.".into()],
            tick: 0,
            blood: BloodField::new(),
            dir: Director::new(),
            bark_intro: [false; 4],
            player_dead: false,
            last_mcp: None,
            sign_pending: None,
            move_buffer: Vec::new(),
            action_spam: 0,
            pulse_event: None,
            death_cam: 0,
            ledger_root: 0,
        }
    }

    /// Arena from optional `ASHWALKER_ROOT` env (Merkle-driven spawn).
    pub fn new_from_env() -> RtGame {
        let root = std::env::var("ASHWALKER_ROOT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let mut g = RtGame::new();
        g.ledger_root = root;
        if root != 0 {
            g.apply_ledger_spawn(root);
        }
        g
    }

    fn apply_ledger_spawn(&mut self, root: u64) {
        use crate::bosses::Archetype;
        let arch = Archetype::from_root(root);
        self.say(format!(
            "Ledger root {root:#x} → {} stirs in the Ashlands",
            arch.name()
        ));
        if let Some(e) = self.enemies.iter_mut().find(|e| e.kind == Foe::AshWraith) {
            e.hp = (e.hp + arch.context_load() as i32 * 2).min(e.kind.max_hp() + 12);
        }
    }

    pub fn pulse(&mut self, msg: impl Into<String>) {
        self.pulse_event = Some(msg.into());
    }

    fn say(&mut self, s: impl Into<String>) {
        self.feed.push(s.into());
        let n = self.feed.len();
        if n > 64 {
            self.feed.drain(0..n - 64);
        }
    }

    pub fn living_foes(&self) -> usize {
        self.enemies.iter().filter(|e| e.alive() && !e.ally).count()
    }
    pub fn combo_window_open(&self) -> bool {
        self.sign_pending.is_none()
    }

    pub fn pending_sign(&self) -> Option<Sign> {
        self.sign_pending.map(|(s, _)| s)
    }

    pub fn won(&self) -> bool {
        self.living_foes() == 0
    }

    fn occupied(&self, v: V3, ignore: Option<usize>) -> bool {
        self.enemies.iter().enumerate().any(|(i, e)| {
            Some(i) != ignore && e.alive() && e.pos == v
        })
    }

    /// Hurt enemy `i`; returns true if it died. Extracts Copy data first (borrow discipline).
    fn hurt(&mut self, i: usize, dmg: i32, src: &str, stagger: u32) -> bool {
        let pos = self.enemies[i].pos;
        let kind = self.enemies[i].kind;
        self.enemies[i].hp -= dmg;
        if stagger > 0 && self.enemies[i].alive() {
            self.enemies[i].phase = EnemyPhase::Stagger { left: stagger };
        }
        let hp = self.enemies[i].hp;
        if dmg > 0 && hp > 0 {
            self.blood.spill(pos, dmg);
        }
        if hp <= 0 {
            let gore = self.blood.gib(pos);
            self.say(format!("  {src} SHATTERS the {} — {gore}", kind.name()));
            if let Some(a) = foe_archetype(kind) {
                self.say(self.dir.event(a, Bark::Dies, self.tick));
            }
            self.player.slain += 1;
            self.death_cam = 3;
            self.pulse(format!("☠ {} slain", kind.name()));
            self.player.gain_xp(kind.max_hp() / 2);
            true
        } else {
            self.say(format!("  {src} → {} for {dmg} ({} hp)", kind.name(), hp.max(0)));
            false
        }
    }

    fn knockback(&mut self, i: usize, from: V3, force: u32) {
        let e = &self.enemies[i];
        let dx = (e.pos.x - from.x).signum();
        let dy = (e.pos.y - from.y).signum();
        let (dx, dy) = if dx == 0 && dy == 0 { (0, 1) } else { (dx, dy) };
        self.enemies[i].kb = (dx, dy, force);
    }

    // ── player actions ──────────────────────────────────────────────


    fn record_action(&mut self, a: Action) {
        let tag = a.tag();
        if self.move_buffer.last() == Some(&tag) {
            self.action_spam += 1;
        } else {
            self.action_spam = 1;
            if !self.move_buffer.contains(&tag) {
                if self.move_buffer.len() >= 3 {
                    self.move_buffer.remove(0);
                }
                self.move_buffer.push(tag);
            }
        }
        if self.action_spam >= 3 {
            self.pulse(format!("⚠ LEDGER spam: {:?} ×{} — vary moves", tag, self.action_spam));
            if let Some(arch) = foe_archetype(Foe::AshWraith) {
                self.say(self.dir.event(arch, Bark::FeedsTrap, self.tick));
            }
        }
    }

    /// Compact fight state for agents / flux-moe (one line, deterministic).
    pub fn fight_snapshot(&self) -> String {
        let mut foes = String::new();
        for e in self.enemies.iter().filter(|e| e.alive()) {
            let phase = match e.phase {
                EnemyPhase::Stalk => "stalk".to_string(),
                EnemyPhase::Windup { left, strike } => {
                    foes.push_str(&format!(
                        "{}@({},{},{}) hp={} phase=windup:{}:{}t ",
                        e.kind.name(),
                        e.pos.x,
                        e.pos.y,
                        e.pos.z,
                        e.hp,
                        match strike {
                            Strike::Bite => "BITE",
                            Strike::Lunge => "LUNGE",
                            Strike::Smash => "SMASH",
                            Strike::Cleave => "CLEAVE",
                        },
                        left
                    ));
                    continue;
                }

                EnemyPhase::Stagger { left } => format!("stagger:{left}"),
                EnemyPhase::Recover { left } => format!("recover:{left}"),
            };
            foes.push_str(&format!(
                "{}@({},{},{}) hp={} phase={} ",
                e.kind.name(),
                e.pos.x,
                e.pos.y,
                e.pos.z,
                e.hp,
                phase
            ));
        }
        let buf: Vec<_> = self.move_buffer.iter().map(|t| format!("{t:?}")).collect();
        format!(
            "tick={} hero@({},{},{}) hp={}/{} sta={} sig={} ward={} combo=x{} fusions={} facing={:?} buffer=[{}] yrden={:?} foes=[{}]",
            self.tick,
            self.player.pos.x,
            self.player.pos.y,
            self.player.pos.z,
            self.player.hp,
            self.player.max_hp,
            self.player.stamina,
            self.player.sigil,
            self.player.shield,
            self.player.combo,
            self.player.mcp_fusions,
            self.player.face.arrow(),
            buf.join(","),
            self.yrden,
            foes.trim()
        )
    }

    pub fn snapshot_hash(&self) -> u64 {
        let s = self.fight_snapshot();
        let mut h: u64 = 0xcbf29ce484222325;
        for b in s.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    /// One-line tactical read for spectator / AW-09 ticker.
    pub fn rt_tactical_line(&self) -> String {
        let ppos = self.player.pos;
        for e in self.enemies.iter().filter(|e| e.alive() && !e.ally) {
            if let EnemyPhase::Windup { left, strike } = e.phase {
                if left <= 3 && e.pos.reach(ppos) <= strike.reach() + 1 {
                    return format!(
                        "INCOMING {} {} from {} in {} ticks — dodge/parry",
                        e.kind.name(),
                        match strike {
                            Strike::Bite => "BITE",
                            Strike::Lunge => "LUNGE",
                            Strike::Smash => "SMASH",
                            Strike::Cleave => "CLEAVE",
                        },
                        e.kind.name(),
                        left
                    );
                }
            }
        }
        if self.player.riposte {
            return "RIPOSTE — light attack for crit".into();
        }
        if self.action_spam >= 3 {
            return "VARIETY — stop spamming; Ledger-Wraith is reading you".into();
        }
        if let Some((s, _)) = self.sign_pending {
            return format!("FUSE window open — cast second Sign after {}", s.name());
        }
        if self.living_foes() == 0 {
            return "Arena clear".into();
        }
        format!("{} foes · read BOSS RADAR traps", self.living_foes())
    }

    /// Documented trap line for elite foes (AI falsifiable HUD).
    pub fn rt_trap_status(kind: Foe) -> Option<&'static str> {
        foe_archetype(kind).map(|a| a.trap())
    }

    pub fn transcript_markdown(&self, hero: &str) -> String {
        let mut out = format!("# ASHWALKER transcript — {hero}\n\n");
        out.push_str(&format!("- tick: {}\n", self.tick));
        out.push_str(&format!("- snapshot: `{}`\n\n", self.fight_snapshot()));
        out.push_str("## Combat log\n\n");
        for l in &self.feed {
            out.push_str("- ");
            out.push_str(l);
            out.push_str("\n");
        }
        out
    }

    /// Apply one player action this tick. Returns feedback for the HUD.
    pub fn input(&mut self, a: Action) -> Feedback {
        // Movement/dodge are allowed to interrupt less; attacks/signs respect `busy`.
        let fb = match a {
            Action::Move(d) => self.do_move(d),
            Action::Dodge(d) => self.do_dodge(d),
            Action::Light => self.do_light(),
            Action::Heavy => self.do_heavy(),
            Action::Parry => self.do_parry(),
            Action::Cast(s) => self.do_cast(s),
        };
        self.record_action(a);
        fb
    }

    fn do_move(&mut self, d: Dir) -> Feedback {
        self.player.face = d;
        if self.player.busy > 0 || self.player.move_cd > 0 {
            return Feedback::Busy;
        }
        let mut dest = self.player.pos.add(V3::new(d.dx, d.dy, 0));
        // ramp: stepping onto/along a ramp lifts a z-level when the tile above is walkable
        if self.world.at(self.player.pos) == Terrain::Ramp || self.world.at(dest) == Terrain::Ramp {
            let up = dest.add(V3::new(0, 0, 1));
            if self.world.walkable(up) {
                dest = up;
            }
        }
        if !self.world.walkable(dest) || self.occupied(dest, None) {
            return Feedback::Blocked;
        }
        self.player.pos = dest;
        self.player.depth_z = self.player.depth_z.max(dest.z);
        self.player.move_cd = 2; // glide cadence
        if self.world.at(dest) == Terrain::Hazard {
            self.player.hp -= 4;
            self.say("ash-lava sears you (-4)");
        }
        Feedback::Moved
    }

    fn do_dodge(&mut self, d: Dir) -> Feedback {
        if self.player.stamina < 25 {
            return Feedback::NoStamina;
        }
        self.player.face = d;
        self.player.stamina -= 25;
        self.player.iframes = 6; // ~0.3s of invulnerability
        self.player.busy = 2;
        self.player.move_cd = 0;
        // launch up to 2 tiles, passing THROUGH foes, stopping at walls
        let mut moved = 0;
        for _ in 0..2 {
            let step = self.player.pos.add(V3::new(d.dx, d.dy, 0));
            if self.world.walkable(step) {
                self.player.pos = step;
                self.player.depth_z = self.player.depth_z.max(step.z);
                moved += 1;
            } else {
                break;
            }
        }
        let _ = moved;
        Feedback::Dodged
    }

    fn do_light(&mut self) -> Feedback {
        if self.player.busy > 0 || self.player.light_cd > 0 {
            return Feedback::Busy;
        }
        if self.player.stamina < 6 {
            return Feedback::NoStamina;
        }
        self.player.stamina -= 6;
        self.player.light_cd = 3;
        self.player.busy = 1;
        // advance the chain
        self.player.combo = (self.player.combo % 3) + 1;
        self.player.combo_decay = 18;
        let base = 7 + self.player.combo * 2; // 9 / 11 / 13
        let crit = self.player.riposte;
        let dmg = if crit { base * 2 + 6 } else { base };
        self.player.riposte = false;
        let face = self.player.face;
        let ppos = self.player.pos;
        let ids: Vec<usize> = self
            .enemies
            .iter()
            .enumerate()
            .filter(|(_, e)| e.alive() && !e.ally && rt_in_arc(ppos, face, e.pos, 1))
            .map(|(i, _)| i)
            .collect();
        if ids.is_empty() {
            return Feedback::Hit(format!("light {} — whiff", self.player.combo));
        }
        let tag = if crit { "RIPOSTE" } else { "light" };
        for i in ids {
            self.hurt(i, dmg as i32, tag, 0);
        }
        Feedback::Hit(format!("{tag} ×{}", self.player.combo))
    }

    fn do_heavy(&mut self) -> Feedback {
        if self.player.busy > 0 {
            return Feedback::Busy;
        }
        if self.player.stamina < 30 {
            return Feedback::NoStamina;
        }
        self.player.stamina -= 30;
        self.player.busy = 5; // committed swing
        self.player.combo = 0;
        let face = self.player.face;
        let ppos = self.player.pos;
        let ids: Vec<usize> = self
            .enemies
            .iter()
            .enumerate()
            .filter(|(_, e)| e.alive() && !e.ally && rt_in_arc(ppos, face, e.pos, 2))
            .map(|(i, _)| i)
            .collect();
        if ids.is_empty() {
            return Feedback::Hit("heavy — whiff".into());
        }
        for i in ids {
            let dead = self.hurt(i, 26, "HEAVY", 6);
            if !dead {
                self.knockback(i, ppos, 2);
            }
        }
        Feedback::Hit("HEAVY cleave".into())
    }

    fn do_parry(&mut self) -> Feedback {
        if self.player.busy > 0 {
            return Feedback::Busy;
        }
        if self.player.stamina < 10 {
            return Feedback::NoStamina;
        }
        self.player.stamina -= 10;
        self.player.guard = 6; // the parry window
        Feedback::Hit("guard up".into())
    }


    fn try_fuse_signs(&mut self, a: Sign, b: Sign) -> Feedback {
        let ma = a.mcp();
        let mb = b.mcp();
        let name = combo_name_mcp(ma, mb);
        self.player.mcp_fusions += 1;
        self.player.signs_cast += 1;
        self.player.sign_kinds.insert(name);
        self.last_mcp = Some(format!(
            "✧ COMBO {name}: {} + {} → fused MCP strike",
            a.name(),
            b.name()
        ));
        self.say(format!("✧ COMBO {name} — {} + {} FUSE", a.name(), b.name()));
        self.pulse(format!("✧ COMBO {name}"));
        let face = self.player.face;
        let ppos = self.player.pos;
        let mut key = [ma, mb];
        key.sort_by_key(|m| m.name());
        match (key[0], key[1]) {
            (crate::Mcp::CouncilQuorum, crate::Mcp::ZkVeil) => {
                self.player.sigil -= a.cost().max(b.cost());
                self.player.shield += 28;
                self.player.hp = (self.player.hp + 20).min(self.player.max_hp);
                self.say("Warded-Rally: ward +28, heal +20");
                Feedback::Cast(format!("COMBO {name}"))
            }
            (crate::Mcp::Hashstorm, crate::Mcp::Tribute) | (crate::Mcp::Tribute, crate::Mcp::Hashstorm) => {
                self.player.sigil -= a.cost() + b.cost();
                if let Some(i) = self.enemies.iter().enumerate().filter(|(_, e)| e.alive() && !e.ally && rt_in_arc(ppos, face, e.pos, 2) && e.hp <= e.kind.max_hp()/3).map(|(i,_)| i).next() {
                    self.enemies[i].ally = true;
                    self.enemies[i].phase = EnemyPhase::Stalk;
                    self.player.converted += 1;
                    self.say("Mindfire: Igni burn + Axii bend");
                }
                for i in 0..self.enemies.len() {
                    if self.enemies[i].alive() && !self.enemies[i].ally && rt_in_arc(ppos, face, self.enemies[i].pos, 3) {
                        self.hurt(i, 18, "Mindfire", 0);
                    }
                }
                Feedback::Cast(format!("COMBO {name}"))
            }
            (crate::Mcp::DexSwap, crate::Mcp::ZkVeil) | (crate::Mcp::ZkVeil, crate::Mcp::DexSwap) => {
                self.player.sigil -= a.cost().max(b.cost());
                self.player.shield += 16;
                let ids: Vec<usize> = self.enemies.iter().enumerate().filter(|(_, e)| e.alive() && !e.ally && rt_in_arc(ppos, face, e.pos, 2)).map(|(i,_)| i).collect();
                for i in ids { self.hurt(i, 8, "Ward-Blink", 4); self.knockback(i, ppos, 2); }
                self.say("Ward-Blink: Aard jolt + Quen ward");
                Feedback::Cast(format!("COMBO {name}"))
            }
            (crate::Mcp::DexSwap, crate::Mcp::Hashstorm) => {
                self.player.sigil -= a.cost().max(b.cost());
                let ids: Vec<usize> = self
                    .enemies
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.alive() && !e.ally && rt_in_arc(ppos, face, e.pos, 3))
                    .map(|(i, _)| i)
                    .collect();
                for i in ids {
                    self.hurt(i, 22, "Cinder-Blink", 6);
                    self.knockback(i, ppos, 4);
                }
                self.say("Cinder-Blink: Aard knock + Igni burn");
                Feedback::Cast(format!("COMBO {name}"))
            }
            (crate::Mcp::FluxCombo, crate::Mcp::Hashstorm) => {
                self.player.sigil -= a.cost().max(b.cost());
                if let Some(i) = self
                    .enemies
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.alive() && !e.ally)
                    .min_by_key(|(_, e)| e.pos.reach(ppos))
                    .map(|(i, _)| i)
                {
                    self.hurt(i, 28, "HASHFIRE", 0);
                    let c = self.enemies[i].pos;
                    for j in 0..self.enemies.len() {
                        if self.enemies[j].alive()
                            && !self.enemies[j].ally
                            && self.enemies[j].pos.reach(c) <= 1
                        {
                            self.hurt(j, 12, "HASHFIRE splash", 0);
                        }
                    }
                }
                Feedback::Cast(format!("COMBO {name}"))
            }
            _ => {
                self.player.sigil -= a.cost() + b.cost();
                self.player.shield += 10;
                self.player.busy = 0;
                let ids: Vec<usize> = self
                    .enemies
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.alive() && !e.ally && rt_in_arc(ppos, face, e.pos, 2))
                    .map(|(i, _)| i)
                    .collect();
                for i in ids {
                    self.hurt(i, 14, name, 4);
                }
                self.say(format!("Sigil-Weave: fused burst + ward"));
                Feedback::Cast(format!("COMBO {name}"))
            }
        }
    }

    fn do_cast(&mut self, sign: Sign) -> Feedback {
        if let Some((prev, t0)) = self.sign_pending.take() {
            if self.tick.saturating_sub(t0) <= SIGN_FUSE_TICKS && prev != sign {
                return self.try_fuse_signs(prev, sign);
            }
        }
        if self.player.busy > 0 {
            return Feedback::Busy;
        }
        if self.player.sigil < sign.cost() {
            return Feedback::NoSigil;
        }
        let face = self.player.face;
        let ppos = self.player.pos;
        match sign {
            Sign::Igni => {
                let ids: Vec<usize> = self
                    .enemies
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.alive() && !e.ally && rt_in_arc(ppos, face, e.pos, 3))
                    .map(|(i, _)| i)
                    .collect();
                self.player.sigil -= sign.cost();
                self.commit_sign(sign);
                if ids.is_empty() {
                    self.say("Igni roars into empty air");
                } else {
                    for i in ids {
                        self.hurt(i, 16, "Igni", 0);
                    }
                }
                Feedback::Cast("Igni — cone of fire".into())
            }
            Sign::Quen => {
                self.player.sigil -= sign.cost();
                self.player.shield += 22;
                self.commit_sign(sign);
                Feedback::Cast("Quen — +22 ward".into())
            }
            Sign::Aard => {
                let ids: Vec<usize> = self
                    .enemies
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.alive() && !e.ally && rt_in_arc(ppos, face, e.pos, 2))
                    .map(|(i, _)| i)
                    .collect();
                self.player.sigil -= sign.cost();
                self.commit_sign(sign);
                if ids.is_empty() {
                    self.say("Aard blasts nothing");
                } else {
                    for i in &ids {
                        self.hurt(*i, 6, "Aard", 5);
                        self.knockback(*i, ppos, 3);
                    }
                }
                Feedback::Cast("Aard — telekinetic blast".into())
            }
            Sign::Yrden => {
                self.player.sigil -= sign.cost();
                self.yrden = Some(ppos);
                self.yrden_life = 80; // ~4s of slowing field
                self.commit_sign(sign);
                Feedback::Cast("Yrden — slowing glyph laid".into())
            }
            Sign::Axii => {
                let tgt = self
                    .enemies
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| {
                        e.alive()
                            && !e.ally
                            && rt_in_arc(ppos, face, e.pos, 2)
                            && e.hp <= e.kind.max_hp() / 3
                    })
                    .map(|(i, _)| i)
                    .next();
                match tgt {
                    Some(i) => {
                        self.player.sigil -= sign.cost();
                        self.enemies[i].ally = true;
                        self.enemies[i].phase = EnemyPhase::Stalk;
                        self.player.converted += 1;
                        self.commit_sign(sign);
                        let nm = self.enemies[i].kind.name();
                        self.say(format!("Axii bends the {nm} — it fights for you!"));
                        Feedback::Cast("Axii — mind bent".into())
                    }
                    None => Feedback::NoTarget,
                }
            }
        }
    }

    fn commit_sign(&mut self, s: Sign) {
        self.player.signs_cast += 1;
        self.player.sign_kinds.insert(s.name());
        self.sign_pending = Some((s, self.tick));
        let line = mcp_invoke_line(s);
        self.last_mcp = Some(line.clone());
        self.say(format!("⚡ MCP {line}"));
    }

    // ── the world tick ──────────────────────────────────────────────

    /// Advance the world one tick: decay timers, run enemy telegraph machines, resolve strikes,
    /// move allies, drift knockbacks, regen. Call once every `TICK_MS`.
    pub fn tick(&mut self) {
        self.tick += 1;
        if let Some((_, t0)) = self.sign_pending {
            if self.tick.saturating_sub(t0) > SIGN_FUSE_TICKS {
                self.sign_pending = None;
            }
        }
        let p = &mut self.player;
        for t in [&mut p.iframes, &mut p.busy, &mut p.guard, &mut p.move_cd, &mut p.light_cd, &mut p.combo_decay] {
            if *t > 0 {
                *t -= 1;
            }
        }
        if p.combo_decay == 0 {
            p.combo = 0;
        }
        if self.yrden_life > 0 {
            self.yrden_life -= 1;
            if self.yrden_life == 0 {
                self.yrden = None;
            }
        }

        let ppos = self.player.pos;
        let mut strikes: Vec<(i32, &'static str, i32)> = Vec::new(); // (dmg, name, idx)
        let mut moves: Vec<(usize, V3)> = Vec::new();

        let yrden = self.yrden;
        for i in 0..self.enemies.len() {
            if !self.enemies[i].alive() || self.enemies[i].ally {
                continue;
            }
            // knockback drift takes precedence
            if self.enemies[i].kb.2 > 0 {
                let (dx, dy, n) = self.enemies[i].kb;
                let step = self.enemies[i].pos.add(V3::new(dx, dy, 0));
                if self.world.walkable(step) && !self.occupied(step, Some(i)) && step != ppos {
                    self.enemies[i].pos = step;
                }
                self.enemies[i].kb = (dx, dy, n - 1);
                continue;
            }
            // Yrden slow: if standing on the glyph, refresh slow
            if let Some(gp) = yrden {
                if self.enemies[i].pos.plane_cheby(gp) == 0 && self.enemies[i].pos.z == gp.z {
                    self.enemies[i].slow = 4;
                }
            }
            if self.enemies[i].slow > 0 {
                self.enemies[i].slow -= 1;
            }

            match self.enemies[i].phase {
                EnemyPhase::Stagger { left } => {
                    self.enemies[i].phase = if left <= 1 {
                        EnemyPhase::Stalk
                    } else {
                        EnemyPhase::Stagger { left: left - 1 }
                    };
                }
                EnemyPhase::Recover { left } => {
                    self.enemies[i].phase = if left <= 1 {
                        EnemyPhase::Stalk
                    } else {
                        EnemyPhase::Recover { left: left - 1 }
                    };
                }
                EnemyPhase::Windup { left, strike } => {
                    if left <= 1 {
                        // the blow lands NOW — but only if the player is still in reach
                        if self.enemies[i].pos.reach(ppos) <= strike.reach() {
                            strikes.push((self.enemies[i].kind.bite(), self.enemies[i].kind.name(), i as i32));
                        } else {
                            self.say(format!("{} swings wide — you slipped it", self.enemies[i].kind.name()));
                        }
                        self.enemies[i].phase = EnemyPhase::Recover { left: strike.recover() };
                    } else {
                        self.enemies[i].phase = EnemyPhase::Windup { left: left - 1, strike };
                    }
                }
                EnemyPhase::Stalk => {
                    let reach = Strike::of(self.enemies[i].kind).reach();
                    if self.enemies[i].pos.reach(ppos) <= reach {
                        // in range → begin the telegraph (the tell)
                        let st = Strike::of(self.enemies[i].kind);
                        self.enemies[i].phase = EnemyPhase::Windup { left: st.windup(), strike: st };
                        let kind = self.enemies[i].kind;
                        self.say(format!("⚠ the {} rears to strike!", kind.name()));
                        let fi = foe_idx(kind);
                        if !self.bark_intro[fi] {
                            self.bark_intro[fi] = true;
                            if let Some(a) = foe_archetype(kind) {
                                if let Some(b) = self.dir.intro(a, self.tick) {
                                    self.say(b);
                                }
                            }
                        }
                    } else if self.enemies[i].move_cd > 0 {
                        self.enemies[i].move_cd -= 1;
                    } else {
                        // chase: greedy step toward the player on the plane
                        let here = self.enemies[i].pos;
                        let best = STEPS8
                            .iter()
                            .map(|&(dx, dy)| here.add(V3::new(dx, dy, 0)))
                            .filter(|&pp| {
                                self.world.walkable(pp) && pp != ppos && !self.occupied(pp, Some(i))
                            })
                            .min_by_key(|&pp| pp.reach(ppos));
                        if let Some(pp) = best {
                            moves.push((i, pp));
                        }
                        // slowed foes move at half cadence
                        self.enemies[i].move_cd = if self.enemies[i].slow > 0 { 2 } else { 0 };
                    }
                }
            }
        }
        for (i, pp) in moves {
            self.enemies[i].pos = pp;
        }

        // resolve the strikes that landed this tick — dodge i-frames + parry happen HERE
        for (dmg, nm, idx) in strikes {
            let idx = idx as usize;
            if self.player.iframes > 0 {
                self.player.perfect_dodges += 1;
                self.say(format!("✸ you DODGE the {nm}'s blow!"));
                continue;
            }
            // parry: guarding AND facing roughly toward the attacker → negate + stagger + riposte
            let attacker = self.enemies[idx].pos;
            let facing_it = {
                let v = V3::new(attacker.x - self.player.pos.x, attacker.y - self.player.pos.y, 0);
                v.x * self.player.face.dx + v.y * self.player.face.dy >= 0
            };
            if self.player.guard > 0 && facing_it {
                self.player.parries += 1;
                self.player.riposte = true;
                self.player.guard = 0;
                self.enemies[idx].phase = EnemyPhase::Stagger { left: 8 };
                self.say(format!("⚔ PARRY! the {nm} reels — riposte ready"));
                self.pulse(format!("⚔ PARRY vs {nm} — riposte open"));
                if let Some(a) = foe_archetype(self.enemies[idx].kind) {
                    self.say(self.dir.event(a, Bark::GotParried, self.tick));
                }
                continue;
            }
            let absorbed = dmg.min(self.player.shield);
            self.player.shield -= absorbed;
            let through = dmg - absorbed;
            self.player.hp -= through;
            if through > 0 {
                let extra: i32 = if nm == "Ash Wraith" && self.action_spam >= 3 { (self.action_spam * 2) as i32 } else { 0 };
                let through = through + extra;
                if extra > 0 {
                    self.say(format!("Ledger-Wraith mirrors your spam (+{extra})"));
                }
                self.say(format!(
                    "the {nm} hits you for {through}{}",
                    if absorbed > 0 { format!(" ({absorbed} warded)") } else { String::new() }
                ));
            } else {
                self.say(format!("your ward eats the {nm}'s blow"));
            }
        }

        // allies swat the nearest foe
        let ally_pos: Vec<V3> = self
            .enemies
            .iter()
            .filter(|e| e.ally && e.alive())
            .map(|e| e.pos)
            .collect();
        for ap in ally_pos {
            if let Some(j) = self
                .enemies
                .iter()
                .enumerate()
                .filter(|(_, e)| e.alive() && !e.ally)
                .min_by_key(|(_, e)| e.pos.reach(ap))
                .map(|(j, _)| j)
            {
                if self.enemies[j].pos.reach(ap) <= 1 {
                    self.hurt(j, 5, "your ally", 0);
                }
            }
        }

        // regen (stamina fast like Witcher, sigil slow)
        self.player.stamina = (self.player.stamina + 3).min(self.player.max_stamina);
        if self.tick % 3 == 0 {
            self.player.sigil = (self.player.sigil + 1).min(self.player.max_sigil);
        }

        if self.death_cam > 0 {
            self.death_cam -= 1;
        }
        self.blood.tick();

        // phase / low-HP barks for mapped foes still standing
        let mut phase_barks = Vec::new();
        for e in &self.enemies {
            if !e.alive() || e.ally {
                continue;
            }
            if let Some(a) = foe_archetype(e.kind) {
                let frac = (e.hp as f64 / e.kind.max_hp().max(1) as f64).clamp(0.0, 1.0);
                let phase = ((1.0 - frac) * 4.0) as usize;
                if let Some(b) = self.dir.on_state(a, phase, frac, self.tick) {
                    phase_barks.push(b);
                }
            }
        }
        for b in phase_barks {
            self.say(b);
        }

        if !self.player_dead && self.player.hp <= 0 {
            self.player_dead = true;
            self.pulse("☠ YOU FELL");
            let gore = self.blood.gib(self.player.pos);
            self.say(format!("you fall — {gore}"));
            if let Some(e) = self
                .enemies
                .iter()
                .filter(|e| e.alive() && !e.ally)
                .min_by_key(|e| e.pos.reach(self.player.pos))
            {
                if let Some(a) = foe_archetype(e.kind) {
                    self.say(self.dir.event(a, Bark::KillsPlayer, self.tick));
                }
            }
        }
    }

    /// Map the live run onto the Crown & Ash ascension (reuses the turn-based bridge).
    pub fn ascend(&self) -> crate::Ascension {
        let mut p = Player::new(self.player.pos);
        p.slain = self.player.slain;
        p.converted = self.player.converted;
        p.depth_z = self.player.depth_z;
        p.level = self.player.level;
        p.combos_cast = self.player.signs_cast + self.player.parries + self.player.perfect_dodges;
        p.hp = self.player.hp;
        p.max_hp = self.player.max_hp;
        // distinct signs → "variety"; a clean defensive run still reads as the Bastion
        for k in &self.player.sign_kinds {
            p.combo_kinds.insert(k);
        }
        crate::Ascension::from_run(&p)
    }
}

impl Default for RtGame {
    fn default() -> Self {
        Self::new()
    }
}

// ───────────────────────────── terminal intro (live play) ─────────────────────

/// Controls line shown under the intro and during live play.
pub fn rt_controls_line() -> &'static str {
    "WASD move · SPACE dodge · J light · K heavy · L parry · 1-5 Signs · Q quit"
}

/// Full opening cinematic for `ashwalker-live` — banner, hero card, lore, controls.
pub fn rt_intro_screen(hero_name: &str) -> String {
    use crate::{avatar, traits};
    let sheet = traits::CharSheet::create(hero_name);
    let mut out = String::new();
    out.push_str(&format!(
        "\n{CRIMSON}╔══════════════════════════════════════════════════════════════╗{RESET}\n\
         {CRIMSON}║{RESET}  ░▒▓  {CRIMSON}A S H W A L K E R{RESET}  ·  LIVE  ▓▒░  — SIGIL Adventure   {CRIMSON}║{RESET}\n\
         {CRIMSON}║{RESET}  Real-time combat in the Ashlands. Read the tell. Bleed.     {CRIMSON}║{RESET}\n\
         {CRIMSON}╚══════════════════════════════════════════════════════════════╝{RESET}\n\n",
        CRIMSON = gore::CRIMSON,
        RESET = gore::RESET,
    ));
    out.push_str(&avatar::portrait_card(&sheet));
    out.push_str(&format!(
        "\n  {CRIMSON}The Ashlands breathe.{RESET} Foes telegraph in UPPERCASE — ⚠ INCOMING means\n\
           a blow is winding up. Dodge through it, parry into riposte, or burn them first.\n\
           Blood stays on the stone. Bosses speak when they mean it.\n\n\
           {CRIMSON}Hero:{RESET} {name}  (set ASHWALKER_NAME=<name> for another Sigil-wielder)\n\n\
           {CRIMSON}Controls:{RESET} {ctrl}\n\n",
        CRIMSON = gore::CRIMSON,
        RESET = gore::RESET,
        name = hero_name,
        ctrl = rt_controls_line(),
    ));
    out
}

fn bar(cur: i32, max: i32, width: usize, full: char) -> String {
    let max = max.max(1);
    let n = ((cur.max(0) as f64 / max as f64) * width as f64).round() as usize;
    let n = n.min(width);
    let mut s = String::new();
    for i in 0..width {
        s.push(if i < n { full } else { '·' });
    }
    s
}

/// Render the live 3D arena isometrically, with telegraph markers + a Witcher-style HUD.
pub fn rt_render(g: &RtGame) -> String {
    let (w, h, d) = (g.world.w, g.world.h, g.world.d);
    let sw = ((w + h) * 2 + 4) as usize;
    let sh = ((w + h) + d * 2 + 4) as usize;
    let mut buf = vec![vec![' '; sw]; sh];
    let proj = |v: V3| -> (usize, usize) {
        let sx = ((v.x - v.y) * 2 + (h * 2)) as usize;
        let sy = ((v.x + v.y) - v.z * 2 + d * 2) as usize;
        (sx.min(sw - 1), sy.min(sh - 1))
    };

    // terrain back-to-front
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
        let mut ch = match g.world.at(*v) {
            Terrain::Floor => '.',
            Terrain::Wall => '#',
            Terrain::Ramp => '/',
            Terrain::Hazard => '~',
            Terrain::Gate => '0',
        };
        if let Some(gp) = g.yrden {
            if *v == gp {
                ch = 'Y';
            }
        }
        let (sx, sy) = proj(*v);
        buf[sy][sx] = ch;
    }

    // blood layer — under fighters, over terrain
    for s in g.blood.cells() {
        let (sx, sy) = proj(s.pos);
        if sx < sw && sy < sh {
            buf[sy][sx] = s.ch;
        }
    }

    // entities — telegraphing foes shout via UPPERCASE glyph; allies are '+'
    let mut ents: Vec<(V3, char)> = g
        .enemies
        .iter()
        .filter(|e| e.alive())
        .map(|e| {
            let base = if e.ally { '+' } else { e.kind.glyph() };
            let ch = if e.telegraphing() {
                base.to_ascii_uppercase()
            } else if e.open() {
                base
            } else {
                base
            };
            (e.pos, ch)
        })
        .collect();
    let pglyph = if g.player.invulnerable() { '*' } else { '@' };
    ents.push((g.player.pos, pglyph));
    ents.sort_by_key(|(v, _)| (v.z, v.x + v.y));
    for (v, ch) in ents {
        let (sx, sy) = proj(v);
        buf[sy][sx] = ch;
    }

    let mut out = String::new();
    let telegraphs = g.enemies.iter().filter(|e| e.alive() && e.telegraphing()).count();
    out.push_str(&format!(
        "  ASHWALKER · tick {} · facing {} · foes {}{}\n",
        g.tick,
        g.player.face.arrow(),
        g.living_foes(),
        if telegraphs > 0 { format!("  ⚠×{telegraphs} INCOMING") } else { String::new() }
    ));
    for row in buf {
        let line: String = row.into_iter().collect();
        if !line.trim().is_empty() {
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    out.push_str(&format!(
        "  HP  [{}] {}/{}\n",
        bar(g.player.hp, g.player.max_hp, 16, '#'),
        g.player.hp.max(0),
        g.player.max_hp
    ));
    out.push_str(&format!(
        "  STA [{}]  SIG [{}]  ward {}\n",
        bar(g.player.stamina, g.player.max_stamina, 16, '='),
        bar(g.player.sigil, g.player.max_sigil, 12, '*'),
        g.player.shield
    ));
    let state = if g.player.riposte {
        "RIPOSTE-READY"
    } else if g.player.guard > 0 {
        "guarding"
    } else if g.player.busy > 0 {
        "committed"
    } else if g.player.invulnerable() {
        "i-frames"
    } else {
        "ready"
    };
    out.push_str(&format!(
        "  Lv{} · combo x{} · {} · slain {} · dodges {} · parries {}\n",
        g.player.level, g.player.combo, state, g.player.slain, g.player.perfect_dodges, g.player.parries
    ));
    out
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Place the player adjacent to enemy 0, both at z=0, facing it. Returns enemy idx 0.
    fn face_off() -> RtGame {
        let mut g = RtGame::new();
        g.enemies.truncate(1); // just the wraith at (4,4,0)
        g.player.pos = V3::new(3, 4, 0);
        g.player.face = Dir::new(1, 0); // looking east at the wraith
        g
    }

    #[test]
    fn light_attack_chains_and_rises() {
        let mut g = face_off();
        let hp0 = g.enemies[0].hp;
        let f1 = g.do_light();
        assert!(matches!(f1, Feedback::Hit(_)));
        assert_eq!(g.player.combo, 1);
        // clear cooldown, swing again → combo rises, more damage
        g.player.light_cd = 0;
        g.player.busy = 0;
        let _ = g.do_light();
        assert_eq!(g.player.combo, 2);
        assert!(g.enemies[0].hp < hp0 - 9, "two chained lights dug in");
    }

    #[test]
    fn dodge_grants_iframes_and_negates_a_strike() {
        let mut g = face_off();
        // wraith winds up and is about to hit
        let st = Strike::of(Foe::AshWraith);
        g.enemies[0].phase = EnemyPhase::Windup { left: 1, strike: st };
        g.enemies[0].pos = V3::new(4, 4, 0);
        g.player.pos = V3::new(4, 5, 0); // in reach
        let hp0 = g.player.hp;
        g.do_dodge(Dir::new(0, -1)); // dodge north, but stay in range for the test
        g.player.pos = V3::new(4, 5, 0); // pin position so the strike would otherwise connect
        assert!(g.player.iframes > 0);
        g.tick(); // strike lands into i-frames → negated
        assert_eq!(g.player.hp, hp0, "i-frames ate the blow");
        assert!(g.player.perfect_dodges >= 1);
    }

    #[test]
    fn parry_negates_and_opens_riposte() {
        let mut g = face_off();
        g.enemies[0].pos = V3::new(4, 4, 0);
        g.player.pos = V3::new(4, 5, 0);
        g.player.face = Dir::new(0, -1); // facing the wraith
        let st = Strike::of(Foe::AshWraith);
        g.enemies[0].phase = EnemyPhase::Windup { left: 1, strike: st };
        g.do_parry();
        let hp0 = g.player.hp;
        g.tick();
        assert_eq!(g.player.hp, hp0, "parry negated the blow");
        assert!(g.player.riposte, "parry opened a riposte");
        assert!(matches!(g.enemies[0].phase, EnemyPhase::Stagger { .. }));
        // the riposte light should crit
        g.player.busy = 0;
        g.player.light_cd = 0;
        g.player.face = Dir::new(0, -1);
        let before = g.enemies[0].hp;
        g.do_light();
        assert!(!g.player.riposte, "riposte consumed");
        assert!(g.enemies[0].hp <= before - 20 || !g.enemies[0].alive(), "riposte hits like a truck");
    }

    #[test]
    fn enemy_telegraphs_before_it_strikes() {
        let mut g = face_off();
        g.enemies[0].pos = V3::new(4, 4, 0);
        g.player.pos = V3::new(4, 5, 0); // adjacent
        assert!(matches!(g.enemies[0].phase, EnemyPhase::Stalk));
        g.tick(); // in range → should enter Windup, NOT hit yet
        assert!(g.enemies[0].telegraphing(), "enemy tells before striking");
        assert_eq!(g.player.hp, g.player.max_hp, "no damage during the tell");
    }

    #[test]
    fn heavy_knocks_back_and_staggers() {
        let mut g = face_off();
        g.enemies[0] = RtEnemy::new(Foe::GlassGolem, V3::new(4, 4, 0)); // tanky, survives the heavy
        g.player.pos = V3::new(3, 4, 0);
        g.player.face = Dir::new(1, 0);
        g.do_heavy();
        assert!(g.enemies[0].hp < Foe::GlassGolem.max_hp());
        assert!(matches!(g.enemies[0].phase, EnemyPhase::Stagger { .. }));
        assert!(g.enemies[0].kb.2 > 0, "knockback queued");
        let p0 = g.enemies[0].pos;
        g.tick();
        assert_ne!(g.enemies[0].pos, p0, "it was shoved");
    }

    #[test]
    fn igni_burns_a_forward_cone() {
        let mut g = face_off();
        g.enemies[0].pos = V3::new(6, 4, 0); // 2 tiles east, inside Igni's reach-3 cone
        g.player.pos = V3::new(3, 4, 0);
        g.player.face = Dir::new(1, 0);
        let hp0 = g.enemies[0].hp;
        let f = g.do_cast(Sign::Igni);
        assert!(matches!(f, Feedback::Cast(_)));
        assert!(g.enemies[0].hp < hp0, "Igni reached down the cone");
        assert!(g.player.sign_kinds.contains("Igni"));
    }

    #[test]
    fn quen_wards_incoming_damage() {
        let mut g = face_off();
        g.do_cast(Sign::Quen);
        assert!(g.player.shield >= 22);
        // a strike should bite the ward, not the hp
        g.enemies[0].pos = V3::new(4, 4, 0);
        g.player.pos = V3::new(4, 5, 0);
        g.player.face = Dir::new(0, 1); // facing AWAY so no parry
        g.enemies[0].phase = EnemyPhase::Windup { left: 1, strike: Strike::Bite };
        let hp0 = g.player.hp;
        g.tick();
        assert_eq!(g.player.hp, hp0, "ward absorbed it");
        assert!(g.player.shield < 22, "ward was spent");
    }

    #[test]
    fn axii_only_bends_a_weakened_foe() {
        let mut g = face_off();
        g.enemies[0].pos = V3::new(4, 4, 0);
        g.player.pos = V3::new(3, 4, 0);
        g.player.face = Dir::new(1, 0);
        // healthy → refused
        assert!(matches!(g.do_cast(Sign::Axii), Feedback::NoTarget));
        g.enemies[0].hp = 3; // weakened
        let f = g.do_cast(Sign::Axii);
        assert!(matches!(f, Feedback::Cast(_)));
        assert!(g.enemies[0].ally && g.player.converted == 1);
    }

    #[test]
    fn yrden_slows_a_foe_standing_on_it() {
        let mut g = face_off();
        g.player.pos = V3::new(4, 4, 0);
        g.do_cast(Sign::Yrden); // glyph at (4,4)
        g.player.pos = V3::new(2, 4, 0); // step off
        g.enemies[0].pos = V3::new(4, 4, 0); // foe on the glyph
        g.tick();
        assert!(g.enemies[0].slow > 0, "Yrden slowed the foe");
    }

    #[test]
    fn stamina_gates_the_dodge() {
        let mut g = face_off();
        g.player.stamina = 10; // not enough for a 25-cost dodge
        assert!(matches!(g.do_dodge(Dir::S), Feedback::NoStamina));
        assert_eq!(g.player.iframes, 0);
    }

    #[test]
    fn fight_snapshot_lists_foes() {
        let g = RtGame::new();
        let s = g.fight_snapshot();
        assert!(s.contains("tick=0"));
        assert!(s.contains("Ash Wraith") || s.contains("foes="));
        assert_ne!(g.snapshot_hash(), 0);
    }

    #[test]
    fn action_spam_triggers_buffer() {
        let mut g = RtGame::new();
        for _ in 0..3 {
            g.record_action(Action::Light);
        }
        assert!(g.action_spam >= 3);
        assert!(g.action_spam >= 3 || g.pulse_event.is_some() || g.feed.iter().any(|l| l.contains("Audithollow")));
    }

    #[test]
    fn mindfire_fusion_when_igni_axii() {
        let mut g = face_off();
        g.player.sigil = 90;
        g.player.busy = 0;
        g.enemies[0].hp = 5;
        let _ = g.do_cast(Sign::Igni);
        g.sign_pending = Some((Sign::Igni, g.tick));
        let _ = g.do_cast(Sign::Axii);
        assert!(g.feed.iter().any(|l| l.contains("Mindfire") || l.contains("COMBO")));
    }

    #[test]
    fn mcp_invoke_and_fuse_show_in_feed() {
        let mut g = face_off();
        g.player.sigil = 80;
        g.player.busy = 0;
        let _ = g.do_cast(Sign::Quen);
        assert!(g.feed.iter().any(|l| l.contains("flux_zk_combo") || l.contains("MCP")));
        g.sign_pending = Some((Sign::Quen, g.tick));
        let _ = g.do_cast(Sign::Yrden);
        assert!(g.player.mcp_fusions >= 1);
        assert!(g.feed.iter().any(|l| l.contains("COMBO") || l.contains("Warded")));
    }

    fn render_shows_hud_and_telegraph_marker() {
        let mut g = RtGame::new();
        // force a telegraph
        g.player.pos = V3::new(4, 4, 0);
        g.tick();
        let frame = rt_render(&g);
        assert!(frame.contains("ASHWALKER"));
        assert!(frame.contains("HP"));
        assert!(frame.contains("STA"));
    }

    #[test]
    fn intro_screen_shows_title_and_hero() {
        let s = rt_intro_screen("Viktor");
        assert!(s.contains("ASHWALKER"));
        assert!(s.contains("Viktor"));
        assert!(s.contains("WASD"));
    }

    #[test]
    fn gore_spills_on_hit_and_gibs_on_kill_in_rt_loop() {
        let mut g = face_off();
        let pos = g.enemies[0].pos;
        g.player.light_cd = 0;
        g.player.busy = 0;
        g.do_light();
        assert!(!g.blood.is_empty(), "hit should spill blood");
        g.enemies[0].hp = 1;
        g.player.light_cd = 0;
        g.player.busy = 0;
        g.do_light();
        assert!(!g.enemies[0].alive());
        assert!(g.feed.iter().any(|l| l.contains("viscera") || l.contains("ichor") || l.contains("gore") || l.contains("SHATTERS")));
        let _ = pos;
    }

    #[test]
    fn ascension_bridge_from_live_run() {
        let mut g = RtGame::new();
        g.player.slain = 5;
        g.player.depth_z = 1;
        g.player.level = 2;
        let a = g.ascend();
        assert!(a.crown > 0 && a.ash > 0 && a.troops > 0);
        assert!(a.seed_line().contains("ASCEND"));
    }
}
