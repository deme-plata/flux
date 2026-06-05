//! Two-node gossipsub integration test — reproduces and verifies the fix for
//! the rocky-83 / P3-demo bug: "drain_events surfaces NO GossipsubMessage
//! events even though publish() succeeds + peers=N in the mesh."
//!
//! Boots two `NetworkManager` instances on ephemeral 127.0.0.1 ports, dials
//! the second from the first via the listen multiaddr (no Kademlia bootstrap
//! since we know each other directly), waits for the gossipsub mesh to form,
//! has one publish a 32-byte sentinel on a shared topic, and asserts the
//! other surfaces it through `drain_events` within a bounded wait window.
//!
//! Run with: `fluxc test --package=flux-p2p --test two_node_gossipsub`
//! Or directly: `cargo test --package flux-p2p --test two_node_gossipsub -- --nocapture`.

use std::time::Duration;

use flux_p2p::{NetworkConfig, NetworkManager, SwarmAppEvent};

const TOPIC: &str = "/flux-p2p-test/gossipsub-receive/v1";

/// Build a default-ish NetworkConfig for the test, listening on the given
/// ephemeral port with the test topic subscribed.
fn config(node_id: &str, port: u16, dial_addr: Option<String>) -> NetworkConfig {
    NetworkConfig {
        node_id: node_id.to_string(),
        listen_addr: format!("/ip4/127.0.0.1/tcp/{port}"),
        bootstrap_peers: dial_addr.into_iter().collect(),
        dagknight_enabled: false,
        sap_enabled: false,
        x_algo_enabled: false,
        entanglement_enabled: false,
        gossipsub_topics: vec![TOPIC.to_string()],
    }
}

/// Two-node receive test. Sender publishes a sentinel; receiver should drain
/// it within 8 s. With the rocky-sigil-86 mesh fix (mesh_n_low=1,
/// mesh_outbound_min=1) and gossipsub-0.47's default flood_publish=true,
/// this should comfortably pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_nodes_deliver_gossipsub_message_via_drain_events() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,flux_p2p=debug,libp2p_gossipsub=debug".into()),
        )
        .with_test_writer()
        .try_init();

    // Use deterministic-but-unlikely-to-collide ports. The test would fail if
    // the port were in use; switching to 0 + post-bind discovery would be the
    // P1 hardening, but for a single-machine local test these are fine.
    let port_a: u16 = 41511;
    let port_b: u16 = 41512;
    let addr_b = format!("/ip4/127.0.0.1/tcp/{port_b}");

    // Node A dials B as its only bootstrap. Note the bootstrap entry is a
    // plain multiaddr without /p2p/<peer_id>, which is enough for the TCP
    // dial — gossipsub does its own protocol negotiation after.
    let mut a = NetworkManager::new(config("node-a", port_a, Some(addr_b.clone())));
    let mut b = NetworkManager::new(config("node-b", port_b, None));

    b.start().await.expect("node B start");
    // Give B a brief moment to actually bind before A dials.
    tokio::time::sleep(Duration::from_millis(200)).await;
    a.start().await.expect("node A start");

    // Wait for both sides to mutually count >=1 peer. The retry loop in
    // NetworkManager::start re-dials every 5s; the mesh is small so libp2p
    // forms it during the first heartbeat after the connection is up.
    let connected = wait_for(Duration::from_secs(8), || {
        a.peer_count() > 0 && b.peer_count() > 0
    })
    .await;
    assert!(
        connected,
        "Peers never mutually connected after 8s — A.peers={}, B.peers={}",
        a.peer_count(), b.peer_count()
    );

    // Brief settle window for the gossipsub Subscribe to round-trip and the
    // mesh to form. Default heartbeat is 1s; ours is 500ms.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // A publishes a sentinel payload that B must surface in drain_events.
    let payload: Vec<u8> = b"flux-p2p-rocky-83-drain-test-sentinel-0001".to_vec();
    a.publish(TOPIC, payload.clone()).expect("publish on A");

    // Poll B's drain_events for up to 8s. If the mesh fix worked, the message
    // should appear within ~1s.
    let mut got: Option<SwarmAppEvent> = None;
    let started = std::time::Instant::now();
    while started.elapsed() < Duration::from_secs(8) {
        for ev in b.drain_events() {
            match &ev {
                SwarmAppEvent::GossipsubMessage { topic, data, .. } => {
                    if topic == TOPIC && data == &payload {
                        got = Some(ev);
                        break;
                    }
                }
                _ => { /* peer-connected events etc. — ignore */ }
            }
        }
        if got.is_some() { break; }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let _ = a.stop().await;
    let _ = b.stop().await;

    assert!(
        got.is_some(),
        "drain_events never surfaced the published sentinel on B within 8s — \
         this is the rocky-83 bug. peer_count_b={}, peer_count_a={}",
        b.peer_count(), a.peer_count()
    );
}

/// Polls `f` every 100ms until it returns true or the timeout expires.
async fn wait_for<F: FnMut() -> bool>(timeout: Duration, mut f: F) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < timeout {
        if f() { return true; }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    f()
}
