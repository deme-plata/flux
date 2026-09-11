//! Cross-implementation test vectors, written to `tests/vectors/sigil-ble-v1.json`.
//!
//! The Kotlin (Android wallet) and JS (browser) ports load this file in their own unit
//! tests, so a change here that is not a deliberate protocol bump fails on all three
//! sides at once. Run with `SIGIL_BLE_WRITE_VECTORS=1` to regenerate.

use super::frame;
use super::link::{Ephemeral, Link, Role};
use super::session::{Identity, Session};
use serde_json::{json, Value};

pub fn vectors_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/sigil-ble-v1.json")
}

pub fn generate() -> Value {
    // ── frames ────────────────────────────────────────────────────────────────
    let mut frames = Vec::new();
    for (msg_id, mtu, payload) in [
        (1u16, 23usize, b"hello".to_vec()),
        (0xBEEF, 23, (0u8..40).collect::<Vec<u8>>()),
        (7, 517, (0u8..=255).cycle().take(1013).collect::<Vec<u8>>()),
        (300, 185, b"\x00{\"t\":\"bye\"}".to_vec()),
    ] {
        let chunks = frame::encode(msg_id, &payload, mtu).unwrap();
        frames.push(json!({
            "msg_id": msg_id, "mtu": mtu, "payload_hex": hex::encode(&payload),
            "chunks_hex": chunks.iter().map(hex::encode).collect::<Vec<_>>(),
        }));
    }

    // ── link ──────────────────────────────────────────────────────────────────
    let sk_c = [0x11u8; 32];
    let sk_p = [0x22u8; 32];
    let ec = Ephemeral::from_secret(sk_c);
    let ep = Ephemeral::from_secret(sk_p);
    let (epk_c, epk_p) = (ec.public_bytes(), ep.public_bytes());
    let shared = x25519_dalek::StaticSecret::from(sk_c).diffie_hellman(&x25519_dalek::PublicKey::from(epk_p));
    let mut lc = ec.link(Role::Central, &epk_p).unwrap();
    let mut lp = ep.link(Role::Peripheral, &epk_c).unwrap();
    assert_eq!(lc.sas_code(), lp.sas_code());
    let probe = Link::derive(Role::Central, shared.as_bytes(), &epk_c, &epk_p);
    let c2p_0 = lc.seal(b"{\"t\":\"bye\"}");
    let c2p_1 = lc.seal(b"second");
    let p2c_0 = lp.seal(b"{\"t\":\"coin_ack\",\"id\":\"00\",\"ok\":true}");
    assert_eq!(lp.open(&c2p_0).unwrap(), b"{\"t\":\"bye\"}");
    let link = json!({
        "salt": String::from_utf8(super::link::SALT.to_vec()).unwrap(),
        "aad": String::from_utf8(super::link::AAD.to_vec()).unwrap(),
        "sk_central_hex": hex::encode(sk_c), "sk_peripheral_hex": hex::encode(sk_p),
        "epk_central_hex": hex::encode(epk_c), "epk_peripheral_hex": hex::encode(epk_p),
        "shared_hex": hex::encode(shared.as_bytes()),
        "sas": probe.sas_code(),
        "sealed": [
            {"dir": "c2p", "ctr": 0, "plaintext_utf8": "{\"t\":\"bye\"}", "sealed_hex": hex::encode(&c2p_0)},
            {"dir": "c2p", "ctr": 1, "plaintext_utf8": "second", "sealed_hex": hex::encode(&c2p_1)},
            {"dir": "p2c", "ctr": 0, "plaintext_utf8": "{\"t\":\"coin_ack\",\"id\":\"00\",\"ok\":true}", "sealed_hex": hex::encode(&p2c_0)},
        ],
    });

    // ── a whole session, deterministic ────────────────────────────────────────
    let mut c = Session::with_ephemeral(Role::Central, Identity::full("c".repeat(64), Some("laptop".into())), 517, Ephemeral::from_secret(sk_c));
    let mut p = Session::with_ephemeral(Role::Peripheral, Identity::full("p".repeat(64), Some("phone".into())), 517, Ephemeral::from_secret(sk_p));
    let info = p.info_bytes();
    c.on_info(&info).unwrap();
    let session = json!({
        "peripheral_info_utf8": String::from_utf8(info).unwrap(),
        "central_hello_utf8": String::from_utf8(c.info_bytes()).unwrap(),
        "sas": c.sas_code().unwrap(),
    });

    json!({
        "protocol": "sigil-ble", "version": super::PROTOCOL_VERSION,
        "service_uuid": super::SERVICE_UUID, "info_uuid": super::INFO_UUID,
        "rx_uuid": super::RX_UUID, "tx_uuid": super::TX_UUID,
        "network": super::NETWORK,
        "frames": frames, "link": link, "session": session,
    })
}

#[test]
fn vectors_are_stable() {
    let v = generate();
    let path = vectors_path();
    let text = serde_json::to_string_pretty(&v).unwrap() + "\n";
    if std::env::var("SIGIL_BLE_WRITE_VECTORS").is_ok() || !path.exists() {
        std::fs::write(&path, &text).unwrap();
        eprintln!("wrote {}", path.display());
        return;
    }
    let on_disk: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(on_disk, v, "vectors drifted — protocol change? regenerate with SIGIL_BLE_WRITE_VECTORS=1");
}
