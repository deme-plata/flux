//! The sealed link: ephemeral X25519 → HKDF-SHA256 → AES-256-GCM, one key per
//! direction, counter nonces, and a 6-digit comparison code.
//!
//! ```text
//!   shared = X25519(my_esk, peer_epk)
//!   prk    = HKDF-Extract(salt = "sigil-ble-v1", ikm = shared)
//!   k_c2p  = HKDF-Expand(prk, "c2p" ‖ epk_central ‖ epk_peripheral, 32)
//!   k_p2c  = HKDF-Expand(prk, "p2c" ‖ epk_central ‖ epk_peripheral, 32)
//!   sas    = HKDF-Expand(prk, "sas" ‖ epk_central ‖ epk_peripheral, 4)  as u32 BE  mod 1_000_000
//!   nonce  = dir(1 byte: 0 = c→p, 1 = p→c) ‖ 0x00×3 ‖ counter u64 BE
//!   sealed = nonce(12) ‖ AES-256-GCM(key_dir, nonce, plaintext, aad = "sigil-ble-v1")
//! ```
//!
//! Binding both public keys into the expand `info` means a man-in-the-middle who swaps
//! one key gets a different `sas` on each side — that is the property the two people
//! comparing screens are relying on. Nothing else authenticates the link; say so in UI.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

pub const SALT: &[u8] = b"sigil-ble-v1";
pub const AAD: &[u8] = b"sigil-ble-v1";
pub const NONCE_LEN: usize = 12;
pub const TAG_LEN: usize = 16;

/// Which end of the link this is. Decides which derived key seals outgoing bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Advertises and runs the GATT server (the phone).
    Peripheral,
    /// Scans and connects (Chrome, the laptop, or the other phone).
    Central,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LinkError {
    #[error("peer key must be 32 bytes")]
    BadPeerKey,
    #[error("sealed message too short")]
    TooShort,
    #[error("authentication failed")]
    Auth,
    #[error("nonce not the next expected ({expected}), got {got}")]
    Replay { expected: u64, got: u64 },
    #[error("wrong direction byte")]
    Direction,
}

/// An ephemeral X25519 keypair for one session.
pub struct Ephemeral {
    secret: StaticSecret,
    public: PublicKey,
}

impl Ephemeral {
    pub fn generate() -> Self {
        let secret = StaticSecret::random_from_rng(rand::rngs::OsRng);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    /// Deterministic, for tests and vectors only.
    pub fn from_secret(sk: [u8; 32]) -> Self {
        let secret = StaticSecret::from(sk);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    pub fn public_bytes(&self) -> [u8; 32] {
        self.public.to_bytes()
    }

    pub fn public_hex(&self) -> String {
        hex::encode(self.public_bytes())
    }

    /// Complete the handshake against the peer's ephemeral key.
    pub fn link(self, role: Role, peer_epk: &[u8]) -> Result<Link, LinkError> {
        let peer: [u8; 32] = peer_epk.try_into().map_err(|_| LinkError::BadPeerKey)?;
        let shared = self.secret.diffie_hellman(&PublicKey::from(peer));
        let (epk_c, epk_p) = match role {
            Role::Central => (self.public_bytes(), peer),
            Role::Peripheral => (peer, self.public_bytes()),
        };
        Ok(Link::derive(role, shared.as_bytes(), &epk_c, &epk_p))
    }
}

/// The keyed link. Seals in one direction, opens the other.
pub struct Link {
    role: Role,
    k_c2p: [u8; 32],
    k_p2c: [u8; 32],
    sas: u32,
    send_ctr: u64,
    recv_ctr: u64,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link").field("role", &self.role).field("sas", &self.sas).finish()
    }
}

impl Link {
    /// Derive from the raw shared secret. Public so vectors and the JNI/JS ports can be
    /// checked at this seam; normal callers go through [`Ephemeral::link`].
    pub fn derive(role: Role, shared: &[u8; 32], epk_central: &[u8; 32], epk_peripheral: &[u8; 32]) -> Self {
        let hk = Hkdf::<Sha256>::new(Some(SALT), shared);
        let mut info = Vec::with_capacity(3 + 64);
        let mut expand = |label: &[u8], out: &mut [u8]| {
            info.clear();
            info.extend_from_slice(label);
            info.extend_from_slice(epk_central);
            info.extend_from_slice(epk_peripheral);
            hk.expand(&info, out).expect("HKDF output length is small");
        };
        let mut k_c2p = [0u8; 32];
        let mut k_p2c = [0u8; 32];
        let mut sas = [0u8; 4];
        expand(b"c2p", &mut k_c2p);
        expand(b"p2c", &mut k_p2c);
        expand(b"sas", &mut sas);
        Self {
            role,
            k_c2p,
            k_p2c,
            sas: u32::from_be_bytes(sas) % 1_000_000,
            send_ctr: 0,
            recv_ctr: 0,
        }
    }

