// flux-sqisign — SQIsign post-quantum isogeny signatures (NIST PQC Level 5, ~AES-256)
// API via sqisign-rs 0.3 (SQIsign v2.0): generate::<Level5>(), SigningKey::sign(), PublicKey::verify()
// Standard wire format at Level 5: 292-byte signature, 129-byte public key
// 1. SIGNATURE-AS-KEY — 292-byte sig as content-addressable flux-db key
// 2. CROSS-PRIMITIVE AGILITY — PqSigner trait
// 3. QUALITY TIERS — medium-aware scheme selection

use sqisign_rs::{generate, Level5, PublicKey, SigningKey, Verifier};

/// Require-both SQIsign+Ed25519 hybrid provenance signatures (defense-in-depth).
pub mod hybrid;

pub fn keygen() -> (Vec<u8>, Vec<u8>) {
    let mut rng = rand::rngs::OsRng;
    let (pk, sk): (PublicKey<Level5>, SigningKey<Level5>) = generate::<Level5>(&mut rng);
    (sk.to_bytes().unwrap(), pk.to_bytes().to_vec())
}

/// Domain separator for wallet-derived SQIsign keys. Changing it changes every derived
/// key, so it is frozen.
const SQI_SEED_DOMAIN: &[u8] = b"flux-sqisign-wallet-key-v1";

/// Derive a SQIsign L5 keypair DETERMINISTICALLY from a 32-byte wallet seed.
///
/// [`keygen`] draws from `OsRng`, which is right for a fresh identity and wrong for a
/// wallet: a wallet's SQIsign key must be recoverable from the same seed phrase that
/// recovers everything else, or it becomes a second secret the owner has to back up
/// separately — and on this chain, losing it means permanent lockout from the shielded
/// ramps, because a registered key has no removal path.
///
/// The seed is domain-separated through BLAKE3 before it reaches the RNG, so this key is
/// independent of every other key derived from the same wallet seed (spend key, note
/// blinding, X25519 delivery key). Learning one must not yield another.
///
/// ChaCha20 rather than `StdRng`: `StdRng`'s algorithm is explicitly allowed to change
/// between `rand` releases, which for KEY DERIVATION means a routine dependency bump
/// silently hands the user a different key and locks them out. ChaCha20Rng's stream is
/// specified.
pub fn keygen_from_seed(seed: &[u8; 32]) -> (Vec<u8>, Vec<u8>) {
    use rand::SeedableRng;
    let mut h = blake3::Hasher::new();
    h.update(SQI_SEED_DOMAIN);
    h.update(seed);
    let mut rng = rand_chacha::ChaCha20Rng::from_seed(*h.finalize().as_bytes());
    let (pk, sk): (PublicKey<Level5>, SigningKey<Level5>) = generate::<Level5>(&mut rng);
    (sk.to_bytes().unwrap(), pk.to_bytes().to_vec())
}

pub fn sign(msg: &[u8], sk_bytes: &[u8], pk_bytes: &[u8]) -> Result<Vec<u8>, String> {
    // SEC-010: enforce the Level-5 public-key length before from_bytes so a
    // shorter (downgraded-level) key can't slip through.
    if pk_bytes.len() != public_key_size() {
        return Err(format!("SQIsign: pk must be {} bytes (Level 5), got {}", public_key_size(), pk_bytes.len()));
    }
    let sk: SigningKey<Level5> = SigningKey::<Level5>::from_bytes(sk_bytes)
        .map_err(|e| format!("SQIsign: invalid sk: {:?}", e))?;
    let pk: PublicKey<Level5> = PublicKey::<Level5>::from_bytes(pk_bytes)
        .map_err(|e| format!("SQIsign: invalid pk: {:?}", e))?;
    // Reconstruct the signing key with its public key
    let sig = sk.sign(msg, &mut rand::rngs::OsRng)
        .map_err(|e| format!("SQIsign sign: {:?}", e))?;
    // Verify locally before returning
    pk.verify(msg, &sig).map_err(|e| format!("SQIsign self-verify: {:?}", e))?;
    Ok(sig.to_bytes().to_vec())
}

pub fn verify(msg: &[u8], sig_bytes: &[u8], pk_bytes: &[u8]) -> Result<bool, String> {
    // SEC-010: enforce the exact Level-5 sizes BEFORE from_bytes. Without this a
    // 148-byte Level-1 signature (or a short pk) could be accepted as Level 5 —
    // a signature-strength downgrade. signature_size()/public_key_size() are the
    // canonical L5 wire lengths (292 / 129).
    if sig_bytes.len() != signature_size() {
        return Err(format!("SQIsign: sig must be {} bytes (Level 5), got {}", signature_size(), sig_bytes.len()));
    }
    if pk_bytes.len() != public_key_size() {
        return Err(format!("SQIsign: pk must be {} bytes (Level 5), got {}", public_key_size(), pk_bytes.len()));
    }
    let pk: PublicKey<Level5> = PublicKey::<Level5>::from_bytes(pk_bytes)
        .map_err(|e| format!("SQIsign: invalid pk: {:?}", e))?;
    let sig: sqisign_rs::Signature<Level5> = sqisign_rs::Signature::<Level5>::from_bytes(sig_bytes)
        .map_err(|e| format!("SQIsign: invalid sig: {:?}", e))?;
    match pk.verify(msg, &sig) {
        Ok(()) => Ok(true),
        Err(_) => Ok(false),
    }
}

