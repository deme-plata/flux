//! Identity, attestation, citizenship.
use flux_quorum::{verify_quorum, QuorumPolicy, QuorumVerifier, SignedShare};
use serde::{Deserialize, Serialize};

/// 32-byte id.
pub type Hash = [u8; 32];

/// A citizen / state-actor identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// Human alias (e.g. "rocky", "ministry-of-health") — humans use this.
    pub alias: String,
    /// Public key (SQiSign in prod) — the protocol root of the identity.
    pub pubkey: Vec<u8>,
}

impl Identity {
    /// New identity.
    pub fn new(alias: impl Into<String>, pubkey: Vec<u8>) -> Self {
        Identity { alias: alias.into(), pubkey }
    }
    /// Stable protocol id = BLAKE3(pubkey). Aliases are for humans; this is for
    /// the chain. Two aliases can never collide on an id without colliding keys.
    pub fn id(&self) -> Hash {
        *blake3::hash(&self.pubkey).as_bytes()
    }
}

/// The canonical bytes an attestor signs: "identity `id` runs on a machine
/// measured as `quote`." Binding both means a stolen quote can't be re-bound to
/// another identity, and a swapped identity can't reuse a quote.
pub fn attestation_msg(identity: &Hash, quote: &Hash) -> Vec<u8> {
    let mut m = Vec::with_capacity(72);
    m.extend_from_slice(b"flux-nations/attest/v1");
    m.extend_from_slice(identity);
    m.extend_from_slice(quote);
    m
}

/// Verify a citizen's attestation: a QUORUM of the nation's attestors must have
/// signed `(identity ‖ quote)`. No single attestor suffices — sovereign trust is
/// distributed. Returns true iff the quorum formed over valid attestor sigs.
pub fn verify_attestation(
    identity: &Hash,
    quote: &Hash,
    attestors: &QuorumPolicy,
    shares: &[SignedShare],
    verifier: &dyn QuorumVerifier,
) -> bool {
    let msg = attestation_msg(identity, quote);
    verify_quorum(attestors, &msg, shares, verifier).reached()
}

fn merkle(leaves: &[Hash]) -> Hash {
    if leaves.is_empty() { return [0u8; 32]; }
    let mut lvl: Vec<Hash> = leaves.to_vec();
    lvl.sort_unstable(); // commit the SET, order-independent
    while lvl.len() > 1 {
        let mut nx = Vec::with_capacity((lvl.len() + 1) / 2);
        for p in lvl.chunks(2) {
            let mut h = blake3::Hasher::new();
            h.update(&p[0]);
            h.update(p.get(1).unwrap_or(&p[0]));
            nx.push(*h.finalize().as_bytes());
        }
        lvl = nx;
    }
    lvl[0]
}

/// Merkle root over a citizen-id set (the committed citizenship root).
pub fn citizen_root(citizens: &[Hash]) -> Hash {
    merkle(citizens)
}

/// A sovereign nation: a name, a charter commitment, a citizen set, and the
/// attestor quorum that admits citizens.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Nation {
    /// Display name.
    pub name: String,
    /// Charter/genesis commitment (BLAKE3 of the founding doc).
    pub charter: Hash,
    /// Admitted citizen ids.
    pub citizens: Vec<Hash>,
}

impl Nation {
    /// Found a nation.
    pub fn new(name: impl Into<String>, charter: Hash) -> Self {
        Nation { name: name.into(), charter, citizens: Vec::new() }
    }
    /// Admit a citizen IFF their attestation passes the attestor quorum.
    pub fn admit(
        &mut self,
        identity: &Identity,
        quote: &Hash,
        attestors: &QuorumPolicy,
        shares: &[SignedShare],
        verifier: &dyn QuorumVerifier,
    ) -> bool {
        let id = identity.id();
        if self.citizens.contains(&id) {
            return true;
        }
        if verify_attestation(&id, quote, attestors, shares, verifier) {
            self.citizens.push(id);
            true
        } else {
            false
        }
    }
    /// Is this id a citizen?
    pub fn is_citizen(&self, id: &Hash) -> bool {
        self.citizens.contains(id)
    }
    /// The committed citizenship root (verify in ~10 ms; same shape as a state root).
    pub fn citizen_root(&self) -> Hash {
        citizen_root(&self.citizens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_quorum::quorum::blake3_mac;
    use flux_quorum::{Blake3MacVerifier, QuorumMember, SignedShare};

    fn attestor(id: u32) -> QuorumMember {
        QuorumMember { id, pubkey: format!("attestor-{id}").into_bytes() }
    }
    fn policy(m: usize, n: u32) -> QuorumPolicy {
        QuorumPolicy::new(m, (0..n).map(attestor).collect())
    }
    /// An attestor's signed share over the attestation message.
    fn share(att_id: u32, msg: &[u8]) -> SignedShare {
        SignedShare { member: att_id, sig: blake3_mac(format!("attestor-{att_id}").as_bytes(), msg) }
    }

    #[test]
    fn identity_id_and_alias() {
        let i = Identity::new("ministry-of-health", b"pk-moh".to_vec());
        assert_eq!(i.alias, "ministry-of-health");
        assert_eq!(i.id(), *blake3::hash(b"pk-moh").as_bytes());
    }

    #[test]
    fn quorum_attestation_admits_citizen() {
        let mut nation = Nation::new("SIGIL Nation", [9u8; 32]);
        let citizen = Identity::new("rocky", b"pk-rocky".to_vec());
        let quote = *blake3::hash(b"tpm-quote-of-rockys-machine").as_bytes();
        let attestors = policy(3, 5); // 3-of-5 attestors
        let msg = attestation_msg(&citizen.id(), &quote);
        let shares = vec![share(0, &msg), share(1, &msg), share(2, &msg)];
        assert!(nation.admit(&citizen, &quote, &attestors, &shares, &Blake3MacVerifier));
        assert!(nation.is_citizen(&citizen.id()));
        assert_ne!(nation.citizen_root(), [0u8; 32]);
    }

    #[test]
    fn tampered_quote_is_rejected() {
        let mut nation = Nation::new("SIGIL Nation", [9u8; 32]);
        let citizen = Identity::new("imposter", b"pk-imp".to_vec());
        let real_quote = *blake3::hash(b"real").as_bytes();
        let attestors = policy(2, 3);
        // attestors signed the REAL quote...
        let msg = attestation_msg(&citizen.id(), &real_quote);
        let shares = vec![share(0, &msg), share(1, &msg)];
        // ...but we try to admit with a DIFFERENT quote → sigs don't match → reject.
        let fake_quote = *blake3::hash(b"fake").as_bytes();
        assert!(!nation.admit(&citizen, &fake_quote, &attestors, &shares, &Blake3MacVerifier));
        assert!(!nation.is_citizen(&citizen.id()));
    }

    #[test]
    fn below_quorum_is_rejected() {
        let mut nation = Nation::new("N", [0u8; 32]);
        let c = Identity::new("x", b"pk-x".to_vec());
        let quote = [1u8; 32];
        let attestors = policy(3, 5);
        let msg = attestation_msg(&c.id(), &quote);
        let shares = vec![share(0, &msg), share(1, &msg)]; // only 2 < 3
        assert!(!nation.admit(&c, &quote, &attestors, &shares, &Blake3MacVerifier));
    }

    #[test]
    fn citizen_root_is_order_independent() {
        let a = citizen_root(&[[1u8; 32], [2u8; 32], [3u8; 32]]);
        let b = citizen_root(&[[3u8; 32], [1u8; 32], [2u8; 32]]);
        assert_eq!(a, b, "the citizen SET commits, not the insertion order");
    }
}
