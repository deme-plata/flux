// QRBT1 — the wallet-signed robot-command envelope.
//
// Canonical signed bytes (fixed layout, no JSON in the signature input so
// key-order/whitespace can never desync signer and verifier):
//
//   payload = b"QRBT1"
//           ‖ wallet(32)              — the operator's Quillon address bytes
//           ‖ nonce(8, LE)           — strictly increasing per wallet
//           ‖ timestamp(8, LE)       — unix seconds
//           ‖ blake3(command_json)(32)
//
//   sig = Ed25519_sign(wallet_secret, payload)
//
// The command itself travels as JSON beside the signature; the signature
// covers its BLAKE3 hash, so any byte of tampering breaks verification. The
// verifying key is the wallet address itself (client-managed Quillon wallets:
// address = Ed25519 pubkey — the same property q-narwhalknight's
// wallet_auth.rs uses for X-Wallet-Auth). PQ-scheme wallets (Dilithium5 etc.,
// where address = HASH(pubkey)) canNOT be verified this way; the daemon
// rejects them rather than guessing.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use std::collections::HashMap;

pub const MAGIC: &[u8; 5] = b"QRBT1";

/// Commands older (or more in the future) than this many seconds are rejected
/// even with a valid signature — bounds how long a captured envelope stays
/// dangerous if the nonce store is ever lost.
pub const FRESHNESS_WINDOW_SECS: u64 = 300;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct SignedCommand {
    /// `qnk`-prefixed 64-hex Quillon address (= Ed25519 pubkey).
    pub wallet: String,
    pub nonce: u64,
    /// Unix seconds at signing time.
    pub timestamp: u64,
    /// The command body, e.g. {"kind":"task","instruction":"pick up the cup"}.
    pub command: serde_json::Value,
    /// 128-hex Ed25519 signature over `canonical_bytes`.
    pub sig: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EnvelopeError {
    BadWallet(String),
    BadSignatureEncoding,
    SignatureInvalid,
    /// Nonce ≤ the last accepted nonce for this wallet.
    Replayed { last_accepted: u64 },
    Stale { skew_secs: u64 },
}

impl std::fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnvelopeError::BadWallet(w) => write!(f, "wallet is not a 32-byte Ed25519 address: {w}"),
            EnvelopeError::BadSignatureEncoding => write!(f, "signature is not 64 bytes of hex"),
            EnvelopeError::SignatureInvalid => write!(f, "Ed25519 signature does not verify"),
            EnvelopeError::Replayed { last_accepted } => {
                write!(f, "nonce replay: last accepted nonce is {last_accepted}")
            }
            EnvelopeError::Stale { skew_secs } => {
                write!(f, "timestamp outside ±{FRESHNESS_WINDOW_SECS}s window (skew {skew_secs}s)")
            }
        }
    }
}

/// Decode a `qnk…`/bare-hex wallet into its 32 address bytes.
pub fn wallet_bytes(wallet: &str) -> Result<[u8; 32], EnvelopeError> {
    let h = wallet.strip_prefix("qnk").unwrap_or(wallet);
    let v = hex::decode(h).map_err(|_| EnvelopeError::BadWallet(wallet.into()))?;
    v.try_into().map_err(|_| EnvelopeError::BadWallet(wallet.into()))
}

/// The exact bytes the signature covers. Kept public so any client (the
/// TypeScript MCP tool included) can reproduce them byte-for-byte.
pub fn canonical_bytes(
    wallet: &[u8; 32],
    nonce: u64,
    timestamp: u64,
    command_json: &str,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + 32 + 8 + 8 + 32);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(wallet);
    out.extend_from_slice(&nonce.to_le_bytes());
    out.extend_from_slice(&timestamp.to_le_bytes());
    out.extend_from_slice(blake3::hash(command_json.as_bytes()).as_bytes());
    out
}

