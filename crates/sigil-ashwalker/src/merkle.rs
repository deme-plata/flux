//! merkle.rs — the Merkle-tree BOSS GENERATOR (prototype 2).
//!
//! Both games write **decisions** into one shared ledger: ASHWALKER run events (combos woven, foes
//! slain/converted, depth reached) and CROWN & ASH realm turns (alliances, betrayals, conquests).
//! The ledger is hashed into a **Merkle tree**; its **root** deterministically generates a UNIQUE
//! boss — name, stats, abilities, and a committed **weakness-combo** (two MCP tools). Because every
//! decision is a leaf, the boss is *provably* shaped by the choices: a Merkle **proof** shows a given
//! decision is committed in the boss, and tampering any decision changes the root → a different boss.
//!
//! Cross-game effect: the root is shared. A betrayal in Crown & Ash changes the next ASHWALKER boss;
//! an ASHWALKER ascension writes decisions that seed the realm. The `Merkle-Touched` trait
//! ([`crate::traits::Trait`]) lets a hero READ the committed weakness-combo before the fight.

use crate::{Mcp, Player};

// ───────────────────────── hashing ─────────────────────────

fn fnv1a(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.bytes() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); }
    h
}
fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e3779b97f4a7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}
/// Hash two child digests into a parent (order-sensitive — a real Merkle interior node).
fn node(l: u64, r: u64) -> u64 { splitmix(l.rotate_left(17) ^ r.wrapping_mul(0x100000001b3) ^ 0xD1B54A32D192ED03) }

// ───────────────────────── decisions + tree ─────────────────────────

