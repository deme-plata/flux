//! ASHWALKER — a SIGIL Adventure (prototype 1).
//!
//! A terminal isometric ARPG. A Sigil-wielder moves in **3D** through the Ashlands (x,y,z — z is
//! real elevation, reached via ramps, Diablo-style oblique view), casts **MCP-combo** spells to
//! break enemies, and on victory **ascends into a House of Crown & Ash** — the run's stats decide
//! which House + title + starting realm resources they carry into the strategy game.
//!
//! This crate is the pure engine + an isometric ASCII renderer; the `ashwalker` bin runs a playable
//! scripted demo. FLUXFOOD: std-only, zero deps, fully unit-testable.

use std::collections::BTreeSet;

/// Real-time Witcher-3-style combat (dodge/light/heavy/parry + the five Signs + enemy telegraphs).
/// The turn-based [`Game`] below stays for the scripted demo + ascension; `rt` is the live heart.
pub mod rt;
pub mod spectator;
pub mod campaign;
pub mod bestiary;
/// Character creator — deterministic unique heroes (Origin + 2 Traits + palette) from a name-seed.
pub mod traits;
/// Rich-text terminal avatar — 24-bit ANSI half-block portrait from a [`traits::CharSheet`].
pub mod avatar;
/// Merkle-tree boss generator — decisions (both games) → root → unique boss + proofs (P2).
pub mod merkle;
/// Prototype 3 — The Ten Crowns of Ash: roster of mechanically-distinct bosses + reference brains.
pub mod bosses;
/// Live single-boss arena — wires the boss brains into the real-time kit (ashwalker-boss bin).
pub mod bossfight;
/// Blood & gore overlay — decaying splatter particles + dismemberment flavour (red ANSI).
pub mod gore;
/// Boss barks — grim one-liners authored by DeepSeek-V4, baked as static data (offline at runtime).
pub mod dialog;
/// Item tiers, sets, store/wild/boss-drop loot, and gear-gated boss difficulty.
pub mod item;
/// Pets (combat companions) + mounts (faster travel, hazard-crossing).
pub mod companion;
/// Multiplayer co-op protocol (std-only) behind a Transport trait — flux-p2p is the prod transport.
pub mod net;
/// Simulated body — muscle groups, fatigue, stamina, reflex reaction-time, balance/stagger.
pub mod body;
/// Agility/dexterity attributes, combo action-timers, and archery aim (calibration + aim-while-moving).
pub mod skill;
/// Adaptive enemy AI brain (DeepSeek-authored, human-verified) — decides from a Perception; learns each bout.
pub mod ai;
/// P6 capstone — `flux_game_scaffold`: stamp a fresh SIGIL ARPG crate from this mold.
pub mod scaffold;

// ───────────────────────────── 3D space ─────────────────────────────

/// A 3D grid position. z is elevation (floors/ramps).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct V3 { pub x: i32, pub y: i32, pub z: i32 }
impl V3 {
    pub fn new(x: i32, y: i32, z: i32) -> Self { Self { x, y, z } }
    pub fn add(self, d: V3) -> V3 { V3::new(self.x + d.x, self.y + d.y, self.z + d.z) }
    /// Chebyshev distance on the horizontal plane (8-dir melee/AoE range; z handled separately).
    pub fn plane_cheby(self, o: V3) -> i32 { (self.x - o.x).abs().max((self.y - o.y).abs()) }
    /// True 3D-ish range used for targeting (plane + a z penalty — high ground matters).
    pub fn reach(self, o: V3) -> i32 { self.plane_cheby(o) + (self.z - o.z).abs() }
}

/// The 8 horizontal step directions (ARPG free-ish movement).
pub const STEPS8: [(i32, i32); 8] = [(1,0),(-1,0),(0,1),(0,-1),(1,1),(1,-1),(-1,1),(-1,-1)];

// ───────────────────────────── world ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terrain { Floor, Wall, Ramp, Hazard, Gate }