/// Sign a command with a raw Ed25519 secret. The wallet address is derived
/// from the key (address = pubkey), never passed in — so a signer can't
/// accidentally claim someone else's wallet.
pub fn sign_command(
    secret: &SigningKey,
    nonce: u64,
    timestamp: u64,
    command: &serde_json::Value,
) -> SignedCommand {
    let pubkey = secret.verifying_key().to_bytes();
    let command_json = command.to_string();
    let payload = canonical_bytes(&pubkey, nonce, timestamp, &command_json);
    let sig = secret.sign(&payload);
    SignedCommand {
        wallet: format!("qnk{}", hex::encode(pubkey)),
        nonce,
        timestamp,
        command: command.clone(),
        sig: hex::encode(sig.to_bytes()),
    }
}

/// Per-wallet strictly-increasing nonce store. In-memory: restarting the
/// daemon forgets nonces, which is why the freshness window exists as the
/// second, time-based line of defence.
#[derive(Default)]
pub struct ReplayGuard {
    last: HashMap<[u8; 32], u64>,
}

impl ReplayGuard {
    /// Verify signature + freshness + nonce; on success, burn the nonce.
    ///
    /// Order matters: the signature is checked FIRST so an attacker can't use
    /// nonce/timestamp error messages as an oracle while forging.
    pub fn verify_and_burn(
        &mut self,
        cmd: &SignedCommand,
        now: u64,
    ) -> Result<(), EnvelopeError> {
        let wallet = wallet_bytes(&cmd.wallet)?;
        let vk = VerifyingKey::from_bytes(&wallet)
            .map_err(|_| EnvelopeError::BadWallet(cmd.wallet.clone()))?;
        let sig_bytes: [u8; 64] = hex::decode(&cmd.sig)
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or(EnvelopeError::BadSignatureEncoding)?;
        let payload =
            canonical_bytes(&wallet, cmd.nonce, cmd.timestamp, &cmd.command.to_string());
        vk.verify(&payload, &Signature::from_bytes(&sig_bytes))
            .map_err(|_| EnvelopeError::SignatureInvalid)?;

        let skew = now.abs_diff(cmd.timestamp);
        if skew > FRESHNESS_WINDOW_SECS {
            return Err(EnvelopeError::Stale { skew_secs: skew });
        }

        let last = self.last.entry(wallet).or_insert(0);
        if cmd.nonce <= *last {
            return Err(EnvelopeError::Replayed { last_accepted: *last });
        }
        *last = cmd.nonce;
        Ok(())
    }
}

