//! Buzz event model — every message, commit, and agent action is one signed,
//! content-addressed event.
//!
//! Flux-native crypto (deliberately NOT Nostr-wire-compatible):
//!   * event id  = blake3 hex of the canonical bytes
//!   * signature = Ed25519 over the same canonical bytes
//!
//! Canonical bytes = compact JSON of the array
//! `[pubkey, created_at, kind, tags, content]` — chosen because
//! `JSON.stringify` in a browser and `serde_json::to_vec` in Rust produce the
//! same bytes for this shape, so a WebCrypto Ed25519 key in the browser can
//! sign events without any Rust/wasm helper. (Known edge: exotic control
//! characters could in theory escape differently; ordinary chat text is safe.)
//!
//! Because the id is derivable from the canonical bytes, clients that cannot
//! compute blake3 (browsers) may submit an event with an empty `id` and the
//! relay fills it in before verification. The signature is always mandatory.

use anyhow::{anyhow, bail, Context, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Profile / identity announcement (content = display name or JSON profile).
pub const KIND_PROFILE: u32 = 0;
/// Plain chat message.
pub const KIND_CHAT: u32 = 1;
/// Autonomous agent action report (tool call, build result, review verdict…).
pub const KIND_AGENT_ACTION: u32 = 20;
/// flux-rev provenance stamp: tags carry `["rev", <64-hex full id>]` +
/// `["dir", path]`; content is the human-readable line. Lets releases and
/// working trees announce a content-address the whole room can verify.
pub const KIND_PROVENANCE: u32 = 30;
/// Git repository announcement (content = repo name/URL).
pub const KIND_REPO: u32 = 3617;
/// Git commit event (tags carry repo + commit hash, content = subject line).
pub const KIND_COMMIT: u32 = 3618;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BuzzEvent {
    /// blake3 hex of the canonical bytes. May be empty on submission; the
    /// relay computes it. Never empty once stored.
    #[serde(default)]
    pub id: String,
    /// Ed25519 public key, 32 bytes hex. This IS the participant's identity —
    /// human and AI agents are indistinguishable at the protocol level.
    pub pubkey: String,
    /// Unix time in milliseconds.
    pub created_at: u64,
    pub kind: u32,
    /// Free-form tag lists. Convention: `["c", "<channel>"]` scopes an event
    /// to a channel; `["repo", name]` / `["commit", hash]` for git events.
    #[serde(default)]
    pub tags: Vec<Vec<String>>,
    pub content: String,
    /// Ed25519 signature over the canonical bytes, 64 bytes hex.
    pub sig: String,
}

/// The exact byte string that is hashed (id) and signed (sig).
pub fn canonical_bytes(
    pubkey: &str,
    created_at: u64,
    kind: u32,
    tags: &[Vec<String>],
    content: &str,
) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!([pubkey, created_at, kind, tags, content]))
        .expect("canonical json is infallible for these types")
}

impl BuzzEvent {
    pub fn canonical(&self) -> Vec<u8> {
        canonical_bytes(&self.pubkey, self.created_at, self.kind, &self.tags, &self.content)
    }

    pub fn compute_id(&self) -> String {
        blake3::hash(&self.canonical()).to_hex().to_string()
    }

    /// The channel this event is scoped to, if any (`["c", name]` tag).
    pub fn channel(&self) -> Option<&str> {
        self.tags
            .iter()
            .find(|t| t.len() >= 2 && t[0] == "c")
            .map(|t| t[1].as_str())
    }

    /// Full verification: id matches canonical bytes AND signature is valid
    /// for the claimed pubkey. Anything that fails here never enters the log.
    pub fn verify(&self) -> Result<()> {
        let canonical = self.canonical();
        let want_id = blake3::hash(&canonical).to_hex().to_string();
        if self.id != want_id {
            bail!("event id mismatch: got {}, want {}", self.id, want_id);
        }
        let pk: [u8; 32] = hex::decode(&self.pubkey)
            .context("pubkey is not hex")?
            .try_into()
            .map_err(|_| anyhow!("pubkey must be 32 bytes"))?;
        let vk = VerifyingKey::from_bytes(&pk).context("pubkey is not a valid Ed25519 point")?;
        let sig: [u8; 64] = hex::decode(&self.sig)
            .context("sig is not hex")?
            .try_into()
            .map_err(|_| anyhow!("sig must be 64 bytes"))?;
        vk.verify(&canonical, &Signature::from_bytes(&sig))
            .context("signature does not verify")?;
        Ok(())
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// One participant — human or agent — holding an Ed25519 keypair.
pub struct Identity {
    signing: SigningKey,
}

#[derive(Serialize, Deserialize)]
struct IdentityFile {
    sk: String,
    pk: String,
}

impl Identity {
    pub fn generate() -> Self {
        Self { signing: SigningKey::generate(&mut rand::rngs::OsRng) }
    }

    pub fn from_sk_hex(sk_hex: &str) -> Result<Self> {
        let sk: [u8; 32] = hex::decode(sk_hex.trim())
            .context("secret key is not hex")?
            .try_into()
            .map_err(|_| anyhow!("secret key must be 32 bytes"))?;
        Ok(Self { signing: SigningKey::from_bytes(&sk) })
    }

    pub fn pubkey_hex(&self) -> String {
        hex::encode(self.signing.verifying_key().to_bytes())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = IdentityFile {
            sk: hex::encode(self.signing.to_bytes()),
            pk: self.pubkey_hex(),
        };
        std::fs::write(path, serde_json::to_vec_pretty(&file)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let file: IdentityFile = serde_json::from_slice(
            &std::fs::read(path).with_context(|| format!("reading {}", path.display()))?,
        )?;
        Self::from_sk_hex(&file.sk)
    }

    pub fn load_or_generate(path: &Path) -> Result<Self> {
        if path.exists() {
            Self::load(path)
        } else {
            let id = Self::generate();
            id.save(path)?;
            Ok(id)
        }
    }

    /// Build a fully-signed event ready for publishing.
    pub fn sign_event(&self, kind: u32, tags: Vec<Vec<String>>, content: String) -> BuzzEvent {
        let pubkey = self.pubkey_hex();
        let created_at = now_ms();
        let canonical = canonical_bytes(&pubkey, created_at, kind, &tags, &content);
        let sig = hex::encode(self.signing.sign(&canonical).to_bytes());
        let id = blake3::hash(&canonical).to_hex().to_string();
        BuzzEvent { id, pubkey, created_at, kind, tags, content, sig }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_roundtrip() {
        let id = Identity::generate();
        let ev = id.sign_event(
            KIND_CHAT,
            vec![vec!["c".into(), "general".into()]],
            "hello buzz".into(),
        );
        assert_eq!(ev.channel(), Some("general"));
        ev.verify().expect("freshly signed event must verify");
    }

    #[test]
    fn tampered_content_rejected() {
        let id = Identity::generate();
        let mut ev = id.sign_event(KIND_CHAT, vec![], "original".into());
        ev.content = "forged".into();
        assert!(ev.verify().is_err(), "tampered content must fail id check");
        // Even re-computing the id must fail the signature check.
        ev.id = ev.compute_id();
        assert!(ev.verify().is_err(), "tampered content must fail sig check");
    }

    #[test]
    fn wrong_key_rejected() {
        let a = Identity::generate();
        let b = Identity::generate();
        let mut ev = a.sign_event(KIND_CHAT, vec![], "hi".into());
        // Claim b's pubkey on a's signature.
        ev.pubkey = b.pubkey_hex();
        ev.id = ev.compute_id();
        assert!(ev.verify().is_err());
    }
}