/// A 3D arena. Tiles are stored [z][y][x]. A tile is walkable if Floor/Ramp/Gate; Ramp lets you
/// change z to an adjacent floor one level up/down.
#[derive(Debug, Clone)]
pub struct World {
    pub w: i32, pub h: i32, pub d: i32,
    tiles: Vec<Terrain>,
    pub gate: V3,
}
impl World {
    fn idx(&self, v: V3) -> Option<usize> {
        if v.x < 0 || v.y < 0 || v.z < 0 || v.x >= self.w || v.y >= self.h || v.z >= self.d { return None; }
        Some(((v.z * self.h + v.y) * self.w + v.x) as usize)
    }
    pub fn at(&self, v: V3) -> Terrain { self.idx(v).map(|i| self.tiles[i]).unwrap_or(Terrain::Wall) }
    fn set(&mut self, v: V3, t: Terrain) { if let Some(i) = self.idx(v) { self.tiles[i] = t; } }
    pub fn walkable(&self, v: V3) -> bool { matches!(self.at(v), Terrain::Floor | Terrain::Ramp | Terrain::Gate) }

    /// The demo arena: a two-tier ashland — a lower floor (z=0), a ramp up to a raised platform
    /// (z=1) where the gate to Crown & Ash sits, scattered walls + a hazard pool.
    pub fn arena() -> World {
        let (w, h, d) = (9, 9, 2);
        let mut world = World { w, h, d, tiles: vec![Terrain::Wall; (w*h*d) as usize], gate: V3::new(7,7,1) };
        // lower floor: open
        for y in 0..h { for x in 0..w { world.set(V3::new(x,y,0), Terrain::Floor); } }
        // border walls on z0
        for x in 0..w { world.set(V3::new(x,0,0), Terrain::Wall); world.set(V3::new(x,h-1,0), Terrain::Wall); }
        for y in 0..h { world.set(V3::new(0,y,0), Terrain::Wall); world.set(V3::new(w-1,y,0), Terrain::Wall); }
        // a hazard pool (ash-lava) lower-left
        for (hx,hy) in [(2,2),(3,2),(2,3)] { world.set(V3::new(hx,hy,0), Terrain::Hazard); }
        // a few pillars
        for (px,py) in [(5,3),(3,5),(6,5)] { world.set(V3::new(px,py,0), Terrain::Wall); }
        // raised platform z=1 (upper-right quadrant) + a ramp from z0 to z1
        for y in 5..8 { for x in 5..8 { world.set(V3::new(x,y,1), Terrain::Floor); } }
        world.set(V3::new(5,5,0), Terrain::Ramp);  // step onto the ramp at z0…
        world.set(V3::new(5,5,1), Terrain::Ramp);  // …emerge on the platform at z1
        world.set(V3::new(7,7,1), Terrain::Gate);  // the gate to Crown & Ash
        world
    }
}

// ───────────────────────────── entities ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Foe { AshWraith, CinderHound, GlassGolem, EmberLord }
impl Foe {
    pub fn glyph(self) -> char { match self { Foe::AshWraith=>'w', Foe::CinderHound=>'h', Foe::GlassGolem=>'G', Foe::EmberLord=>'L' } }
    pub fn name(self) -> &'static str { match self { Foe::AshWraith=>"Ash Wraith", Foe::CinderHound=>"Cinder Hound", Foe::GlassGolem=>"Glass Golem", Foe::EmberLord=>"Ember Lord" } }
    pub fn max_hp(self) -> i32 { match self { Foe::AshWraith=>18, Foe::CinderHound=>24, Foe::GlassGolem=>40, Foe::EmberLord=>70 } }
    pub fn bite(self) -> i32 { match self { Foe::AshWraith=>5, Foe::CinderHound=>8, Foe::GlassGolem=>10, Foe::EmberLord=>16 } }
}

#[derive(Debug, Clone)]
pub struct Enemy { pub pos: V3, pub hp: i32, pub kind: Foe, pub ally: bool }
impl Enemy {
    pub fn new(kind: Foe, pos: V3) -> Self { Self { pos, hp: kind.max_hp(), kind, ally: false } }
    pub fn alive(&self) -> bool { self.hp > 0 }
}

