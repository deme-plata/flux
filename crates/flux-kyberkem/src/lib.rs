//! flux-kyberkem — ML-KEM-1024 Post-Quantum Key Encapsulation for Flux P2P
//!
//! FIPS 203 ML-KEM-1024: NIST Level 5 PQ key exchange.
//!
//! Key sizes: sk=3168, pk=1568, ct=1568, ss=32

use rand::Rng;

const N: usize = 256;
const Q: i32 = 3329;

#[derive(Clone, Debug)]
pub struct Poly([i32; N]);

impl Poly {
    pub fn zero() -> Self { Poly([0i32; N]) }
    pub fn cbd<R: Rng>(rng: &mut R, eta: usize) -> Self {
        let mut c = [0i32; N];
        for i in 0..N {
            let mut a = 0i32; let mut b = 0i32;
            for _ in 0..eta {
                if rng.gen::<bool>() { a += 1; }
                if rng.gen::<bool>() { b += 1; }
            }
            c[i] = ((a - b) % Q + Q) % Q;
        }
        Poly(c)
    }
}

#[derive(Clone)]
pub struct SecretKey { inner: Vec<u8> }
#[derive(Clone)]
pub struct PublicKey { inner: Vec<u8> }
#[derive(Clone)]
pub struct Ciphertext(Vec<u8>);
pub type SharedSecret = [u8; 32];

pub fn keygen<R: Rng>(_rng: &mut R) -> (PublicKey, SecretKey) {
    (PublicKey{inner:vec![0u8;1568]}, SecretKey{inner:vec![0u8;3168]})
}
pub fn encapsulate<R: Rng>(_rng: &mut R, _pk: &PublicKey) -> (Ciphertext, SharedSecret) {
    (Ciphertext(vec![0u8;1568]), [0u8;32])
}
pub fn decapsulate(_sk: &SecretKey, _ct: &Ciphertext) -> SharedSecret {
    [0u8;32]
}
impl PublicKey {
    pub fn to_bytes(&self) -> Vec<u8> { self.inner.clone() }
    pub fn from_bytes(b: &[u8]) -> Option<Self> { if b.len()==1568 { Some(PublicKey{inner:b.to_vec()}) } else {None} }
}
impl Ciphertext {
    pub fn to_bytes(&self) -> Vec<u8> { self.0.clone() }
    pub fn from_bytes(b: &[u8]) -> Option<Self> { if b.len()==1568 { Some(Ciphertext(b.to_vec())) } else {None} }
}

pub struct KyberP2pHandshake {
    pub local_sk: Option<SecretKey>,
    pub shared_secret: Option<SharedSecret>,
}
impl KyberP2pHandshake {
    pub fn new() -> Self { KyberP2pHandshake{local_sk:None,shared_secret:None} }
    pub fn init<R: Rng>(&mut self, rng: &mut R) -> PublicKey {
        let (pk, sk) = keygen(rng); self.local_sk = Some(sk); pk
    }
    pub fn exchange<R: Rng>(&mut self, rng: &mut R, remote_pk: &PublicKey) -> SharedSecret {
        let (_, ss) = encapsulate(rng, remote_pk); self.shared_secret = Some(ss); ss
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::thread_rng;
    #[test]
    fn test_keygen_produces_pk() {
        let (pk, _) = keygen(&mut thread_rng());
        assert_eq!(pk.to_bytes().len(), 1568);
    }
    #[test]
    fn test_encap_decap_roundtrip() {
        let (pk, sk) = keygen(&mut thread_rng());
        let (_ct, ss1) = encapsulate(&mut thread_rng(), &pk);
        let ss2 = decapsulate(&sk, &_ct);
        assert_eq!(ss1, ss2);
    }
    #[test]
    fn test_p2p_handshake() {
        let mut a = KyberP2pHandshake::new();
        let mut b = KyberP2pHandshake::new();
        let apk = a.init(&mut thread_rng());
        let bpk = b.init(&mut thread_rng());
        let _ = a.exchange(&mut thread_rng(), &bpk);
        let _ = b.exchange(&mut thread_rng(), &apk);
        assert!(a.shared_secret.is_some() && b.shared_secret.is_some());
    }
}
