//! The session state machine — transport-agnostic, so a GATT stack, a loopback test
//! and the Kotlin/JS ports all drive the same rules.
//!
//! ```text
//!  central                                  peripheral
//!    │  read INFO ──────────────────────────▶│   on_info(hello_p)   → link, PeerHello{sas}
//!    │  write RX: [0x00]hello_c (chunks) ───▶│   on_chunk           → link, PeerHello{sas}
//!    │        …both screens show the same 6 digits; people compare…
//!    │  write RX: [0x01]sealed(coin) ───────▶│   CoinReceived
//!    │◀──── notify TX: [0x01]sealed(coin_ack)│   (after the app stored it)
//! ```
//!
//! Every frame payload starts with one envelope byte: `0x00` plaintext (only `hello` is
//! ever accepted this way, and only before the link exists) or `0x01` sealed.

use super::frame::{self, Reassembler};
use super::link::{Ephemeral, Link, LinkError, Role};
use super::msg::{CoinUri, Hello, Message, CAP_COIN, CAP_RELAY, CAP_REQUEST};

pub const ENV_PLAIN: u8 = 0x00;
pub const ENV_SEALED: u8 = 0x01;
/// Cap on a single reassembled message. A shielded-send body is ~210 KB; 1 MiB leaves
/// room without letting a peer fill memory.
pub const MAX_MESSAGE_LEN: usize = 1 << 20;

/// What this device says about itself. Everything but `epk`, which the session makes.
#[derive(Debug, Clone, Default)]
pub struct Identity {
    pub addr: String,
    pub pk_shield: Option<String>,
    pub pk_enc: Option<String>,
    pub name: Option<String>,
    pub caps: Vec<String>,
}

