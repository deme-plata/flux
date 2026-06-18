//! Sender authentication — signed **inside** the sealed envelope.
//!
//! The base [`seal`](crate::seal) is anonymous: anyone with the recipient's
//! public key can produce a valid envelope, so a decrypted message proves
//! *nothing* about who sent it. On a money/coordination bus that is dangerous
//! (a forged `from: rocky → route QUG to X` would open cleanly).
//!
//! This module fixes it the privacy-preserving way. The sender signs
//! `DOMAIN ‖ recipient_pk ‖ len(aad) ‖ aad ‖ plaintext` with Ed25519 and places
//! `sender_vk ‖ signature ‖ plaintext` *inside* the sealed envelope, then seals
//! the whole thing to the recipient. Therefore:
//!
//!   * the **recipient** decrypts, then cryptographically verifies the sender —
//!     authorship + integrity + non-repudiation to the recipient;
//!   * a **passive observer** sees only ciphertext: the signature and the
//!     sender's identity are confidential, so this does NOT deanonymize who is
//!     talking to whom (unlike a cleartext signature on the frame);
//!   * binding `recipient_pk` into the signed message stops a captured inner
//!     payload from being replayed *to a different recipient*;
//!   * the **AAD** lets the frame's routing metadata (`msg_id`, `ts_ms`) be
//!     authenticated too, so a man-in-the-middle cannot rewrite the timestamp to
//!     dodge the [`ReplayGuard`](crate::replay::ReplayGuard) — see
//!     [`seal_authed_frame`] / [`try_open_authed_frame`].

use crate::bus::SecretFrame;
use crate::{open, seal, PublicKey, SealedEnvelope, SecretError, SecretIdentity};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;

const AUTH_DOMAIN: &str = "flux-swarm-secret v1 authed-msg";

/// A sender's Ed25519 signing identity (separate from the X25519 encryption
/// identity so the two key roles never alias).
pub struct SigningIdentity {
    sk: SigningKey,
    vk: VerifyingKey,
}

impl SigningIdentity {
    pub fn generate() -> Self {
        let sk = SigningKey::generate(&mut OsRng);
        let vk = sk.verifying_key();
        Self { sk, vk }
    }

    pub fn from_sk_bytes(b: [u8; 32]) -> Self {
        let sk = SigningKey::from_bytes(&b);
        let vk = sk.verifying_key();
        Self { sk, vk }
    }

    /// The sender's verifying key bytes — what a recipient learns as the author.
    pub fn verifying_key_bytes(&self) -> [u8; 32] {
        self.vk.to_bytes()
    }

    pub fn public_hex(&self) -> String {
        hex::encode(self.vk.to_bytes())
    }

    pub fn to_sk_bytes(&self) -> [u8; 32] {
        self.sk.to_bytes()
    }
}

/// The exact bytes that get signed. AAD is length-prefixed so the aad/plaintext
/// boundary is unambiguous (no splicing attacks).
fn signing_msg(recipient_pk: &PublicKey, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(AUTH_DOMAIN.len() + 32 + 8 + aad.len() + plaintext.len());
    m.extend_from_slice(AUTH_DOMAIN.as_bytes());
    m.extend_from_slice(&recipient_pk.to_bytes());
    m.extend_from_slice(&(aad.len() as u64).to_le_bytes());
    m.extend_from_slice(aad);
    m.extend_from_slice(plaintext);
    m
}

/// Frame metadata bound into the signature: `msg_id ‖ ts_ms` (LE).
fn frame_aad(msg_id: u64, ts_ms: u64) -> Vec<u8> {
    let mut a = Vec::with_capacity(16);
    a.extend_from_slice(&msg_id.to_le_bytes());
    a.extend_from_slice(&ts_ms.to_le_bytes());
    a
}

/// Result of opening an authenticated envelope: the verified sender plus body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedAuthed {
    /// Sender's Ed25519 verifying key (confidential — only the recipient sees it).
    pub sender: [u8; 32],
    pub plaintext: Vec<u8>,
}