    pub fn role(&self) -> Role {
        self.role
    }

    /// The 6-digit code both screens show. Zero-padded.
    pub fn sas_code(&self) -> String {
        format!("{:06}", self.sas)
    }

    pub fn sas(&self) -> u32 {
        self.sas
    }

    fn keys(&self) -> ([u8; 32], [u8; 32], u8, u8) {
        // (send key, recv key, send dir byte, recv dir byte)
        match self.role {
            Role::Central => (self.k_c2p, self.k_p2c, 0, 1),
            Role::Peripheral => (self.k_p2c, self.k_c2p, 1, 0),
        }
    }

    fn nonce(dir: u8, ctr: u64) -> [u8; NONCE_LEN] {
        let mut n = [0u8; NONCE_LEN];
        n[0] = dir;
        n[4..].copy_from_slice(&ctr.to_be_bytes());
        n
    }

    /// Seal an outgoing message. Advances the send counter.
    pub fn seal(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let (k, _, dir, _) = self.keys();
        let nonce = Self::nonce(dir, self.send_ctr);
        self.send_ctr += 1;
        let cipher = Aes256Gcm::new((&k).into());
        let ct = cipher
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext, aad: AAD })
            .expect("AES-GCM encrypt cannot fail for in-memory buffers");
        let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        out
    }

    /// Open an incoming sealed message. Enforces strict counter order (no replay, no
    /// reordering) — on a single GATT characteristic there is no legitimate reordering.
    pub fn open(&mut self, sealed: &[u8]) -> Result<Vec<u8>, LinkError> {
        if sealed.len() < NONCE_LEN + TAG_LEN {
            return Err(LinkError::TooShort);
        }
        let (_, k, _, recv_dir) = self.keys();
        let nonce = &sealed[..NONCE_LEN];
        if nonce[0] != recv_dir {
            return Err(LinkError::Direction);
        }
        let ctr = u64::from_be_bytes(nonce[4..].try_into().unwrap());
        if ctr != self.recv_ctr {
            return Err(LinkError::Replay { expected: self.recv_ctr, got: ctr });
        }
        let cipher = Aes256Gcm::new((&k).into());
        let pt = cipher
            .decrypt(Nonce::from_slice(nonce), Payload { msg: &sealed[NONCE_LEN..], aad: AAD })
            .map_err(|_| LinkError::Auth)?;
        self.recv_ctr += 1;
        Ok(pt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (Link, Link) {
        let c = Ephemeral::generate();
        let p = Ephemeral::generate();
        let cpk = c.public_bytes();
        let ppk = p.public_bytes();
        (c.link(Role::Central, &ppk).unwrap(), p.link(Role::Peripheral, &cpk).unwrap())
    }

    #[test]
    fn both_ends_agree_and_roundtrip_both_ways() {
        let (mut c, mut p) = pair();
        assert_eq!(c.sas_code(), p.sas_code());
        assert_eq!(c.sas_code().len(), 6);
        for i in 0..5u8 {
            let m = vec![i; 40 + i as usize];
            assert_eq!(p.open(&c.seal(&m)).unwrap(), m);
            assert_eq!(c.open(&p.seal(&m)).unwrap(), m);
        }
    }

    #[test]
    fn replay_reorder_tamper_and_direction_are_refused() {
        let (mut c, mut p) = pair();
        let s1 = c.seal(b"one");
        let s2 = c.seal(b"two");
        assert_eq!(p.open(&s2), Err(LinkError::Replay { expected: 0, got: 1 }));
        assert_eq!(p.open(&s1).unwrap(), b"one");
        assert_eq!(p.open(&s1), Err(LinkError::Replay { expected: 1, got: 0 }));
        let mut bad = s2.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert_eq!(p.open(&bad), Err(LinkError::Auth));
        assert_eq!(p.open(&s2).unwrap(), b"two");
        // a message sealed BY the peripheral fed back to the peripheral: wrong direction
        let own = p.seal(b"x");
        assert_eq!(p.open(&own), Err(LinkError::Direction));
        assert_eq!(p.open(&[0u8; 5]), Err(LinkError::TooShort));
    }

    #[test]
    fn mitm_produces_different_codes() {
        let a = Ephemeral::generate();
        let b = Ephemeral::generate();
        let m1 = Ephemeral::generate();
        let m2 = Ephemeral::generate();
        let (apk, bpk, m1pk, m2pk) = (a.public_bytes(), b.public_bytes(), m1.public_bytes(), m2.public_bytes());
        // A talks to M1 (posing as B); B talks to M2 (posing as A).
        let la = a.link(Role::Central, &m1pk).unwrap();
        let lb = b.link(Role::Peripheral, &m2pk).unwrap();
        let _ = (apk, bpk);
        // Overwhelmingly likely to differ; the point is they are not forced equal.
        assert_ne!(la.sas_code(), lb.sas_code());
    }
}
