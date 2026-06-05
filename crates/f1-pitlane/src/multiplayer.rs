//! Multiplayer over **flux-p2p**. F1-Pitlane is wallet-addressed end to end
//! ([`crate::drivers::GridDriver`]), so a networked race is just these messages
//! gossiped on a topic: peers join a lobby, the host sets the grid and drops the
//! lights with a shared seed, then everyone streams telemetry, crashes and crash
//! *reactions* live. Because the sim is deterministic from the seed, every peer
//! computes the same race — flux-p2p only needs to carry intentions + reactions,
//! not authoritative physics.
//!
//! This module is the **protocol + lobby logic** (serde message types, grid
//! assembly, deterministic seed agreement). The wire transport is flux-p2p's
//! gossipsub (`flux-p2p` crate, `drain_events` receive path) — wiring the topic
//! pub/sub is the deployment step; the game-level protocol lives here and is
//! fully testable offline.

use crate::drivers::{starting_grid, GridDriver};
use serde::{Deserialize, Serialize};

/// The gossipsub topic F1-Pitlane races are coordinated on.
pub const RACE_TOPIC: &str = "/f1-pitlane/v1/race";

/// One message on the race topic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RaceMsg {
    /// A peer enters the lobby (anteing SIGIL if a human).
    Join { wallet: String, handle: String, persona: String, entry_sigil: f64 },
    /// A peer signals it has built its car and is ready to race.
    Ready { wallet: String },
    /// The host fixes the grid order (handle → grid slot) before lights out.
    GridSet { order: Vec<(String, u8)> },
    /// Lights out: the agreed seed + venue everyone simulates deterministically.
    LightsOut { seed: u64, track: String, laps: u32 },
    /// Live telemetry beacon from one car.
    Telemetry { wallet: String, lap: u32, position: u8, gap_s: f64, tyre_pct: f64, flag: String },
    /// An incident occurred — broadcast so every peer raises the same flags.
    Incident { lap: u32, corner: u8, kind: String, wallet_involved: String },
    /// A driver's rapid reaction to an incident ahead (the multiplayer drama).
    Reaction { wallet: String, lap: u32, corner: u8, reaction: String },
    /// A car finished (or retired); carries the SIGIL payout.
    Finish { wallet: String, position: u8, points: u32, sigil_won: f64 },
}

impl RaceMsg {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("RaceMsg serialises")
    }
    pub fn from_json(s: &str) -> Option<Self> {
        serde_json::from_str(s).ok()
    }
}

/// Host-side lobby state: collects joins, then assembles the grid.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Lobby {
    pub topic: String,
    pub host_wallet: String,
    pub players: Vec<GridDriver>,
    pub ready: Vec<String>,
    pub seed: u64,
    pub track: String,
    pub laps: u32,
    pub started: bool,
}

impl Lobby {
    pub fn host(host: GridDriver, track: &str, laps: u32, seed: u64) -> Self {
        Lobby {
            topic: RACE_TOPIC.to_string(),
            host_wallet: host.wallet.clone(),
            players: vec![host],
            ready: Vec::new(),
            seed,
            track: track.to_string(),
            laps,
            started: false,
        }
    }

    /// Apply an inbound message to the lobby. Returns true if it changed state.
    pub fn apply(&mut self, msg: &RaceMsg) -> bool {
        match msg {
            RaceMsg::Join { wallet, handle, persona, entry_sigil } => {
                if self.players.iter().any(|p| &p.wallet == wallet) {
                    return false; // idempotent — don't double-join
                }
                self.players.push(crate::drivers::sigil_human(
                    handle, wallet, *entry_sigil, 0.85, 0.5, 0.8, 0.85,
                ));
                // keep the persona the peer announced
                if let Some(p) = self.players.last_mut() {
                    p.persona = persona.clone();
                }
                true
            }
            RaceMsg::Ready { wallet } => {
                if self.ready.iter().any(|w| w == wallet) {
                    false
                } else {
                    self.ready.push(wallet.clone());
                    true
                }
            }
            RaceMsg::LightsOut { seed, track, laps } => {
                self.seed = *seed;
                self.track = track.clone();
                self.laps = *laps;
                self.started = true;
                true
            }
            _ => false,
        }
    }

    pub fn all_ready(&self) -> bool {
        !self.players.is_empty() && self.players.iter().all(|p| self.ready.contains(&p.wallet))
    }

    /// Assemble the 20-car grid from the joined peers (host first), filling the
    /// rest with the AI siblings + NPCs.
    pub fn build_grid(&self) -> Vec<GridDriver> {
        let mut peers = self.players.clone();
        let host = peers.remove(0);
        starting_grid(host, peers, self.seed)
    }

    /// The LightsOut message the host broadcasts to start the race.
    pub fn lights_out(&self) -> RaceMsg {
        RaceMsg::LightsOut { seed: self.seed, track: self.track.clone(), laps: self.laps }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::sigil_human;

    #[test]
    fn messages_roundtrip_through_json() {
        let msgs = vec![
            RaceMsg::Join { wallet: "qnkA".into(), handle: "alice".into(), persona: "rookie".into(), entry_sigil: 20.0 },
            RaceMsg::LightsOut { seed: 7, track: "Monaco".into(), laps: 78 },
            RaceMsg::Reaction { wallet: "qnkA".into(), lap: 12, corner: 1, reaction: "Brake".into() },
            RaceMsg::Finish { wallet: "qnkA".into(), position: 3, points: 15, sigil_won: 40.0 },
        ];
        for m in msgs {
            assert_eq!(RaceMsg::from_json(&m.to_json()).unwrap(), m);
        }
    }

    #[test]
    fn lobby_collects_joins_builds_a_full_grid() {
        let host = sigil_human("Viktor", "qnkhost", 25.0, 0.9, 0.5, 0.85, 0.9);
        let mut lobby = Lobby::host(host, "Circuit de Monaco", 78, 2026);
        lobby.apply(&RaceMsg::Join { wallet: "qnkA".into(), handle: "alice".into(), persona: "privateer".into(), entry_sigil: 25.0 });
        lobby.apply(&RaceMsg::Join { wallet: "qnkB".into(), handle: "bob".into(), persona: "privateer".into(), entry_sigil: 25.0 });
        // duplicate join is ignored
        assert!(!lobby.apply(&RaceMsg::Join { wallet: "qnkA".into(), handle: "alice".into(), persona: "x".into(), entry_sigil: 25.0 }));
        assert_eq!(lobby.players.len(), 3);
        let grid = lobby.build_grid();
        assert_eq!(grid.len(), 20);
        assert_eq!(grid[0].wallet, "qnkhost");
        assert!(grid.iter().any(|d| d.name == "Rocky"));
    }

    #[test]
    fn ready_gate_and_lights_out() {
        let host = sigil_human("Viktor", "qnkhost", 0.0, 0.9, 0.5, 0.85, 0.9);
        let mut lobby = Lobby::host(host, "Monaco", 78, 1);
        lobby.apply(&RaceMsg::Join { wallet: "qnkA".into(), handle: "alice".into(), persona: "p".into(), entry_sigil: 0.0 });
        assert!(!lobby.all_ready());
        lobby.apply(&RaceMsg::Ready { wallet: "qnkhost".into() });
        lobby.apply(&RaceMsg::Ready { wallet: "qnkA".into() });
        assert!(lobby.all_ready());
        let lo = lobby.lights_out();
        assert!(matches!(lo, RaceMsg::LightsOut { laps: 78, .. }));
    }
}
