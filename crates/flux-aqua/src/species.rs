//! The species taxonomy, lifted from the Water Robots & Kingdom of Life
//! whitepaper (Appendix A, "Water Robot Species Specifications").
//!
//! In the source repository this table existed only as LaTeX prose and a handful
//! of `println!` lines in `reticular_demo_simple.rs` — nothing in `void-walker`
//! actually had a species type, so no code could ever be specialised by role.
//! Here it is a real enum with the paper's numbers attached, which is what makes
//! per-role capacity limits (see `Droplet::store_dna`) and role-aware mesh duty
//! (see `crate::mesh`) possible.

use serde::{Deserialize, Serialize};

/// What a species is *for* in the colony.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    /// General computation.
    Compute,
    /// Long-term storage.
    Storage,
    /// Point-to-point transport.
    Transport,
    /// One-to-many announcement.
    Broadcast,
    /// Environmental sensing and discovery.
    Sensing,
    /// Security and threat response.
    Defence,
    /// Marketplace operations.
    Market,
    /// Resource harvesting and transformation.
    Harvest,
}

/// The ten species of the WR-KoL ecosystem: eight primaries plus two hybrids.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Species {
    /// *Homo computaticus* — general-purpose computation.
    Processor,
    /// *Bibliothecus perpetuus* — archival storage.
    Memory,
    /// *Mercurius velocis* — fast point-to-point transport.
    Messenger,
    /// *Radiophorus globalis* — system-wide announcements.
    Broadcaster,
    /// *Explorator aquaticus* — environmental sensing.
    Scout,
    /// *Protector systemicus* — security and toxin response.
    Guardian,
    /// *Mercator economicus* — OptiMart operations.
    Trader,
    /// *Collector abundantia* — resource harvesting.
    Resource,
    /// *Scientificus adaptabilis* — Processor × Memory hybrid.
    Scholar,
    /// *Negotiator pacificus* — Messenger × Trader hybrid.
    Diplomat,
}

impl Species {
    /// Every species, in taxonomy order.
    pub const ALL: [Species; 10] = [
        Species::Processor,
        Species::Memory,
        Species::Messenger,
        Species::Broadcaster,
        Species::Scout,
        Species::Guardian,
        Species::Trader,
        Species::Resource,
        Species::Scholar,
        Species::Diplomat,
    ];

