//! The captured-frame value type.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Wire/at-rest encoding of a frame's bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameFormat {
    Png,
    Jpeg,
}

impl FrameFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            FrameFormat::Png => "png",
            FrameFormat::Jpeg => "jpeg",
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            FrameFormat::Png => "png",
            FrameFormat::Jpeg => "jpg",
        }
    }

    /// Identify a frame by its magic bytes.
    ///
    /// Sniffing rather than trusting a file extension is deliberate: the
    /// file/command sources accept output from tools we do not control, and a
    /// `.png` that is really a JPEG would otherwise propagate a wrong
    /// `format` field all the way onto the wire.
    pub fn sniff(bytes: &[u8]) -> Option<FrameFormat> {
        if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
            Some(FrameFormat::Png)
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(FrameFormat::Jpeg)
        } else {
            None
        }
    }
}

/// One captured image plus the provenance an agent needs to trust it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub format: FrameFormat,
    /// BLAKE3 of `data`, hex. Content-addresses the frame so a relayed copy can
    /// be proven identical to what the capturing node actually saw.
    pub hash: String,
    pub captured_at_ms: u64,
    /// Which source produced it (`synthetic`, `file:…`, `command:…`).
    pub source: String,
    #[serde(skip)]
    pub data: Vec<u8>,
}

impl Frame {
    pub fn new(
        width: u32,
        height: u32,
        format: FrameFormat,
        data: Vec<u8>,
        source: impl Into<String>,
    ) -> Self {
        let hash = blake3::hash(&data).to_hex().to_string();
        Frame {
            width,
            height,
            format,
            hash,
            captured_at_ms: now_ms(),
            source: source.into(),
            data,
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Re-derive the hash and compare. Used after any transport hop — a frame
    /// that fails this is counted as a decode error and costs SAP accuracy.
    pub fn verify(&self) -> bool {
        blake3::hash(&self.data).to_hex().to_string() == self.hash
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_identifies_formats() {
        assert_eq!(
            FrameFormat::sniff(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]),
            Some(FrameFormat::Png)
        );
        assert_eq!(FrameFormat::sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(FrameFormat::Jpeg));
        assert_eq!(FrameFormat::sniff(b"not an image"), None);
        assert_eq!(FrameFormat::sniff(&[]), None);
    }

    #[test]
    fn hash_is_content_addressed_and_verifies() {
        let f = Frame::new(1, 1, FrameFormat::Png, vec![1, 2, 3], "test");
        assert!(f.verify());
        assert_eq!(f.hash.len(), 64, "BLAKE3 hex is 64 chars");

        let g = Frame::new(1, 1, FrameFormat::Png, vec![1, 2, 3], "other-source");
        assert_eq!(f.hash, g.hash, "same bytes must content-address identically");

        let h = Frame::new(1, 1, FrameFormat::Png, vec![1, 2, 4], "test");
        assert_ne!(f.hash, h.hash, "different bytes must differ");
    }

    #[test]
    fn tampering_is_detected() {
        let mut f = Frame::new(1, 1, FrameFormat::Png, vec![9, 9, 9], "test");
        f.data[0] = 8;
        assert!(!f.verify(), "a mutated frame must fail verification");
    }
}
