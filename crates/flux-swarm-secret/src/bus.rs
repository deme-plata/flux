//! Push transport for sealed swarm messages.
//!
//! The legacy bus (`flux-swarm-tools`) appends plaintext lines to a `/tmp` file
//! and recipients **poll**. This module rides the real **push** transport —
//! flux-p2p gossipsub — and carries only [`SealedEnvelope`]s.
//!
//! Delivery model: everyone subscribes to one topic ([`SECRET_TOPIC`]); a frame
//! names its recipient, but security does NOT rest on that field — gossipsub is
//! a broadcast, and only the holder of the matching X25519 secret key can `open`
//! the envelope. So a single published frame is readable by exactly one agent.
//! ([`SecretFrame::to`] is a fast-skip hint, not an access control.)
//!
//! `flux_p2p` is an OPTIONAL dependency behind the `p2p` feature, so the crypto
//! core stays libp2p-free and fast to build/test. With `--features p2p`,
//! [`flux_p2p::NetworkManager`] implements [`PushTransport`] directly and
//! [`recv_sealed`] spawns a reader that hands back decrypted-for-me messages.

use crate::{open, seal, PublicKey, SealedEnvelope, SecretIdentity};
use serde::{Deserialize, Serialize};

/// The gossipsub topic all secret frames ride. One topic; per-frame addressing
/// is by recipient public key, enforced cryptographically by `open`.
pub const SECRET_TOPIC: &str = "/flux/swarm/secret/v1";

/// A routable, encrypted swarm frame. The body ([`env`](Self::env)) is opaque to
/// every node except the recipient. `from`/`to`/`msg_id`/`ts_ms` are cleartext
/// routing metadata — `from` is NOT authenticated yet (see crate-level honest
/// scope: sender-auth is the Ed25519 companion slice).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecretFrame {
    pub v: u8,
    /// Sender's swarm id/label (advisory; unauthenticated until the Ed25519 slice).
    pub from: String,
    /// Recipient X25519 public key (hex). Fast-skip hint; not access control.
    pub to: String,
    pub msg_id: u64,
    pub ts_ms: u64,
    pub env: SealedEnvelope,
}

impl SecretFrame {
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("SecretFrame serializes")
    }
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        serde_json::from_slice(b).ok()
    }
}

/// Build a sealed frame addressed to `recipient_pk`.
pub fn seal_frame(
    from: &str,
    recipient_pk: &PublicKey,
    plaintext: &[u8],
    msg_id: u64,
    ts_ms: u64,
) -> SecretFrame {
    SecretFrame {
        v: 1,
        from: from.to_string(),
        to: hex::encode(recipient_pk.to_bytes()),
        msg_id,
        ts_ms,
        env: seal(plaintext, recipient_pk),
    }
}

/// Try to open a frame as `id`. Returns the plaintext iff the frame is addressed
/// to this identity AND the envelope opens (so a non-recipient — or a tampered
/// frame — yields `None`, never a panic).
pub fn try_open_frame(frame: &SecretFrame, id: &SecretIdentity) -> Option<Vec<u8>> {
    if frame.to != id.public_hex() {
        return None;
    }
    open(&frame.env, id).ok()
}

/// Any push channel that can broadcast bytes on a topic. `flux_p2p::NetworkManager`
/// satisfies this directly under `--features p2p`.
pub trait PushTransport {
    fn publish(&self, topic: &str, data: Vec<u8>) -> Result<(), String>;
}

/// Seal-and-push: publish a frame onto `topic` via any [`PushTransport`].
pub fn publish_sealed<T: PushTransport>(
    transport: &T,
    topic: &str,
    frame: &SecretFrame,
) -> Result<(), String> {
    transport.publish(topic, frame.to_bytes())
}

/// In-memory transport for tests and local fan-out (records every publish).
#[derive(Default)]
pub struct LoopbackTransport {
    pub sent: std::sync::Mutex<Vec<(String, Vec<u8>)>>,
}

impl PushTransport for LoopbackTransport {
    fn publish(&self, topic: &str, data: Vec<u8>) -> Result<(), String> {
        self.sent
            .lock()
            .map_err(|_| "loopback lock poisoned".to_string())?
            .push((topic.to_string(), data));
        Ok(())
    }
}

// ── Real flux-p2p gossipsub binding (optional, behind `p2p` feature) ──────────

#[cfg(feature = "p2p")]
impl PushTransport for flux_p2p::NetworkManager {
    fn publish(&self, topic: &str, data: Vec<u8>) -> Result<(), String> {
        flux_p2p::NetworkManager::publish(self, topic, data)
    }
}

/// Subscribe to [`SECRET_TOPIC`] over flux-p2p and receive only the messages that
/// decrypt for `id`. Spawns a reader thread; the returned channel yields
/// `(frame, plaintext)` for each frame addressed to and openable by this identity.
#[cfg(feature = "p2p")]
pub fn recv_sealed(
    net: &flux_p2p::NetworkManager,
    id: &SecretIdentity,
) -> std::sync::mpsc::Receiver<(SecretFrame, Vec<u8>)> {
    let raw = net.subscribe_blocking(SECRET_TOPIC);
    let (tx, rx) = std::sync::mpsc::channel();
    // Move a clone of the identity into the reader thread.
    let me = SecretIdentity::from_sk_bytes(id.to_sk_bytes());
    std::thread::spawn(move || {
        for (_topic, bytes) in raw {
            if let Some(frame) = SecretFrame::from_bytes(&bytes) {
                if let Some(pt) = try_open_frame(&frame, &me) {
                    if tx.send((frame, pt)).is_err() {
                        break; // consumer dropped
                    }
                }
            }
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SecretIdentity;

    #[test]
    fn frame_rides_loopback_and_only_recipient_opens() {
        let bob = SecretIdentity::generate();
        let carol = SecretIdentity::generate();
        let transport = LoopbackTransport::default();

        let frame = seal_frame("rocky", &bob.public_key(), b"route via pool 955c", 42, 1_781_700_000_000);
        publish_sealed(&transport, SECRET_TOPIC, &frame).unwrap();

        // pull what hit the wire
        let (topic, bytes) = transport.sent.lock().unwrap()[0].clone();
        assert_eq!(topic, SECRET_TOPIC);
        let onwire = SecretFrame::from_bytes(&bytes).unwrap();

        // only bob opens it; carol gets None
        assert_eq!(try_open_frame(&onwire, &bob).unwrap(), b"route via pool 955c");
        assert!(try_open_frame(&onwire, &carol).is_none());
    }

    #[test]
    fn tampered_frame_yields_none_not_panic() {
        let bob = SecretIdentity::generate();
        let mut frame = seal_frame("rocky", &bob.public_key(), b"x", 1, 1);
        let mut ct = hex::decode(&frame.env.ct).unwrap();
        ct[0] ^= 0x01;
        frame.env.ct = hex::encode(ct);
        assert!(try_open_frame(&frame, &bob).is_none());
    }

    #[test]
    fn frame_bytes_roundtrip() {
        let bob = SecretIdentity::generate();
        let frame = seal_frame("rocky", &bob.public_key(), b"hello bus", 7, 9);
        let back = SecretFrame::from_bytes(&frame.to_bytes()).unwrap();
        assert_eq!(frame, back);
        assert_eq!(back.from, "rocky");
        assert_eq!(back.to, bob.public_hex());
    }
}
