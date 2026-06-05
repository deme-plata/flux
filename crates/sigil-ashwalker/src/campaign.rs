#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Biome {
    Cinderwaste,
    BasaltReach,
    EmberMarsh,
    CrownRuins,
    AshenSea,
    ObsidianSpire,
    GlassDunes,
    TheGate,
}

impl Biome {
    pub fn name(&self) -> &'static str {
        match self {
            Biome::Cinderwaste => "Cinderwaste",
            Biome::BasaltReach => "Basalt Reach",
            Biome::EmberMarsh => "Ember Marsh",
            Biome::CrownRuins => "Crown Ruins",
            Biome::AshenSea => "Ashen Sea",
            Biome::ObsidianSpire => "Obsidian Spire",
            Biome::GlassDunes => "Glass Dunes",
            Biome::TheGate => "The Gate",
        }
    }

    pub fn hazard(&self) -> &'static str {
        match self {
            Biome::Cinderwaste => "Scalding ashstorms peel flesh from bone.",
            Biome::BasaltReach => "Sinkholes of molten rock swallow the unwary.",
            Biome::EmberMarsh => "Sulfurous fumes ignite without warning.",
            Biome::CrownRuins => "Ancient curses seep through fractured stone.",
            Biome::AshenSea => "Restless ghosts drag the living into silt.",
            Biome::ObsidianSpire => "Shards of volcanic glass rain from above.",
            Biome::GlassDunes => "Heat mirages drive travellers mad with thirst.",
            Biome::TheGate => "The air itself screams with unreality.",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeKind {
    Combat,
    Elite,
    MiniBoss,
    Boss,
    Loot,
    Shrine,
    Rest,
    Merchant,
    Mystery,
}

pub struct RunNode {
    pub id: u8,
    pub biome: Biome,
    pub kind: NodeKind,
    pub tier: u8,
    pub depth: u8,
    pub next: Vec<u8>,
    pub blurb: &'static str,
}

pub struct Campaign {
    pub seed: u64,
    pub nodes: Vec<RunNode>,
    pub start: u8,
}

impl Campaign {
    pub fn neighbors(&self, id: u8) -> Vec<u8> {
        if let Some(node) = self.node(id) {
            node.next.clone()
        } else {
            Vec::new()
        }
    }

    pub fn boss_node(&self) -> &RunNode {
        self.nodes.iter().find(|n| matches!(n.kind, NodeKind::Boss)).expect("No Boss node found")
    }

    pub fn node(&self, id: u8) -> Option<&RunNode> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.state
    }

    fn next_u8(&mut self) -> u8 {
        self.next_u64() as u8
    }

    fn next_range(&mut self, lo: u8, hi: u8) -> u8 {
        lo + (self.next_u64() % ((hi - lo + 1) as u64)) as u8
    }
}

