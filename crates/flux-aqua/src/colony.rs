//! The colony: many droplets, one chain, one set of tunable gain weights.
//!
//! This is the piece `void-walker` never had. Its `WaterRobotNetwork` held a
//! `HashMap<String, Arc<RwLock<AquaKAtto>>>` and exposed exactly two methods —
//! `register_robot` and `get_network_stats` — with no way for one droplet to
//! affect another and no shared ledger. A colony that cannot interact is a list.

use std::collections::BTreeMap;

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::droplet::{tune_topology, Droplet, EwodDrive, Stimulus};
use crate::ledger::BridgeChain;
use crate::score::{score_bridge, BridgeScore, GainInputs, GainWeights};
use crate::species::Species;

/// The outcome of one bridge attempt.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgeAttempt {
    pub producer: String,
    pub target: String,
    pub score: BridgeScore,
    /// `true` when the attempt cleared the stability bar and was appended.
    pub committed: bool,
    /// Height the block landed at, when committed.
    pub height: Option<u64>,
}

/// A population of droplets sharing one bridge chain.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AquaColony {
    /// Droplets by id. `BTreeMap` so iteration order — and therefore every
    /// derived result — is deterministic.
    pub droplets: BTreeMap<String, Droplet>,
    pub chain: BridgeChain,
    pub weights: GainWeights,
    /// Monotonic millisecond clock supplied by the caller.
    pub clock_ms: u64,
    seed: u64,
    #[serde(skip)]
    rng: Option<ChaCha8Rng>,
}

impl AquaColony {
    pub fn new(seed: u64) -> Self {
        Self {
            droplets: BTreeMap::new(),
            chain: BridgeChain::new(),
            weights: GainWeights::default().clamped(),
            clock_ms: 0,
            seed,
            rng: Some(ChaCha8Rng::seed_from_u64(seed)),
        }
    }

    /// Restore transient state after deserialization. Idempotent.
    pub fn rehydrate(&mut self) {
        if self.rng.is_none() {
            self.rng = Some(ChaCha8Rng::seed_from_u64(self.seed));
        }
        for d in self.droplets.values_mut() {
            d.rehydrate();
        }
    }

    fn rng(&mut self) -> &mut ChaCha8Rng {
        self.rehydrate();
        self.rng.as_mut().expect("rehydrated above")
    }

    /// Spawn a droplet of `species`. The id is derived from the species and the
    /// droplet's isotopic signature, so it is stable across runs.
    pub fn spawn(&mut self, species: Species) -> String {
        let seed = self
            .seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(self.droplets.len() as u64);
        let d = Droplet::new(species, seed);
        let id = format!("{}-{}", species.name(), hex::encode(&d.iso_sig[..6]));
        self.droplets.insert(id.clone(), d);
        id
    }

    /// Spawn one droplet of every species. Returns the ids in taxonomy order.
    pub fn spawn_all_species(&mut self) -> Vec<String> {
        Species::ALL.iter().map(|s| self.spawn(*s)).collect()
    }

    pub fn len(&self) -> usize {
        self.droplets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.droplets.is_empty()
    }

    /// Advance the colony clock.
    pub fn tick_ms(&mut self, delta_ms: u64) {
        self.clock_ms = self.clock_ms.saturating_add(delta_ms);
    }

    /// Drive every droplet with the same stimulus, then let it dephase.
    pub fn stimulate_all(&mut self, amplitude: f64, temp_k: f64, dt_s: f64) {
        for d in self.droplets.values_mut() {
            d.stimulate(Stimulus::new(amplitude));
            d.thermal_dephase(temp_k, dt_s);
        }
        self.tick_ms((dt_s * 1000.0).max(1.0) as u64);
    }

    /// Actuate one droplet with EWOD.
    pub fn ewod(&mut self, id: &str, volts: f64) -> bool {
        match self.droplets.get_mut(id) {
            Some(d) => {
                d.ewod(EwodDrive { volts });
                true
            }
            None => false,
        }
    }

