//! Messages that cross a SIGIL BLE link, as JSON tagged by `t`.
//!
//! JSON rather than a packed binary on purpose: three independent implementations
//! (this crate, Kotlin on the phone, JS in Chrome) have to agree byte-for-byte, and a
//! schema-free text format is the cheapest thing to keep in agreement. The one large
//! payload — a `relay` body — is the node's own `/v1/shielded_send` JSON, carried
//! verbatim.

use serde::{Deserialize, Serialize};

/// What a peer can do. Sent in `hello`; a sender checks before offering.
pub const CAP_COIN: &str = "coin";
pub const CAP_REQUEST: &str = "request";
pub const CAP_RELAY: &str = "relay";

/// The plaintext greeting. The peripheral serves it from INFO; the central writes its
/// own to RX first thing. Nothing in it is secret — addresses and shield keys are what
/// a payment request already publishes — except that `epk` is what the whole link's
/// confidentiality hangs on, and it is *authenticated* only by the two people comparing
/// the code on their screens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// Protocol version. A mismatch ends the session before any money message.
    pub v: u8,
    /// Network id, e.g. `sigil-g2`.
    pub net: String,
    /// Transparent wallet address (64 hex).
    pub addr: String,
    /// Shielded spend public key, hex — so the peer can build a payment request to us.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pk_shield: Option<String>,
    /// Shielded encryption public key, hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pk_enc: Option<String>,
    /// Ephemeral X25519 public key, hex (32 bytes). Fresh per session.
    pub epk: String,
    /// Human name shown in the peer picker. Not trusted for anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Capabilities, see `CAP_*`.
    #[serde(default)]
    pub caps: Vec<String>,
    /// ATT MTU the peripheral saw negotiated for this connection, if it knows. Web
    /// Bluetooth cannot ask its own stack, so the phone tells it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtu: Option<u16>,
}

/// Every message. `hello` is the only one that ever travels in the clear; the rest are
/// sealed by [`super::link::Link`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Message {
    Hello(Hello),
    /// A bearer coin. `uri` is the money.
    Coin {
        /// Sender-chosen id so the ack can name it.
        id: String,
        uri: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        memo: Option<String>,
    },
    CoinAck {
        id: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        err: Option<String>,
    },
    /// A `sigil:<addr>?amount=…` payment request, for the peer to pay when it can.
    Request { uri: String },
    /// A pre-built `/v1/shielded_send` body the peer should submit when online.
    Relay { id: String, body: serde_json::Value },
    RelayAck {
        id: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        txid: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        err: Option<String>,
    },
    Bye,
}

impl Message {
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("Message is always serialisable")
    }
    pub fn from_bytes(b: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(b)
    }
}

/// The `sigil:coin?…` URI grammar, mirrored from the Android `CoinTag` so the receiver
/// can refuse a malformed or wrong-network coin *before* acking it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoinUri {
    pub key_hex: String,
    pub amount_glyphs: Option<u128>,
    pub network: String,
}

impl CoinUri {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let body = if let Some(b) = strip_prefix_ci(s, "sigil:coin?") {
            b
        } else if let Some(b) = strip_prefix_ci(s, "sigil://coin?") {
            b
        } else {
            return None;
        };
        let mut key = None;
        let mut amount = None;
        let mut network = super::NETWORK.to_string();
        let mut version = 1u32;
        for pair in body.split('&') {
            let Some((k, v)) = pair.split_once('=') else { continue };
            match k.to_ascii_lowercase().as_str() {
                "v" => version = v.parse().ok()?,
                "k" => key = Some(v.trim().to_ascii_lowercase()),
                "a" => amount = Some(v.trim().parse::<u128>().ok()?),
                "n" => network = v.trim().to_string(),
                _ => {}
            }
        }
        if version != 1 {
            return None;
        }
        let key = key?;
        if key.len() != 64 || !key.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
            return None;
        }
        Some(Self { key_hex: key, amount_glyphs: amount, network })
    }

    pub fn to_uri(&self) -> String {
        let mut s = format!("sigil:coin?v=1&k={}", self.key_hex);
        if let Some(a) = self.amount_glyphs {
            s.push_str(&format!("&a={a}"));
        }
        s.push_str(&format!("&n={}", self.network));
        s
    }
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_shape_is_tagged_snake_case() {
        let m = Message::CoinAck { id: "ab".into(), ok: true, err: None };
        assert_eq!(String::from_utf8(m.to_bytes()).unwrap(), r#"{"t":"coin_ack","id":"ab","ok":true}"#);
        assert_eq!(String::from_utf8(Message::Bye.to_bytes()).unwrap(), r#"{"t":"bye"}"#);
        let h: Message = serde_json::from_str(
            r#"{"t":"hello","v":1,"net":"sigil-g2","addr":"aa","epk":"bb","caps":["coin"],"extra":1}"#,
        )
        .unwrap();
        match h {
            Message::Hello(h) => {
                assert_eq!(h.caps, vec!["coin"]);
                assert!(h.pk_shield.is_none());
            }
            _ => panic!(),
        }
        assert!(Message::from_bytes(br#"{"t":"nope"}"#).is_err());
    }

    #[test]
    fn coin_uri_grammar_matches_android() {
        let k = "0123456789abcdef".repeat(4);
        let c = CoinUri::parse(&format!("sigil:coin?v=1&k={k}&a=10000000000&n=sigil-g2")).unwrap();
        assert_eq!(c.amount_glyphs, Some(10_000_000_000));
        assert_eq!(c.network, "sigil-g2");
        assert_eq!(c.to_uri(), format!("sigil:coin?v=1&k={k}&a=10000000000&n=sigil-g2"));
        assert!(CoinUri::parse(&format!("SIGIL://COIN?k={}", k.to_uppercase())).is_some());
        assert!(CoinUri::parse(&format!("sigil:{k}")).is_none(), "a payment request is not a coin");
        assert!(CoinUri::parse(&format!("sigil:coin?v=2&k={k}")).is_none(), "unknown version refused");
        assert!(CoinUri::parse("sigil:coin?v=1&k=abc").is_none());
        assert!(CoinUri::parse(&format!("sigil:coin?v=1&k={k}&a=ten")).is_none(), "bad amount refused");
    }
}