impl OpenedAuthed {
    pub fn sender_hex(&self) -> String {
        hex::encode(self.sender)
    }
}

/// Sign `plaintext` (binding `aad`) as `sender`, bind it to `recipient_pk`, seal it.
pub fn seal_authed_with_aad(
    sender: &SigningIdentity,
    recipient_pk: &PublicKey,
    aad: &[u8],
    plaintext: &[u8],
) -> SealedEnvelope {
    let sig = sender.sk.sign(&signing_msg(recipient_pk, aad, plaintext));
    let mut inner = Vec::with_capacity(32 + 64 + plaintext.len());
    inner.extend_from_slice(&sender.vk.to_bytes());
    inner.extend_from_slice(&sig.to_bytes());
    inner.extend_from_slice(plaintext);
    seal(&inner, recipient_pk)
}

/// Open + authenticate, verifying the signature over the same `aad` the sender used.
pub fn open_authed_with_aad(
    env: &SealedEnvelope,
    id: &SecretIdentity,
    aad: &[u8],
) -> Result<OpenedAuthed, SecretError> {
    let inner = open(env, id)?;
    if inner.len() < 96 {
        return Err(SecretError::Malformed);
    }
    let vk_bytes: [u8; 32] = inner[..32].try_into().expect("32-byte slice");
    let sig_bytes: [u8; 64] = inner[32..96].try_into().expect("64-byte slice");
    let pt = inner[96..].to_vec();

    let vk = VerifyingKey::from_bytes(&vk_bytes).map_err(|_| SecretError::AuthFailed)?;
    let sig = Signature::from_bytes(&sig_bytes);
    // The recipient verifies against ITS OWN public key (what the sender bound).
    vk.verify(&signing_msg(&id.public_key(), aad, &pt), &sig)
        .map_err(|_| SecretError::AuthFailed)?;

    Ok(OpenedAuthed { sender: vk_bytes, plaintext: pt })
}

/// Sign + seal with no associated data (standalone authenticated message).
pub fn seal_authed(
    sender: &SigningIdentity,
    recipient_pk: &PublicKey,
    plaintext: &[u8],
) -> SealedEnvelope {
    seal_authed_with_aad(sender, recipient_pk, &[], plaintext)
}

/// Open + authenticate a standalone (no-aad) authenticated message.
pub fn open_authed(env: &SealedEnvelope, id: &SecretIdentity) -> Result<OpenedAuthed, SecretError> {
    open_authed_with_aad(env, id, &[])
}

// ── Push-bus integration ──────────────────────────────────────────────────────

/// Build a wire-ready, authenticated [`SecretFrame`] whose signature also covers
/// `msg_id`+`ts_ms` — so those fields cannot be rewritten in transit (replay/freshness).
pub fn seal_authed_frame(
    sender: &SigningIdentity,
    from_label: &str,
    recipient_pk: &PublicKey,
    plaintext: &[u8],
    msg_id: u64,
    ts_ms: u64,
) -> SecretFrame {
    SecretFrame {
        v: 1,
        from: from_label.to_string(),
        to: hex::encode(recipient_pk.to_bytes()),
        msg_id,
        ts_ms,
        env: seal_authed_with_aad(sender, recipient_pk, &frame_aad(msg_id, ts_ms), plaintext),
    }
}