    /// Attempt a bridge from `producer` towards `target`, scoring it with the
    /// colony's current weights and appending it to the chain when stable.
    ///
    /// `drive_tev` is the lightning energy feeding the topological tuner.
    pub fn attempt_bridge(
        &mut self,
        producer: &str,
        target: &str,
        drive_tev: f64,
    ) -> Option<BridgeAttempt> {
        let target_sig = self.droplets.get(target)?.iso_sig;
        let (species, coherence, energy, phase, iso_sig) = {
            let p = self.droplets.get(producer)?;
            (p.species, p.coherence, p.energy, p.phase, p.iso_sig)
        };

        let cap = self.weights.charge_cap;
        let charge = tune_topology(self.rng(), drive_tev, cap);
        let affinity = Droplet::affinity(&iso_sig, &target_sig);

        let score = score_bridge(
            &self.weights,
            &GainInputs {
                coherence,
                energy,
                phase,
                affinity,
                charge,
            },
        );

        let mut attempt = BridgeAttempt {
            producer: producer.to_string(),
            target: target.to_string(),
            score,
            committed: false,
            height: None,
        };

        if score.stable {
            // `next_block` quantises the floats and refuses anything that must
            // never reach consensus, so a NaN gain can only ever cost an
            // attempt — it can never become a block.
            match self.chain.next_block(
                score.displacement,
                charge,
                target_sig,
                species,
                producer,
                score.normalized,
                self.clock_ms,
            ) {
                Ok(block) => {
                    let height = block.height;
                    // A push can still fail — most plausibly because the
                    // caller's clock went backwards — and when it does we
                    // refuse rather than rewrite history.
                    if self.chain.push(block).is_ok() {
                        attempt.committed = true;
                        attempt.height = Some(height);
                    }
                }
                Err(_) => { /* unrepresentable work: attempt is simply lost */ }
            }
        }

        // A bridge attempt costs the producer energy whether or not it lands.
        if let Some(p) = self.droplets.get_mut(producer) {
            p.energy = (p.energy - 0.02).clamp(0.0, 1.0);
            let _ = p.advance_cycle();
        }

        Some(attempt)
    }

    /// Retire every droplet that has reached its species lifespan. Returns the
    /// ids removed.
    pub fn retire_expired(&mut self) -> Vec<String> {
        let dead: Vec<String> = self
            .droplets
            .iter()
            .filter(|(_, d)| d.cycles >= d.species.lifespan_cycles())
            .map(|(id, _)| id.clone())
            .collect();
        for id in &dead {
            self.droplets.remove(id);
        }
        dead
    }

    /// A snapshot of colony health — the input a Cortex loop optimises against.
    pub fn metrics(&self) -> ColonyMetrics {
        let n = self.droplets.len();
        let (coh, en) = self.droplets.values().fold((0.0, 0.0), |(c, e), d| {
            (c + d.coherence, e + d.energy)
        });
        let hot = self.droplets.values().filter(|d| d.is_hot()).count();
        let stats = self.chain.stats();

        ColonyMetrics {
            droplets: n,
            hot_droplets: hot,
            mean_coherence: if n == 0 { 0.0 } else { coh / n as f64 },
            mean_energy: if n == 0 { 0.0 } else { en / n as f64 },
            chain_height: self.chain.height(),
            total_work: self.chain.total_work(),
            mean_bridge_score: stats.mean_score,
            weights: self.weights,
        }
    }
}