/// One-shot verification without a nonce store (used by tests and by any
/// stateless auditor replaying a receipts log).
pub fn verify_envelope(cmd: &SignedCommand) -> Result<(), EnvelopeError> {
    let wallet = wallet_bytes(&cmd.wallet)?;
    let vk = VerifyingKey::from_bytes(&wallet)
        .map_err(|_| EnvelopeError::BadWallet(cmd.wallet.clone()))?;
    let sig_bytes: [u8; 64] = hex::decode(&cmd.sig)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or(EnvelopeError::BadSignatureEncoding)?;
    let payload = canonical_bytes(&wallet, cmd.nonce, cmd.timestamp, &cmd.command.to_string());
    vk.verify(&payload, &Signature::from_bytes(&sig_bytes))
        .map_err(|_| EnvelopeError::SignatureInvalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SigningKey {
        // Deterministic test key — NOT a real wallet.
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn cmd() -> serde_json::Value {
        serde_json::json!({"kind": "task", "instruction": "wave at Viktor"})
    }

    #[test]
    fn sign_verify_roundtrip() {
        let sc = sign_command(&key(), 1, 1_000, &cmd());
        assert!(sc.wallet.starts_with("qnk"));
        assert_eq!(verify_envelope(&sc), Ok(()));
    }

    #[test]
    fn tampered_command_fails() {
        let mut sc = sign_command(&key(), 1, 1_000, &cmd());
        sc.command = serde_json::json!({"kind": "task", "instruction": "walk into traffic"});
        assert_eq!(verify_envelope(&sc), Err(EnvelopeError::SignatureInvalid));
    }

    #[test]
    fn tampered_nonce_and_timestamp_fail() {
        let mut a = sign_command(&key(), 1, 1_000, &cmd());
        a.nonce = 2;
        assert_eq!(verify_envelope(&a), Err(EnvelopeError::SignatureInvalid));
        let mut b = sign_command(&key(), 1, 1_000, &cmd());
        b.timestamp = 1_001;
        assert_eq!(verify_envelope(&b), Err(EnvelopeError::SignatureInvalid));
    }

    #[test]
    fn wallet_swap_fails() {
        // A valid envelope re-attributed to a different wallet must not verify:
        // the signature covers the wallet bytes.
        let other = SigningKey::from_bytes(&[9u8; 32]);
        let mut sc = sign_command(&key(), 1, 1_000, &cmd());
        sc.wallet = format!("qnk{}", hex::encode(other.verifying_key().to_bytes()));
        assert_eq!(verify_envelope(&sc), Err(EnvelopeError::SignatureInvalid));
    }

    #[test]
    fn replay_guard_burns_nonces() {
        let mut g = ReplayGuard::default();
        let now = 1_000;
        let sc1 = sign_command(&key(), 1, now, &cmd());
        assert_eq!(g.verify_and_burn(&sc1, now), Ok(()));
        // Exact replay: rejected.
        assert_eq!(
            g.verify_and_burn(&sc1, now),
            Err(EnvelopeError::Replayed { last_accepted: 1 })
        );
        // Lower nonce, freshly signed: still rejected (strictly increasing).
        let sc0 = sign_command(&key(), 1, now, &cmd());
        assert!(matches!(
            g.verify_and_burn(&sc0, now),
            Err(EnvelopeError::Replayed { .. })
        ));
        // Next nonce: accepted.
        let sc2 = sign_command(&key(), 2, now, &cmd());
        assert_eq!(g.verify_and_burn(&sc2, now), Ok(()));
    }

    #[test]
    fn stale_timestamp_rejected_even_with_valid_sig() {
        let mut g = ReplayGuard::default();
        let sc = sign_command(&key(), 1, 1_000, &cmd());
        let now = 1_000 + FRESHNESS_WINDOW_SECS + 1;
        assert!(matches!(g.verify_and_burn(&sc, now), Err(EnvelopeError::Stale { .. })));
    }

    #[test]
    fn nonces_are_per_wallet() {
        let mut g = ReplayGuard::default();
        let a = key();
        let b = SigningKey::from_bytes(&[9u8; 32]);
        assert_eq!(g.verify_and_burn(&sign_command(&a, 5, 1_000, &cmd()), 1_000), Ok(()));
        // Wallet B using nonce 1 is fine — the guard is per-wallet.
        assert_eq!(g.verify_and_burn(&sign_command(&b, 1, 1_000, &cmd()), 1_000), Ok(()));
    }

    #[test]
    fn canonical_bytes_layout_is_pinned() {
        // Pin the exact layout so a refactor can't silently desync the
        // TypeScript signer: 5 magic + 32 wallet + 8 nonce + 8 ts + 32 hash.
        let w = [1u8; 32];
        let b = canonical_bytes(&w, 0x0102030405060708, 0x1112131415161718, "{}");
        assert_eq!(b.len(), 85);
        assert_eq!(&b[..5], b"QRBT1");
        assert_eq!(&b[5..37], &w);
        assert_eq!(&b[37..45], &0x0102030405060708u64.to_le_bytes());
        assert_eq!(&b[45..53], &0x1112131415161718u64.to_le_bytes());
        assert_eq!(&b[53..85], blake3::hash(b"{}").as_bytes());
    }
}
