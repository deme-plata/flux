//! Chunking for MTU-limited GATT writes and notifications.
//!
//! ```text
//!  0      1        2..3       4..5      6..7     8..
//! ┌──────┬────────┬──────────┬─────────┬────────┬──────────────┐
//! │ 0x5B │ ver=1  │ msg_id   │ seq     │ total  │ payload      │   all u16 little-endian
//! └──────┴────────┴──────────┴─────────┴────────┴──────────────┘
//! ```
//!
//! `msg_id` is chosen by the sender per message (random); `seq` runs `0..total`. A
//! receiver keeps one partial message per `msg_id`, in order, and hands the payload back
//! when `seq == total − 1` arrives. Chunks of an unknown or already-complete message are
//! dropped, as is anything with a bad magic or version. Out-of-order delivery does not
//! happen on a single GATT characteristic, so it is refused rather than buffered.

use std::collections::HashMap;

/// First byte of every chunk.
pub const MAGIC: u8 = 0x5B;
/// Header length in bytes.
pub const HEADER_LEN: usize = 8;
/// The ATT protocol's own overhead on a write/notify PDU.
pub const ATT_OVERHEAD: usize = 3;
/// Smallest MTU BLE guarantees. The floor every implementation must work at.
pub const MIN_MTU: usize = 23;
/// Largest MTU an ATT link can negotiate.
pub const MAX_MTU: usize = 517;

/// Payload bytes one chunk can carry at a given ATT MTU.
pub fn chunk_payload(mtu: usize) -> usize {
    let mtu = mtu.clamp(MIN_MTU, MAX_MTU);
    mtu - ATT_OVERHEAD - HEADER_LEN
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("message too large: {0} bytes would need more than 65535 chunks at this MTU")]
    TooLarge(usize),
    #[error("empty message")]
    Empty,
}

/// Cut `payload` into ready-to-write chunks for an ATT MTU of `mtu`.
pub fn encode(msg_id: u16, payload: &[u8], mtu: usize) -> Result<Vec<Vec<u8>>, FrameError> {
    if payload.is_empty() {
        return Err(FrameError::Empty);
    }
    let per = chunk_payload(mtu);
    let total = payload.len().div_ceil(per);
    if total > u16::MAX as usize {
        return Err(FrameError::TooLarge(payload.len()));
    }
    let total16 = total as u16;
    let mut out = Vec::with_capacity(total);
    for (seq, part) in payload.chunks(per).enumerate() {
        let mut c = Vec::with_capacity(HEADER_LEN + part.len());
        c.push(MAGIC);
        c.push(super::PROTOCOL_VERSION);
        c.extend_from_slice(&msg_id.to_le_bytes());
        c.extend_from_slice(&(seq as u16).to_le_bytes());
        c.extend_from_slice(&total16.to_le_bytes());
        c.extend_from_slice(part);
        out.push(c);
    }
    Ok(out)
}

/// A parsed chunk header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub msg_id: u16,
    pub seq: u16,
    pub total: u16,
}

/// Parse a chunk's header. `None` for a malformed chunk.
pub fn parse_header(chunk: &[u8]) -> Option<Header> {
    if chunk.len() < HEADER_LEN || chunk[0] != MAGIC || chunk[1] != super::PROTOCOL_VERSION {
        return None;
    }
    let u = |i: usize| u16::from_le_bytes([chunk[i], chunk[i + 1]]);
    let h = Header { msg_id: u(2), seq: u(4), total: u(6) };
    if h.total == 0 || h.seq >= h.total {
        return None;
    }
    Some(h)
}

/// Why a chunk was dropped, for logging. Reassembly never fails loudly — a dropped
/// message simply never completes, and the sender's ack timeout is the signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drop {
    BadHeader,
    /// `seq` was not the next expected one for this `msg_id`.
    OutOfOrder,
    /// `total` disagreed with earlier chunks of the same `msg_id`.
    TotalChanged,
    /// The reassembled message would exceed the configured cap.
    TooLarge,
}

/// Stateful reassembler — one per link direction.
#[derive(Debug, Default)]
pub struct Reassembler {
    partial: HashMap<u16, Partial>,
    /// Refuse messages above this many bytes. 0 = no cap.
    pub max_len: usize,
}

#[derive(Debug)]
struct Partial {
    total: u16,
    next_seq: u16,
    buf: Vec<u8>,
}

impl Reassembler {
    /// A reassembler that refuses anything above `max_len` bytes (0 = unlimited).
    pub fn with_cap(max_len: usize) -> Self {
        Self { partial: HashMap::new(), max_len }
    }