#[derive(Debug, Clone)]
pub struct Player {
    pub pos: V3,
    pub hp: i32, pub max_hp: i32,
    pub sigil: i32, pub max_sigil: i32, // "mana" — Sigil energy that powers MCP casts
    pub shield: i32,
    pub level: i32, pub xp: i32,
    // run telemetry → feeds the Crown & Ash ascension
    pub slain: i32, pub combos_cast: i32, pub solo_casts: i32,
    pub converted: i32, pub depth_z: i32, pub combo_kinds: BTreeSet<&'static str>,
}
impl Player {
    pub fn new(pos: V3) -> Self {
        Self { pos, hp: 60, max_hp: 60, sigil: 30, max_sigil: 30, shield: 0, level: 1, xp: 0,
               slain: 0, combos_cast: 0, solo_casts: 0, converted: 0, depth_z: pos.z, combo_kinds: BTreeSet::new() }
    }
    pub fn alive(&self) -> bool { self.hp > 0 }
    fn gain_xp(&mut self, x: i32) { self.xp += x; while self.xp >= self.level * 20 { self.xp -= self.level*20; self.level += 1; self.max_hp += 10; self.max_sigil += 6; self.hp = self.max_hp; } }
}

// ───────────────────────────── MCP spells + combos ─────────────────────────────

/// The castable MCP tools — each is a spell. Casting TWO in one turn FUSES them into a combo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mcp { FluxCombo, DexSwap, ZkVeil, CouncilQuorum, Tribute, Hashstorm }
impl Mcp {
    pub fn name(self) -> &'static str { match self {
        Mcp::FluxCombo=>"flux_combo", Mcp::DexSwap=>"dex_swap", Mcp::ZkVeil=>"flux_zk_combo",
        Mcp::CouncilQuorum=>"council_consensus", Mcp::Tribute=>"send_token", Mcp::Hashstorm=>"mining_status" } }
    pub fn cost(self) -> i32 { match self {
        Mcp::FluxCombo=>10, Mcp::DexSwap=>6, Mcp::ZkVeil=>7, Mcp::CouncilQuorum=>9, Mcp::Tribute=>8, Mcp::Hashstorm=>11 } }
    pub fn blurb(self) -> &'static str { match self {
        Mcp::FluxCombo=>"channel the compiler → AoE nova",
        Mcp::DexSwap=>"swap places with the target (reposition + jab)",
        Mcp::ZkVeil=>"zk-shield: absorb incoming damage",
        Mcp::CouncilQuorum=>"quorum heal + ward",
        Mcp::Tribute=>"bribe a weakened foe to your side",
        Mcp::Hashstorm=>"hash-storm: heavy single-target burn" } }
}

/// What a cast produced — for the log + tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CastOutcome { Ok(String), NoSigil, NoTarget }

// ───────────────────────────── game ─────────────────────────────

#[derive(Debug, Clone)]
pub struct Game {
    pub world: World,
    pub player: Player,
    pub enemies: Vec<Enemy>,
    pub log: Vec<String>,
    pub turn: u32,
    pending: Option<Mcp>, // first half of a combo this turn
}
impl Game {
    pub fn new() -> Self {
        let world = World::arena();
        let player = Player::new(V3::new(2, 6, 0));
        let enemies = vec![
            Enemy::new(Foe::AshWraith,  V3::new(4, 4, 0)),
            Enemy::new(Foe::CinderHound,V3::new(6, 3, 0)),
            Enemy::new(Foe::GlassGolem, V3::new(4, 7, 0)),
            Enemy::new(Foe::EmberLord,  V3::new(6, 6, 1)), // boss on the high platform, guards the gate
        ];
        Self { world, player, enemies, log: vec!["You step into the Ashlands. The gate to Crown & Ash glows on the high platform.".into()], turn: 0, pending: None }
    }

    fn log(&mut self, s: impl Into<String>) { self.log.push(s.into()); }
    pub fn living_foes(&self) -> usize { self.enemies.iter().filter(|e| e.alive() && !e.ally).count() }
    pub fn won(&self) -> bool { self.living_foes() == 0 }