fn blurb_for(biome: &Biome, kind: &NodeKind) -> &'static str {
    match (biome, kind) {
        (Biome::Cinderwaste, NodeKind::Combat) => "Cinderwaste – ash-choked alleys swarm with ember-eyed foes.",
        (Biome::Cinderwaste, NodeKind::Elite) => "Cinderwaste – a blackened colossus rises from the pyre.",
        (Biome::Cinderwaste, NodeKind::MiniBoss) => "Cinderwaste – the ashen lord demands tribute in blood.",
        (Biome::Cinderwaste, NodeKind::Boss) => "Cinderwaste – the Heart of the Furnace pulses molten rage.",
        (Biome::Cinderwaste, NodeKind::Loot) => "Cinderwaste – salvage gleams amid the soot.",
        (Biome::Cinderwaste, NodeKind::Shrine) => "Cinderwaste – a dying flame offers a whispered blessing.",
        (Biome::Cinderwaste, NodeKind::Rest) => "Cinderwaste – a hollowed outcrop shields from the ash.",
        (Biome::Cinderwaste, NodeKind::Merchant) => "Cinderwaste – a charred automaton trades in relics.",
        (Biome::Cinderwaste, NodeKind::Mystery) => "Cinderwaste – a glyph pulses beneath the cinders.",
        (Biome::BasaltReach, NodeKind::Combat) => "Basalt Reach – jagged pillars hide patrolling constructs.",
        (Biome::BasaltReach, NodeKind::Elite) => "Basalt Reach – a living avalanche of stone and fury.",
        (Biome::BasaltReach, NodeKind::MiniBoss) => "Basalt Reach – the earth-shaker commands the deep.",
        (Biome::BasaltReach, NodeKind::Boss) => "Basalt Reach – the Obsidian Monarch of the chasm.",
        (Biome::BasaltReach, NodeKind::Loot) => "Basalt Reach – magmatic ore crystallises in veins.",
        (Biome::BasaltReach, NodeKind::Shrine) => "Basalt Reach – a geyser of rejuvenating steam.",
        (Biome::BasaltReach, NodeKind::Rest) => "Basalt Reach – a basalt ledge warmed by geothermal vents.",
        (Biome::BasaltReach, NodeKind::Merchant) => "Basalt Reach – a hermit of the fissure barters in gems.",
        (Biome::BasaltReach, NodeKind::Mystery) => "Basalt Reach – a rune-carved obelisk hums with power.",
        (Biome::EmberMarsh, NodeKind::Combat) => "Ember Marsh – bubbling mud conceals ambush predators.",
        (Biome::EmberMarsh, NodeKind::Elite) => "Ember Marsh – a fiery behemoth wades through the bog.",
        (Biome::EmberMarsh, NodeKind::MiniBoss) => "Ember Marsh – the pyre-witch ignites the very air.",
        (Biome::EmberMarsh, NodeKind::Boss) => "Ember Marsh – the Sunken Conflagration, eternal blaze.",
        (Biome::EmberMarsh, NodeKind::Loot) => "Ember Marsh – glowing spores illuminate lost treasures.",
        (Biome::EmberMarsh, NodeKind::Shrine) => "Ember Marsh – a steam-vent shrine cleanses corruption.",
        (Biome::EmberMarsh, NodeKind::Rest) => "Ember Marsh – a dry hummock offers brief respite.",
        (Biome::EmberMarsh, NodeKind::Merchant) => "Ember Marsh – a floating isle market of alchemical wares.",
        (Biome::EmberMarsh, NodeKind::Mystery) => "Ember Marsh – a mirrored pool shows a different path.",
        (Biome::CrownRuins, NodeKind::Combat) => "Crown Ruins – crumbling halls echo with spectral battle.",
        (Biome::CrownRuins, NodeKind::Elite) => "Crown Ruins – a fallen knight rises in golden armour.",
        (Biome::CrownRuins, NodeKind::MiniBoss) => "Crown Ruins – the royal shade commands legions of ash.",
        (Biome::CrownRuins, NodeKind::Boss) => "Crown Ruins – the Crimson King enthroned in memory.",
        (Biome::CrownRuins, NodeKind::Loot) => "Crown Ruins – regal jewels glitter amid the rubble.",
        (Biome::CrownRuins, NodeKind::Shrine) => "Crown Ruins – an ancient altar offers a crown's boon.",
        (Biome::CrownRuins, NodeKind::Rest) => "Crown Ruins – a collapsed throne room provides shelter.",
        (Biome::CrownRuins, NodeKind::Merchant) => "Crown Ruins – a ghostly merchant trades in forgotten lore.",
        (Biome::CrownRuins, NodeKind::Mystery) => "Crown Ruins – a ghostly door flickers in and out of time.",
        (Biome::AshenSea, NodeKind::Combat) => "Ashen Sea – grey waves break with skeletal sailors.",
        (Biome::AshenSea, NodeKind::Elite) => "Ashen Sea – a leviathan of ash and brine surfaces.",
        (Biome::AshenSea, NodeKind::MiniBoss) => "Ashen Sea – the Drowned Navigator steers a ghost ship.",
        (Biome::AshenSea, NodeKind::Boss) => "Ashen Sea – the Tidal Pyre, a sentient inferno at sea.",
        (Biome::AshenSea, NodeKind::Loot) => "Ashen Sea – sunken chests glow with otherworldly light.",
        (Biome::AshenSea, NodeKind::Shrine) => "Ashen Sea – a floating shrine of salt and ash.",
        (Biome::AshenSea, NodeKind::Rest) => "Ashen Sea – a derelict raft offers meagre comfort.",
        (Biome::AshenSea, NodeKind::Merchant) => "Ashen Sea – a drowned merchant's soul trades in cursed coin.",
        (Biome::AshenSea, NodeKind::Mystery) => "Ashen Sea – a whirlpool reveals a submerged gate.",
        (Biome::ObsidianSpire, NodeKind::Combat) => "Obsidian Spire – jagged ledges provide treacherous footing.",
        (Biome::ObsidianSpire, NodeKind::Elite) => "Obsidian Spire – a crystalline golem shatters the air.",
        (Biome::ObsidianSpire, NodeKind::MiniBoss) => "Obsidian Spire – the shard-queen weaves glassy death.",
        (Biome::ObsidianSpire, NodeKind::Boss) => "Obsidian Spire – the Void Mirror reflects oblivion.",
        (Biome::ObsidianSpire, NodeKind::Loot) => "Obsidian Spire – faceted gems store fragments of light.",
        (Biome::ObsidianSpire, NodeKind::Shrine) => "Obsidian Spire – a prismatic altar refracts pain into power.",
        (Biome::ObsidianSpire, NodeKind::Rest) => "Obsidian Spire – a glass cavern muffles all sound.",
        (Biome::ObsidianSpire, NodeKind::Merchant) => "Obsidian Spire – a faceless merchant trades in reflections.",
        (Biome::ObsidianSpire, NodeKind::Mystery) => "Obsidian Spire – a mirror cracks, showing a future self.",
        (Biome::GlassDunes, NodeKind::Combat) => "Glass Dunes – mirage warriors shift and strike without form.",
        (Biome::GlassDunes, NodeKind::Elite) => "Glass Dunes – a sandworm of molten glass burrows through reality.",
        (Biome::GlassDunes, NodeKind::MiniBoss) => "Glass Dunes – the Sun-Scorched Sultan commands the dunes.",
        (Biome::GlassDunes, NodeKind::Boss) => "Glass Dunes – the Prismatic Sandglass, eternal timekeeper.",
        (Biome::GlassDunes, NodeKind::Loot) => "Glass Dunes – heat-warped coins glimmer under the sun.",
        (Biome::GlassDunes, NodeKind::Shrine) => "Glass Dunes – a microscopic oasis offers a moment of clarity.",
        (Biome::GlassDunes, NodeKind::Rest) => "Glass Dunes – the shade of a glass pillar breaks the heat.",
        (Biome::GlassDunes, NodeKind::Merchant) => "Glass Dunes – a nomad's caravan appears from a heat shimmer.",
        (Biome::GlassDunes, NodeKind::Mystery) => "Glass Dunes – a sand-whirlpool reveals an underground sanctum.",
        (Biome::TheGate, NodeKind::Combat) => "The Gate – reality fractures as guardians of the threshold attack.",
        (Biome::TheGate, NodeKind::Elite) => "The Gate – a Warden of the In-Between wields pure entropy.",
        (Biome::TheGate, NodeKind::MiniBoss) => "The Gate – the Keybearer demands a toll of sanity.",
        (Biome::TheGate, NodeKind::Boss) => "The Gate – the One Who Watches cannot be fully perceived.",
        (Biome::TheGate, NodeKind::Loot) => "The Gate – a fragment of a broken cosmos pulses with potential.",
        (Biome::TheGate, NodeKind::Shrine) => "The Gate – a kneeling figure offers a choice.",
        (Biome::TheGate, NodeKind::Rest) => "The Gate – a pocket dimension of serene nothing.",
        (Biome::TheGate, NodeKind::Merchant) => "The Gate – a being of pure want trades in forgotten desires.",
        (Biome::TheGate, NodeKind::Mystery) => "The Gate – a door that leads to all doors and none.",
    }
}

