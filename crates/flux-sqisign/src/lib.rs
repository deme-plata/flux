// flux-sqisign — SQIsign post-quantum isogeny signatures (NIST PQC Level 5, ~AES-256)
// API via sqisign-rs 0.3 (SQIsign v2.0): generate::<Level5>(), SigningKey::sign(), PublicKey::verify()
// Standard wire format at Level 5: 292-byte signature, 129-byte public key
// 1. SIGNATURE-AS-KEY — 292-byte sig as content-addressable flux-db key
// 2. CROSS-PRIMITIVE AGILITY — PqSigner trait
// 3. QUALITY TIERS — medium-aware scheme selection

use sqisign_rs::{generate, Level5, PublicKey, SigningKey, Verifier};

pub fn keygen() -> (Vec<u8>, Vec<u8>) {
    let mut rng = rand::rngs::OsRng;
    let (pk, sk): (PublicKey<Level5>, SigningKey<Level5>) = generate::<Level5>(&mut rng);
    (sk.to_bytes().unwrap(), pk.to_bytes().to_vec())
}

pub fn sign(msg: &[u8], sk_bytes: &[u8], pk_bytes: &[u8]) -> Result<Vec<u8>, String> {
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
    use super::*;
    #[test] fn test_roundtrip() { let (sk,pk)=keygen(); let s=sign(b"hi",&sk,&pk).unwrap(); assert!(verify(b"hi",&s,&pk).unwrap()); }
    #[test] fn test_wrong_msg() { let (sk,pk)=keygen(); let s=sign(b"a",&sk,&pk).unwrap(); assert!(!verify(b"b",&s,&pk).unwrap()); }
    #[test] fn test_wrong_key() { let (sk,pk)=keygen(); let (_,wp)=keygen(); let s=sign(b"x",&sk,&pk).unwrap(); assert!(!verify(b"x",&s,&wp).unwrap()); }
    #[test] fn test_sig_size() { let (sk,pk)=keygen(); assert_eq!(sign(b"x",&sk,&pk).unwrap().len(), 292); }
    #[test] fn test_bench() { let r=benchmark(3); assert!(r.keygen_avg_us>0.0); assert_eq!(r.sig_size,292); }
    #[test] fn test_tiers() { assert_eq!(recommend_for_medium("dns_txt",20),SigTier::SqiOnly); assert_eq!(recommend_for_medium("auth_session",0),SigTier::DilithiumPreferred); }
}