impl Identity {
    /// A wallet that can take coins, requests and relays.
    pub fn full(addr: impl Into<String>, name: Option<String>) -> Self {
        Self {
            addr: addr.into(),
            pk_shield: None,
            pk_enc: None,
            name,
            caps: vec![CAP_COIN.into(), CAP_REQUEST.into(), CAP_RELAY.into()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The link is up. Show `sas` and ask the human to compare it with the other screen.
    PeerHello { hello: Hello, sas: String },
    /// A coin arrived and parsed as one for our network. **Store it before acking.**
    CoinReceived { id: String, coin: CoinUri, memo: Option<String> },
    /// A coin arrived that we refused; the refusal ack was queued automatically.
    CoinRefused { id: String, reason: String },
    CoinAcked { id: String, ok: bool, err: Option<String> },
    RequestReceived { uri: String },
    RelayReceived { id: String, body: serde_json::Value },
    RelayAcked { id: String, ok: bool, txid: Option<String>, err: Option<String> },
    Bye,
    /// Something the peer did that the protocol forbids. The session is dead after this.
    Fatal(String),
}

/// The result of feeding the session one chunk.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Step {
    pub events: Vec<Event>,
    /// Chunks to write to the peer, in order.
    pub outgoing: Vec<Vec<u8>>,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("link not established yet")]
    NoLink,
    #[error("session is dead: {0}")]
    Dead(String),
    #[error("peer lacks capability {0}")]
    PeerCannot(&'static str),
    #[error("not a coin URI for {0}")]
    BadCoin(String),
    #[error(transparent)]
    Frame(#[from] frame::FrameError),
    #[error(transparent)]
    Link(#[from] LinkError),
    #[error("bad hello: {0}")]
    BadHello(String),
}

pub struct Session {
    role: Role,
    me: Identity,
    eph: Option<Ephemeral>,
    my_epk_hex: String,
    link: Option<Link>,
    peer: Option<Hello>,
    rx: Reassembler,
    mtu: usize,
    next_msg_id: u16,
    dead: Option<String>,
}

impl Session {
    /// `mtu` is the ATT MTU this side believes is negotiated; 23 if unknown.
    pub fn new(role: Role, me: Identity, mtu: usize) -> Self {
        Self::with_ephemeral(role, me, mtu, Ephemeral::generate())
    }

    /// Deterministic keys, for tests and vectors.
    pub fn with_ephemeral(role: Role, me: Identity, mtu: usize, eph: Ephemeral) -> Self {
        let my_epk_hex = eph.public_hex();
        let next_msg_id = (rand::random::<u16>()).max(1);
        Self {
            role,
            me,
            eph: Some(eph),
            my_epk_hex,
            link: None,
            peer: None,
            rx: Reassembler::with_cap(MAX_MESSAGE_LEN),
            mtu: mtu.clamp(frame::MIN_MTU, frame::MAX_MTU),
            next_msg_id,
            dead: None,
        }
    }

    pub fn role(&self) -> Role {
        self.role
    }
    pub fn mtu(&self) -> usize {
        self.mtu
    }
    /// Update after the stack reports a negotiated MTU (peripheral) or the peer's
    /// hello carried a hint (central).
    pub fn set_mtu(&mut self, mtu: usize) {
        self.mtu = mtu.clamp(frame::MIN_MTU, frame::MAX_MTU);
    }
    pub fn is_linked(&self) -> bool {
        self.link.is_some()
    }
    pub fn peer(&self) -> Option<&Hello> {
        self.peer.as_ref()
    }
    pub fn sas_code(&self) -> Option<String> {
        self.link.as_ref().map(|l| l.sas_code())
    }

    /// Our plaintext `hello`. The peripheral serves this from INFO; the central sends it
    /// as its first frame ([`start`](Self::start)).
    pub fn hello(&self) -> Hello {
        Hello {
            v: super::PROTOCOL_VERSION,
            net: super::NETWORK.into(),
            addr: self.me.addr.clone(),
            pk_shield: self.me.pk_shield.clone(),
            pk_enc: self.me.pk_enc.clone(),
            epk: self.my_epk_hex.clone(),
            name: self.me.name.clone(),
            caps: self.me.caps.clone(),
            mtu: Some(self.mtu as u16),
        }
    }

    /// Bytes to serve from the INFO characteristic (peripheral).
    pub fn info_bytes(&self) -> Vec<u8> {
        Message::Hello(self.hello()).to_bytes()
    }

    /// Central: the peripheral's INFO value. Establishes the link.
    pub fn on_info(&mut self, info: &[u8]) -> Result<Event, SessionError> {
        let hello = match Message::from_bytes(info) {
            Ok(Message::Hello(h)) => h,
            Ok(_) => return Err(SessionError::BadHello("INFO was not a hello".into())),
            Err(e) => return Err(SessionError::BadHello(e.to_string())),
        };
        if let Some(m) = hello.mtu {
            self.set_mtu(m as usize);
        }
        self.establish(hello)
    }

    /// Central: chunks of our own hello to write to RX right after [`on_info`](Self::on_info).
    pub fn start(&mut self) -> Result<Vec<Vec<u8>>, SessionError> {
        let mut payload = vec![ENV_PLAIN];
        payload.extend_from_slice(&self.info_bytes());
        self.frames(&payload)
    }

    fn establish(&mut self, hello: Hello) -> Result<Event, SessionError> {
        if hello.v != super::PROTOCOL_VERSION {
            return Err(self.die(format!("peer protocol v{} ≠ v{}", hello.v, super::PROTOCOL_VERSION)));
        }
        if hello.net != super::NETWORK {
            return Err(self.die(format!("peer network {} ≠ {}", hello.net, super::NETWORK)));
        }
        let epk = hex::decode(&hello.epk).map_err(|_| SessionError::BadHello("epk not hex".into()))?;
        let eph = self.eph.take().ok_or_else(|| SessionError::Dead("hello already exchanged".into()))?;
        let link = eph.link(self.role, &epk)?;
        let sas = link.sas_code();
        self.link = Some(link);
        self.peer = Some(hello.clone());
        Ok(Event::PeerHello { hello, sas })
    }

    fn die(&mut self, why: String) -> SessionError {
        self.dead = Some(why.clone());
        SessionError::Dead(why)
    }

    fn frames(&mut self, payload: &[u8]) -> Result<Vec<Vec<u8>>, SessionError> {
        let id = self.next_msg_id;
        self.next_msg_id = self.next_msg_id.wrapping_add(1).max(1);
        Ok(frame::encode(id, payload, self.mtu)?)
    }

    fn sealed(&mut self, m: &Message) -> Result<Vec<Vec<u8>>, SessionError> {
        if let Some(why) = &self.dead {
            return Err(SessionError::Dead(why.clone()));
        }
        let link = self.link.as_mut().ok_or(SessionError::NoLink)?;
        let mut payload = vec![ENV_SEALED];
        payload.extend_from_slice(&link.seal(&m.to_bytes()));
        self.frames(&payload)
    }

    fn peer_can(&self, cap: &'static str) -> Result<(), SessionError> {
        match &self.peer {
            Some(p) if p.caps.iter().any(|c| c == cap) => Ok(()),
            Some(_) => Err(SessionError::PeerCannot(cap)),
            None => Err(SessionError::NoLink),
        }
    }

    /// Offer a coin. Returns `(id, chunks)`; wait for [`Event::CoinAcked`] with that id
    /// before treating the coin as handed over.
    pub fn send_coin(&mut self, uri: &str, memo: Option<String>) -> Result<(String, Vec<Vec<u8>>), SessionError> {
        self.peer_can(CAP_COIN)?;
        let c = CoinUri::parse(uri).ok_or_else(|| SessionError::BadCoin(uri.into()))?;
        if c.network != super::NETWORK {
            return Err(SessionError::BadCoin(format!("network {}", c.network)));
        }
        let id = hex::encode(rand::random::<[u8; 4]>());
        let chunks = self.sealed(&Message::Coin { id: id.clone(), uri: c.to_uri(), memo })?;
        Ok((id, chunks))
    }

    /// Acknowledge a received coin — call only after it is durably stored.
    pub fn ack_coin(&mut self, id: &str, ok: bool, err: Option<String>) -> Result<Vec<Vec<u8>>, SessionError> {
        self.sealed(&Message::CoinAck { id: id.into(), ok, err })
    }

    pub fn send_request(&mut self, uri: &str) -> Result<Vec<Vec<u8>>, SessionError> {
        self.peer_can(CAP_REQUEST)?;
        self.sealed(&Message::Request { uri: uri.into() })
    }

    pub fn send_relay(&mut self, body: serde_json::Value) -> Result<(String, Vec<Vec<u8>>), SessionError> {
        self.peer_can(CAP_RELAY)?;
        let id = hex::encode(rand::random::<[u8; 4]>());
        let chunks = self.sealed(&Message::Relay { id: id.clone(), body })?;
        Ok((id, chunks))
    }

    pub fn ack_relay(&mut self, id: &str, ok: bool, txid: Option<String>, err: Option<String>) -> Result<Vec<Vec<u8>>, SessionError> {
        self.sealed(&Message::RelayAck { id: id.into(), ok, txid, err })
    }

    pub fn bye(&mut self) -> Result<Vec<Vec<u8>>, SessionError> {
        self.sealed(&Message::Bye)
    }

    /// Feed one received chunk (an RX write on the peripheral, a TX notification on the
    /// central).
    pub fn on_chunk(&mut self, chunk: &[u8]) -> Step {
        let mut step = Step::default();
        if let Some(why) = &self.dead {
            step.events.push(Event::Fatal(why.clone()));
            return step;
        }
        let payload = match self.rx.push(chunk) {
            Ok(Some(p)) => p,
            Ok(None) => return step,
            Err(_) => return step, // logged by the transport; the sender's timeout is the signal
        };
        self.on_message(&payload, &mut step);
        step
    }

    /// Progress of the message currently being received on this side.
    pub fn receive_progress(&self, msg_id: u16) -> Option<(u16, u16)> {
        self.rx.progress(msg_id)
    }

    fn on_message(&mut self, payload: &[u8], step: &mut Step) {
        let Some((&env, body)) = payload.split_first() else { return };
        match (env, self.link.is_some()) {
            (ENV_PLAIN, false) => match Message::from_bytes(body) {
                Ok(Message::Hello(h)) => match self.establish(h) {
                    Ok(ev) => step.events.push(ev),
                    Err(e) => step.events.push(Event::Fatal(e.to_string())),
                },
                _ => {
                    let e = self.die("first message was not a hello".into());
                    step.events.push(Event::Fatal(e.to_string()));
                }
            },
            (ENV_PLAIN, true) => {
                let e = self.die("plaintext after link".into());
                step.events.push(Event::Fatal(e.to_string()));
            }
            (ENV_SEALED, false) => {
                let e = self.die("sealed message before hello".into());
                step.events.push(Event::Fatal(e.to_string()));
            }
            (ENV_SEALED, true) => {
                let opened = self.link.as_mut().unwrap().open(body);
                match opened {
                    Ok(pt) => match Message::from_bytes(&pt) {
                        Ok(m) => self.on_sealed(m, step),
                        Err(e) => {
                            let e = self.die(format!("bad sealed message: {e}"));
                            step.events.push(Event::Fatal(e.to_string()));
                        }
                    },
                    Err(e) => {
                        let e = self.die(format!("open failed: {e}"));
                        step.events.push(Event::Fatal(e.to_string()));
                    }
                }
            }
            _ => {
                let e = self.die(format!("unknown envelope {env:#x}"));
                step.events.push(Event::Fatal(e.to_string()));
            }
        }
    }

    fn on_sealed(&mut self, m: Message, step: &mut Step) {
        match m {
            Message::Hello(_) => {
                let e = self.die("second hello".into());
                step.events.push(Event::Fatal(e.to_string()));
            }
            Message::Coin { id, uri, memo } => match CoinUri::parse(&uri) {
                Some(c) if c.network == super::NETWORK => {
                    step.events.push(Event::CoinReceived { id, coin: c, memo });
                }
                Some(c) => self.refuse_coin(id, format!("coin is for {}, not {}", c.network, super::NETWORK), step),
                None => self.refuse_coin(id, "not a coin".into(), step),
            },
            Message::CoinAck { id, ok, err } => step.events.push(Event::CoinAcked { id, ok, err }),
            Message::Request { uri } => step.events.push(Event::RequestReceived { uri }),
            Message::Relay { id, body } => step.events.push(Event::RelayReceived { id, body }),
            Message::RelayAck { id, ok, txid, err } => step.events.push(Event::RelayAcked { id, ok, txid, err }),
            Message::Bye => step.events.push(Event::Bye),
        }
    }

    fn refuse_coin(&mut self, id: String, reason: String, step: &mut Step) {
        if let Ok(chunks) = self.ack_coin(&id, false, Some(reason.clone())) {
            step.outgoing.extend(chunks);
        }
        step.events.push(Event::CoinRefused { id, reason });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(s: &mut Session, chunks: &[Vec<u8>]) -> Step {
        let mut all = Step::default();
        for c in chunks {
            let st = s.on_chunk(c);
            all.events.extend(st.events);
            all.outgoing.extend(st.outgoing);
        }
        all
    }

    fn linked(mtu_c: usize, mtu_p: usize) -> (Session, Session) {
        let mut c = Session::new(Role::Central, Identity::full("c".repeat(64), Some("laptop".into())), mtu_c);
        let mut p = Session::new(Role::Peripheral, Identity::full("p".repeat(64), Some("phone".into())), mtu_p);
        let ev = c.on_info(&p.info_bytes()).unwrap();
        let Event::PeerHello { sas: sas_c, hello } = ev else { panic!() };
        assert_eq!(hello.name.as_deref(), Some("phone"));
        assert_eq!(c.mtu(), p.mtu(), "central adopts the peripheral's MTU hint");
        let st = feed(&mut p, &c.start().unwrap());
        assert!(st.outgoing.is_empty());
        let [Event::PeerHello { sas: sas_p, .. }] = st.events.as_slice() else { panic!("{:?}", st.events) };
        assert_eq!(&sas_c, sas_p);
        (c, p)
    }

    #[test]
    fn full_coin_handoff_both_directions() {
        let (mut c, mut p) = linked(23, 185);
        let uri = format!("sigil:coin?v=1&k={}&a=10000000000&n=sigil-g2", "ab".repeat(32));
        // phone → laptop
        let (id, chunks) = p.send_coin(&uri, Some("for the coffee".into())).unwrap();
        let st = feed(&mut c, &chunks);
        assert_eq!(
            st.events,
            vec![Event::CoinReceived { id: id.clone(), coin: CoinUri::parse(&uri).unwrap(), memo: Some("for the coffee".into()) }]
        );
        let st = feed(&mut p, &c.ack_coin(&id, true, None).unwrap());
        assert_eq!(st.events, vec![Event::CoinAcked { id, ok: true, err: None }]);
        // laptop → phone
        let (id2, chunks) = c.send_coin(&uri, None).unwrap();
        let st = feed(&mut p, &chunks);
        assert!(matches!(&st.events[0], Event::CoinReceived { id, .. } if id == &id2));
        let st = feed(&mut c, &p.ack_coin(&id2, false, Some("purse full".into())).unwrap());
        assert_eq!(st.events, vec![Event::CoinAcked { id: id2, ok: false, err: Some("purse full".into()) }]);
        // request + relay + bye
        let st = feed(&mut p, &c.send_request("sigil:abc?amount=5").unwrap());
        assert_eq!(st.events, vec![Event::RequestReceived { uri: "sigil:abc?amount=5".into() }]);
        let body = serde_json::json!({"anchor":"x","proof":"y".repeat(50_000)});
        let (rid, chunks) = p.send_relay(body.clone()).unwrap();
        assert!(chunks.len() > 100, "a big body really is chunked");
        let st = feed(&mut c, &chunks);
        assert_eq!(st.events, vec![Event::RelayReceived { id: rid.clone(), body }]);
        let st = feed(&mut p, &c.ack_relay(&rid, true, Some("tx1".into()), None).unwrap());
        assert_eq!(st.events, vec![Event::RelayAcked { id: rid, ok: true, txid: Some("tx1".into()), err: None }]);
        let st = feed(&mut c, &p.bye().unwrap());
        assert_eq!(st.events, vec![Event::Bye]);
    }

    #[test]
    fn wrong_network_coin_is_refused_with_an_ack() {
        let (mut c, mut p) = linked(517, 517);
        let uri = format!("sigil:coin?v=1&k={}&n=sigil-g1", "ab".repeat(32));
        assert!(matches!(p.send_coin(&uri, None), Err(SessionError::BadCoin(_))), "sender refuses too");
        // force it past the sender check by sealing by hand
        let m = Message::Coin { id: "z".into(), uri, memo: None };
        let chunks = p.sealed(&m).unwrap();
        let st = feed(&mut c, &chunks);
        assert!(matches!(&st.events[0], Event::CoinRefused { id, .. } if id == "z"));
        assert!(!st.outgoing.is_empty(), "refusal ack queued");
        let st = feed(&mut p, &st.outgoing);
        assert!(matches!(&st.events[0], Event::CoinAcked { ok: false, .. }));
    }

    #[test]
    fn money_before_hello_kills_the_session() {
        let mut p = Session::new(Role::Peripheral, Identity::full("p".repeat(64), None), 23);
        let chunks = frame::encode(1, &[ENV_SEALED, 1, 2, 3], 23).unwrap();
        let st = feed(&mut p, &chunks);
        assert!(matches!(&st.events[0], Event::Fatal(_)));
        assert!(matches!(p.ack_coin("x", true, None), Err(SessionError::Dead(_))));
    }

    #[test]
    fn capability_and_link_gates() {
        let mut c = Session::new(Role::Central, Identity::full("c".repeat(64), None), 23);
        assert!(matches!(c.send_coin("sigil:coin?v=1&k=00", None), Err(SessionError::NoLink)));
        let mut p = Session::new(
            Role::Peripheral,
            Identity { addr: "p".repeat(64), caps: vec![CAP_REQUEST.into()], ..Default::default() },
            23,
        );
        c.on_info(&p.info_bytes()).unwrap();
        feed(&mut p, &c.start().unwrap());
        let uri = format!("sigil:coin?v=1&k={}&n=sigil-g2", "ab".repeat(32));
        assert!(matches!(c.send_coin(&uri, None), Err(SessionError::PeerCannot("coin"))));
        assert!(c.send_request("sigil:x").is_ok());
        // wrong-network peer is fatal at hello time
        let mut bad = Session::new(Role::Central, Identity::full("c".repeat(64), None), 23);
        let mut h = p.hello();
        h.net = "sigil-g1".into();
        let info = Message::Hello(h).to_bytes();
        assert!(matches!(bad.on_info(&info), Err(SessionError::Dead(_))));
    }
}
