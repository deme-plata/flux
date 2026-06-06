//! Mesh Chat Protocol — Post-quantum encrypted group chat over BLE mesh.
//!
//! Features that beat BitChat:
//!
//! - **PQ E2EE** — every message is signed with SQIsign
//! - **Content-addressed** — messages are identified by BLAKE3 hash
//! - **Rooms** — topic-based channels (auto-join on discovery)
//! - **Replay protection** — monotonic timestamp chain per sender
//! - **Off-grid** — works without internet over BLE mesh
//! - **Federated** — bridge nodes relay to libp2p network

use std::collections::{HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::crypto::{BluetoothCrypto, PQIdentity, SignedPayload};

/// A chat room.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRoom {
    pub name: String,
    pub topic: String,
    pub created_at: u64,
    pub member_count: u32,
    /// Post-quantum verification key for the room (if private)
    pub room_key: Option<[u8; 32]>,
}

/// A single chat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// BLAKE3 hash of this message (content address)
    pub hash: [u8; 32],
    /// Room name
    pub room: String,
    /// Sender's PQ address
    pub sender: [u8; 32],
    /// Sender's human-readable name
    pub sender_name: String,
    /// Message text
    pub text: String,
    /// Unix timestamp (ms)
    pub timestamp: u64,
    /// Monotonic sequence number (per-sender, for replay protection)
    pub seq: u64,
    /// Parent message hash (for threads)
    pub reply_to: Option<[u8; 32]>,
}

/// A signed chat message (wraps ChatMessage with PQ signature).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedMessage {
    pub message: ChatMessage,
    pub signature: SignedPayload,
}

/// The chat protocol engine.
pub struct ChatProtocol {
    /// Message history (per room)
    history: HashMap<String, VecDeque<ChatMessage>>,
    /// Max messages per room
    max_per_room: usize,
    /// Sequence counter (per sender address)
    sequences: HashMap<[u8; 32], u64>,
    /// Known rooms
    rooms: HashMap<String, ChatRoom>,
    /// Broadcast channel for new messages
    message_tx: broadcast::Sender<ChatMessage>,
}

impl ChatProtocol {
    /// Create a new chat protocol engine.
    pub fn new(max_per_room: usize, message_tx: broadcast::Sender<ChatMessage>) -> Self {
        let mut rooms = HashMap::new();
        rooms.insert("flux-general".into(), ChatRoom {
            name: "Flux General".into(),
            topic: "flux-general".into(),
            created_at: now_ms(),
            member_count: 0,
            room_key: None,
        });

        Self {
            history: HashMap::new(),
            max_per_room,
            sequences: HashMap::new(),
            rooms,
            message_tx,
        }
    }

    /// Create a new chat message.
    pub async fn create_message(
        &mut self,
        room: &str,
        text: &str,
        identity: &PQIdentity,
    ) -> Result<ChatMessage> {
        let seq = {
            let entry = self.sequences.entry(identity.address).or_insert(0);
            *entry += 1;
            *entry
        };

        let msg = ChatMessage {
            hash: [0u8; 32], // filled after construction
            room: room.to_string(),
            sender: identity.address,
            sender_name: identity.name.clone(),
            text: text.to_string(),
            timestamp: now_ms(),
            seq,
            reply_to: None,
        };

        // Compute content hash
        let hash = compute_message_hash(&msg);
        let mut msg_with_hash = msg;
        msg_with_hash.hash = hash;

        Ok(msg_with_hash)
    }

