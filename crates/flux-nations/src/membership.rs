//! Citizen membership proofs — prove you're a citizen WITHOUT revealing the set.
//!
//! A [`MembershipProof`] is a merkle inclusion path from a citizen id up to the
//! nation's `citizen_root`. A citizen (or a light client) verifies citizenship
//! in ~log(N) hashes against the committed root, learning nothing about the
//! other citizens. Same sorted-set merkle as [`citizen_root`](crate::citizen_root).

use serde::{Deserialize, Serialize};

use crate::nations::Hash;

fn pair(a: &Hash, b: &Hash) -> Hash {
    let mut h = blake3::Hasher::new();
    h.update(a);
    h.update(b);
    *h.finalize().as_bytes()
}

/// A merkle inclusion path: each step is (sibling, sibling_is_on_the_right).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipProof {
    /// Sibling hashes from leaf → root, with their side.
    pub siblings: Vec<(Hash, bool)>,
}

/// Build a proof that `id` is in `citizens` (None if not a citizen).
pub fn prove_membership(citizens: &[Hash], id: &Hash) -> Option<MembershipProof> {
    let mut lvl: Vec<Hash> = citizens.to_vec();
    lvl.sort_unstable();
    let mut idx = lvl.iter().position(|x| x == id)?;
    let mut siblings = Vec::new();
    while lvl.len() > 1 {
        let sib_is_right = idx % 2 == 0;
        let sib_idx = if sib_is_right { idx + 1 } else { idx - 1 };
        let sib = *lvl.get(sib_idx).unwrap_or(&lvl[idx]); // odd tail → duplicate self
        siblings.push((sib, sib_is_right));
        let mut nx = Vec::with_capacity((lvl.len() + 1) / 2);
        for p in lvl.chunks(2) {
            nx.push(pair(&p[0], p.get(1).unwrap_or(&p[0])));
        }
        lvl = nx;
        idx /= 2;
    }
    Some(MembershipProof { siblings })
}

/// Verify `id` hashes up to `root` via `proof`. Constant-memory, ~log(N) hashes.
pub fn verify_membership(root: &Hash, id: &Hash, proof: &MembershipProof) -> bool {
    let mut cur = *id;
    for (sib, sib_is_right) in &proof.siblings {
        cur = if *sib_is_right { pair(&cur, sib) } else { pair(sib, &cur) };
    }
    &cur == root
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::citizen_root;

    fn ids(n: u8) -> Vec<Hash> {
        (1..=n).map(|i| *blake3::hash(&[i]).as_bytes()).collect()
    }

    #[test]
    fn citizen_proves_membership_against_root() {
        for n in [1u8, 2, 3, 5, 8, 13] {
            let set = ids(n);
            let root = citizen_root(&set);
            for c in &set {
                let proof = prove_membership(&set, c).expect("citizen has a proof");
                assert!(verify_membership(&root, c, &proof), "n={n} member must verify");
            }
        }
    }

    #[test]
    fn non_member_has_no_proof_and_cannot_forge() {
        let set = ids(6);
        let root = citizen_root(&set);
        let outsider = *blake3::hash(b"not-a-citizen").as_bytes();
        assert!(prove_membership(&set, &outsider).is_none());
        // even a real member's proof doesn't validate the outsider against the root.
        let real = prove_membership(&set, &set[0]).unwrap();
        assert!(!verify_membership(&root, &outsider, &real));
    }

    #[test]
    fn proof_reveals_nothing_but_log_n_hashes() {
        let set = ids(8);
        let proof = prove_membership(&set, &set[3]).unwrap();
        assert!(proof.siblings.len() <= 3, "8 leaves → ≤3 siblings (log2 8)");
    }
}