fn choose_kind(rng: &mut Lcg, depth: u8, is_boss: bool) -> NodeKind {
    if is_boss {
        return NodeKind::Boss;
    }
    if depth == 0 {
        return NodeKind::Combat;
    }
    // mid-run gate: depth 3 or 4? We'll enforce one MiniBoss at a random depth >=3
    // But we handle MiniBoss placement separately.
    let roll = rng.next_u8() % 100;
    if roll < 30 {
        NodeKind::Combat
    } else if roll < 50 {
        NodeKind::Loot
    } else if roll < 65 {
        NodeKind::Shrine
    } else if roll < 75 {
        NodeKind::Rest
    } else if roll < 85 {
        NodeKind::Merchant
    } else if roll < 95 {
        NodeKind::Mystery
    } else {
        NodeKind::Elite
    }
}

pub fn generate(seed: u64) -> Campaign {
    let mut rng = Lcg::new(seed);
    const D_MAX: u8 = 5;

    // Determine number of nodes per depth (min 1 per depth, max 4 for depths 1-4)
    let mut counts_per_depth = [1u8; 6]; // indices 0..5
    for d in 1..=4 {
        counts_per_depth[d as usize] = 1 + (rng.next_u8() % 4); // 1..4
    }
    // adjust total to be between 12 and 16
    let total_no_boss: u8 = counts_per_depth[0] + counts_per_depth[1] + counts_per_depth[2] + counts_per_depth[3] + counts_per_depth[4];
    let total = total_no_boss + counts_per_depth[5]; // = total including boss = total_no_boss + 1
    // total ranges from 6 to 18. Need to adjust to 12-16.
    let mut extra_needed: i8 = 12 - (total as i8);
    if extra_needed > 0 {
        // add random nodes to depths 1-4
        for _ in 0..extra_needed {
            let d = 1 + (rng.next_u8() % 4);
            counts_per_depth[d as usize] += 1;
        }
    } else if total > 16 {
        let mut excess = (total - 16) as i8;
        while excess > 0 {
            let d = 1 + (rng.next_u8() % 4);
            if counts_per_depth[d as usize] > 1 {
                counts_per_depth[d as usize] -= 1;
                excess -= 1;
            }
        }
    }

    // Build nodes
    let mut nodes: Vec<RunNode> = Vec::new();
    let mut id_counter: u8 = 0;
    let mut miniboss_depth: u8 = 0;
    // choose a depth for MiniBoss (>=3)
    let candidate_depths: Vec<u8> = (3..=4).collect();
    let idx = rng.next_range(0, candidate_depths.len() as u8 - 1) as usize;
    miniboss_depth = candidate_depths[idx];
    let mut miniboss_placed = false;

    for depth in 0..=D_MAX {
        let count = counts_per_depth[depth as usize];
        for _ in 0..count {
            let is_boss = depth == D_MAX;
            let is_miniboss = if !miniboss_placed && depth == miniboss_depth { true } else { false };
            let kind = if is_boss {
                NodeKind::Boss
            } else if is_miniboss {
                miniboss_placed = true;
                NodeKind::MiniBoss
            } else {
                choose_kind(&mut rng, depth, false)
            };
            // assign biome randomly
            let biomes = [
                Biome::Cinderwaste,
                Biome::BasaltReach,
                Biome::EmberMarsh,
                Biome::CrownRuins,
                Biome::AshenSea,
                Biome::ObsidianSpire,
                Biome::GlassDunes,
                Biome::TheGate,
            ];
            let biome_idx = rng.next_u8() as usize % biomes.len();
            let biome = biomes[biome_idx].clone();
            let tier = depth + 1;
            let blurb = blurb_for(&biome, &kind);
            let node = RunNode {
                id: id_counter,
                biome,
                kind,
                tier,
                depth,
                next: Vec::new(),
                blurb,
            };
            nodes.push(node);
            id_counter += 1;
        }
    }

    // Build adjacency: connect each node to nodes at strictly higher depth.
    // First, group node indices by depth
    let mut depth_indices: Vec<Vec<usize>> = vec![Vec::new(); (D_MAX + 1) as usize];
    for (i, node) in nodes.iter().enumerate() {
        depth_indices[node.depth as usize].push(i);
    }

    // For each depth, ensure each child has at least one parent, then add extra random edges.
    for d in 0..D_MAX as usize {
        let parents = &depth_indices[d];
        let children = &depth_indices[d + 1];
        if parents.is_empty() || children.is_empty() {
            continue;
        }
        // guarantee each child has at least one incoming edge
        for &child_idx in children {
            let parent_idx = parents[rng.next_u8() as usize % parents.len()];
            let _cid = nodes[child_idx].id; nodes[parent_idx].next.push(_cid);
        }
        // extra edges: for each parent, add some children with probability
        for &parent_idx in parents {
            let already_connected: std::collections::HashSet<u8> = nodes[parent_idx].next.iter().cloned().collect();
            for &child_idx in children {
                let child_id = nodes[child_idx].id;
                if !already_connected.contains(&child_id) && rng.next_u8() % 3 == 0 {
                    nodes[parent_idx].next.push(child_id);
                }
            }
        }
    }

    // Optionally add skip connections (depth d to depth d+2) for extra branching? Not required, but can add variety.
    for d in 0..D_MAX as usize {
        if d + 2 > D_MAX as usize {
            break;
        }
        let parents = &depth_indices[d];
        let grandchildren = &depth_indices[d + 2];
        if parents.is_empty() || grandchildren.is_empty() {
            continue;
        }
        for &parent_idx in parents {
            if rng.next_u8() % 4 == 0 {
                let grandchild_idx = grandchildren[rng.next_u8() as usize % grandchildren.len()];
                let _gid = nodes[grandchild_idx].id; nodes[parent_idx].next.push(_gid);
            }
        }
    }

    let start_id = nodes[0].id;
    Campaign {
        seed,
        nodes,
        start: start_id,
    }
}