pub const fn signature_size() -> usize { 292 }
pub const fn public_key_size() -> usize { 129 }

// ── Signature-as-Key ──

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}
fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    (0..hex.len()).step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i+2], 16).map_err(|e| format!("hex: {}", e)))
        .collect()
}

pub fn store_signed(db: &flux_db::Database, content: &[u8], sk: &[u8], pk: &[u8]) -> Result<String, String> {
    let sig = sign(content, sk, pk)?;
    let key = format!("sqisig:{}", hex_encode(&sig));
    db.put(key.as_bytes(), content).map_err(|e| format!("flux-db: {}", e))?;
    Ok(hex_encode(&sig))
}

pub fn retrieve_signed(db: &flux_db::Database, sig_hex: &str, pk: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let sig = hex_decode(sig_hex)?;
    let key = format!("sqisig:{}", hex_encode(&sig));
    match db.get(key.as_bytes()).map_err(|e| format!("flux-db: {}", e))? {
        Some(data) => {
            if verify(&data, &sig, pk)? { Ok(Some(data)) }
            else { Err("SQIsign verification failed".into()) }
        }
        None => Ok(None),
    }
}

// ── PqSigner Trait ──

pub trait PqSigner {
    fn keygen() -> (Vec<u8>, Vec<u8>);
    fn sign(msg: &[u8], sk: &[u8], pk: &[u8]) -> Result<Vec<u8>, String>;
    fn verify(msg: &[u8], sig: &[u8], pk: &[u8]) -> Result<bool, String>;
    fn signature_size() -> usize;
    fn public_key_size() -> usize;
    fn scheme_name() -> &'static str;
    fn is_post_quantum() -> bool { true }
}

pub struct SqisignSigner;
impl PqSigner for SqisignSigner {
    fn keygen() -> (Vec<u8>, Vec<u8>) { keygen() }
    fn sign(m: &[u8], sk: &[u8], pk: &[u8]) -> Result<Vec<u8>, String> { sign(m, sk, pk) }
    fn verify(m: &[u8], s: &[u8], pk: &[u8]) -> Result<bool, String> { verify(m, s, pk) }
    fn signature_size() -> usize { 292 }
    fn public_key_size() -> usize { 129 }
    fn scheme_name() -> &'static str { "SQIsign-V (NIST L5)" }
}

// ── Quality Tiers ──

#[derive(Debug, Clone, PartialEq)]
pub enum SigTier { SqiOnly, Both, DilithiumPreferred }

pub fn recommend_for_medium(medium: &str, payload_bytes: usize) -> SigTier {
    let (d, s) = (4595usize, 292usize);
    match medium {
        "dns_txt" | "ble_advertisement" =>
            if s+payload_bytes <= 255 && d+payload_bytes > 255 { SigTier::SqiOnly } else { SigTier::Both },
        "qr_code" =>
            if s+payload_bytes <= 2900 && d+payload_bytes > 2900 { SigTier::SqiOnly } else { SigTier::Both },
        "nfc_ntag215" =>
            if s+payload_bytes <= 504 && d+payload_bytes > 504 { SigTier::SqiOnly } else { SigTier::Both },
        "blockchain_tx" => SigTier::SqiOnly,
        "auth_session" => SigTier::DilithiumPreferred,
        _ => SigTier::Both,
    }
}

// ── Benchmark ──

#[derive(Debug, Clone)]
pub struct BenchmarkResult { pub iterations: usize, pub sig_size: usize, pub pk_size: usize,
    pub keygen_avg_us: f64, pub sign_avg_us: f64, pub verify_avg_us: f64 }

pub fn benchmark(iterations: usize) -> BenchmarkResult {
    use std::time::Instant;
    let msg = b"Flux SQIsign benchmark";
    let mut kt=0u128; let mut st=0u128; let mut vt=0u128;
    for _ in 0..iterations {
        let t0=Instant::now(); let (sk,pk)=keygen(); kt+=t0.elapsed().as_nanos();
        let t1=Instant::now(); let sig=sign(msg,&sk,&pk).unwrap(); st+=t1.elapsed().as_nanos();
        let t2=Instant::now(); assert!(verify(msg,&sig,&pk).unwrap()); vt+=t2.elapsed().as_nanos();
    }
    BenchmarkResult { iterations, sig_size: signature_size(), pk_size: public_key_size(),
        keygen_avg_us: (kt/iterations as u128/1000) as f64,
        sign_avg_us: (st/iterations as u128/1000) as f64,
        verify_avg_us: (vt/iterations as u128/1000) as f64 }
}

