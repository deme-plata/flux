//! The Merkle-rooted tower state — Quillon Graph expressed as architecture.
//!
//! At any moment the whole organism folds to one 32-byte root:
//!
//!   R_tower = Merkle(R_structure, R_transport, R_vault, R_bank,
//!                    R_operating, R_physics)
//!
//! and the building's full node periodically commits
//!
//!   C = BLAKE3(R_tower ‖ H_vault ‖ twin-version)
//!
//! to Quillon Graph. The vault anchors (vault.rs) freeze custody history;
//! this commitment freezes the STATE OF THE WHOLE BUILDING — transport
//! metrics, ledger, operating index, physics envelopes — into the same
//! externally witnessed timeline. Gold beneath it, people and machines
//! inside it, the bank operating through it, the twin predicting it, the
//! node witnessing it — and the light at the crown is the visible heartbeat
//! of exactly this value changing.

use crate::Building;
use serde::Serialize;

/// The twin's version, bound into every commitment so a replayed history
/// also pins WHICH simulator produced it.
pub const TWIN_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize)]
pub struct TowerStateCommitment {
    pub root: [u8; 32],
    pub commitment: [u8; 32],
    pub twin_version: &'static str,
}

fn leaf<T: Serialize>(label: &str, value: &T) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(label.as_bytes());
    h.update(serde_json::to_string(value).expect("state serializes").as_bytes());
    *h.finalize().as_bytes()
}

fn merkle(mut layer: Vec<[u8; 32]>) -> [u8; 32] {
    if layer.is_empty() {
        return [0u8; 32];
    }
    while layer.len() > 1 {
        let mut next = Vec::with_capacity(layer.len().div_ceil(2));
        for pair in layer.chunks(2) {
            if pair.len() == 2 {
                let mut h = blake3::Hasher::new();
                h.update(&pair[0]);
                h.update(&pair[1]);
                next.push(*h.finalize().as_bytes());
            } else {
                next.push(pair[0]);
            }
        }
        layer = next;
    }
    layer[0]
}

/// Fold the building to its Merkle root, then bind it to the vault head and
/// the twin's version. Deterministic: the same building state always folds
/// to the same commitment.
pub fn tower_state(b: &Building) -> TowerStateCommitment {
    let leaves = vec![
        leaf("structure", &b.spec),
        leaf("transport", &b.transport.metrics()),
        leaf("vault", &(b.vault.head_hex(), hex::encode(b.vault.inventory_root()))),
        leaf("bank", &b.bank.balance(crate::bank::TREASURY)),
        leaf("operating", &b.cortex.building_health().to_bits()),
        leaf("physics", &crate::physics::bridge_movement(&b.spec).map(|r| r.joint_budget_m.to_bits())),
    ];
    let root = merkle(leaves);
    let mut h = blake3::Hasher::new();
    h.update(&root);
    h.update(b.vault.head_hex().as_bytes());
    h.update(TWIN_VERSION.as_bytes());
    TowerStateCommitment { root, commitment: *h.finalize().as_bytes(), twin_version: TWIN_VERSION }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::{Clearance, GoldBar};

    #[test]
    fn same_state_same_commitment() {
        let a = tower_state(&Building::quillon_default().unwrap());
        let b = tower_state(&Building::quillon_default().unwrap());
        assert_eq!(a.root, b.root);
        assert_eq!(a.commitment, b.commitment);
    }

    #[test]
    fn any_subsystem_change_moves_the_root() {
        let mut b = Building::quillon_default().unwrap();
        let before = tower_state(&b);
        b.vault
            .deposit(
                GoldBar { serial: "AU-STATE-1".into(), weight_g: 12_400, fineness_ppm: 999_900 },
                "bank",
                Clearance::Officer,
                1,
            )
            .unwrap();
        let after = tower_state(&b);
        assert_ne!(before.root, after.root);
        assert_ne!(before.commitment, after.commitment);
        assert_eq!(after.twin_version, TWIN_VERSION);
    }
}
