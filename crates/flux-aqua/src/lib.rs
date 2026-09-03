//! # flux-aqua — the Water-Robot colony, ported to Flux
//!
//! Quillon Graph carries a "Kingdom of Life" of water robots: a 49 KB LaTeX
//! whitepaper (WR-KoL), a 16-module `void-walker` crate subtitled *Aqua-K-Atto:
//! attosecond laser-Tor analytics species for quantum water robots*, a
//! `q-robot-cli`, and a `q-trading-bot` whose flagship strategy is literally
//! `water_bot.rs`. Most of that is narrative — warp drives, Tegmark Level IV,
//! eternal inflation, a `MultiverseWarpDrive`. Narrative is not a defect; it is
//! just not something you can compile against.
//!
//! Underneath the narrative there are **three mechanisms worth keeping**, and
//! this crate is those three, rebuilt so they hold:
//!
//! 1. **A bounded stochastic agent** — [`droplet::Droplet`]. Phase, coherence,
//!    energy, thermal dephasing, saturating EWOD actuation. Strip the EEG
//!    vocabulary and it is a perfectly ordinary state machine with well-behaved
//!    bounds, useful anywhere you need agents that degrade and recover.
//! 2. **A multiplicative score with saturating terms** — [`score::bridge_gain`].
//!    In the source this was five magic numbers inline in one method. Look at
//!    its *shape* and it is the same object as `flux-p2p`'s SAP scorer. Named
//!    and bounded, it becomes tunable — see [`cortex`].
//! 3. **An accumulated-work chain** — [`ledger::BridgeChain`]. "Longest bridge
//!    wins" is a real fork-choice rule once it means *total work across a
//!    chain* rather than *sorting one chain's blocks by length*, which is what
//!    the source did.
//!
//! ## What the analysis found
//!
//! Four defects, each fixed here and each pinned by a test that fails against
//! the original behaviour:
//!
//! | Defect | Where it was | Fixed by |
//! |---|---|---|
//! | Mesh never transmitted — `send_to_peer` logged the byte count and returned `Ok` | `tor_mesh.rs` | [`mesh`] over real libp2p gossipsub |
//! | Chain had no tamper-evidence — the block hash excluded `previous_hash`, and a post-push sort broke every parent pointer | `ledger.rs` | [`ledger::BridgeBlock::hash`] covers the parent; append-only |
//! | Non-determinism — a seeded RNG mixed with `rand::thread_rng()` | `k_parameter.rs`, `brane.rs` | one `ChaCha8Rng` per droplet |
//! | "Attosecond" timestamps were whole seconds, off by 10^18 | everywhere | honest `recorded_at_ms` |
//!
//! ## Features
//!
//! The core is dependency-light and always compiled. Both integrations are
//! opt-in, for the reason `flux-p2p` already learned about its own
//! `cortex-optimizer`: `flux-cortex` is a ~40-crate transitive closure, and
//! nobody who wants a droplet model should pay for it.
//!
//! * `cortex` — [`cortex::AquaCortex`], Cortex-tuned gain weights.
//! * `p2p` — [`mesh::AquaMesh`], real gossipsub transport.
//!
//! ## Shape of a session
//!
//! ```
//! use flux_aqua::{AquaColony, Species};
//!
//! let mut colony = AquaColony::new(1337);
//! let ids = colony.spawn_all_species();
//!
//! colony.stimulate_all(30.0, 300.0, 0.01);
//! colony.ewod(&ids[0], 25.0);
//! colony.tick_ms(100);
//!
//! let attempt = colony.attempt_bridge(&ids[0], &ids[1], 4.0).unwrap();
//! println!("gain {:.3} committed={}", attempt.score.normalized, attempt.committed);
//! assert!(colony.chain.verify().is_ok());
//! ```

pub mod colony;
pub mod cortex;
pub mod droplet;
pub mod ledger;
pub mod mesh;
pub mod score;
pub mod species;

pub use colony::{AquaColony, BridgeAttempt, ColonyMetrics};
pub use cortex::{feedback_tune, TuningResult, MAX_STEP, TARGET_COMMIT_RATE};
pub use droplet::{Droplet, EwodDrive, IsotopicSig, Stimulus, TopoCharge};
pub use ledger::{
    quantize_micro, BlockHash, BridgeBlock, BridgeChain, ChainError, ChainStats, MICRO,
    SCORE_ONE,
};
pub use mesh::{AquaMessage, Envelope, WireError, TOPICS, WIRE_VERSION};
pub use score::{
    bridge_gain, normalized_gain, score_bridge, BridgeScore, GainInputs, GainWeights,
    STABILITY_THRESHOLD,
};
pub use species::{Role, Species};

