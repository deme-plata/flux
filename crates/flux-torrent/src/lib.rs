//! flux-torrent — flux-aether over BitTorrent.
//!
//! A [`FileBlock`](flux_aether::FileBlock)'s shards are already BLAKE3 content-
//! addressed, so they map 1:1 onto torrent pieces. flux-torrent turns a
//! FileBlock into a torrent ([`TorrentInfo`] + a [`magnet`] URI) and treats
//! each encrypted [`Shard`](flux_aether::Shard) as a piece. **Music in aether
//! is available ONLY through torrent download**: you discover + fetch K
//! encrypted shards from the swarm (DHT/peers), then decrypt + reassemble
//! locally (you must hold the FileBlock + key). The swarm only ever carries
//! mixed ciphertext — here, there, and (assembled) nowhere.
//!
//! Transport is the ZenTorrent Rust engine (`/opt/orobit/shared/ZenTorrent`,
//! tokio + serde_bencode, DHT/peer/piece/streaming) — flux-torrent provides the
//! aether↔torrent mapping; ZenTorrent moves the bytes (wired in a later lane).
#![warn(missing_docs)]
pub mod torrent;
pub use torrent::{magnet, parse_magnet, piece_cids, TorrentInfo};