pub fn encounter(biome: &Biome, tier: u8, seed: u64) -> Vec<&'static str> {
    let mut rng = Lcg::new(seed);
    let mut foe_pool = Vec::new();
    // basic foes always available
    foe_pool.push("ashling-swarm");
    foe_pool.push("cinder-wraith");
    let additional = match tier {
        0 | 1 => vec![],
        2 => vec!["basalt-golem"],
        3 => vec!["basalt-golem", "ember-drake"],
        4 => vec!["basalt-golem", "ember-drake", "crown-revenant"],
        5 => vec!["basalt-golem", "ember-drake", "crown-revenant", "ash-mariner"],
        _ => vec!["basalt-golem", "ember-drake", "crown-revenant", "ash-mariner"],
    };
    foe_pool.extend(additional.into_iter());

    // optional biome-specific foes? Not required, but add some flavor
    match biome {
        Biome::Cinderwaste => foe_pool.push("ashling-swarm"), // already present
        Biome::BasaltReach => foe_pool.push("basalt-golem"),
        Biome::EmberMarsh => foe_pool.push("ember-drake"),
        Biome::CrownRuins => foe_pool.push("crown-revenant"),
        Biome::AshenSea => foe_pool.push("ash-mariner"),
        Biome::ObsidianSpire => foe_pool.push("cinder-wraith"),
        Biome::GlassDunes => foe_pool.push("ashling-swarm"),
        Biome::TheGate => foe_pool.push("crown-revenant"),
    }

    let count = 1 + (rng.next_u8() % 4); // 1..4
    let mut foes = Vec::new();
    for _ in 0..count {
        let idx = rng.next_u8() as usize % foe_pool.len();
        foes.push(foe_pool[idx]);
    }
    foes
}

