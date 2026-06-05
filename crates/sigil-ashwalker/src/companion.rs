//! companion.rs — pets (combat companions) + mounts (faster travel).
//!
//! A hero may keep a **pet** (a passive battle companion — regen / damage / ward / boss-sight) and a
//! **mount** (faster movement, some can cross hazards). Both are tamed/found like gear (wild, quest,
//! rare boss drop) and are deterministic from a seed. Pets compose with traits — a `Merkle-Owl` pet
//! grants the same boss-weakness sight as the `Merkle-Touched` trait.

fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e3779b97f4a7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

// ───────────────────────── pets ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PetKind { AshSprite, CinderPup, GlassMoth, MerkleOwl }
impl PetKind {
    pub fn all() -> [PetKind; 4] { [PetKind::AshSprite, PetKind::CinderPup, PetKind::GlassMoth, PetKind::MerkleOwl] }
    pub fn name(self) -> &'static str { match self { PetKind::AshSprite=>"Ash Sprite", PetKind::CinderPup=>"Cinder Pup", PetKind::GlassMoth=>"Glass Moth", PetKind::MerkleOwl=>"Merkle Owl" } }
    pub fn glyph(self) -> char { match self { PetKind::AshSprite=>'✦', PetKind::CinderPup=>'⊙', PetKind::GlassMoth=>'❉', PetKind::MerkleOwl=>'☉' } }
    pub fn perk(self) -> &'static str { match self {
        PetKind::AshSprite=>"+2 Sigil regen each turn",
        PetKind::CinderPup=>"bites the nearest foe each turn (+dmg)",
        PetKind::GlassMoth=>"+3 ward each turn",
        PetKind::MerkleOwl=>"reveals a boss's committed weakness-combo" } }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pet { pub name: String, pub kind: PetKind, pub level: i32 }
impl Pet {
    pub fn sigil_regen(&self) -> i32 { if self.kind == PetKind::AshSprite { 2 + self.level / 2 } else { 0 } }
    pub fn bite(&self) -> i32 { if self.kind == PetKind::CinderPup { 4 + self.level * 2 } else { 0 } }
    pub fn ward(&self) -> i32 { if self.kind == PetKind::GlassMoth { 3 + self.level } else { 0 } }
    pub fn reveals_weakness(&self) -> bool { self.kind == PetKind::MerkleOwl }
}

// ───────────────────────── mounts ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountKind { AshStrider, DuneServal, VoidBarge, CrownElk }
impl MountKind {
    pub fn all() -> [MountKind; 4] { [MountKind::AshStrider, MountKind::DuneServal, MountKind::VoidBarge, MountKind::CrownElk] }
    pub fn name(self) -> &'static str { match self { MountKind::AshStrider=>"Ash Strider", MountKind::DuneServal=>"Dune Serval", MountKind::VoidBarge=>"Void Barge", MountKind::CrownElk=>"Crown Elk" } }
    /// tiles travelled per move action (1 = on foot)
    pub fn speed(self) -> i32 { match self { MountKind::AshStrider=>2, MountKind::DuneServal=>3, MountKind::VoidBarge=>1, MountKind::CrownElk=>2 } }
    /// can it ride OVER ash-lava hazard tiles?
    pub fn crosses_hazard(self) -> bool { matches!(self, MountKind::VoidBarge) }
    pub fn perk(self) -> &'static str { match self {
        MountKind::AshStrider=>"steady 2-tile stride",
        MountKind::DuneServal=>"fastest — 3-tile bound",
        MountKind::VoidBarge=>"slow, but floats across ash-lava unharmed",
        MountKind::CrownElk=>"2-tile stride + Crown favour (more crown on ascension)" } }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount { pub name: String, pub kind: MountKind }
impl Mount { pub fn speed(&self) -> i32 { self.kind.speed() } pub fn crosses_hazard(&self) -> bool { self.kind.crosses_hazard() } }

// ───────────────────────── stable (what the hero keeps) ─────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stable { pub pet: Option<Pet>, pub mount: Option<Mount> }
impl Stable {
    /// tiles per move action (1 on foot, more when mounted)
    pub fn move_speed(&self) -> i32 { self.mount.as_ref().map(|m| m.speed()).unwrap_or(1) }
    pub fn crosses_hazard(&self) -> bool { self.mount.as_ref().map(|m| m.crosses_hazard()).unwrap_or(false) }
    pub fn sigil_regen_bonus(&self) -> i32 { self.pet.as_ref().map(|p| p.sigil_regen()).unwrap_or(0) }
    pub fn ward_bonus(&self) -> i32 { self.pet.as_ref().map(|p| p.ward()).unwrap_or(0) }
    pub fn pet_bite(&self) -> i32 { self.pet.as_ref().map(|p| p.bite()).unwrap_or(0) }
    pub fn sees_boss_weakness(&self) -> bool { self.pet.as_ref().map(|p| p.reveals_weakness()).unwrap_or(false) }
    pub fn crown_favour(&self) -> bool { matches!(self.mount.as_ref().map(|m| m.kind), Some(MountKind::CrownElk)) }
}

const PET_NAMES: [&str; 4] = ["Pip", "Ember", "Sil", "Glyph"];
const MOUNT_NAMES: [&str; 4] = ["Strider", "Dust", "Barge", "Antler"];

/// Tame a pet from a seed (wild / quest reward).
pub fn tame_pet(seed: u64) -> Pet {
    let s = mix(seed ^ 0x9157);
    let kind = PetKind::all()[(s & 3) as usize];
    Pet { name: format!("{} the {}", PET_NAMES[((s >> 4) & 3) as usize], kind.name()), kind, level: 1 + (s >> 8) as i32 % 3 }
}
/// Catch a mount from a seed (wild / quest / rare boss drop).
pub fn find_mount(seed: u64) -> Mount {
    let s = mix(seed ^ 0x3171);
    let kind = MountKind::all()[(s & 3) as usize];
    Mount { name: format!("{} ({})", MOUNT_NAMES[((s >> 4) & 3) as usize], kind.name()), kind }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_speeds_up_travel_and_void_barge_crosses_hazard() {
        let mut st = Stable::default();
        assert_eq!(st.move_speed(), 1, "on foot = 1 tile");
        st.mount = Some(Mount { name: "x".into(), kind: MountKind::DuneServal });
        assert_eq!(st.move_speed(), 3, "serval is fastest");
        assert!(!st.crosses_hazard());
        st.mount = Some(Mount { name: "y".into(), kind: MountKind::VoidBarge });
        assert!(st.crosses_hazard(), "void barge floats over ash-lava");
    }

    #[test]
    fn pets_grant_their_perk() {
        let mut st = Stable::default();
        st.pet = Some(Pet { name:"a".into(), kind: PetKind::AshSprite, level: 2 });
        assert!(st.sigil_regen_bonus() >= 2);
        st.pet = Some(Pet { name:"b".into(), kind: PetKind::MerkleOwl, level: 1 });
        assert!(st.sees_boss_weakness(), "merkle owl reveals the boss weakness");
        st.pet = Some(Pet { name:"c".into(), kind: PetKind::CinderPup, level: 3 });
        assert!(st.pet_bite() > 0);
    }

    #[test]
    fn tame_and_find_are_deterministic() {
        assert_eq!(tame_pet(42), tame_pet(42));
        assert_eq!(find_mount(7), find_mount(7));
        assert!(find_mount(7).speed() >= 1);
    }

    #[test]
    fn crown_elk_grants_favour() {
        let st = Stable { pet: None, mount: Some(Mount { name:"e".into(), kind: MountKind::CrownElk }) };
        assert!(st.crown_favour());
    }
}
