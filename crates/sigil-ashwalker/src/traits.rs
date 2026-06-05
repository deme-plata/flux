//! traits.rs — ASHWALKER character creator.
//!
//! A character is generated **deterministically from a name** (FNV-1a seed) so every name yields a
//! unique-but-reproducible Sigil-wielder: an Origin (base build) + two distinct Traits (perks that
//! actually change play) + a colour palette for the avatar. Same name → same hero, always.

/// Stable 64-bit hash (FNV-1a) — reproducible across runs/versions (unlike DefaultHasher).
pub fn seed_of(name: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in name.trim().to_lowercase().bytes() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); }
    if h == 0 { 0x9e3779b97f4a7c15 } else { h }
}

/// Base build — sets the stat lean + a signature MCP tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin { Ashborn, CompilerMonk, ExiledValidator, VoidpactBroker }
impl Origin {
    pub fn all() -> [Origin; 4] { [Origin::Ashborn, Origin::CompilerMonk, Origin::ExiledValidator, Origin::VoidpactBroker] }
    pub fn name(self) -> &'static str { match self {
        Origin::Ashborn=>"Ashborn", Origin::CompilerMonk=>"Compiler-Monk",
        Origin::ExiledValidator=>"Exiled Validator", Origin::VoidpactBroker=>"Voidpact Broker" } }
    pub fn glyph(self) -> char { match self { Origin::Ashborn=>'🜂', Origin::CompilerMonk=>'⟁', Origin::ExiledValidator=>'⊢', Origin::VoidpactBroker=>'◈' } }
    pub fn blurb(self) -> &'static str { match self {
        Origin::Ashborn=>"forged in the ash — tough, melee-leaning",
        Origin::CompilerMonk=>"channels the compiler — deep Sigil, loves flux_combo",
        Origin::ExiledValidator=>"warded exile — quorum heals + zk shields",
        Origin::VoidpactBroker=>"deals in defections — turns foes, hoards crown" } }
    /// (hp, sigil, ward) modifiers.
    pub fn mods(self) -> (i32, i32, i32) { match self {
        Origin::Ashborn=>(20, -4, 4), Origin::CompilerMonk=>(-6, 14, 0),
        Origin::ExiledValidator=>(4, 4, 10), Origin::VoidpactBroker=>(0, 6, 2) } }
    pub fn signature(self) -> &'static str { match self {
        Origin::Ashborn=>"mining_status", Origin::CompilerMonk=>"flux_combo",
        Origin::ExiledValidator=>"council_consensus", Origin::VoidpactBroker=>"send_token" } }
}

/// Unique perks that change play. Two are rolled per character (distinct).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trait { LockFree, MerkleTouched, Forkbound, AshLunged, ZeroCopy, QuorumKin }
impl Trait {
    pub fn all() -> [Trait; 6] { [Trait::LockFree, Trait::MerkleTouched, Trait::Forkbound, Trait::AshLunged, Trait::ZeroCopy, Trait::QuorumKin] }
    pub fn name(self) -> &'static str { match self {
        Trait::LockFree=>"Lock-Free", Trait::MerkleTouched=>"Merkle-Touched", Trait::Forkbound=>"Forkbound",
        Trait::AshLunged=>"Ash-Lunged", Trait::ZeroCopy=>"Zero-Copy", Trait::QuorumKin=>"Quorum-Kin" } }
    pub fn blurb(self) -> &'static str { match self {
        Trait::LockFree=>"+3 Sigil regen each turn (no contention)",
        Trait::MerkleTouched=>"you can READ a boss's committed weakness-combo",
        Trait::Forkbound=>"MCP combos cost 4 less Sigil",
        Trait::AshLunged=>"immune to ash-lava hazard",
        Trait::ZeroCopy=>"+6 starting ward (move without cost)",
        Trait::QuorumKin=>"+12 max HP (the council stands with you)" } }
}