    /// Move the player one step (8-dir + ramp z-change). Returns false if blocked.
    pub fn step(&mut self, dx: i32, dy: i32) -> bool {
        let mut dest = self.player.pos.add(V3::new(dx, dy, 0));
        // ramp logic: stepping onto a ramp tile lifts/drops a z-level if the platform above is walkable
        if self.world.at(self.player.pos) == Terrain::Ramp || self.world.at(dest) == Terrain::Ramp {
            let up = dest.add(V3::new(0,0,1));
            if self.world.walkable(up) { dest = up; }
        }
        if !self.world.walkable(dest) || self.enemies.iter().any(|e| e.alive() && e.pos == dest) { return false; }
        self.player.pos = dest;
        self.player.depth_z = self.player.depth_z.max(dest.z);
        if self.world.at(dest) == Terrain::Hazard { self.player.hp -= 6; self.log("The ash-lava sears you (-6)."); }
        true
    }

    fn nearest_foe(&self) -> Option<usize> {
        self.enemies.iter().enumerate().filter(|(_,e)| e.alive() && !e.ally)
            .min_by_key(|(_,e)| self.player.pos.reach(e.pos)).map(|(i,_)| i)
    }

    fn damage_foe(&mut self, i: usize, dmg: i32, src: &str) {
        // take only Copy data out of the enemy so the &mut borrow ends before we touch self.log/self.player
        self.enemies[i].hp -= dmg;
        let hp = self.enemies[i].hp;
        let kind = self.enemies[i].kind;
        let nm = kind.name();
        if hp <= 0 {
            self.log(format!("  {src} shatters the {nm}!"));
            self.player.slain += 1;
            self.player.gain_xp(kind.max_hp() / 2);
        } else {
            self.log(format!("  {src} hits {nm} for {dmg} ({} hp left).", hp.max(0)));
        }
    }

    /// Cast a single MCP spell. If one was already cast this turn, the two FUSE into a combo.
    pub fn cast(&mut self, mcp: Mcp, target: Option<usize>) -> CastOutcome {
        if self.player.sigil < mcp.cost() { self.log(format!("Not enough Sigil for {}.", mcp.name())); return CastOutcome::NoSigil; }
        self.player.sigil -= mcp.cost();
        match self.pending.take() {
            Some(first) => { self.player.combos_cast += 1; self.fuse(first, mcp, target) }
            None => { self.pending = Some(mcp); self.player.solo_casts += 1; self.apply_single(mcp, target) }
        }
    }

    /// Explicitly cast a two-tool COMBO in one go (the headline mechanic).
    pub fn combo(&mut self, a: Mcp, b: Mcp, target: Option<usize>) -> CastOutcome {
        let cost = a.cost() + b.cost();
        if self.player.sigil < cost { self.log(format!("Not enough Sigil for {} + {}.", a.name(), b.name())); return CastOutcome::NoSigil; }
        self.player.sigil -= cost;
        self.player.combos_cast += 1;
        self.fuse(a, b, target)
    }