    /// Short identifier used in logs, topics and species ids.
    pub fn name(&self) -> &'static str {
        match self {
            Species::Processor => "processor",
            Species::Memory => "memory",
            Species::Messenger => "messenger",
            Species::Broadcaster => "broadcaster",
            Species::Scout => "scout",
            Species::Guardian => "guardian",
            Species::Trader => "trader",
            Species::Resource => "resource",
            Species::Scholar => "scholar",
            Species::Diplomat => "diplomat",
        }
    }

    /// The binomial from the paper.
    pub fn binomial(&self) -> &'static str {
        match self {
            Species::Processor => "Homo computaticus",
            Species::Memory => "Bibliothecus perpetuus",
            Species::Messenger => "Mercurius velocis",
            Species::Broadcaster => "Radiophorus globalis",
            Species::Scout => "Explorator aquaticus",
            Species::Guardian => "Protector systemicus",
            Species::Trader => "Mercator economicus",
            Species::Resource => "Collector abundantia",
            Species::Scholar => "Scientificus adaptabilis",
            Species::Diplomat => "Negotiator pacificus",
        }
    }

    pub fn role(&self) -> Role {
        match self {
            Species::Processor => Role::Compute,
            Species::Memory => Role::Storage,
            Species::Messenger => Role::Transport,
            Species::Broadcaster => Role::Broadcast,
            Species::Scout => Role::Sensing,
            Species::Guardian => Role::Defence,
            Species::Trader => Role::Market,
            Species::Resource => Role::Harvest,
            // Hybrids take the role of their dominant half.
            Species::Scholar => Role::Compute,
            Species::Diplomat => Role::Market,
        }
    }

    /// Droplet volume envelope in nanolitres, `(min, max)`.
    pub fn volume_nl(&self) -> (u32, u32) {
        match self {
            Species::Processor => (50, 100),
            Species::Memory => (200, 500),
            Species::Messenger => (20, 50),
            Species::Broadcaster => (100, 150),
            Species::Scout => (30, 80),
            Species::Guardian => (150, 300),
            Species::Trader => (80, 120),
            Species::Resource => (100, 200),
            Species::Scholar => (100, 300),
            Species::Diplomat => (60, 110),
        }
    }

    /// DNA budget in base pairs. This is a real capacity limit, enforced by
    /// [`crate::droplet::Droplet::store_dna`].
    pub fn dna_load_bp(&self) -> usize {
        match self {
            Species::Processor => 1_000_000,
            Species::Memory => 100_000_000,
            Species::Messenger => 10_000,
            Species::Broadcaster => 100_000,
            Species::Scout => 50_000,
            Species::Guardian => 1_000_000,
            Species::Trader => 100_000,
            Species::Resource => 1_000_000,
            Species::Scholar => 50_000_000,
            Species::Diplomat => 100_000,
        }
    }

    /// Lifespan in lifecycle cycles before division or retirement.
    pub fn lifespan_cycles(&self) -> u64 {
        match self {
            Species::Processor => 10_000,
            Species::Memory => 1_000_000,
            Species::Messenger => 1_000,
            Species::Broadcaster => 100_000,
            Species::Scout => 10_000,
            Species::Guardian => 100_000,
            Species::Trader => 20_000,
            Species::Resource => 100_000,
            Species::Scholar => 500_000,
            Species::Diplomat => 20_000,
        }
    }

    /// True for the two hybrid species.
    pub fn is_hybrid(&self) -> bool {
        matches!(self, Species::Scholar | Species::Diplomat)
    }

    /// How many peers a droplet of this species should fan out to, given a mesh
    /// of `peers`. Broadcasters flood; messengers go point-to-point; everything
    /// else takes the √n middle road that flux-p2p already uses for its own
    /// gossip fan-out.
    pub fn fan_out(&self, peers: u32) -> u32 {
        match self.role() {
            Role::Broadcast => peers,
            Role::Transport => 1.min(peers),
            _ => (peers as f64).sqrt().round() as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_species_have_distinct_names() {
        let mut names: Vec<&str> = Species::ALL.iter().map(|s| s.name()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len());
    }

    #[test]
    fn volume_envelopes_are_ordered_and_nonzero() {
        for s in Species::ALL {
            let (lo, hi) = s.volume_nl();
            assert!(lo > 0 && lo < hi, "{:?} has a bad envelope", s);
        }
    }

    #[test]
    fn memory_species_has_the_largest_dna_budget_of_the_primaries() {
        let primaries = Species::ALL.iter().filter(|s| !s.is_hybrid());
        let max = primaries.map(|s| s.dna_load_bp()).max().unwrap();
        assert_eq!(max, Species::Memory.dna_load_bp());
    }

    #[test]
    fn messenger_is_the_shortest_lived() {
        let min = Species::ALL.iter().map(|s| s.lifespan_cycles()).min().unwrap();
        assert_eq!(min, Species::Messenger.lifespan_cycles());
    }

    #[test]
    fn fan_out_matches_role() {
        assert_eq!(Species::Broadcaster.fan_out(16), 16);
        assert_eq!(Species::Messenger.fan_out(16), 1);
        assert_eq!(Species::Processor.fan_out(16), 4);
        assert_eq!(Species::Messenger.fan_out(0), 0);
    }

    #[test]
    fn hybrids_are_exactly_two() {
        assert_eq!(Species::ALL.iter().filter(|s| s.is_hybrid()).count(), 2);
    }
}