pub fn ascii_map(c: &Campaign) -> String {
    let mut output = String::new();
    // find max depth
    let max_depth = c.nodes.iter().map(|n| n.depth).max().unwrap_or(0);
    for depth in 0..=max_depth {
        output.push_str(&format!("--- Depth {} ---\n", depth));
        let nodes_at_depth: Vec<&RunNode> = c.nodes.iter().filter(|n| n.depth == depth).collect();
        for node in nodes_at_depth {
            let kind_code = match node.kind {
                NodeKind::Combat => "C",
                NodeKind::Elite => "E",
                NodeKind::MiniBoss => "M",
                NodeKind::Boss => "B",
                NodeKind::Loot => "L",
                NodeKind::Shrine => "S",
                NodeKind::Rest => "R",
                NodeKind::Merchant => "$",
                NodeKind::Mystery => "?",
            };
            let next_str: Vec<String> = node.next.iter().map(|id| id.to_string()).collect();
            let next_display = if next_str.is_empty() {
                String::from("terminal")
            } else {
                next_str.join(",")
            };
            output.push_str(&format!("  {} (id:{}) {} -> [{}]\n", kind_code, node.id, node.blurb, next_display));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_determinism() {
        let c1 = generate(42);
        let c2 = generate(42);
        assert_eq!(c1.nodes.len(), c2.nodes.len());
        for (n1, n2) in c1.nodes.iter().zip(c2.nodes.iter()) {
            assert_eq!(n1.id, n2.id);
            assert_eq!(n1.depth, n2.depth);
            assert_eq!(n1.kind as u8, n2.kind as u8);
            assert_eq!(n1.next.len(), n2.next.len());
            for (a, b) in n1.next.iter().zip(n2.next.iter()) {
                assert_eq!(a, b);
            }
        }
    }

    #[test]
    fn exactly_one_boss() {
        for seed in 0..100 {
            let c = generate(seed);
            let boss_count = c.nodes.iter().filter(|n| matches!(n.kind, NodeKind::Boss)).count();
            assert_eq!(boss_count, 1, "seed {} has {} bosses", seed, boss_count);
        }
    }

    #[test]
    fn all_next_ids_valid() {
        for seed in 0..100 {
            let c = generate(seed);
            let valid_ids: std::collections::HashSet<u8> = c.nodes.iter().map(|n| n.id).collect();
            for node in &c.nodes {
                for &next_id in &node.next {
                    assert!(valid_ids.contains(&next_id), "seed {} node {} has invalid next id {}", seed, node.id, next_id);
                }
            }
        }
    }

    #[test]
    fn depth_non_decreasing() {
        for seed in 0..100 {
            let c = generate(seed);
            // depth must never decrease when following a path from start.
            // Since DAG, we can check that each node's next have depth > node.depth.
            for node in &c.nodes {
                for &next_id in &node.next {
                    let next_node = c.node(next_id).expect("invalid next id");
                    assert!(next_node.depth > node.depth, "seed {} node {} depth {} has next {} depth {} which is not greater", seed, node.id, node.depth, next_id, next_node.depth);
                }
            }
        }
    }
}