    fn apply_single(&mut self, mcp: Mcp, target: Option<usize>) -> CastOutcome {
        let tgt = target.or_else(|| self.nearest_foe());
        match mcp {
            Mcp::FluxCombo => {
                let center = tgt.map(|i| self.enemies[i].pos).unwrap_or(self.player.pos);
                let mut hit = 0;
                let ids: Vec<usize> = self.enemies.iter().enumerate()
                    .filter(|(_,e)| e.alive() && !e.ally && e.pos.reach(center) <= 1).map(|(i,_)| i).collect();
                for i in ids { self.damage_foe(i, 12, "flux_combo nova"); hit += 1; }
                CastOutcome::Ok(format!("flux_combo nova bursts ({hit} caught)"))
            }
            Mcp::DexSwap => match tgt {
                Some(i) => { let ep = self.enemies[i].pos; self.enemies[i].pos = self.player.pos; self.player.pos = ep;
                    self.damage_foe(i, 6, "dex_swap jab"); CastOutcome::Ok("dex_swap: positions swapped".into()) }
                None => { self.player.sigil += Mcp::DexSwap.cost(); CastOutcome::NoTarget }
            },
            Mcp::ZkVeil => { self.player.shield += 14; CastOutcome::Ok("flux_zk_combo: +14 shield".into()) }
            Mcp::CouncilQuorum => { self.player.hp = (self.player.hp + 18).min(self.player.max_hp); self.player.shield += 4;
                CastOutcome::Ok("council_consensus: +18 hp, +4 ward".into()) }
            Mcp::Tribute => match tgt {
                Some(i) if self.enemies[i].hp <= self.enemies[i].kind.max_hp()/3 => {
                    self.enemies[i].ally = true; self.player.converted += 1;
                    CastOutcome::Ok(format!("send_token: the {} defects to you!", self.enemies[i].kind.name())) }
                Some(_) => { self.player.sigil += Mcp::Tribute.cost(); self.log("Tribute fails — foe too strong."); CastOutcome::Ok("send_token rebuffed".into()) }
                None => { self.player.sigil += Mcp::Tribute.cost(); CastOutcome::NoTarget }
            },
            Mcp::Hashstorm => match tgt {
                Some(i) => { self.damage_foe(i, 22, "mining_status hashstorm"); CastOutcome::Ok("mining_status: hashstorm burns".into()) }
                None => { self.player.sigil += Mcp::Hashstorm.cost(); CastOutcome::NoTarget }
            },
        }
    }

