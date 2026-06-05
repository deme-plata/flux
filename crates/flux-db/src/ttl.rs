//! v0.16: per-key TTL.
//!
//! `put_with_ttl(k, v, expiry_unix)` stores the value with a u64 expiry
//! prepended. `get()` returns None for expired keys; `compact()` drops them.
//!
//! Wire format on disk:
//!     [expiry_unix: u64 LE][value bytes]
//! `expiry_unix == 0` means "never expires" (sentinel for non-TTL writes).

use std::time::{SystemTime, UNIX_EPOCH};

const PREFIX: usize = 8;

/// Wrap a raw value with a TTL header. `expiry_unix` is seconds-since-epoch.
pub fn wrap(value: &[u8], expiry_unix: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(PREFIX + value.len());
    out.extend_from_slice(&expiry_unix.to_le_bytes());
    out.extend_from_slice(value);
    out
}

/// Unwrap a TTL-tagged value. Returns:
///   * Some(value) if not expired
///   * None        if expired
///   * Some(stored) if no TTL header (legacy / non-TTL value) — backwards-compat
pub fn unwrap(stored: &[u8], now_unix: u64) -> Option<Vec<u8>> {
    if stored.len() < PREFIX {
        // Legacy / non-TTL value — return as-is.
        return Some(stored.to_vec());
    }
    let expiry = u64::from_le_bytes(stored[..PREFIX].try_into().ok()?);
    if expiry != 0 && expiry <= now_unix {
        return None;
    }
    Some(stored[PREFIX..].to_vec())
}

/// True if a TTL-tagged value is expired NOW.
pub fn is_expired(stored: &[u8]) -> bool {
    if stored.len() < PREFIX {
        return false;
    }
    let expiry = u64::from_le_bytes(stored[..PREFIX].try_into().unwrap());
    if expiry == 0 {
        return false;
    }
    expiry <= now_unix()
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap_unwrap_no_expiry() {
        let w = wrap(b"hello", 0);
        assert_eq!(unwrap(&w, 1_000_000), Some(b"hello".to_vec()));
    }

    #[test]
    fn test_unwrap_expired() {
        let w = wrap(b"old", 100);
        assert_eq!(unwrap(&w, 200), None);
        assert_eq!(unwrap(&w, 50), Some(b"old".to_vec()));
    }

    #[test]
    fn test_legacy_value_passthrough() {
        // A raw 5-byte value with no TTL header — unwrap returns it as-is.
        let raw = b"hello";
        assert_eq!(unwrap(raw, 1_000_000), Some(raw.to_vec()));
    }
}