    /// Receive a signed message and add to history.
    /// Returns the verified message if valid.
    pub async fn receive_message(
        &mut self,
        signed: &SignedMessage,
    ) -> Result<Option<ChatMessage>> {
        // Verify PQ signature
        BluetoothCrypto::verify(&signed.signature)
            .context("PQ signature verification failed")?;

        let msg = &signed.message;

        // Verify content hash
        let computed = compute_message_hash(msg);
        if computed != msg.hash {
            anyhow::bail!("content hash mismatch");
        }

        // Replay protection: seq must be strictly increasing per sender
        let last_seq = self.sequences.get(&msg.sender).copied().unwrap_or(0);
        if msg.seq <= last_seq {
            anyhow::bail!("replay detected: seq {} <= {}", msg.seq, last_seq);
        }
        self.sequences.insert(msg.sender, msg.seq);

        // Add to history
        let history = self.history.entry(msg.room.clone()).or_insert_with(|| {
            VecDeque::with_capacity(self.max_per_room)
        });
        if history.len() >= self.max_per_room {
            history.pop_front();
        }
        history.push_back(msg.clone());

        // Broadcast to subscribers
        let _ = self.message_tx.send(msg.clone());

        Ok(Some(msg.clone()))
    }

    /// Get message history for a room.
    pub fn room_history(&self, room: &str) -> Vec<&ChatMessage> {
        self.history
            .get(room)
            .map(|h| h.iter().collect())
            .unwrap_or_default()
    }

    /// List known rooms.
    pub fn rooms(&self) -> Vec<&ChatRoom> {
        self.rooms.values().collect()
    }

    /// Join or create a room.
    pub fn join_room(&mut self, name: String, topic: String) {
        self.rooms.entry(topic.clone()).or_insert(ChatRoom {
            name,
            topic,
            created_at: now_ms(),
            member_count: 0,
            room_key: None,
        });
    }
}

/// Compute the BLAKE3 content hash for a chat message.
fn compute_message_hash(msg: &ChatMessage) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(b"flux-bt-chat-v1");
    h.update(msg.room.as_bytes());
    h.update(&msg.sender);
    h.update(msg.text.as_bytes());
    h.update(&msg.timestamp.to_le_bytes());
    h.update(&msg.seq.to_le_bytes());
    if let Some(reply) = &msg.reply_to {
        h.update(reply);
    }
    *h.finalize().as_bytes()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::BluetoothCrypto;

    #[tokio::test]
    async fn test_create_and_verify_message() {
        let crypto = BluetoothCrypto::new(None).unwrap();
        let (tx, _rx) = broadcast::channel(100);
        let mut chat = ChatProtocol::new(100, tx);

        let msg = chat.create_message("flux-general", "hello mesh!", crypto.identity()).await.unwrap();
        assert_ne!(msg.hash, [0u8; 32]);
        assert_eq!(msg.room, "flux-general");
        assert_eq!(msg.text, "hello mesh!");
        assert_eq!(msg.seq, 1);
    }

    #[tokio::test]
    async fn test_replay_protection() {
        let alice = BluetoothCrypto::new(None).unwrap();
        let (tx, _rx) = broadcast::channel(100);
        let mut chat = ChatProtocol::new(100, tx);

        let msg1 = chat.create_message("flux-general", "msg 1", alice.identity()).await.unwrap();
        let msg1_signed = alice.sign_message(&serde_json::to_vec(&msg1).unwrap()).await.unwrap();

        let signed1 = SignedMessage {
            message: msg1,
            signature: msg1_signed,
        };

        // First delivery
        assert!(chat.receive_message(&signed1).await.unwrap().is_some());

        // Second delivery (same seq) should be rejected
        let result = chat.receive_message(&signed1).await;
        assert!(result.is_err() || result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_room_history() {
        let alice = BluetoothCrypto::new(None).unwrap();
        let (tx, _rx) = broadcast::channel(100);
        let mut chat = ChatProtocol::new(100, tx);

        let msg = chat.create_message("flux-general", "hello", alice.identity()).await.unwrap();
        let signed = alice.sign_message(&serde_json::to_vec(&msg).unwrap()).await.unwrap();
        chat.receive_message(&SignedMessage { message: msg, signature: signed }).await.unwrap();

        let history = chat.room_history("flux-general");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].text, "hello");
    }
}