/// One choice from either game. Its string form is the Merkle leaf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision { pub game: &'static str, pub kind: &'static str, pub value: String }
impl Decision {
    pub fn ashwalker(kind: &'static str, value: impl Into<String>) -> Decision { Decision { game: "ashwalker", kind, value: value.into() } }
    pub fn crown_ash(kind: &'static str, value: impl Into<String>) -> Decision { Decision { game: "crown_ash", kind, value: value.into() } }
    pub fn leaf_hash(&self) -> u64 { splitmix(fnv1a(&format!("{}:{}:{}", self.game, self.kind, self.value))) }
}

/// A Merkle tree over the decision ledger. Odd levels duplicate the last node (Bitcoin-style).
#[derive(Debug, Clone)]
pub struct MerkleTree { pub leaves: Vec<u64>, levels: Vec<Vec<u64>> }
impl MerkleTree {
    pub fn build(decisions: &[Decision]) -> MerkleTree {
        let mut leaves: Vec<u64> = decisions.iter().map(|d| d.leaf_hash()).collect();
        if leaves.is_empty() { leaves.push(splitmix(0xA5A5_5A5A)); } // empty ledger still has a (genesis) root
        let mut levels = vec![leaves.clone()];
        while levels.last().unwrap().len() > 1 {
            let cur = levels.last().unwrap();
            let mut next = Vec::with_capacity((cur.len() + 1) / 2);
            let mut i = 0;
            while i < cur.len() {
                let l = cur[i];
                let r = if i + 1 < cur.len() { cur[i + 1] } else { cur[i] }; // duplicate last if odd
                next.push(node(l, r));
                i += 2;
            }
            levels.push(next);
        }
        MerkleTree { leaves, levels }
    }
    pub fn root(&self) -> u64 { *self.levels.last().unwrap().first().unwrap() }

    /// Merkle proof for leaf `index`: the sibling digests from leaf → root (with side).
    pub fn proof(&self, index: usize) -> Vec<(u64, bool)> {
        let mut proof = Vec::new();
        let mut idx = index;
        for level in &self.levels {
            if level.len() <= 1 { break; }
            let sib = if idx % 2 == 0 { (idx + 1).min(level.len() - 1) } else { idx - 1 };
            let sib_is_right = sib > idx;
            proof.push((level[sib], sib_is_right));
            idx /= 2;
        }
        proof
    }
    /// Verify a leaf is committed under `root` via its proof.
    pub fn verify(leaf: u64, proof: &[(u64, bool)], root: u64) -> bool {
        let mut acc = leaf;
        for &(sib, sib_is_right) in proof {
            acc = if sib_is_right { node(acc, sib) } else { node(sib, acc) };
        }
        acc == root
    }
}

// ───────────────────────── boss generation ─────────────────────────

/// A unique boss generated from a Merkle root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerkleBoss {
    pub name: String,
    pub title: &'static str,
    pub root: u64,
    pub hp: i32,
    pub bite: i32,
    pub abilities: Vec<&'static str>,
    /// the committed weakness — chaining THIS MCP-combo breaks the boss fastest (Merkle-Touched reads it)
    pub weakness: [Mcp; 2],
    pub taunt: String,
}

const PRE:  [&str; 8] = ["Oss", "Vyr", "Mor", "Kael", "Zeth", "Ash", "Grim", "Nyx"];
const MID:  [&str; 8] = ["u", "a", "o", "ae", "i", "y", "ue", "ei"];
const SUF:  [&str; 8] = ["ary", "rax", "oth", "ynn", "thar", "ux", "eth", "or"];
const TITLES: [&str; 8] = ["the Hollow Frame", "of Unforked Ash", "Gate-Warden", "the Merkle-Bound",
    "Root of Cinders", "the Last Commit", "Warden of the Weave", "the Severed Branch"];
const ABILITIES: [&str; 8] = ["ash-nova", "bone-lattice trap", "glass shatter", "ember tithe",
    "rollback howl", "fork-bomb", "void garnish", "hash-quake"];
const MCPS: [Mcp; 6] = [Mcp::FluxCombo, Mcp::DexSwap, Mcp::ZkVeil, Mcp::CouncilQuorum, Mcp::Tribute, Mcp::Hashstorm];

/// Deterministically generate the boss from a Merkle root. Same root → same boss, always.
pub fn gen_boss(root: u64) -> MerkleBoss {
    let b = root;
    let name = format!("{}{}{}", PRE[(b & 7) as usize], MID[((b >> 3) & 7) as usize], SUF[((b >> 6) & 7) as usize]);
    let title = TITLES[((b >> 9) & 7) as usize];
    let hp = 70 + (b % 90) as i32;
    let bite = 12 + ((b >> 13) % 14) as i32;
    let a0 = ABILITIES[((b >> 17) & 7) as usize];
    let a1 = ABILITIES[((b >> 21) & 7) as usize];
    let abilities = if a0 == a1 { vec![a0] } else { vec![a0, a1] };
    // weakness-combo: two DISTINCT MCP tools from the root
    let w0 = ((b >> 25) % 6) as usize;
    let mut w1 = ((b >> 31) % 6) as usize;
    if w1 == w0 { w1 = (w1 + 1) % 6; }
    let weakness = [MCPS[w0], MCPS[w1]];
    let taunt = format!(
        "I am {name}, {title}. Your choices forged me — now they will break on me. \
         (committed weakness: {} ⊗ {})", weakness[0].name(), weakness[1].name());
    MerkleBoss { name, title, root, hp, bite, abilities, weakness, taunt }
}

/// Build the tree + generate the boss in one call.
pub fn roll_boss(decisions: &[Decision]) -> (MerkleTree, MerkleBoss) {
    let tree = MerkleTree::build(decisions);
    let boss = gen_boss(tree.root());
    (tree, boss)
}

/// Distil an ASHWALKER run into decisions (the choices that will shape the NEXT boss / the realm).
pub fn decisions_from_run(p: &Player) -> Vec<Decision> {
    let mut v = vec![
        Decision::ashwalker("slain", p.slain.to_string()),
        Decision::ashwalker("converted", p.converted.to_string()),
        Decision::ashwalker("depth", p.depth_z.to_string()),
        Decision::ashwalker("level", p.level.to_string()),
    ];
    for k in &p.combo_kinds { v.push(Decision::ashwalker("combo", *k)); }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Decision> {
        vec![
            Decision::ashwalker("combo", "Blink-Nova"),
            Decision::ashwalker("slain", "4"),
            Decision::crown_ash("allied", "House Ember"),
            Decision::crown_ash("betrayed", "House Glass"),
        ]
    }

    #[test]
    fn boss_is_deterministic_from_decisions() {
        let (_, a) = roll_boss(&sample());
        let (_, b) = roll_boss(&sample());
        assert_eq!(a, b, "same decisions → same boss");
        assert!(a.hp > 0 && a.bite > 0 && !a.abilities.is_empty());
        assert_ne!(a.weakness[0], a.weakness[1], "weakness-combo is two distinct tools");
    }

    #[test]
    fn a_changed_decision_changes_the_boss() {
        let (t1, b1) = roll_boss(&sample());
        let mut d2 = sample();
        d2[3] = Decision::crown_ash("allied", "House Glass"); // betrayal → alliance: a different choice
        let (t2, b2) = roll_boss(&d2);
        assert_ne!(t1.root(), t2.root(), "different decisions → different merkle root");
        assert!(b1.name != b2.name || b1.weakness != b2.weakness || b1.hp != b2.hp, "the boss actually differs");
    }

    #[test]
    fn merkle_proof_commits_a_decision() {
        let ds = sample();
        let tree = MerkleTree::build(&ds);
        let root = tree.root();
        // every decision is provably committed in the boss's root
        for (i, d) in ds.iter().enumerate() {
            let proof = tree.proof(i);
            assert!(MerkleTree::verify(d.leaf_hash(), &proof, root), "decision {i} proves under root");
        }
        // a decision that wasn't made does NOT verify
        let forged = Decision::crown_ash("allied", "House Nobody").leaf_hash();
        let proof0 = tree.proof(0);
        assert!(!MerkleTree::verify(forged, &proof0, root), "a non-committed decision must fail its proof");
    }

    #[test]
    fn cross_game_decisions_both_count() {
        // adding a Crown & Ash turn changes the Ashwalker boss → the games are linked through the root
        let only_aw = vec![Decision::ashwalker("slain", "3")];
        let with_ca = vec![Decision::ashwalker("slain", "3"), Decision::crown_ash("conquered", "Ashreach")];
        assert_ne!(MerkleTree::build(&only_aw).root(), MerkleTree::build(&with_ca).root());
    }

    #[test]
    fn run_to_decisions_then_boss() {
        let mut p = Player::new(crate::V3::new(0,0,0));
        p.slain = 5; p.combos_cast = 3; p.depth_z = 1; p.combo_kinds.insert("Blink-Nova"); p.combo_kinds.insert("Hashfire");
        let ds = decisions_from_run(&p);
        assert!(ds.len() >= 6);
        let (_, boss) = roll_boss(&ds);
        assert!(boss.taunt.contains(&boss.name) && boss.taunt.contains("weakness"));
    }
}