// ── Tests ──

#[cfg(test)]
mod tests {

    /// A wallet key must be RECOVERABLE. If this ever fails, everyone who registered a
    /// derived key is locked out of the shielded ramps — there is no removal path.
    #[test]
    fn seed_derivation_is_deterministic_and_seed_separated() {
        let seed_a = [7u8; 32];
        let mut seed_b = [7u8; 32];
        seed_b[31] = 8;

        let (sk1, pk1) = keygen_from_seed(&seed_a);
        let (sk2, pk2) = keygen_from_seed(&seed_a);
        assert_eq!(pk1, pk2, "same seed must yield the SAME public key, every time");
        assert_eq!(sk1, sk2, "and the same secret key");

        let (_, pk_other) = keygen_from_seed(&seed_b);
        assert_ne!(pk1, pk_other, "a different seed must yield a different key");

        assert_eq!(pk1.len(), public_key_size(), "derived key must be Level 5");

        // And it must actually work as a key, not merely be the right shape.
        let msg = b"shielded ramp authorization";
        let sig = sign(msg, &sk1, &pk1).expect("derived key signs");
        assert!(verify(msg, &sig, &pk1).unwrap_or(false), "derived key verifies its own signature");
        assert_eq!(sig.len(), signature_size(), "Level 5 signature size");
    }

    /// The derived key must be INDEPENDENT of the raw seed bytes — domain separation is
    /// what stops one compromised derived key from revealing the others (spend key, note
    /// blinding, X25519 delivery key) that come from the same wallet seed.
    #[test]
    fn derived_key_is_not_the_raw_seed() {
        let seed = [0x42u8; 32];
        let (sk, pk) = keygen_from_seed(&seed);
        assert!(!sk.windows(32).any(|w| w == seed), "seed must not appear verbatim in the secret key");
        assert!(!pk.windows(32).any(|w| w == seed), "nor in the public key");
    }

    use super::*;
    #[test] fn test_roundtrip() { let (sk,pk)=keygen(); let s=sign(b"hi",&sk,&pk).unwrap(); assert!(verify(b"hi",&s,&pk).unwrap()); }
    #[test] fn test_wrong_msg() { let (sk,pk)=keygen(); let s=sign(b"a",&sk,&pk).unwrap(); assert!(!verify(b"b",&s,&pk).unwrap()); }
    #[test] fn test_wrong_key() { let (sk,pk)=keygen(); let (_,wp)=keygen(); let s=sign(b"x",&sk,&pk).unwrap(); assert!(!verify(b"x",&s,&wp).unwrap()); }
    #[test] fn test_sig_size() { let (sk,pk)=keygen(); assert_eq!(sign(b"x",&sk,&pk).unwrap().len(), 292); }
    #[test] fn test_bench() { let r=benchmark(3); assert!(r.keygen_avg_us>0.0); assert_eq!(r.sig_size,292); }
    #[test] fn test_tiers() {
        // blockchain_tx genuinely yields SqiOnly (unconditional, line ~127).
        assert_eq!(recommend_for_medium("blockchain_tx",0),SigTier::SqiOnly);
        // dns_txt: the corrected 292-byte SQIsign sig already exceeds the
        // 255-byte DNS-TXT single-string limit, so neither scheme fits alone
        // -> Both. (This asserted SqiOnly back when the sig was mis-sized at
        // 148/177 B; the Level-5 size fix exposed the stale expectation.)
        assert_eq!(recommend_for_medium("dns_txt",20),SigTier::Both);
        assert_eq!(recommend_for_medium("auth_session",0),SigTier::DilithiumPreferred);
    }

    #[test]
    fn test_rejects_wrong_length_sig_and_pk() {
        // SEC-010: a downgraded-length (e.g. Level-1 148-byte) sig must be a hard
        // error, never silently accepted; likewise a short public key.
        let (sk, pk) = keygen();
        let sig = sign(b"x", &sk, &pk).unwrap();
        assert_eq!(sig.len(), signature_size());
        // truncated sig → Err, not Ok(false)
        assert!(verify(b"x", &sig[..148], &pk).is_err());
        assert!(verify(b"x", &[], &pk).is_err());
        // short pk → Err
        assert!(verify(b"x", &sig, &pk[..64]).is_err());
        // correct lengths still verify
        assert!(verify(b"x", &sig, &pk).unwrap());
    }
}
