//! End to end over real loopback TCP: app → SOCKS5 proxy → Noise tunnel → relay → target.

use flux_irontunnel::proxy::{probe, start_proxy, ProxyConfig};
use flux_irontunnel::relay::{start_relay, RelayConfig, RelayHandle};
use flux_irontunnel::secure::StaticKeypair;
use flux_irontunnel::socks5::{reply, TargetAddr};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A target that echoes everything until the client half-closes.
async fn echo_server() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.unwrap();
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                tokio::io::copy(&mut r, &mut w).await.ok();
                w.shutdown().await.ok();
            });
        }
    });
    addr
}

struct Rig {
    relay: RelayHandle,
    relay_authorized: std::sync::Arc<std::sync::RwLock<HashSet<flux_irontunnel::secure::PublicKey>>>,
    cfg: ProxyConfig,
}

async fn rig(allow_private: bool, authorize_client: bool) -> Rig {
    let relay_key = StaticKeypair::generate().unwrap();
    let client_key = StaticKeypair::generate().unwrap();
    let relay_pub = *relay_key.public();
    let mut authorized = HashSet::new();
    if authorize_client {
        authorized.insert(*client_key.public());
    }
    let mut rc = RelayConfig::new("127.0.0.1:0".parse().unwrap(), relay_key, authorized);
    rc.egress.allow_private = allow_private;
    let relay_authorized = rc.authorized.clone();
    let relay = start_relay(rc).await.unwrap();
    let cfg = ProxyConfig::new(relay.local_addr.to_string(), client_key, relay_pub);
    Rig { relay, relay_authorized, cfg }
}

/// Minimal SOCKS5 client: returns the stream and the server's reply code.
async fn socks_connect(proxy: SocketAddr, target: &TargetAddr) -> (TcpStream, u8) {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut sel = [0u8; 2];
    s.read_exact(&mut sel).await.unwrap();
    assert_eq!(sel, [5, 0]);
    let mut req = vec![5, 1, 0];
    req.extend(target.encode());
    s.write_all(&req).await.unwrap();
    let mut rep = [0u8; 10];
    s.read_exact(&mut rep).await.unwrap();
    (s, rep[1])
}

#[tokio::test]
async fn one_megabyte_round_trip_through_the_tunnel() {
    let echo = echo_server().await;
    let r = rig(true, true).await;
    let proxy = start_proxy(r.cfg.clone()).await.unwrap();
    let (mut s, code) = socks_connect(proxy.local_addr(), &TargetAddr::Ip(echo)).await;
    assert_eq!(code, reply::SUCCEEDED);

    let data: Vec<u8> = (0..1_048_576u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    let (mut rd, mut wr) = s.split();
    let send = async {
        wr.write_all(&data).await.unwrap();
        wr.shutdown().await.unwrap();
    };
    let mut back = Vec::new();
    let recv = async { rd.read_to_end(&mut back).await.unwrap() };
    tokio::join!(send, recv);
    assert_eq!(back.len(), data.len());
    assert!(back == data, "echo through the tunnel must be byte-identical");

    let st = proxy.stats();
    assert_eq!((st.bytes_up, st.bytes_down), (1_048_576, 1_048_576));
    assert_eq!(r.relay.stats.connects_ok.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn relay_refuses_its_own_loopback_by_default() {
    let echo = echo_server().await;
    let r = rig(false, true).await;
    let proxy = start_proxy(r.cfg.clone()).await.unwrap();
    let (_s, code) = socks_connect(proxy.local_addr(), &TargetAddr::Ip(echo)).await;
    assert_eq!(code, reply::NOT_ALLOWED);
    // A DNS name that resolves to loopback is refused too (checked after resolution).
    let (_s, code) = socks_connect(proxy.local_addr(), &TargetAddr::Domain("localhost".into(), echo.port())).await;
    assert_eq!(code, reply::NOT_ALLOWED);
    assert_eq!(r.relay.stats.connects_denied.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn domain_is_resolved_on_the_relay() {
    let echo = echo_server().await;
    let r = rig(true, true).await;
    let proxy = start_proxy(r.cfg.clone()).await.unwrap();
    let (mut s, code) = socks_connect(proxy.local_addr(), &TargetAddr::Domain("localhost".into(), echo.port())).await;
    assert_eq!(code, reply::SUCCEEDED);
    s.write_all(b"ping").await.unwrap();
    let mut b = [0u8; 4];
    s.read_exact(&mut b).await.unwrap();
    assert_eq!(&b, b"ping");
}

#[tokio::test]
async fn unauthorized_client_cannot_use_the_relay() {
    let echo = echo_server().await;
    let r = rig(true, false).await;
    assert!(probe(&r.cfg).await.is_err());
    let proxy = start_proxy(r.cfg.clone()).await.unwrap();
    let (_s, code) = socks_connect(proxy.local_addr(), &TargetAddr::Ip(echo)).await;
    assert_eq!(code, reply::GENERAL_FAILURE);
    assert_eq!(r.relay.stats.connects_ok.load(Ordering::Relaxed), 0);
    assert!(r.relay.stats.handshakes_refused.load(Ordering::Relaxed) >= 2);
}

#[tokio::test]
async fn probe_reports_a_live_relay() {
    let r = rig(false, true).await;
    let rtt = probe(&r.cfg).await.unwrap();
    assert!(rtt.as_secs() < 5);
}

#[tokio::test]
async fn proxy_will_not_listen_beyond_loopback() {
    let r = rig(false, true).await;
    let mut cfg = r.cfg.clone();
    cfg.listen = "0.0.0.0:0".parse().unwrap();
    assert!(start_proxy(cfg).await.is_err());
}

#[tokio::test]
async fn a_client_can_be_authorized_and_revoked_while_the_relay_runs() {
    let r = rig(false, false).await;
    assert!(probe(&r.cfg).await.is_err(), "not on the list yet");
    let me = *r.cfg.client_key.public();
    r.relay_authorized.write().unwrap().insert(me);
    assert!(probe(&r.cfg).await.is_ok(), "added without a restart");
    r.relay_authorized.write().unwrap().remove(&me);
    assert!(probe(&r.cfg).await.is_err(), "revoked without a restart");
}