    /// Fuse two MCP tools into a named combo — stronger than the sum, the heart of "MCP combos".
    fn fuse(&mut self, a: Mcp, b: Mcp, target: Option<usize>) -> CastOutcome {
        let mut key = [a, b]; key.sort_by_key(|m| m.name());
        let kinds = self.combo_name(key[0], key[1]);
        self.player.combo_kinds.insert(kinds);
        let tgt = target.or_else(|| self.nearest_foe());
        let name = kinds;
        match (key[0], key[1]) {
            // dex_swap + flux_combo → BLINK NOVA: swap to the foe, then a boosted nova there
            (Mcp::FluxCombo, Mcp::DexSwap) | (Mcp::DexSwap, Mcp::FluxCombo) => {
                if let Some(i) = tgt { let ep = self.enemies[i].pos; self.enemies[i].pos = self.player.pos; self.player.pos = ep; }
                let center = self.player.pos;
                let ids: Vec<usize> = self.enemies.iter().enumerate().filter(|(_,e)| e.alive() && !e.ally && e.pos.reach(center) <= 2).map(|(i,_)| i).collect();
                for i in ids { self.damage_foe(i, 20, "BLINK-NOVA"); }
                CastOutcome::Ok(format!("⚡ COMBO {name}: blink + boosted nova"))
            }
            // flux_zk_combo + council_consensus → WARDED RALLY: big shield + heal + summon ally
            (Mcp::CouncilQuorum, Mcp::ZkVeil) | (Mcp::ZkVeil, Mcp::CouncilQuorum) => {
                self.player.shield += 24; self.player.hp = (self.player.hp + 24).min(self.player.max_hp);
                let p = self.player.pos.add(V3::new(0,1,0));
                if self.world.walkable(p) && !self.enemies.iter().any(|e| e.pos==p) {
                    let mut ally = Enemy::new(Foe::AshWraith, p); ally.ally = true; self.enemies.push(ally);
                }
                CastOutcome::Ok(format!("✦ COMBO {name}: +24 ward, +24 hp, spectral ally summoned"))
            }
            // mining_status + flux_combo → HASHFIRE: burst + splash
            (Mcp::FluxCombo, Mcp::Hashstorm) | (Mcp::Hashstorm, Mcp::FluxCombo) => {
                if let Some(i) = tgt { self.damage_foe(i, 30, "HASHFIRE core"); let c = self.enemies[i].pos;
                    let ids: Vec<usize> = self.enemies.iter().enumerate().filter(|(_,e)| e.alive() && !e.ally && e.pos.reach(c)<=1).map(|(i,_)| i).collect();
                    for j in ids { self.damage_foe(j, 10, "HASHFIRE splash"); } }
                CastOutcome::Ok(format!("🔥 COMBO {name}: hashfire core + splash"))
            }
            // generic fusion: both effects, +synergy
            _ => {
                self.apply_single(a, target); self.apply_single(b, target);
                self.player.shield += 4;
                CastOutcome::Ok(format!("✧ COMBO {name}: fused effect + synergy ward"))
            }
        }
    }
    fn combo_name(&self, a: Mcp, b: Mcp) -> &'static str {
        match (a, b) {
            (Mcp::DexSwap, Mcp::FluxCombo) => "Blink-Nova",
            (Mcp::CouncilQuorum, Mcp::ZkVeil) => "Warded-Rally",
            (Mcp::FluxCombo, Mcp::Hashstorm) => "Hashfire",
            _ => "Sigil-Weave",
        }
    }

    /// End the player's turn: enemies act (move toward + bite), hazards tick, sigil regen.
    pub fn end_turn(&mut self) {
        self.pending = None;
        self.turn += 1;
        let ppos = self.player.pos;
        let mut bites: Vec<(i32, &'static str)> = Vec::new();
        let mut moves: Vec<(usize, V3)> = Vec::new();
        for (i, e) in self.enemies.iter().enumerate() {
            if !e.alive() || e.ally { continue; }
            if e.pos.reach(ppos) <= 1 {
                bites.push((e.kind.bite(), e.kind.name()));
            } else {
                // greedy step toward the player on the plane
                let best = STEPS8.iter().map(|&(dx,dy)| e.pos.add(V3::new(dx,dy,0)))
                    .filter(|&p| self.world.walkable(p) && p != ppos && !self.enemies.iter().any(|o| o.alive() && o.pos==p))
                    .min_by_key(|&p| p.reach(ppos));
                if let Some(p) = best { moves.push((i, p)); }
            }
        }
        for (i, p) in moves { self.enemies[i].pos = p; }
        for (dmg, nm) in bites {
            let absorbed = dmg.min(self.player.shield);
            self.player.shield -= absorbed;
            let through = dmg - absorbed;
            self.player.hp -= through;
            if through > 0 { self.log(format!("The {nm} bites you for {through}{}.", if absorbed>0 {format!(" ({absorbed} warded)")} else {String::new()})); }
            else { self.log(format!("Your ward absorbs the {nm}'s bite.")); }
        }
        // allies swat the nearest foe
        let ally_positions: Vec<V3> = self.enemies.iter().filter(|e| e.ally && e.alive()).map(|e| e.pos).collect();
        for ap in ally_positions {
            if let Some(j) = self.enemies.iter().enumerate().filter(|(_,e)| e.alive() && !e.ally).min_by_key(|(_,e)| e.pos.reach(ap)).map(|(j,_)| j) {
                if self.enemies[j].pos.reach(ap) <= 2 { self.damage_foe(j, 6, "your ally"); }
            }
        }
        self.player.sigil = (self.player.sigil + 4).min(self.player.max_sigil);
    }

    /// On victory at the gate: convert the run into a Crown & Ash ascension.
    pub fn ascend(&self) -> Ascension { Ascension::from_run(&self.player) }
}

impl Default for Game { fn default() -> Self { Self::new() } }

// ───────────────────────────── isometric renderer ─────────────────────────────

