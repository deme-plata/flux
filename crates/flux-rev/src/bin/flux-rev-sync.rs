//! flux-rev-sync — propagate flux-rev revisions over a real flux-p2p gossipsub mesh.
//!
//!   flux-rev-sync <dir> --port <P> [--peer <multiaddr>] [--seconds <N>] [--node <id>]
//!
//! Announces this dir's HEAD + object closure on the sync topic, serves objects peers `Want`, and
//! applies (`checkout`) any newer revision it can fully fetch. Two of these on two ports = the
//! 2-node propagation proof; point `--peer` at the other's listen multiaddr.
use flux_rev::sync::{missing, store_verified, Msg, TOPIC};
use flux_rev::{checkout, closure, is_ancestor, snapshot_if_changed, Store};
use flux_p2p::{NetworkConfig, NetworkManager, SwarmAppEvent};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn arg(a: &[String], f: &str) -> Option<String> {
    a.iter().position(|x| x == f).and_then(|i| a.get(i + 1)).cloned()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = a.first().cloned().unwrap_or_else(|| ".".into());
    let work = PathBuf::from(&dir);
    let port: u16 = arg(&a, "--port").and_then(|p| p.parse().ok()).unwrap_or(9100);
    let peer = arg(&a, "--peer");
    let secs: u64 = arg(&a, "--seconds").and_then(|s| s.parse().ok()).unwrap_or(30); // 0 = run forever (daemon)
    let node = arg(&a, "--node").unwrap_or_else(|| format!("rev-{port}"));
    let watch = a.iter().any(|x| x == "--watch"); // auto-snapshot the work dir on change
    let watch_secs: u64 = arg(&a, "--watch-secs").and_then(|s| s.parse().ok()).unwrap_or(3);
    let author = std::env::var("FLUX_REV_AUTHOR").unwrap_or_else(|_| "claude-desktop-viktor".into());

    let store = Store::open(&work).expect("open store");
    let cfg = NetworkConfig {
        node_id: node.clone(),
        listen_addr: format!("/ip4/127.0.0.1/tcp/{port}"),
        bootstrap_peers: peer.into_iter().collect(),
        dagknight_enabled: false,
        sap_enabled: false,
        x_algo_enabled: false,
        entanglement_enabled: false,
        gossipsub_topics: vec![TOPIC.to_string()],
    };
    let mut nm = NetworkManager::new(cfg);
    nm.start().await.expect("start p2p");
    println!("[{node}] flux-rev-sync up on :{port} · HEAD={}", store.read_head().unwrap_or_else(|| "∅".into()));

    // What we're trying to fetch+apply (a peer's announced head we don't yet hold fully).
    let mut target: Option<(String, Vec<String>)> = None;
    let mut applied_from_peer = false;

    if watch { println!("[{node}] 👁 watch: auto-snapshot every {watch_secs}s (author {author})"); }
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut last_announce = Instant::now() - Duration::from_secs(10);
    let mut last_watch = Instant::now();

    while secs == 0 || Instant::now() < deadline {
        // (0) daemon: auto-snapshot the working tree if it changed → HEAD advances → propagated
        if watch && last_watch.elapsed() > Duration::from_secs(watch_secs) {
            if let Some(h) = store.read_head() {
                if let Ok(r) = store.get_revision(&h) {
                    if let Ok(Some(rev)) = snapshot_if_changed(&work, &store, &r.genesis, &r.workspace_version, &author, "auto-snapshot") {
                        println!("[{node}] ✎ auto-snapshot {} — HEAD advanced, announcing", &rev.id[..16]);
                        last_announce = Instant::now() - Duration::from_secs(10); // announce immediately
                    }
                }
            }
            last_watch = Instant::now();
        }

        // (1) periodically announce our own HEAD + closure
        if last_announce.elapsed() > Duration::from_secs(2) {
            if let Some(head) = store.read_head() {
                if let Ok(clos) = closure(&store, &head) {
                    let ver = store.get_revision(&head).map(|r| r.workspace_version).unwrap_or_default();
                    let _ = nm.publish(TOPIC, Msg::Announce { head, ver, closure: clos }.encode());
                }
            }
            last_announce = Instant::now();
        }

        // (2) handle inbound
        for ev in nm.drain_events() {
            if let SwarmAppEvent::GossipsubMessage { topic, data, .. } = ev {
                if topic != TOPIC { continue; }
                let Some(msg) = Msg::decode(&data) else { continue };
                match msg {
                    Msg::Announce { head, ver, closure } => {
                        if Some(&head) == store.read_head().as_ref() { continue; } // already at it
                        let miss = missing(&store, &closure);
                        if miss.is_empty() {
                            // we hold everything → apply if it moves HEAD forward
                            if maybe_apply(&store, &work, &head, &node) { applied_from_peer = true; }
                        } else {
                            println!("[{node}] ← announce {} (v{ver}) · missing {} object(s) → want", &head[..16], miss.len());
                            target = Some((head, closure));
                            let _ = nm.publish(TOPIC, Msg::Want { hashes: miss }.encode());
                        }
                    }
                    Msg::Want { hashes } => {
                        for h in hashes {
                            if let Ok(bytes) = store.get(&h) {
                                let _ = nm.publish(TOPIC, Msg::Have { hash: h, hex: hex::encode(bytes) }.encode());
                            }
                        }
                    }
                    Msg::Have { hash, hex } => {
                        if let Ok(bytes) = hex::decode(&hex) {
                            if store_verified(&store, &hash, &bytes) {
                                // did this complete the target closure?
                                if let Some((head, clos)) = target.clone() {
                                    if missing(&store, &clos).is_empty() {
                                        if maybe_apply(&store, &work, &head, &node) { applied_from_peer = true; }
                                        target = None;
                                    }
                                }
                            } else {
                                println!("[{node}] ⚠ rejected tampered object {}", &hash[..16.min(hash.len())]);
                            }
                        }
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    println!("[{node}] done · HEAD={} · applied_from_peer={}", store.read_head().unwrap_or_else(|| "∅".into()), applied_from_peer);
}

/// Apply a revision ONLY if it moves HEAD forward (candidate is at-or-ahead of current HEAD).
/// Prevents flapping back to an ancestor when announces arrive out of order. Returns true if applied.
fn maybe_apply(store: &Store, work: &PathBuf, head: &str, node: &str) -> bool {
    let forward = match store.read_head() {
        None => true,
        Some(h) if h == head => false,                  // already at it
        Some(h) => is_ancestor(store, &h, head),         // candidate descends from current HEAD
    };
    if !forward { return false; }
    match checkout(store, head, work) {
        Ok(n) => { let _ = store.write_head(head); println!("[{node}] ✅ APPLIED rev {} → {} files (HEAD forward)", &head[..16], n); true }
        Err(e) => { println!("[{node}] ✗ checkout {}: {e}", &head[..16]); false }
    }
}