#[cfg(feature = "cortex")]
pub use cortex::AquaCortex;
#[cfg(feature = "p2p")]
pub use mesh::AquaMesh;

/// Crate version, from the workspace.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod integration_tests {
    use super::*;

    /// A full colony session: spawn, drive, bridge, gossip-encode, tune —
    /// end to end, with every invariant checked.
    #[test]
    fn a_colony_runs_bridges_and_stays_consistent() {
        let mut colony = AquaColony::new(20260903);
        let ids = colony.spawn_all_species();
        assert_eq!(ids.len(), Species::ALL.len());

        let mut attempts = 0usize;
        let mut committed = 0usize;

        for round in 0..12 {
            colony.stimulate_all(28.0, 300.0, 0.01);
            for id in &ids {
                colony.ewod(id, 16.0);
            }
            colony.tick_ms(25);

            for i in 0..ids.len() {
                let j = (i + 1 + round) % ids.len();
                if i == j {
                    continue;
                }
                if let Some(a) = colony.attempt_bridge(&ids[i], &ids[j], 3.5) {
                    attempts += 1;
                    if a.committed {
                        committed += 1;
                        // Anything that committed must have cleared the bar.
                        assert!(a.score.normalized >= STABILITY_THRESHOLD);
                    }
                }
                colony.tick_ms(1);
            }
        }

        assert!(attempts > 0);
        assert!(committed > 0, "nothing committed — the run proves nothing");
        assert_eq!(colony.chain.height(), committed as u64);
        assert!(colony.chain.verify().is_ok(), "chain must stay linked");
        assert!(colony.chain.total_work() >= 0.0);

        // Every committed block must survive the wire byte-for-byte.
        for block in colony.chain.blocks().iter().skip(1) {
            let before = block.hash();
            let (topic, bytes) = mesh::encode(
                "epsilon",
                block.recorded_at_ms,
                AquaMessage::Bridge(Box::new(block.clone())),
            );
            match mesh::decode(topic, &bytes).unwrap().message {
                AquaMessage::Bridge(b) => assert_eq!(b.hash(), before),
                other => panic!("wrong variant: {other:?}"),
            }
        }

        // And the measured commit rate must produce a legal tuning step.
        let rate = committed as f64 / attempts as f64;
        let tuned = feedback_tune(colony.weights, rate, attempts);
        assert_eq!(tuned.after, tuned.after.clamped());
        assert!(tuned.after.max_gain() > 0.0);
    }

    #[test]
    fn two_colonies_from_one_seed_agree_on_the_chain_digest() {
        let run = || {
            let mut c = AquaColony::new(555);
            let ids = c.spawn_all_species();
            for r in 0..6 {
                c.stimulate_all(25.0, 300.0, 0.01);
                c.tick_ms(20);
                for i in 0..ids.len() - 1 {
                    c.attempt_bridge(&ids[i], &ids[i + 1], 2.0 + r as f64);
                    c.tick_ms(1);
                }
            }
            c.chain.digest()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn a_heavier_chain_wins_the_fork() {
        // Every round re-actuates the whole colony with EWOD. Without that the
        // droplets decohere and later rounds commit nothing — see
        // `colony::tests::an_unactuated_colony_decoheres_and_stops_bridging`.
        let build = |rounds: usize, seed: u64| {
            let mut c = AquaColony::new(seed);
            let ids = c.spawn_all_species();
            for _ in 0..rounds {
                c.stimulate_all(30.0, 300.0, 0.01);
                for id in &ids {
                    c.ewod(id, 20.0);
                }
                c.tick_ms(20);
                for i in 0..ids.len() - 1 {
                    c.attempt_bridge(&ids[i], &ids[i + 1], 5.0);
                    c.tick_ms(1);
                }
            }
            c.chain
        };
        let light = build(1, 11);
        let heavy = build(8, 11);
        assert!(heavy.total_work() > light.total_work());
        assert!(light.should_adopt(&heavy));
        assert!(!heavy.should_adopt(&light));
    }
}