/// Render the 3D world to an isometric ASCII frame (2:1 oblique, z lifts tiles up — Diablo-style).
/// Back-to-front painting so higher/nearer things overwrite.
pub fn render(g: &Game) -> String {
    let (w, h, d) = (g.world.w, g.world.h, g.world.d);
    // screen size for the iso projection
    let sw = ((w + h) * 2 + 4) as usize;
    let sh = ((w + h) + d * 2 + 4) as usize;
    let mut buf = vec![vec![' '; sw]; sh];
    let proj = |v: V3| -> (usize, usize) {
        let sx = ((v.x - v.y) * 2 + (h * 2)) as usize;
        let sy = ((v.x + v.y) - v.z * 2 + d * 2) as usize;
        (sx.min(sw-1), sy.min(sh-1))
    };
    // paint terrain back-to-front: lower z first, then by (x+y)
    let mut cells: Vec<V3> = Vec::new();
    for z in 0..d { for y in 0..h { for x in 0..w { cells.push(V3::new(x,y,z)); } } }
    cells.sort_by_key(|v| (v.z, v.x + v.y));
    for v in &cells {
        let ch = match g.world.at(*v) {
            Terrain::Floor => '.', Terrain::Wall => '#', Terrain::Ramp => '/',
            Terrain::Hazard => '~', Terrain::Gate => '0',
        };
        if ch == '#' && v.z == 0 { /* keep walls */ }
        let (sx, sy) = proj(*v);
        buf[sy][sx] = ch;
    }
    // entities on top (sorted so closer/higher overwrite)
    let mut ents: Vec<(V3, char)> = g.enemies.iter().filter(|e| e.alive())
        .map(|e| (e.pos, if e.ally { '+' } else { e.kind.glyph() })).collect();
    ents.push((g.player.pos, '@'));
    ents.sort_by_key(|(v,_)| (v.z, v.x + v.y));
    for (v, ch) in ents { let (sx, sy) = proj(v); buf[sy][sx] = ch; }

    let mut out = String::new();
    out.push_str(&format!("  ASHWALKER · turn {} · z-depth {}\n", g.turn, g.player.depth_z));
    for row in buf { let line: String = row.into_iter().collect(); if !line.trim().is_empty() { out.push_str(line.trim_end()); out.push('\n'); } }
    out.push_str(&format!(
        "  @ HP {}/{}  Sigil {}/{}  Ward {}  Lv{}  · foes left: {}  · slain {} combos {}\n",
        g.player.hp, g.player.max_hp, g.player.sigil, g.player.max_sigil, g.player.shield,
        g.player.level, g.living_foes(), g.player.slain, g.player.combos_cast));
    out.push_str("  legend: @=you +=ally w/h/G/L=foes .=floor #=wall /=ramp ~=ash-lava 0=gate\n");
    out
}

// ───────────────────────────── Crown & Ash ascension ─────────────────────────────