/// A fully-rolled hero — what the creator hands to the game + the avatar renderer.
#[derive(Debug, Clone)]
pub struct CharSheet {
    pub name: String,
    pub seed: u64,
    pub origin: Origin,
    pub traits: Vec<Trait>,
    // resolved starting stats / effects (base + origin + traits)
    pub hp: i32,
    pub sigil: i32,
    pub ward: i32,
    pub sigil_regen: i32,
    pub combo_discount: i32,
    pub hazard_immune: bool,
    pub reads_weakness: bool,
    /// 0–359 base hue for the avatar palette (seed-derived).
    pub hue: u16,
}
impl CharSheet {
    /// Roll a hero from a name (deterministic + unique).
    pub fn create(name: &str) -> CharSheet { Self::from_seed(name, seed_of(name)) }

    /// Roll from an explicit seed (e.g. fused with a Crown & Ash decision root → cross-game identity).
    pub fn from_seed(name: &str, seed: u64) -> CharSheet {
        let origin = Origin::all()[(seed >> 5) as usize % 4];
        // two distinct traits
        let t_all = Trait::all();
        let i0 = (seed >> 9) as usize % t_all.len();
        let mut i1 = (seed >> 17) as usize % t_all.len();
        if i1 == i0 { i1 = (i1 + 1) % t_all.len(); }
        let traits = vec![t_all[i0], t_all[i1]];

        let (hm, sm, wm) = origin.mods();
        let mut sheet = CharSheet {
            name: if name.trim().is_empty() { "Nameless".into() } else { name.trim().into() },
            seed, origin, traits,
            hp: 60 + hm, sigil: 30 + sm, ward: wm.max(0),
            sigil_regen: 4, combo_discount: 0, hazard_immune: false, reads_weakness: false,
            hue: (seed % 360) as u16,
        };
        for t in sheet.traits.clone() {
            match t {
                Trait::LockFree => sheet.sigil_regen += 3,
                Trait::MerkleTouched => sheet.reads_weakness = true,
                Trait::Forkbound => sheet.combo_discount += 4,
                Trait::AshLunged => sheet.hazard_immune = true,
                Trait::ZeroCopy => sheet.ward += 6,
                Trait::QuorumKin => sheet.hp += 12,
            }
        }
        sheet
    }

    /// Human-readable character card (sans avatar).
    pub fn card(&self) -> String {
        let mut s = format!("  {}  «{}»  {}\n", self.origin.glyph(), self.name, self.origin.name());
        s.push_str(&format!("  origin: {}\n", self.origin.blurb()));
        for t in &self.traits { s.push_str(&format!("  ◦ {:<14} — {}\n", t.name(), t.blurb())); }
        s.push_str(&format!("  HP {}  Sigil {}  Ward {}  regen +{}/turn  · signature {}\n",
            self.hp, self.sigil, self.ward, self.sigil_regen, self.origin.signature()));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_name_same_hero_deterministic() {
        let a = CharSheet::create("Rocky");
        let b = CharSheet::create("Rocky");
        assert_eq!(a.seed, b.seed);
        assert_eq!(a.origin, b.origin);
        assert_eq!(a.traits, b.traits);
    }

    #[test]
    fn different_names_diverge() {
        let a = CharSheet::create("Rocky");
        let b = CharSheet::create("Viktor");
        // overwhelmingly likely to differ on at least one axis
        assert!(a.seed != b.seed);
        assert!(a.origin != b.origin || a.traits != b.traits || a.hue != b.hue);
    }

    #[test]
    fn two_distinct_traits_and_effects_applied() {
        let c = CharSheet::create("Ashwalker");
        assert_eq!(c.traits.len(), 2);
        assert_ne!(c.traits[0], c.traits[1], "the two traits are distinct");
        // a Forkbound hero must actually have the discount
        if c.traits.contains(&Trait::Forkbound) { assert!(c.combo_discount >= 4); }
        if c.traits.contains(&Trait::QuorumKin) { assert!(c.hp >= 60); }
        assert!(c.hp > 0 && c.sigil > 0);
    }

    #[test]
    fn card_renders_name_and_traits() {
        let c = CharSheet::create("Codex");
        let card = c.card();
        assert!(card.contains("Codex"));
        assert!(card.contains(c.traits[0].name()));
    }
}