/// Open + authenticate a received frame, verifying that `msg_id`/`ts_ms` are the
/// ones the sender signed. `None` if not for us, not openable, the signature
/// fails, or the routing metadata was tampered — never a panic, never an
/// unauthenticated body.
pub fn try_open_authed_frame(frame: &SecretFrame, id: &SecretIdentity) -> Option<OpenedAuthed> {
    if frame.to != id.public_hex() {
        return None;
    }
    open_authed_with_aad(&frame.env, id, &frame_aad(frame.msg_id, frame.ts_ms)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SecretIdentity;

    #[test]
    fn authed_roundtrip_recovers_verified_sender() {
        let alice = SigningIdentity::generate();
        let bob = SecretIdentity::generate();
        let env = seal_authed(&alice, &bob.public_key(), b"route QUG to pool 955c");
        let opened = open_authed(&env, &bob).unwrap();
        assert_eq!(opened.plaintext, b"route QUG to pool 955c");
        assert_eq!(opened.sender, alice.verifying_key_bytes());
    }

    #[test]
    fn forged_sender_identity_is_rejected() {
        let alice = SigningIdentity::generate();
        let carol = SigningIdentity::generate();
        let bob = SecretIdentity::generate();

        let sig = alice.sk.sign(&signing_msg(&bob.public_key(), &[], b"spoof"));
        let mut inner = Vec::new();
        inner.extend_from_slice(&carol.vk.to_bytes()); // lie about the author
        inner.extend_from_slice(&sig.to_bytes());
        inner.extend_from_slice(b"spoof");
        let env = seal(&inner, &bob.public_key());

        assert!(matches!(open_authed(&env, &bob), Err(SecretError::AuthFailed)));
    }

    #[test]
    fn signature_bound_to_recipient() {
        let alice = SigningIdentity::generate();
        let bob = SecretIdentity::generate();
        let carol = SecretIdentity::generate();

        let sig = alice.sk.sign(&signing_msg(&carol.public_key(), &[], b"misbound"));
        let mut inner = Vec::new();
        inner.extend_from_slice(&alice.vk.to_bytes());
        inner.extend_from_slice(&sig.to_bytes());
        inner.extend_from_slice(b"misbound");
        let env = seal(&inner, &bob.public_key());

        assert!(matches!(open_authed(&env, &bob), Err(SecretError::AuthFailed)));
    }

    #[test]
    fn tampered_ciphertext_fails_at_aead_before_auth() {
        let alice = SigningIdentity::generate();
        let bob = SecretIdentity::generate();
        let mut env = seal_authed(&alice, &bob.public_key(), b"x");
        let mut ct = hex::decode(&env.ct).unwrap();
        ct[0] ^= 0x01;
        env.ct = hex::encode(ct);
        assert!(matches!(open_authed(&env, &bob), Err(SecretError::Open)));
    }

    #[test]
    fn wrong_recipient_cannot_open() {
        let alice = SigningIdentity::generate();
        let bob = SecretIdentity::generate();
        let mallory = SecretIdentity::generate();
        let env = seal_authed(&alice, &bob.public_key(), b"for bob");
        assert!(matches!(open_authed(&env, &mallory), Err(SecretError::Open)));
    }

    #[test]
    fn authed_frame_over_bus_roundtrips() {
        let alice = SigningIdentity::generate();
        let bob = SecretIdentity::generate();
        let frame = seal_authed_frame(&alice, "rocky", &bob.public_key(), b"hi bob", 9, 123);
        let opened = try_open_authed_frame(&frame, &bob).unwrap();
        assert_eq!(opened.plaintext, b"hi bob");
        assert_eq!(opened.sender, alice.verifying_key_bytes());
        let eve = SecretIdentity::generate();
        assert!(try_open_authed_frame(&frame, &eve).is_none());
    }

    #[test]
    fn rewriting_frame_metadata_breaks_auth() {
        // msg_id/ts_ms are bound into the signature → tampering them => AuthFailed.
        let alice = SigningIdentity::generate();
        let bob = SecretIdentity::generate();
        let mut frame = seal_authed_frame(&alice, "rocky", &bob.public_key(), b"pay", 100, 1_000);
        frame.ts_ms = 9_999; // MITM tries to refresh the timestamp
        assert!(try_open_authed_frame(&frame, &bob).is_none());
        let mut frame2 = seal_authed_frame(&alice, "rocky", &bob.public_key(), b"pay", 100, 1_000);
        frame2.msg_id = 101; // MITM tries to change the sequence id
        assert!(try_open_authed_frame(&frame2, &bob).is_none());
    }

    #[test]
    fn signing_identity_persists_across_sk_bytes() {
        let id = SigningIdentity::generate();
        let restored = SigningIdentity::from_sk_bytes(id.to_sk_bytes());
        assert_eq!(id.public_hex(), restored.public_hex());
    }
}
