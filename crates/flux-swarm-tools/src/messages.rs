//! Inter-agent messaging — direct messages, broadcast, inbox semantics.
//!
//! Today every swarm agent learns about another's work by reading the
//! activity log or noticing file-claim notes. That's passive — there's no
//! way to *push* a message at a specific peer. This module adds it.
//!
//! Storage: append-only `/tmp/flux-swarm-messages.jsonl`, one JSON
//! object per line. Append is single-writer per line (kernel guarantees
//! that for files opened `O_APPEND` and writes ≤ PIPE_BUF), so the log
//! survives concurrent writers and never corrupts.
//!
//! Delivery: callers can optionally announce a webhook URL via the
//! existing `flux_webhook_register` infrastructure; when a message is
//! sent to that agent, the swarm fires an HMAC-signed POST to the URL.
//! Recipients without a webhook poll via `inbox()`.

use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// On-disk log path. One JSON object per line.
pub const MESSAGES_LOG: &str = "/tmp/flux-swarm-messages.jsonl";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmMessage {
    /// Monotone u64 id. Allocated on send. Idempotency key for retries.
    pub id: u64,
    /// Sending agent_id.
    pub from: String,
    /// Recipient agent_id. Use `"*"` for broadcast.
    pub to: String,
    /// Unix epoch milliseconds when send() persisted the message.
    pub ts_ms: u64,
    /// Free-form text body (markdown / json / plain — agents agree).
    pub payload: String,
    /// Optional reply-to id chaining messages into a thread.
    pub reply_to: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum MessageError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("invalid agent id (empty)")]
    EmptyAgent,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Allocate a fresh monotone id by counting existing lines + 1.
/// Cheap for log sizes < ~100K; replace with a sidecar counter if it grows.
fn next_id() -> Result<u64, MessageError> {
    if !Path::new(MESSAGES_LOG).exists() {
        return Ok(1);
    }
    let f = std::fs::File::open(MESSAGES_LOG)?;
    let lines = BufReader::new(f).lines().count() as u64;
    Ok(lines + 1)
}

/// Send a message from `from` to `to`. `to == "*"` is broadcast.
/// Returns the persisted [`SwarmMessage`].
pub fn send(
    from: &str,
    to: &str,
    payload: &str,
    reply_to: Option<u64>,
) -> Result<SwarmMessage, MessageError> {
    if from.is_empty() || to.is_empty() {
        return Err(MessageError::EmptyAgent);
    }
    let id = next_id()?;
    let msg = SwarmMessage {
        id,
        from: from.to_string(),
        to: to.to_string(),
        ts_ms: now_ms(),
        payload: payload.to_string(),
        reply_to,
    };
    let line = serde_json::to_string(&msg)?;
    // O_APPEND + line write ≤ PIPE_BUF is atomic on Linux — no lock needed.
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(MESSAGES_LOG)?;
    writeln!(f, "{}", line)?;
    Ok(msg)
}

/// Read messages addressed to `recipient` with `ts_ms > since_ts`.
/// Broadcast messages (to=`"*"`) are included for all recipients.
/// Pass `since_ts = 0` to drain the entire history.
pub fn inbox(recipient: &str, since_ts: u64) -> Result<Vec<SwarmMessage>, MessageError> {
    if recipient.is_empty() {
        return Err(MessageError::EmptyAgent);
    }
    if !Path::new(MESSAGES_LOG).exists() {
        return Ok(Vec::new());
    }
    let f = std::fs::File::open(MESSAGES_LOG)?;
    let mut out = Vec::new();
    for line in BufReader::new(f).lines() {
        let line = line?;
        let m: SwarmMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(_) => continue, // skip corrupt line
        };
        if m.ts_ms <= since_ts {
            continue;
        }
        if m.to == recipient || m.to == "*" {
            out.push(m);
        }
    }
    Ok(out)
}

/// List messages by exact filter — for `flux_swarm_messages_search` style use.
pub fn list_filtered(
    from: Option<&str>,
    to: Option<&str>,
    since_ts: u64,
    limit: usize,
) -> Result<Vec<SwarmMessage>, MessageError> {
    if !Path::new(MESSAGES_LOG).exists() {
        return Ok(Vec::new());
    }
    let f = std::fs::File::open(MESSAGES_LOG)?;
    let mut out = Vec::new();
    for line in BufReader::new(f).lines() {
        let line = line?;
        let m: SwarmMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if m.ts_ms <= since_ts {
            continue;
        }
        if let Some(f) = from {
            if m.from != f {
                continue;
            }
        }
        if let Some(t) = to {
            if m.to != t && m.to != "*" {
                continue;
            }
        }
        out.push(m);
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Tests touch /tmp/flux-swarm-messages.jsonl — serialise to avoid flake.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn clean() {
        let _ = std::fs::remove_file(MESSAGES_LOG);
    }

    #[test]
    fn send_and_inbox_round_trip() {
        let _g = TEST_LOCK.lock().unwrap();
        clean();
        let m = send("rocky", "rocky-sigil", "hello from rocky", None).unwrap();
        assert_eq!(m.from, "rocky");
        assert_eq!(m.to, "rocky-sigil");
        assert_eq!(m.id, 1);
        let inbox = inbox("rocky-sigil", 0).unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].payload, "hello from rocky");
    }

    #[test]
    fn broadcast_visible_to_all() {
        let _g = TEST_LOCK.lock().unwrap();
        clean();
        send("rocky", "*", "broadcast hi", None).unwrap();
        let a = inbox("alice", 0).unwrap();
        let b = inbox("bob", 0).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(a[0].payload, "broadcast hi");
    }

    #[test]
    fn since_filters_old_messages() {
        let _g = TEST_LOCK.lock().unwrap();
        clean();
        send("rocky", "alice", "old", None).unwrap();
        let after_first = now_ms();
        std::thread::sleep(std::time::Duration::from_millis(2));
        send("rocky", "alice", "new", None).unwrap();
        let new = inbox("alice", after_first).unwrap();
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].payload, "new");
    }

    #[test]
    fn reply_chain_preserved() {
        let _g = TEST_LOCK.lock().unwrap();
        clean();
        let m1 = send("rocky", "alice", "Q?", None).unwrap();
        let m2 = send("alice", "rocky", "A!", Some(m1.id)).unwrap();
        assert_eq!(m2.reply_to, Some(m1.id));
        assert_eq!(m2.id, 2);
    }

    #[test]
    fn empty_recipient_rejected() {
        let _g = TEST_LOCK.lock().unwrap();
        assert!(matches!(
            send("rocky", "", "x", None),
            Err(MessageError::EmptyAgent)
        ));
        assert!(matches!(inbox("", 0), Err(MessageError::EmptyAgent)));
    }
}
