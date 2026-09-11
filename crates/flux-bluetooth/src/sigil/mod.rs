//! The SIGIL BLE profile — hand a bearer coin from one device to another with no
//! network, over one Bluetooth Low Energy link.
//!
//! ## What travels over the link
//!
//! A SIGIL coin is a throwaway wallet holding one note; its whole claim material is the
//! `sigil:coin?v=1&k=<64 hex>&a=<glyphs>&n=sigil-g2` URI the NFC path already writes to a
//! tag (~110 bytes). This profile moves that same URI over BLE instead of a tag. Minting
//! still needs the chain (the sender pays a note to the coin key while online) and
//! claiming still needs the chain (the receiver sweeps it when back online) — the part
//! that is offline is the *hand-over*, exactly as with a physical coin in a pocket.
//!
//! **Same guarantee as the tag, stated the same way:** first claim wins. A copy of the
//! URI is a losing ticket in a race, not a second coin. What this profile adds over NFC
//! is *range* — 10 m instead of 2 cm — and range is a threat, so every coin crosses the
//! link inside a sealed envelope (X25519 → HKDF-SHA256 → AES-256-GCM), and both screens
//! show the same 6-digit code derived from the two ephemeral keys. A passive sniffer
//! learns nothing; an active man-in-the-middle produces two *different* codes, which is
//! what the people holding the phones are asked to compare before money moves.
//!
//! ## Roles — and why the phone is the server
//!
//! Chrome's Web Bluetooth can only be a *central* (it connects to a GATT server; it
//! cannot advertise). So the phone advertises the SIGIL service and runs the GATT
//! server; Chrome — or the other phone — connects. Roles in this module:
//!
//! - **Peripheral** (`Role::Peripheral`): advertises, owns INFO/RX/TX, answers.
//! - **Central** (`Role::Central`): scans, connects, reads INFO, writes RX, subscribes TX.
//!
//! Money can flow either direction once the link is up — the role only says who
//! dialled.
//!
//! ## GATT layout
//!
//! ```text
//! service  53494749-4c42-4c45-a000-000000000001   ("SIGI" "LBLE" in the leading bytes)
//!   INFO   …0002   READ            the peripheral's plaintext `hello` (JSON)
//!   RX     …0003   WRITE / WNR     central  → peripheral, chunked frames
//!   TX     …0004   NOTIFY          peripheral → central, chunked frames
//! ```
//!
//! ## Framing
//!
//! A BLE attribute write carries at most `MTU − 3` bytes, and the MTU is whatever the OS
//! negotiated (23 on the floor, 517 on a good day). [`frame`] cuts a message into chunks
//! with an 8-byte header — `magic, version, msg_id u16, seq u16, total u16` — and
//! reassembles them on the other side. Every implementation (this crate, the Android
//! wallet's Kotlin, the browser's JS) is tested against the vectors in
//! `tests/vectors/sigil-ble-v1.json`, which [`vectors`] regenerates.
//!
//! ## Messages ([`msg`])
//!
//! | `t`         | direction      | sealed | meaning |
//! |-------------|----------------|--------|---------|
//! | `hello`     | both, first    | no     | network, address, shield keys, ephemeral X25519 key, capabilities |
//! | `coin`      | either         | yes    | a `sigil:coin?…` URI — **the money** |
//! | `coin_ack`  | reply          | yes    | receiver has stored it (or refused, with a reason) |
//! | `request`   | either         | yes    | a `sigil:<addr>?amount=…` payment request |
//! | `relay`     | either         | yes    | a pre-built `/v1/shielded_send` body for the peer to submit when online |
//! | `relay_ack` | reply          | yes    | submitted (txid) or not |
//! | `bye`       | either         | yes    | orderly close |
//!
//! ## What is still pretend
//!
//! `relay` moves bytes; nothing here *builds* an offline shielded transaction (that needs
//! the sender to hold a recent leaf set). The receive-and-forward half is real.

pub mod frame;
pub mod link;
pub mod msg;
pub mod session;
#[cfg(test)]
pub mod vectors;

/// 128-bit service UUID as a string, the form Web Bluetooth and Android both take.
pub const SERVICE_UUID: &str = "53494749-4c42-4c45-a000-000000000001";
/// INFO characteristic — READ. Returns the peripheral's plaintext `hello` JSON.
pub const INFO_UUID: &str = "53494749-4c42-4c45-a000-000000000002";
/// RX characteristic — WRITE (with or without response). Central → peripheral frames.
pub const RX_UUID: &str = "53494749-4c42-4c45-a000-000000000003";
/// TX characteristic — NOTIFY. Peripheral → central frames.
pub const TX_UUID: &str = "53494749-4c42-4c45-a000-000000000004";

/// Protocol version carried in every frame header and every `hello`.
pub const PROTOCOL_VERSION: u8 = 1;
/// The network this profile hands coins for. A coin from another network is refused.
pub const NETWORK: &str = "sigil-g2";

/// The 16 raw bytes of [`SERVICE_UUID`], for advertisement payloads.
pub const SERVICE_UUID_BYTES: [u8; 16] = [
    0x53, 0x49, 0x47, 0x49, 0x4c, 0x42, 0x4c, 0x45, 0xa0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_uuid_bytes_match_string() {
        let hex_str: String = SERVICE_UUID.chars().filter(|c| *c != '-').collect();
        assert_eq!(hex_str, hex::encode(SERVICE_UUID_BYTES));
        // RFC 4122: version nibble 4, variant bits 10xx.
        assert_eq!(SERVICE_UUID_BYTES[6] >> 4, 4);
        assert_eq!(SERVICE_UUID_BYTES[8] >> 6, 0b10);
    }
}
