//! # flux-skyskraber — the Quillon Graph Skyskraber, as a program
//!
//! OK, so here's the deal. Before a tower exists in steel it should exist as a
//! *system* — something you can run, measure, and argue with. This crate is that
//! system: a digital twin of the Quillon Graph Skyskraber, written so that an
//! entrepreneur can read the modules as an architectural program, and so the
//! building's operating logic is already proven before ground is broken.
//!
//! ## The organs
//!
//! | Module | The real-world thing it is |
//! |---|---|
//! | [`tower`] | The building program: 88 floors + 4 basements, zoned and validated |
//! | [`elevator`] | Robot elevator transport — freight shafts for the robot workforce, cabs for the few humans |
//! | [`vault`] | The basement gold vault — hash-chained AND externally witnessed (anchors) |
//! | [`bank`] | **Quillon Bank, the main organ** — a `flux-bank-core` ledger doing payroll, rent, vault fees |
//! | [`workforce`] | Robots (mostly) + humans (few), scored with the real `flux-p2p` SAP table |
//! | [`auditorium`] | The grand auditorium — big thinkers, wisdom sessions, conflict-free scheduling |
//! | [`culture`] | The Building Operating Index — measured operating health, never self-report |
//! | [`cortex`] | The building cortex — sense → score (SAP + X-algo) → decide → act |
//! | [`emsec`] | Emanations Security posture — the SIGIL Nation EMSEC Doctrine v0, scored |
//! | [`physics`] | Physics v0.1 — bridge differential drift, evacuation envelopes |
//! | [`state_root`] | The Merkle-rooted tower state, committed via the full node |
//! | [`api`] | The MCP/HTTP surface, declared with `flux-api`'s `#[api]` macro |
//! | [`sim`] | A deterministic day-in-the-life: same seed, same day, same report hash |
//!
//! ## What is proven vs. what is still pretend
//!
//! Proven: the scheduling, custody chain, ledger arithmetic, scoring and the
//! culture index all run and are tested deterministically. Pretend (v0, on
//! purpose): elevator cars carry one job at a time (real destination dispatch
//! batches riders); the bank ledger is local (live settlement goes over the
//! Quillon MCP); the vault clearance model is two levels, not a full ACL.
//! Every one of those is a seam, not a wall.

pub mod api;
pub mod auditorium;
pub mod bank;
pub mod cortex;
pub mod culture;
pub mod elevator;
pub mod emsec;
pub mod physics;
pub mod science;
pub mod sim;
pub mod state_root;
pub mod tower;
pub mod vault;
pub mod workforce;

pub use auditorium::Auditorium;
pub use bank::QuillonBank;
pub use cortex::BuildingCortex;
pub use elevator::ElevatorBank;
pub use tower::TowerSpec;
pub use vault::Vault;
pub use workforce::Workforce;

/// The whole building, composed. This is the object the cortex governs and the
/// simulator drives.
pub struct Building {
    pub spec: tower::TowerSpec,
    pub transport: elevator::ElevatorBank,
    pub vault: vault::Vault,
    pub bank: bank::QuillonBank,
    pub workforce: workforce::Workforce,
    pub auditorium: auditorium::Auditorium,
    pub cortex: cortex::BuildingCortex,
}

impl Building {
    /// Stand up the canonical Quillon Graph Skyskraber: default blueprint,
    /// default robot-heavy workforce, funded treasury.
    pub fn quillon_default() -> Result<Self, tower::TowerError> {
        let spec = tower::TowerSpec::quillon_default();
        spec.validate()?;
        let transport = elevator::ElevatorBank::for_spec(&spec);
        let vault = vault::Vault::new(vault::VAULT_CAPACITY_BARS);
        let mut bank = bank::QuillonBank::new();
        let workforce = workforce::Workforce::quillon_default(64, 8);
        bank.bootstrap_accounts(&workforce);
        let auditorium = auditorium::Auditorium::grand();
        let cortex = cortex::BuildingCortex::new(&workforce);
        Ok(Self { spec, transport, vault, bank, workforce, auditorium, cortex })
    }

    /// The same building, born to EMSEC Doctrine v0: the guardian-ring blueprint
    /// (see [`tower::TowerSpec::quillon_hardened`]) with everything else
    /// identical. Paired with the hardened emanations config it scores a perfect
    /// posture on all four principles.
    pub fn quillon_hardened() -> Result<Self, tower::TowerError> {
        let spec = tower::TowerSpec::quillon_hardened();
        spec.validate()?;
        let transport = elevator::ElevatorBank::for_spec(&spec);
        let vault = vault::Vault::new(vault::VAULT_CAPACITY_BARS);
        let mut bank = bank::QuillonBank::new();
        let workforce = workforce::Workforce::quillon_default(64, 8);
        bank.bootstrap_accounts(&workforce);
        let auditorium = auditorium::Auditorium::grand();
        let cortex = cortex::BuildingCortex::new(&workforce);
        Ok(Self { spec, transport, vault, bank, workforce, auditorium, cortex })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_building_stands() {
        let b = Building::quillon_default().expect("blueprint must validate");
        assert_eq!(b.spec.name, "Quillon Graph Skyskraber");
        assert!(b.transport.car_count() > 0);
        assert!(b.workforce.workers.len() == 72);
    }
}