    /// Feed one chunk. `Ok(Some(payload))` when a message completes.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Option<Vec<u8>>, Drop> {
        let h = parse_header(chunk).ok_or(Drop::BadHeader)?;
        let body = &chunk[HEADER_LEN..];
        let p = self.partial.entry(h.msg_id).or_insert_with(|| Partial {
            total: h.total,
            next_seq: 0,
            buf: Vec::new(),
        });
        if p.total != h.total {
            self.partial.remove(&h.msg_id);
            return Err(Drop::TotalChanged);
        }
        if p.next_seq != h.seq {
            // A fresh seq 0 for a message we were mid-way through means the sender
            // restarted it; start over rather than wedge on the stale partial.
            if h.seq == 0 {
                p.next_seq = 0;
                p.buf.clear();
            } else {
                self.partial.remove(&h.msg_id);
                return Err(Drop::OutOfOrder);
            }
        }
        if self.max_len != 0 && p.buf.len() + body.len() > self.max_len {
            self.partial.remove(&h.msg_id);
            return Err(Drop::TooLarge);
        }
        p.buf.extend_from_slice(body);
        p.next_seq += 1;
        if p.next_seq == p.total {
            let done = self.partial.remove(&h.msg_id).expect("just inserted");
            return Ok(Some(done.buf));
        }
        Ok(None)
    }

    /// Progress of an in-flight message, `(received_chunks, total_chunks)`.
    pub fn progress(&self, msg_id: u16) -> Option<(u16, u16)> {
        self.partial.get(&msg_id).map(|p| (p.next_seq, p.total))
    }

    /// Forget every partial message (e.g. on disconnect).
    pub fn clear(&mut self) {
        self.partial.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_mtu_carries_twelve_bytes() {
        assert_eq!(chunk_payload(23), 12);
        assert_eq!(chunk_payload(517), 506);
        assert_eq!(chunk_payload(5), 12, "below-floor MTU clamps up");
        assert_eq!(chunk_payload(9000), 506, "above-max MTU clamps down");
    }

    #[test]
    fn roundtrip_every_mtu_and_size() {
        let payload: Vec<u8> = (0..3000u32).map(|i| (i * 7 % 251) as u8).collect();
        for mtu in [23, 24, 50, 185, 512, 517] {
            for len in [1usize, 11, 12, 13, 100, 2999, 3000] {
                let chunks = encode(0xBEEF, &payload[..len], mtu).unwrap();
                let per = chunk_payload(mtu);
                assert_eq!(chunks.len(), len.div_ceil(per));
                for c in &chunks {
                    assert!(c.len() <= mtu - ATT_OVERHEAD);
                }
                let mut r = Reassembler::default();
                let mut got = None;
                for c in &chunks {
                    got = r.push(c).unwrap();
                }
                assert_eq!(got.as_deref(), Some(&payload[..len]));
                assert!(r.partial.is_empty());
            }
        }
    }

    #[test]
    fn interleaved_messages_reassemble_independently() {
        let a = encode(1, b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", 23).unwrap();
        let b = encode(2, b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", 23).unwrap();
        let mut r = Reassembler::default();
        let mut done = Vec::new();
        for (x, y) in a.iter().zip(b.iter()) {
            if let Some(m) = r.push(x).unwrap() { done.push(m); }
            if let Some(m) = r.push(y).unwrap() { done.push(m); }
        }
        assert_eq!(done.len(), 2);
        assert!(done[0].iter().all(|&c| c == b'a'));
        assert!(done[1].iter().all(|&c| c == b'b'));
    }

    #[test]
    fn out_of_order_and_bad_chunks_are_dropped() {
        let chunks = encode(9, &[1u8; 40], 23).unwrap();
        let mut r = Reassembler::default();
        assert_eq!(r.push(&chunks[0]), Ok(None));
        assert_eq!(r.push(&chunks[2]), Err(Drop::OutOfOrder));
        // dropped state: seq 1 now is out of order for a fresh partial
        assert_eq!(r.push(&chunks[1]), Err(Drop::OutOfOrder));
        // restart from 0 works
        for c in &chunks[..chunks.len() - 1] { r.push(c).unwrap(); }
        assert_eq!(r.push(chunks.last().unwrap()).unwrap().unwrap(), vec![1u8; 40]);

        assert_eq!(r.push(&[0x00; 12]), Err(Drop::BadHeader));
        assert_eq!(r.push(&[MAGIC, 9, 0, 0, 0, 0, 1, 0]), Err(Drop::BadHeader), "wrong version");
        assert_eq!(r.push(&[MAGIC, 1, 0, 0, 0, 0, 0, 0]), Err(Drop::BadHeader), "total 0");
    }

    #[test]
    fn cap_is_enforced() {
        let chunks = encode(3, &[7u8; 100], 23).unwrap();
        let mut r = Reassembler::with_cap(50);
        let mut last = Ok(None);
        for c in &chunks { last = r.push(c); if last.is_err() { break; } }
        assert_eq!(last, Err(Drop::TooLarge));
    }

    #[test]
    fn size_limits() {
        assert_eq!(encode(1, &[], 23), Err(FrameError::Empty));
        let huge = vec![0u8; 12 * 65536];
        assert!(matches!(encode(1, &huge, 23), Err(FrameError::TooLarge(_))));
        // 210 KB shielded-send body fits even at the floor MTU (17,500 chunks).
        let body = vec![0u8; 210_000];
        assert_eq!(encode(1, &body, 23).unwrap().len(), 17_500);
        assert_eq!(encode(1, &body, 517).unwrap().len(), 416);
    }
}
