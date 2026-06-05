//! item.rs — gear: tiers, sets, sources, and **gear-gated** boss difficulty.
//!
//! You start with nothing. Even the best LOW-tier gear must be **bought in a store** or **found in
//! the wild** (quests / conquering). High tiers are NOT sold — only the wild and bosses yield them.
//! **Every boss kill drops loot** (deterministic from the boss's Merkle root, so drops are committed
//! too). A boss carries a `gear_req`: fight it under-geared and it's brutal (it hits harder, you hit
//! softer) — some bosses are simply unbeatable until you've built your set.

use crate::merkle::MerkleBoss;

fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e3779b97f4a7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

/// Quality tiers, low → high. Stores stop at low-Iron; the wild/bosses carry the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier { Ashen, Iron, Sigil, Mythic, Relic }
impl Tier {
    pub fn all() -> [Tier; 5] { [Tier::Ashen, Tier::Iron, Tier::Sigil, Tier::Mythic, Tier::Relic] }
    pub fn name(self) -> &'static str { match self { Tier::Ashen=>"Ashen", Tier::Iron=>"Iron", Tier::Sigil=>"Sigil", Tier::Mythic=>"Mythic", Tier::Relic=>"Relic" } }
    /// power multiplier — escalates hard so a tier gap really matters
    pub fn mult(self) -> i32 { match self { Tier::Ashen=>10, Tier::Iron=>18, Tier::Sigil=>34, Tier::Mythic=>60, Tier::Relic=>104 } }
    pub fn from_rank(r: usize) -> Tier { Tier::all()[r.min(4)] }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot { Weapon, Armor, SigilFocus, Trinket }
impl Slot { pub fn all() -> [Slot; 4] { [Slot::Weapon, Slot::Armor, Slot::SigilFocus, Slot::Trinket] }
    pub fn name(self) -> &'static str { match self { Slot::Weapon=>"Weapon", Slot::Armor=>"Armor", Slot::SigilFocus=>"Sigil-Focus", Slot::Trinket=>"Trinket" } } }

/// Where an item can come from (gates what tiers are reachable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source { Store, Wild, BossDrop }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub tier: Tier,
    pub slot: Slot,
    pub roll: i32,                  // 1..=10 base roll within the tier
    pub set: Option<&'static str>,  // belonging to a set grants a bonus when 2+ equipped
    pub source: Source,
}
impl Item {
    /// effective power = tier multiplier × roll
    pub fn power(&self) -> i32 { self.tier.mult() * self.roll }
    pub fn line(&self) -> String {
        format!("[{}] {} ({}, pow {}{})", self.tier.name(), self.name, self.slot.name(), self.power(),
            self.set.map(|s| format!(", set:{s}")).unwrap_or_default())
    }
}

const SET_NAMES: [&str; 4] = ["Emberweave", "Glassbound", "Forkwright", "Ashen Pact"];
const WEAPON_NAMES: [&str; 4] = ["Ash-Cleaver", "Sigil-Lance", "Cinder-Edge", "Forkblade"];
const ARMOR_NAMES:  [&str; 4] = ["Cinderplate", "Glass-Mail", "Wraithweave", "Bastion-Coat"];
const FOCUS_NAMES:  [&str; 4] = ["Merkle-Eye", "Quorum-Core", "Hash-Prism", "Void-Lens"];
const TRINKET_NAMES:[&str; 4] = ["Ember Sigil", "Lock-Free Ring", "Ashen Charm", "Crown Shard"];

fn name_for(slot: Slot, tier: Tier, seed: u64) -> String {
    let pool = match slot { Slot::Weapon=>WEAPON_NAMES, Slot::Armor=>ARMOR_NAMES, Slot::SigilFocus=>FOCUS_NAMES, Slot::Trinket=>TRINKET_NAMES };
    format!("{} {}", tier.name(), pool[(seed & 3) as usize])
}

/// Roll an item deterministically from a seed, capped to a max tier (its source's ceiling).
pub fn roll_item(seed: u64, max_tier: Tier, source: Source) -> Item {
    let s = mix(seed);
    let slot = Slot::all()[(s & 3) as usize];
    let tier_rank = ((s >> 2) as usize % (Tier::all().iter().position(|t| *t == max_tier).unwrap() + 1)).min(4);
    let tier = Tier::from_rank(tier_rank);
    let roll = 1 + ((s >> 8) % 10) as i32;
    let set = if (s >> 20) & 1 == 1 { Some(SET_NAMES[((s >> 21) & 3) as usize]) } else { None };
    Item { name: name_for(slot, tier, s >> 30), tier, slot, roll, set, source }
}

// ───────────────────────── loadout + gear score ─────────────────────────

/// The hero's equipped gear (one item per slot) + the bag.
#[derive(Debug, Clone, Default)]
pub struct Loadout { pub weapon: Option<Item>, pub armor: Option<Item>, pub focus: Option<Item>, pub trinket: Option<Item>, pub bag: Vec<Item> }
impl Loadout {
    fn equipped(&self) -> Vec<&Item> { [&self.weapon, &self.armor, &self.focus, &self.trinket].into_iter().flatten().collect() }
    /// Equip best-in-slot from an item (keeps the stronger), bagging the rest.
    pub fn equip(&mut self, it: Item) {
        let slot = it.slot;
        let cur = match slot { Slot::Weapon=>&mut self.weapon, Slot::Armor=>&mut self.armor, Slot::SigilFocus=>&mut self.focus, Slot::Trinket=>&mut self.trinket };
        match cur { Some(old) if old.power() >= it.power() => self.bag.push(it),
            _ => { if let Some(old) = cur.take() { self.bag.push(old); } *cur = Some(it); } }
    }
    pub fn take(&mut self, items: Vec<Item>) { for it in items { self.equip(it); } }
    /// set bonus: +15% of equipped power per set with 2+ pieces
    pub fn set_bonus(&self) -> i32 {
        let mut counts: std::collections::BTreeMap<&str, i32> = std::collections::BTreeMap::new();
        let eq = self.equipped();
        for it in &eq { if let Some(s) = it.set { *counts.entry(s).or_default() += 1; } }
        let base: i32 = eq.iter().map(|i| i.power()).sum();
        let sets_active = counts.values().filter(|&&c| c >= 2).count() as i32;
        base * 15 * sets_active / 100
    }
    /// total gear score — what gates the bosses
    pub fn gear_score(&self) -> i32 { self.equipped().iter().map(|i| i.power()).sum::<i32>() + self.set_bonus() }
}

// ───────────────────────── sources ─────────────────────────

/// The store stocks only LOW gear (Ashen + low Iron). You must buy these to start — nothing is free.
pub fn store_catalog() -> Vec<(Item, i32)> {
    // (item, crown cost)
    let mut out = Vec::new();
    for (i, slot) in Slot::all().into_iter().enumerate() {
        for (j, &tier) in [Tier::Ashen, Tier::Iron].iter().enumerate() {
            let it = Item { name: name_for(slot, tier, (i*7+j*13) as u64), tier, slot, roll: 4 + j as i32 * 3, set: None, source: Source::Store };
            let cost = it.power() / 2 + 10;
            out.push((it, cost));
        }
    }
    out
}
/// A wild find (quest reward / exploration) — Iron…Sigil, occasionally set pieces.
pub fn wild_find(seed: u64) -> Item { roll_item(mix(seed ^ 0x5151_5151), Tier::Sigil, Source::Wild) }

// ───────────────────────── boss gating + loot ─────────────────────────

/// Minimum gear-score to fight a boss on fair terms. Scales with the boss's hp + abilities — so the
/// merkle-generated heavies demand a real set (Sigil+), not store starters.
pub fn gear_req(boss: &MerkleBoss) -> i32 { (boss.hp - 60) * 6 + boss.abilities.len() as i32 * 120 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict { Doable, Grim, Suicidal }

/// Forecast a fight given the hero's gear. Under-geared → the boss hits harder / you hit softer.
#[derive(Debug, Clone, Copy)]
pub struct Forecast { pub gear_score: i32, pub gear_req: i32, pub verdict: Verdict, pub dmg_taken_mult: f32, pub dmg_dealt_mult: f32 }
pub fn forecast(loadout: &Loadout, boss: &MerkleBoss) -> Forecast {
    let gs = loadout.gear_score();
    let req = gear_req(boss);
    let ratio = if req == 0 { 2.0 } else { gs as f32 / req as f32 };
    let verdict = if ratio >= 1.0 { Verdict::Doable } else if ratio >= 0.55 { Verdict::Grim } else { Verdict::Suicidal };
    // under-geared: take up to ~3× damage, deal as little as ~0.35× — "really hard to beat"
    let dmg_taken_mult = (1.0 / ratio.clamp(0.33, 1.0)).min(3.0);
    let dmg_dealt_mult = ratio.clamp(0.35, 1.0);
    Forecast { gear_score: gs, gear_req: req, verdict, dmg_taken_mult, dmg_dealt_mult }
}

/// Loot from a slain boss — ALWAYS at least one item, tier scaled to the boss, deterministic from its
/// Merkle root (so the drop is committed by the same decisions that forged the boss). Bigger bosses
/// can yield Mythic/Relic — gear you can't buy anywhere.
pub fn boss_loot(boss: &MerkleBoss) -> Vec<Item> {
    let r = boss.root;
    let count = 1 + (r % 2) as usize; // 1–2 drops, always ≥1
    // max tier scales with boss hp: a 160-hp merkle-heavy can drop Relic
    let max_tier = if boss.hp >= 150 { Tier::Relic } else if boss.hp >= 120 { Tier::Mythic } else { Tier::Sigil };
    (0..count).map(|k| roll_item(mix(r.wrapping_add(k as u64 * 0x9E37)), max_tier, Source::BossDrop)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merkle::{gen_boss};

    #[test]
    fn tiers_escalate_and_power_scales() {
        assert!(Tier::Ashen < Tier::Relic);
        assert!(Tier::Relic.mult() > Tier::Ashen.mult() * 8);
        let lo = Item { name:"x".into(), tier:Tier::Ashen, slot:Slot::Weapon, roll:10, set:None, source:Source::Store };
        let hi = Item { name:"y".into(), tier:Tier::Relic, slot:Slot::Weapon, roll:1, set:None, source:Source::BossDrop };
        assert!(hi.power() > lo.power(), "even a roll-1 Relic beats a roll-10 Ashen");
    }

    #[test]
    fn store_sells_only_low_tiers() {
        let cat = store_catalog();
        assert!(!cat.is_empty());
        assert!(cat.iter().all(|(it,_)| it.tier <= Tier::Iron), "store stops at Iron — the rest is earned");
        assert!(cat.iter().all(|(_,cost)| *cost > 0));
    }

    #[test]
    fn set_bonus_rewards_matching_pieces() {
        let mut l = Loadout::default();
        l.equip(Item{name:"a".into(),tier:Tier::Sigil,slot:Slot::Weapon,roll:5,set:Some("Emberweave"),source:Source::Wild});
        let solo = l.gear_score();
        l.equip(Item{name:"b".into(),tier:Tier::Sigil,slot:Slot::Armor,roll:5,set:Some("Emberweave"),source:Source::Wild});
        // two Emberweave pieces → set bonus kicks in (more than just the added piece's raw power baseline)
        assert!(l.set_bonus() > 0, "2-piece set grants a bonus");
        assert!(l.gear_score() > solo);
    }

    #[test]
    fn under_geared_boss_is_brutal_geared_is_doable() {
        let boss = gen_boss(0x00FF_1234_5678_9ABC); // a sizable merkle boss
        let naked = Loadout::default();
        let f0 = forecast(&naked, &boss);
        assert_eq!(f0.verdict, Verdict::Suicidal, "no gear vs a heavy → suicidal");
        assert!(f0.dmg_taken_mult > 1.5 && f0.dmg_dealt_mult < 1.0);
        // kit up with several boss-tier items
        let mut geared = Loadout::default();
        for k in 0..6 { geared.equip(roll_item(0xABCD ^ k, Tier::Relic, Source::BossDrop)); }
        let f1 = forecast(&geared, &boss);
        assert!(f1.gear_score > f0.gear_score);
        assert!(matches!(f1.verdict, Verdict::Doable | Verdict::Grim), "kitted → at least grim, not suicidal");
    }

    #[test]
    fn every_boss_kill_drops_loot_deterministically() {
        let boss = gen_boss(0x1234_5678);
        let a = boss_loot(&boss);
        let b = boss_loot(&boss);
        assert!(!a.is_empty(), "a kill ALWAYS drops at least one item");
        assert_eq!(a, b, "loot is committed by the boss's merkle root (deterministic)");
        assert!(a.iter().all(|it| it.source == Source::BossDrop));
    }

    #[test]
    fn big_boss_can_drop_unbuyable_tiers() {
        let mut heavy = gen_boss(0x42); heavy.hp = 160; // a merkle-heavy
        let loot = boss_loot(&heavy);
        // store caps at Iron; a heavy can drop far above that
        assert!(loot.iter().any(|it| it.tier >= Tier::Sigil) || heavy.hp < 150);
    }
}