/// What the Sigil-wielder BECOMES in Crown & Ash — the bridge into the strategy game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ascension {
    pub house: &'static str,
    pub title: &'static str,
    pub crown: i32,  // political capital
    pub ash: i32,    // raw resource
    pub troops: i32,
    pub blurb: String,
}
impl Ascension {
    /// The run's *style* decides the House: combo-variety → the Weave; conversions → the Concord;
    /// raw kills → the Ashen Host; survival/ward → the Bastion.
    pub fn from_run(p: &Player) -> Ascension {
        let variety = p.combo_kinds.len() as i32;
        let (house, title) =
            if p.converted >= 2 { ("Concord of Hands", "Envoy-Sovereign") }
            else if variety >= 3 { ("Weave of Sigils", "Archmagus") }
            else if p.slain >= 4 { ("Ashen Host", "Warlord") }
            else { ("Bastion of Ward", "Wardyn-Regent") };
        let crown = 10 + variety * 8 + p.level * 5;
        let ash = 40 + p.slain * 15 + p.depth_z * 20;
        let troops = 3 + p.slain + p.converted * 2 + p.combos_cast;
        let blurb = format!(
            "You pass through the gate. The Ashlands remember: {} slain, {} combos woven ({} distinct), {} foes turned, hp {}/{}. \
             Crown & Ash receives you as {} of the {} — {crown} crown, {ash} ash, {troops} levies at your banner.",
            p.slain, p.combos_cast, variety, p.converted, p.hp.max(0), p.max_hp, title, house);
        Ascension { house, title, crown, ash, troops, blurb }
    }
    /// One-line settlement record (the seed handed to the crown-ash realm sim in prototype 2).
    pub fn seed_line(&self) -> String {
        format!("ASCEND house=\"{}\" title=\"{}\" crown={} ash={} troops={}", self.house, self.title, self.crown, self.ash, self.troops)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_3d_walk_and_ramp() {
        let g = Game::new();
        assert!(g.world.walkable(V3::new(2,6,0)));
        assert!(!g.world.walkable(V3::new(0,0,0)), "border is wall");
        // the gate sits on z=1 (real elevation)
        assert_eq!(g.world.at(V3::new(7,7,1)), Terrain::Gate);
        assert!(g.world.gate.z == 1);
    }

    #[test]
    fn movement_blocks_on_walls_and_foes() {
        let mut g = Game::new();
        g.player.pos = V3::new(1,1,0);
        assert!(!g.step(-1, 0), "into the border wall — blocked");
        assert!(g.step(1, 0), "east onto floor (2,1,0) — ok"); // (2,2) is ash-lava, (2,1) is floor
    }

    #[test]
    fn single_cast_spends_sigil_and_damages() {
        let mut g = Game::new();
        g.player.pos = V3::new(4,5,0); // next to the wraith at (4,4)
        let s0 = g.player.sigil;
        let out = g.cast(Mcp::Hashstorm, Some(0));
        assert!(matches!(out, CastOutcome::Ok(_)));
        assert_eq!(g.player.sigil, s0 - Mcp::Hashstorm.cost());
        assert!(g.enemies[0].hp < g.enemies[0].kind.max_hp());
    }

    #[test]
    fn combo_is_stronger_than_two_singles() {
        // Blink-Nova combo should out-damage the wraith fast + register a combo + a distinct kind.
        let mut g = Game::new();
        g.player.pos = V3::new(4,5,0);
        let before = g.enemies[0].hp;
        let out = g.combo(Mcp::DexSwap, Mcp::FluxCombo, Some(0));
        assert!(matches!(out, CastOutcome::Ok(_)));
        assert!(g.player.combos_cast == 1);
        assert!(g.player.combo_kinds.contains("Blink-Nova"));
        assert!(g.enemies[0].hp < before - 15 || !g.enemies[0].alive(), "combo hits hard");
    }

    #[test]
    fn tribute_converts_a_weakened_foe() {
        let mut g = Game::new();
        g.enemies[0].hp = 3; // weakened wraith
        g.player.pos = g.enemies[0].pos.add(V3::new(1,0,0));
        let out = g.cast(Mcp::Tribute, Some(0));
        assert!(matches!(out, CastOutcome::Ok(_)));
        assert!(g.enemies[0].ally && g.player.converted == 1);
    }

    #[test]
    fn enemies_act_and_can_be_warded() {
        let mut g = Game::new();
        g.player.pos = g.enemies[0].pos.add(V3::new(1,0,0)); // adjacent → it will bite
        g.player.shield = 100; // fully warded
        let hp0 = g.player.hp;
        g.end_turn();
        assert_eq!(g.player.hp, hp0, "full ward absorbs the bite");
        assert!(g.player.shield < 100, "ward was spent");
    }

    #[test]
    fn ascension_reflects_run_style() {
        let mut p = Player::new(V3::new(0,0,0));
        p.combo_kinds.insert("Blink-Nova"); p.combo_kinds.insert("Hashfire"); p.combo_kinds.insert("Warded-Rally");
        p.combos_cast = 5; p.slain = 2; p.depth_z = 1; p.level = 3;
        let a = Ascension::from_run(&p);
        assert_eq!(a.house, "Weave of Sigils"); // 3 distinct combos → the Weave
        assert!(a.crown > 0 && a.ash > 0 && a.troops > 0);
        assert!(a.seed_line().contains("ASCEND") && a.seed_line().contains("Archmagus"));
    }

    #[test]
    fn render_draws_player_and_gate() {
        let g = Game::new();
        let frame = render(&g);
        assert!(frame.contains('@'), "player drawn");
        assert!(frame.contains("ASHWALKER"));
        assert!(frame.contains("legend"));
    }
}
