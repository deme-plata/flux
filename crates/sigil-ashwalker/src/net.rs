//! net.rs — multiplayer co-op protocol (prototype 1).
//!
//! Keeps the crate **std-only**: this layer is the *game* protocol (messages + a session that applies
//! them), behind a [`Transport`] trait. The production transport is **flux-p2p** gossipsub — a thin
//! `FluxP2pTransport` adapter (its own crate/bin, so libp2p stays out of this pure core) implements
//! `Transport` by publishing [`NetMsg`] bytes to a topic and draining peer events. A [`Loopback`]
//! transport is provided here for tests + single-process play.
//!
//! Co-op model: every peer shares the same Ashlands; each carries a position + casts; boss spawns and
//! loot are broadcast so the party sees one consistent fight.

use crate::{Mcp, V3};

pub type PlayerId = u32;

/// A wire message between peers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetMsg {
    Join { id: PlayerId, name: String, seed: u64 },
    Leave { id: PlayerId },
    Move { id: PlayerId, pos: V3 },
    Cast { id: PlayerId, a: Mcp, b: Option<Mcp>, target: i32 }, // target<0 = none
    BossSpawn { root: u64, name: String },
    Loot { id: PlayerId, item: String },
}

fn mcp_name(m: Mcp) -> &'static str { m.name() }
fn mcp_from(s: &str) -> Option<Mcp> {
    Some(match s {
        "flux_combo" => Mcp::FluxCombo, "dex_swap" => Mcp::DexSwap, "flux_zk_combo" => Mcp::ZkVeil,
        "council_consensus" => Mcp::CouncilQuorum, "send_token" => Mcp::Tribute, "mining_status" => Mcp::Hashstorm,
        _ => return None,
    })
}

/// Pipe-delimited wire encoding (std-only; swap for serde/postcard at the flux-p2p boundary later).
pub fn encode(m: &NetMsg) -> String {
    match m {
        NetMsg::Join { id, name, seed } => format!("JOIN|{id}|{name}|{seed}"),
        NetMsg::Leave { id } => format!("LEAVE|{id}"),
        NetMsg::Move { id, pos } => format!("MOVE|{id}|{}|{}|{}", pos.x, pos.y, pos.z),
        NetMsg::Cast { id, a, b, target } => format!("CAST|{id}|{}|{}|{target}", mcp_name(*a), b.map(mcp_name).unwrap_or("-")),
        NetMsg::BossSpawn { root, name } => format!("BOSS|{root}|{name}"),
        NetMsg::Loot { id, item } => format!("LOOT|{id}|{item}"),
    }
}
pub fn decode(s: &str) -> Option<NetMsg> {
    let p: Vec<&str> = s.trim().split('|').collect();
    Some(match *p.first()? {
        "JOIN" => NetMsg::Join { id: p.get(1)?.parse().ok()?, name: (*p.get(2)?).to_string(), seed: p.get(3)?.parse().ok()? },
        "LEAVE" => NetMsg::Leave { id: p.get(1)?.parse().ok()? },
        "MOVE" => NetMsg::Move { id: p.get(1)?.parse().ok()?, pos: V3::new(p.get(2)?.parse().ok()?, p.get(3)?.parse().ok()?, p.get(4)?.parse().ok()?) },
        "CAST" => NetMsg::Cast { id: p.get(1)?.parse().ok()?, a: mcp_from(p.get(2)?)?, b: mcp_from(p.get(3).copied().unwrap_or("-")), target: p.get(4)?.parse().ok()? },
        "BOSS" => NetMsg::BossSpawn { root: p.get(1)?.parse().ok()?, name: (*p.get(2)?).to_string() },
        "LOOT" => NetMsg::Loot { id: p.get(1)?.parse().ok()?, item: (*p.get(2)?).to_string() },
        _ => return None,
    })
}

/// Transport abstraction — flux-p2p (gossipsub), a TCP socket, or the in-process [`Loopback`].
pub trait Transport {
    fn send(&mut self, msg: &NetMsg);
    /// Drain inbound messages (for flux-p2p this is `drain_events` → decode).
    fn recv(&mut self) -> Vec<NetMsg>;
}

/// In-process transport for tests + hot-seat: a shared inbox per peer.
#[derive(Debug, Default)]
pub struct Loopback { inbox: std::collections::VecDeque<NetMsg>, pub sent: Vec<NetMsg> }
impl Loopback { pub fn deliver(&mut self, m: NetMsg) { self.inbox.push_back(m); } }
impl Transport for Loopback {
    fn send(&mut self, msg: &NetMsg) { self.sent.push(msg.clone()); }
    fn recv(&mut self) -> Vec<NetMsg> { self.inbox.drain(..).collect() }
}

