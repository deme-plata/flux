//! A terminal isometric 3D ARPG bestiary for the Ashlands.
//! Contains definitions for 18+ foes and a tier-5 boss.

/// Specification of a single foe in the Ashlands.
pub struct FoeSpec {
    /// Unique identifier string.
    pub key: &'static str,
    /// Display name.
    pub name: &'static str,
    /// ASCII glyph representing the foe.
    pub glyph: char,
    /// Hit points.
    pub hp: i32,
    /// Damage per attack.
    pub damage: i32,
    /// Movement speed (0-255).
    pub speed: u8,
    /// Description of the attack telegraph.
    pub telegraph: &'static str,
    /// Counter play (sign or MCP-combo).
    pub counter: &'static str,
    /// Threat tier (1-5).
    pub tier: u8,
    /// Two-sentence lore fragment.
    pub lore: &'static str,
}

/// Returns a complete bestiary of all known Ashlands foes.
pub fn bestiary() -> Vec<FoeSpec> {
    vec![
        // Tier 1 – Ashling Swarms
        FoeSpec {
            key: "ashling_swarm",
            name: "Ashling Swarm",
            glyph: '·',
            hp: 8,
            damage: 4,
            speed: 12,
            telegraph: "A swirling cloud of ash coalesces into tiny glowing embers.",
            counter: "Sign of Extinction: scatter them with a wide-area gust or flame burst.",
            tier: 1,
            lore: "Born from the first embers of the Ashlands' eternal fires, these fragments of lost souls swarm together. A single sting is harmless, but a thousand bites can strip flesh from bone in heartbeats.",
        },
        // Tier 1 – Cinder-Wraith
        FoeSpec {
            key: "cinder_wraith",
            name: "Cinder-Wraith",
            glyph: 'W',
            hp: 12,
            damage: 6,
            speed: 10,
            telegraph: "The air shimmers with heat as a tall, cloaked figure of ash appears.",
            counter: "MCP-Combo: Move sideways to avoid its lunge, then Cast a frostbolt to destabilise it, then Parry its final swipe.",
            tier: 1,
            lore: "These are the echoes of those who refused to ascend, forever bound to the Ashlands. Their touch spreads the curse of cinder rot, a slow death that turns flesh to embers.",
        },
        // Tier 1 – Ember Imp
        FoeSpec {
            key: "ember_imp",
            name: "Ember Imp",
            glyph: 'i',
            hp: 10,
            damage: 5,
            speed: 14,
            telegraph: "A small, spindly creature leaps from a pile of hot ash, claws glowing red.",
            counter: "Sign of Binding: root it in place, then destroy it from range.",
            tier: 1,
            lore: "Mischievous and cruel, ember imps delight in igniting travellers' belongings. They are the Ashlands' first line of defence against the unworthy.",
        },
        // Tier 1 – Soot Hound
        FoeSpec {
            key: "soot_hound",
            name: "Soot Hound",
            glyph: 'd',
            hp: 14,
            damage: 7,
            speed: 16,
            telegraph: "A low growl precedes a pack of three hounds made of ash and charcoal.",
            counter: "MCP-Combo: Move to separate them, Cast a shockwave to stun them, then Parry the lead hound's bite.",
            tier: 1,
            lore: "Tamed by the ash-mariners long ago, these hounds now run wild through the cinder fields. Their howls can be heard for miles, a warning of greater dangers.",
        },
        // Tier 2 – Ash Walker
        FoeSpec {
            key: "ash_walker",
            name: "Ash Walker",
            glyph: 'A',
            hp: 25,
            damage: 10,
            speed: 8,
            telegraph: "A humanoid shape made entirely of compacted ash rises from the ground.",
            counter: "Sign of Shattering: strike its core with a blunt force spell to break it apart.",
            tier: 2,
            lore: "These constructs were once guardians of ancient Ashland cities. They march endlessly, oblivious to time, still following orders from a forgotten king.",
        },
        // Tier 2 – Pyre Spectre
        FoeSpec {
            key: "pyre_spectre",
            name: "Pyre Spectre",
            glyph: 'S',
            hp: 20,
            damage: 12,
            speed: 10,
            telegraph: "A ghostly blue flame flickers in the air, leaving a trail of cold ash.",
            counter: "MCP-Combo: Move behind it while it charges, Cast a light spell to blind it, then Parry its spectral claw.",
            tier: 2,
            lore: "Not all who burn in the Ashlands die; some become these spectres, bound to guard the pyres of the Crown. They whisper secrets of the dead to those who dare listen.",
        },
        // Tier 2 – Magma Larva
        FoeSpec {
            key: "magma_larva",
            name: "Magma Larva",
            glyph: 'L',
            hp: 18,
            damage: 8,
            speed: 6,
            telegraph: "The ground bulges and a worm-like creature of molten rock bursts forth.",
            counter: "Sign of Frost: freeze it before it spits lava, then shatter it with a strike.",
            tier: 2,
            lore: "These larvae feed on the heat of the Ashlands' deepest fissures. When threatened, they erupt in a shower of liquid stone, burning everything nearby.",
        },
        // Tier 2 – Cinder Gargoyle
        FoeSpec {
            key: "cinder_gargoyle",
            name: "Cinder Gargoyle",
            glyph: 'G',
            hp: 30,
            damage: 14,
            speed: 8,
            telegraph: "A stone statue with glowing eyes detaches from a wall and lunges.",
            counter: "MCP-Combo: Move under its dive, Cast a gravity well to pin it, then Parry its stone fist.",
            tier: 2,
            lore: "These gargoyles were carved from basalt and infused with the spirit of the Ashlands. They are protectors of the first Houses, motionless until a trespasser draws near.",
        },
        // Tier 3 – Basalt Golem
        FoeSpec {
            key: "basalt_golem",
            name: "Basalt Golem",
            glyph: 'B',
            hp: 80,
            damage: 25,
            speed: 4,
            telegraph: "The earth rumbles and a massive humanoid made of dark volcanic rock stands up.",
            counter: "Sign of Weakening: attack its leg joints to slow it, then avoid its ground slam.",
            tier: 3,
            lore: "Forged from the solidified lava flows of the Mount of Ash, these golems are nearly indestructible. They guard the paths to the higher tiers, and no weapon can scratch their hide without proper enchantment.",
        },
        // Tier 3 – Crown-Revenant
        FoeSpec {
            key: "crown_revenant",
            name: "Crown-Revenant",
            glyph: 'C',
            hp: 45,
            damage: 18,
            speed: 10,
            telegraph: "A skeletal figure wearing a rusted crown raises a sword wreathed in black flame.",
            counter: "MCP-Combo: Move to flank it, Cast a silence spell to disrupt its curse, then Parry its overhead strike.",
            tier: 3,
            lore: "These are the remnants of the old crown lineage, cursed to guard the ashes of their ancestors. They remember only their duty: slay all who approach the throne.",
        },
        // Tier 3 – Ember-Drake (adolescent)
        FoeSpec {
            key: "ember_drake",
            name: "Ember-Drake",
            glyph: 'D',
            hp: 60,
            damage: 22,
            speed: 6,
            telegraph: "A serpentine dragon covered in glowing embers unfurls its wings, heating the air.",
            counter: "Sign of Cleansing: douse its flames with a rain spell, then strike its throat.",
            tier: 3,
            lore: "Young ember-drakes are still learning to control their fiery breath. They are fiercely territorial and will pursue intruders across the entire Ashlands.",
        },
        // Tier 3 – Ash-Mariner (survivor)
        FoeSpec {
            key: "ash_mariner",
            name: "Ash-Mariner",
            glyph: 'M',
            hp: 50,
            damage: 16,
            speed: 8,
            telegraph: "A gnarled sailor with a glowing compass and a cutlass made of obsidian.",
            counter: "MCP-Combo: Move to dodge its compass flare, Cast a fog to confuse its navigation, then Parry its slash.",
            tier: 3,
            lore: "Once a navigator of the Cinder Sea, this mariner now wanders the ash wastes, seeking a way home. He offers cryptic advice to adventurers who best him in combat.",
        },
        // Tier 4 – Ash-Knight
        FoeSpec {
            key: "ash_knight",
            name: "Ash-Knight",
            glyph: 'K',
            hp: 90,
            damage: 28,
            speed: 6,
            telegraph: "A knight in full obsidian armour, every step leaving a scorch mark.",
            counter: "Sign of Rupture: crack its armour with repeated focused strikes, then avoid its explosion.",
            tier: 4,
            lore: "These are the elite soldiers of the Ashlands' dead empire, eternally loyal to the Crown. They train endlessly in the Halls of Ash, waiting for the final battle.",
        },
        // Tier 4 – Crown Sentinel
        FoeSpec {
            key: "crown_sentinel",
            name: "Crown Sentinel",
            glyph: 'S',
            hp: 100,
            damage: 30,
            speed: 5,
            telegraph: "A floating orb of ash and ember with a single unblinking eye.",
            counter: "MCP-Combo: Move behind cover when it fires lasers, Cast a reflective barrier, then Parry its energy pulse.",
            tier: 4,
            lore: "These sentinels were built by the last Scrutineer to watch over the path to the House of Crown. They never sleep, never tire, and never show mercy.",
        },
        // Tier 4 – Lava Serpent
        FoeSpec {
            key: "lava_serpent",
            name: "Lava Serpent",
            glyph: 'V',
            hp: 70,
            damage: 24,
            speed: 7,
            telegraph: "A long, sinuous creature of molten rock emerges from a lava pool.",
            counter: "Sign of Solidity: cool its surface with water, then strike its head while it's brittle.",
            tier: 4,
            lore: "These serpents dwell in the hottest vents of the Ashlands, coiling around geysers of flame. Their scales are prized for forging unbreakable weapons.",
        },
        // Tier 4 – Phantasm of Ash
        FoeSpec {
            key: "phantasm_ash",
            name: "Phantasm of Ash",
            glyph: 'P',
            hp: 55,
            damage: 20,
            speed: 12,
            telegraph: "A translucent, shifting shape that flickers between multiple forms.",
            counter: "MCP-Combo: Move to keep it in sight, Cast a reveal truth spell, then Parry its illusionary strike.",
            tier: 4,
            lore: "These phantasms are the memories of the thousands who perished in the Great Burning. They mimic the dead, luring the living into traps of despair.",
        },
        // Tier 5 Boss – The Last Scrutineer of Ash
        FoeSpec {
            key: "last_scrutineer",
            name: "The Last Scrutineer of Ash",
            glyph: '☠',
            hp: 300,
            damage: 50,
            speed: 3,
            telegraph: "The sky darkens as a colossal figure wearing a crown of ash and eyes of pure fire descends.",
            counter: "MCP-Combo: Move through its three phase zones, Cast the combined Sign of Extinction and Cleansing, then Parry its final decree with a counter-spell of your own.",
            tier: 5,
            lore: "The Last Scrutineer is the ancient judge of all who climb the Ashlands, a being of pure will and cinder. He weighs your soul with a single glance; only the worthy may ascend to the House of Crown.",
        },
        FoeSpec {
            key: "glass-stalker", name: "Glass-Stalker", glyph: 'v',
            hp: 48, damage: 14, speed: 7, tier: 2,
            telegraph: "Its glass limbs go still and mirror-bright a half-beat before it lunges.",
            counter: "Parry the lunge; the reflection shatters and staggers it for a free combo.",
            lore: "Born in the Glass Dunes where heat fused the sand to blades. It hunts by reflection, and dies by its own.",
        },
        FoeSpec {
            key: "soot-harrier", name: "Soot-Harrier", glyph: 'h',
            hp: 130, damage: 26, speed: 5, tier: 4,
            telegraph: "Wings of packed soot fold back and a black plume gathers in its throat.",
            counter: "Sign of Cleansing to scatter the plume, then close fast before it re-inhales.",
            lore: "A carrion-thing grown fat on the Ashlands dead. Where it circles, a Crown has fallen.",
        },
        FoeSpec {
            key: "marrow-choir", name: "The Marrow Choir", glyph: 'W',
            hp: 70, damage: 20, speed: 5, tier: 3,
            telegraph: "Three skull-lanterns rise and inhale in unison before the dirge.",
            counter: "Break line-of-sight to one lantern, then Sign of Ward through the chord.",
            lore: "A knot of ash-bound dead that sings as one. Silence a single voice and the whole chorus stutters.",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bestiary_contains_18_or_more_foes() {
        let list = bestiary();
        assert!(
            list.len() >= 18,
            "Bestiary has {} entries, expected at least 18",
            list.len()
        );
    }

    #[test]
    fn all_foes_have_non_empty_fields() {
        for foe in bestiary().iter() {
            assert!(!foe.key.is_empty(), "Empty key for foe {:?}", foe.name);
            assert!(!foe.name.is_empty());
            assert!(!foe.telegraph.is_empty());
            assert!(!foe.counter.is_empty());
            assert!(!foe.lore.is_empty());
            assert!(foe.hp > 0);
            assert!(foe.damage > 0);
            assert!(foe.tier >= 1 && foe.tier <= 5);
        }
    }

    #[test]
    fn tier_5_boss_is_present() {
        let list = bestiary();
        let boss = list.iter().find(|f| f.tier == 5);
        assert!(boss.is_some(), "No tier-5 boss found");
        let boss = boss.unwrap();
        assert_eq!(boss.key, "last_scrutineer");
        assert_eq!(boss.name, "The Last Scrutineer of Ash");
        assert!(boss.hp >= 200);
    }
}