/// Aggregate colony state.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColonyMetrics {
    pub droplets: usize,
    pub hot_droplets: usize,
    pub mean_coherence: f64,
    pub mean_energy: f64,
    pub chain_height: u64,
    pub total_work: f64,
    pub mean_bridge_score: f64,
    pub weights: GainWeights,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colony() -> AquaColony {
        let mut c = AquaColony::new(1234);
        c.spawn_all_species();
        c
    }

    #[test]
    fn spawning_gives_one_droplet_per_species() {
        let c = colony();
        assert_eq!(c.len(), Species::ALL.len());
    }

    #[test]
    fn ids_are_stable_across_runs() {
        let a: Vec<String> = { let mut c = AquaColony::new(7); c.spawn_all_species() };
        let b: Vec<String> = { let mut c = AquaColony::new(7); c.spawn_all_species() };
        assert_eq!(a, b);
    }

    #[test]
    fn stimulating_moves_every_droplet() {
        let mut c = colony();
        let before: Vec<f64> = c.droplets.values().map(|d| d.phase).collect();
        c.stimulate_all(20.0, 300.0, 0.01);
        let after: Vec<f64> = c.droplets.values().map(|d| d.phase).collect();
        assert!(before.iter().zip(&after).all(|(a, b)| a != b));
        assert!(c.clock_ms >= 10);
    }

    #[test]
    fn a_strong_bridge_commits_and_grows_the_chain() {
        let mut c = colony();
        let ids: Vec<String> = c.droplets.keys().cloned().collect();
        // Drive the producer hot first.
        c.ewod(&ids[0], 25.0);
        c.tick_ms(100);

        let mut committed = 0;
        for i in 1..ids.len() {
            if let Some(a) = c.attempt_bridge(&ids[0], &ids[i], 4.0) {
                if a.committed {
                    committed += 1;
                    assert!(a.height.is_some());
                }
            }
            c.tick_ms(10);
        }
        assert!(committed > 0, "no bridge committed — the test proves nothing");
        assert_eq!(c.chain.height(), committed);
        assert!(c.chain.verify().is_ok());
    }

    #[test]
    fn an_unactuated_colony_decoheres_and_stops_bridging() {
        // Coherence only ever falls under thermal noise; EWOD is the sole way it
        // recovers. So a colony nobody actuates must go quiet. Worth pinning: it
        // is the reason a naive "run more rounds -> more work" test fails.
        let mut c = colony();
        let ids: Vec<String> = c.droplets.keys().cloned().collect();
        let mut early = 0;
        let mut late = 0;
        for round in 0..60 {
            c.stimulate_all(30.0, 320.0, 0.05);
            c.tick_ms(20);
            for i in 0..ids.len() - 1 {
                if let Some(a) = c.attempt_bridge(&ids[i], &ids[i + 1], 5.0) {
                    if a.committed {
                        if round < 10 {
                            early += 1;
                        } else if round >= 50 {
                            late += 1;
                        }
                    }
                }
                c.tick_ms(1);
            }
        }
        assert!(early > 0, "a fresh colony should be able to bridge");
        assert_eq!(late, 0, "a decohered colony must go quiet, not keep bridging");
        assert!(c.metrics().mean_coherence < 0.2);
    }

    #[test]
    fn a_dead_droplet_cannot_bridge() {
        let mut c = colony();
        let ids: Vec<String> = c.droplets.keys().cloned().collect();
        c.droplets.get_mut(&ids[0]).unwrap().energy = 0.0;
        c.droplets.get_mut(&ids[0]).unwrap().coherence = 0.0;
        let a = c.attempt_bridge(&ids[0], &ids[1], 4.0).unwrap();
        assert_eq!(a.score.raw, 0.0);
        assert!(!a.committed);
        assert_eq!(c.chain.height(), 0);
    }

    #[test]
    fn unknown_droplet_ids_return_none() {
        let mut c = colony();
        let ids: Vec<String> = c.droplets.keys().cloned().collect();
        assert!(c.attempt_bridge("nope", &ids[0], 1.0).is_none());
        assert!(c.attempt_bridge(&ids[0], "nope", 1.0).is_none());
    }

    #[test]
    fn bridging_is_deterministic_for_a_fixed_seed() {
        let run = || {
            let mut c = AquaColony::new(4242);
            let ids = c.spawn_all_species();
            c.stimulate_all(30.0, 300.0, 0.01);
            let mut out = Vec::new();
            for i in 1..ids.len() {
                c.tick_ms(5);
                let a = c.attempt_bridge(&ids[0], &ids[i], 3.0).unwrap();
                out.push((a.committed, format!("{:.9}", a.score.raw)));
            }
            (out, c.chain.digest())
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn retirement_removes_only_expired_droplets() {
        let mut c = AquaColony::new(1);
        let msg = c.spawn(Species::Messenger);
        let mem = c.spawn(Species::Memory);
        c.droplets.get_mut(&msg).unwrap().cycles = Species::Messenger.lifespan_cycles();
        let dead = c.retire_expired();
        assert_eq!(dead, vec![msg]);
        assert!(c.droplets.contains_key(&mem));
    }

    #[test]
    fn metrics_reflect_the_population() {
        let mut c = colony();
        let m = c.metrics();
        assert_eq!(m.droplets, Species::ALL.len());
        assert!(m.mean_coherence > 0.0 && m.mean_coherence <= 1.0);
        assert!(m.mean_energy > 0.0 && m.mean_energy <= 1.0);
        assert_eq!(m.chain_height, 0);

        let empty = AquaColony::new(0);
        let em = empty.metrics();
        assert_eq!(em.mean_coherence, 0.0, "empty colony must not divide by zero");
    }

    #[test]
    fn serde_roundtrip_preserves_the_chain() {
        let mut c = colony();
        let ids: Vec<String> = c.droplets.keys().cloned().collect();
        c.ewod(&ids[0], 25.0);
        c.tick_ms(50);
        for i in 1..4 {
            c.attempt_bridge(&ids[0], &ids[i], 5.0);
            c.tick_ms(5);
        }
        let json = serde_json::to_string(&c).unwrap();
        let mut back: AquaColony = serde_json::from_str(&json).unwrap();
        back.rehydrate();
        assert_eq!(back.chain.digest(), c.chain.digest());
        assert!(back.chain.verify().is_ok());
    }
}
