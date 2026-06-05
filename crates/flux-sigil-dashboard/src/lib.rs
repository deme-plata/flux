//! flux-sigil-dashboard — generates a static JSON snapshot consumed by the
//! `sigil-wallet` apiShim. While the real sigil-node JSON-RPC isn't wired up
//! to the wallet yet (Phase C 0.3.0 is shipping this shim as an in-flight
//! interim), the dashboard needs SOMETHING coherent to render — height,
//! peers, recent blocks, address balances — and that's what this crate
//! emits.
//!
//! All addresses queried through the shim return a generous default balance
//! (100 SGL) so a freshly-installed wallet immediately shows a non-zero
//! number. Specific seeded addresses (Viktor, Rocky, Adrian, Codex) get
//! distinct values so we can tell them apart in screenshots.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardSnapshot {
    pub status: NodeStatus,
    pub recent_blocks: Vec<Block>,
    pub address_balances: HashMap<String, BalanceEntry>,
    pub default_balance: BalanceEntry,
    pub generated_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeStatus {
    pub network_id: String,
    pub height: u64,
    pub peers: u32,
    pub symbol: String,
    pub version: String,
    pub note: String,
    pub block_time_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub height: u64,
    pub hash: String,
    pub parent_hash: String,
    pub timestamp_ms: u64,
    pub tx_count: u32,
    pub miner: String,
    pub state_roots: StateRoots,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateRoots {
    pub wallet: String,
    pub dex: String,
    pub event_log: String,
    pub contract: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceEntry {
    pub address: String,
    pub balance_sgl: String,
    pub balance_raw: String,
    pub tx_count: u32,
    pub note: Option<String>,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Deterministic short hash from a seed string (BLAKE3 would be overkill here;
/// we just need stable-looking 12-char hex IDs for the static snapshot).
fn pseudo_hash(seed: &str) -> String {
    // FNV-1a 64-bit
    let mut h: u64 = 0xcbf29ce484222325;
    for b in seed.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

fn block_hash(height: u64) -> String {
    format!("sgl_blk_{}", pseudo_hash(&format!("sigil-g0:{}", height)))
}

fn state_roots(height: u64) -> StateRoots {
    StateRoots {
        wallet:    format!("0x{}", pseudo_hash(&format!("wallet:{}", height))),
        dex:       format!("0x{}", pseudo_hash(&format!("dex:{}", height))),
        event_log: format!("0x{}", pseudo_hash(&format!("event_log:{}", height))),
        contract:  format!("0x{}", pseudo_hash(&format!("contract:{}", height))),
    }
}

/// Build a snapshot, anchored at `tip_height`. Most callers want the public
/// [`make_snapshot`] which uses a sensible default tip.
pub fn build_snapshot(tip_height: u64) -> DashboardSnapshot {
    let now = now_ms();
    let block_time_ms = 12_000u64;

    // 12 most-recent blocks, descending.
    let mut recent_blocks: Vec<Block> = Vec::with_capacity(12);
    for i in 0..12 {
        let h = tip_height.saturating_sub(i);
        recent_blocks.push(Block {
            height: h,
            hash: block_hash(h),
            parent_hash: block_hash(h.saturating_sub(1)),
            timestamp_ms: now.saturating_sub(i * block_time_ms),
            tx_count: ((h as u32) * 7) % 11,
            miner: format!("sgl1miner{:03}", (h % 13) + 1),
            state_roots: state_roots(h),
        });
    }

    // Seeded addresses with distinct balances.
    let mut address_balances: HashMap<String, BalanceEntry> = HashMap::new();
    let seeds: &[(&str, &str, &str)] = &[
        ("sgl1viktor",    "1000.00", "operator (Viktor)"),
        ("sgl1rocky",     "650.00",  "rocky-sigil — engineer agent"),
        ("sgl1adrian",    "100.00",  "adrian (Cursor/Erid) — CLAI welcome drop"),
        ("sgl1codex",     "100.00",  "codex (GPT-5.5) — CLAI welcome drop"),
        ("sgl1default",   "100.00",  "default preview balance"),
    ];
    for (addr, sgl, note) in seeds {
        address_balances.insert(
            addr.to_string(),
            BalanceEntry {
                address: addr.to_string(),
                balance_sgl: sgl.to_string(),
                balance_raw: format!("{}{}", sgl.replace('.', ""), "0000000000000000"),
                tx_count: 7,
                note: Some(note.to_string()),
            },
        );
    }

    let default_balance = BalanceEntry {
        address: "sgl1default".to_string(),
        balance_sgl: "100.00".to_string(),
        balance_raw: "100000000000000000000".to_string(),
        tx_count: 0,
        note: Some("SIGIL preview default — any wallet seen for the first time".to_string()),
    };

    DashboardSnapshot {
        status: NodeStatus {
            network_id: "sigil-g0".to_string(),
            height: tip_height,
            peers: 2,
            symbol: "SGL".to_string(),
            version: format!("sigil-dashboard {}", env!("CARGO_PKG_VERSION")),
            note: "Static snapshot from flux-sigil-dashboard. Real sigil-node JSON-RPC lands in Phase D 0.4.0.".to_string(),
            block_time_ms,
        },
        recent_blocks,
        address_balances,
        default_balance,
        generated_ms: now,
    }
}

/// Default tip used by the CLI when no `--tip` is passed: roughly one block
/// per 12 s since the SIGIL g0 launch (2026-05-30). Cheap monotonic approximation.
pub fn default_tip() -> u64 {
    let launch_ms: u64 = 1_780_137_000_000; // 2026-05-30 09:50 UTC
    let now = now_ms();
    let elapsed = now.saturating_sub(launch_ms);
    (elapsed / 12_000).max(1)
}

pub fn make_snapshot() -> DashboardSnapshot {
    build_snapshot(default_tip())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_has_12_blocks() {
        let s = build_snapshot(1000);
        assert_eq!(s.recent_blocks.len(), 12);
        assert_eq!(s.recent_blocks[0].height, 1000);
        assert_eq!(s.recent_blocks[11].height, 989);
    }

    #[test]
    fn parent_hash_chains_correctly() {
        let s = build_snapshot(500);
        // recent[0] is height 500, recent[1] is height 499.
        // recent[0].parent_hash should equal block_hash(499) which is recent[1].hash.
        assert_eq!(s.recent_blocks[0].parent_hash, s.recent_blocks[1].hash);
    }

    #[test]
    fn seeded_balances_are_distinct() {
        let s = build_snapshot(100);
        assert_eq!(s.address_balances["sgl1viktor"].balance_sgl, "1000.00");
        assert_eq!(s.address_balances["sgl1rocky"].balance_sgl, "650.00");
        assert_eq!(s.address_balances["sgl1default"].balance_sgl, "100.00");
    }

    #[test]
    fn pseudo_hash_is_deterministic_and_hex() {
        let a = pseudo_hash("foo");
        let b = pseudo_hash("foo");
        let c = pseudo_hash("bar");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn default_tip_monotonic() {
        // default_tip is at least 1 and bounded by elapsed-since-launch / 12s.
        let t = default_tip();
        assert!(t >= 1);
    }

    #[test]
    fn snapshot_serializes_to_json() {
        let s = build_snapshot(42);
        let j = serde_json::to_string(&s).unwrap();
        assert!(j.contains("sigil-g0"));
        assert!(j.contains("sgl1viktor"));
    }
}