/// A remote party member as this peer sees them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer { pub id: PlayerId, pub name: String, pub pos: V3 }

/// The co-op session: our id + the party roster, kept in sync by applying inbound [`NetMsg`]s.
#[derive(Debug, Default)]
pub struct Session { pub me: PlayerId, pub peers: Vec<Peer>, pub events: Vec<String> }
impl Session {
    pub fn new(me: PlayerId) -> Self { Self { me, peers: Vec::new(), events: Vec::new() } }

    /// Broadcast our move (and update nothing local — the caller owns the local player).
    pub fn broadcast_move<T: Transport>(&self, t: &mut T, pos: V3) { t.send(&NetMsg::Move { id: self.me, pos }); }
    pub fn broadcast_cast<T: Transport>(&self, t: &mut T, a: Mcp, b: Option<Mcp>, target: i32) { t.send(&NetMsg::Cast { id: self.me, a, b, target }); }

    /// Pull inbound messages and fold them into the roster/log. Returns the casts others made
    /// (so the local Game can apply party damage to the shared boss).
    pub fn pump<T: Transport>(&mut self, t: &mut T) -> Vec<NetMsg> {
        let mut casts = Vec::new();
        for m in t.recv() {
            match &m {
                NetMsg::Join { id, name, .. } if *id != self.me => {
                    if !self.peers.iter().any(|p| p.id == *id) { self.peers.push(Peer { id: *id, name: name.clone(), pos: V3::new(0,0,0) }); }
                    self.events.push(format!("{name} joined the party"));
                }
                NetMsg::Leave { id } => { self.peers.retain(|p| p.id != *id); self.events.push(format!("peer {id} left")); }
                NetMsg::Move { id, pos } => { if let Some(p) = self.peers.iter_mut().find(|p| p.id == *id) { p.pos = *pos; } }
                NetMsg::Cast { id, .. } if *id != self.me => casts.push(m.clone()),
                NetMsg::BossSpawn { name, .. } => self.events.push(format!("the party faces {name}")),
                NetMsg::Loot { id, item } => self.events.push(format!("peer {id} looted {item}")),
                _ => {}
            }
        }
        casts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_roundtrip_all_messages() {
        let msgs = [
            NetMsg::Join { id: 2, name: "Rocky".into(), seed: 12345 },
            NetMsg::Move { id: 2, pos: V3::new(3, 4, 1) },
            NetMsg::Cast { id: 2, a: Mcp::DexSwap, b: Some(Mcp::FluxCombo), target: 0 },
            NetMsg::Cast { id: 3, a: Mcp::Hashstorm, b: None, target: -1 },
            NetMsg::BossSpawn { root: 99, name: "Ossuary".into() },
            NetMsg::Loot { id: 2, item: "Relic Forkblade".into() },
            NetMsg::Leave { id: 3 },
        ];
        for m in msgs { assert_eq!(decode(&encode(&m)), Some(m.clone()), "roundtrip {m:?}"); }
    }

    #[test]
    fn session_tracks_party_join_and_move() {
        let mut s = Session::new(1);
        let mut t = Loopback::default();
        t.deliver(NetMsg::Join { id: 2, name: "Viktor".into(), seed: 7 });
        t.deliver(NetMsg::Move { id: 2, pos: V3::new(5, 5, 0) });
        let casts = s.pump(&mut t);
        assert!(casts.is_empty());
        assert_eq!(s.peers.len(), 1);
        assert_eq!(s.peers[0].name, "Viktor");
        assert_eq!(s.peers[0].pos, V3::new(5, 5, 0));
    }

    #[test]
    fn party_casts_surface_for_shared_boss_damage() {
        let mut s = Session::new(1);
        let mut t = Loopback::default();
        t.deliver(NetMsg::Cast { id: 2, a: Mcp::FluxCombo, b: None, target: 0 }); // a teammate's cast
        t.deliver(NetMsg::Cast { id: 1, a: Mcp::ZkVeil, b: None, target: -1 });   // our own echo — ignored
        let casts = s.pump(&mut t);
        assert_eq!(casts.len(), 1, "only the teammate's cast is returned for shared application");
    }

    #[test]
    fn broadcast_uses_transport() {
        let s = Session::new(1);
        let mut t = Loopback::default();
        s.broadcast_move(&mut t, V3::new(2, 2, 0));
        s.broadcast_cast(&mut t, Mcp::DexSwap, Some(Mcp::FluxCombo), 0);
        assert_eq!(t.sent.len(), 2);
        assert!(matches!(t.sent[0], NetMsg::Move { .. }));
    }
}
