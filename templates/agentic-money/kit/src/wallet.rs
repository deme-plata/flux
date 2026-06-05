//! Local-seed wallet bootstrap — the fix for the `create_wallet` no-mnemonic
//! trap.
//!
//! Some `create_wallet` MCP paths hand back an address with NO recoverable
//! secret, so the wallet can receive funds but can never spend them — a dead
//! drop. The fix every agent onboarding should use: **generate the entropy
//! locally, keep it, derive the address from it.** Then the wallet is
//! spendable because YOU hold the seed.
//!
//! ```no_run
//! let w = agentic_money_kit::wallet::bootstrap().unwrap();
//! println!("address {}  seed {}", w.address, w.seed_hex);
//! // hand `w.address` to a funder; keep `w.seed_hex` secret to sign.
//! ```
//!
//! CRYPTO-AGILITY NOTE (Flux/SIGIL Stargate discipline): the address here is a
//! display/identity derivation (`blake3(seed)`), NOT a consensus keypair. Real
//! signing keys must be derived through your chain's keypair scheme
//! (ed25519 / SQIsign / whatever `flux-eternal-cypher` dispatches) — see
//! [`derive_signing_key`] for the single seam to wire that in. Never hardcode a
//! signature algorithm into agent code; dispatch it.

use std::io::Read;

/// A bootstrapped wallet: a stable address plus the seed that controls it.
#[derive(Debug, Clone)]
pub struct Wallet {
    /// 64-hex-char (32-byte) seed — the secret. Persist it; losing it loses
    /// the wallet.
    pub seed_hex: String,
    /// Display/settlement address derived from the seed.
    pub address: String,
}

impl Wallet {
    /// Re-derive a wallet from a known seed (e.g. loaded from disk / env).
    pub fn from_seed_hex(seed_hex: &str) -> Self {
        let address = address_from_seed_hex(seed_hex);
        Self { seed_hex: seed_hex.to_string(), address }
    }
}

/// Generate a fresh wallet from 32 bytes of OS entropy (`/dev/urandom`).
///
/// Production note: `/dev/urandom` is a fine CSPRNG on Linux; if you target
/// other platforms, swap in `getrandom`. We avoid the dependency here to keep
/// the template a tiny static binary.
pub fn bootstrap() -> std::io::Result<Wallet> {
    let mut seed = [0u8; 32];
    let mut f = std::fs::File::open("/dev/urandom")?;
    f.read_exact(&mut seed)?;
    let seed_hex = hex(&seed);
    let address = address_from_seed_hex(&seed_hex);
    Ok(Wallet { seed_hex, address })
}

/// Derive the display address from a seed: `qnk` + blake3(domain || seed).
///
/// Deterministic: the same seed always yields the same address, so a funder
/// can be told the address before the agent ever signs anything.
pub fn address_from_seed_hex(seed_hex: &str) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"agentic-money-kit/address/v1");
    h.update(seed_hex.as_bytes());
    let digest = h.finalize();
    format!("qnk{}", hex(digest.as_bytes()))
}

/// The single seam for real signing-key derivation. A fork wires its chain's
/// keypair scheme in HERE (and only here), so the algorithm stays swappable.
///
/// Returns the raw 32-byte seed material; feed it to your `flux-eternal-cypher`
/// keypair generator. Left as a deliberate stub so the template never pretends
/// to sign with a fixed algorithm.
pub fn derive_signing_key(seed_hex: &str) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(b"agentic-money-kit/signing-seed/v1");
    h.update(seed_hex.as_bytes());
    *h.finalize().as_bytes()
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_is_deterministic_and_prefixed() {
        let a = address_from_seed_hex("ab".repeat(32).as_str());
        let b = address_from_seed_hex("ab".repeat(32).as_str());
        assert_eq!(a, b, "same seed → same address");
        assert!(a.starts_with("qnk"));
        assert_eq!(a.len(), 3 + 64);
    }

    #[test]
    fn different_seeds_diverge() {
        let a = address_from_seed_hex(&"00".repeat(32));
        let b = address_from_seed_hex(&"01".repeat(32));
        assert_ne!(a, b);
    }
}
