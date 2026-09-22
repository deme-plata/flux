// End-to-end: a Quillon-style wallet key drives the live daemon over a real
// TCP socket. Asserts on OBSERVED state transitions and wire responses — not
// on strings in generated code (the flux-db lesson: outcomes, not timing).

use ed25519_dalek::SigningKey;
use flux_groot::{sign_command, Daemon, DaemonConfig};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::TcpStream;

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, serde_json::Value) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect daemon");
    let b = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
        b.len()
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    let status: u16 = resp
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .expect("status line");
    let body_json = resp
        .split("\r\n\r\n")
        .nth(1)
        .and_then(|b| serde_json::from_str(b).ok())
        .unwrap_or(serde_json::Value::Null);
    (status, body_json)
}

fn operator_key() -> SigningKey {
    SigningKey::from_bytes(&[42u8; 32])
}

fn spawn_with_operator() -> Daemon {
    let mut operators = HashSet::new();
    operators.insert(operator_key().verifying_key().to_bytes());
    Daemon::spawn(DaemonConfig { port: 0, operators, telemetry_frames: 3 }).expect("spawn daemon")
}

#[test]
fn signed_task_command_executes_and_pays() {
    let d = spawn_with_operator();
    let key = operator_key();

    let cmd = serde_json::json!({"kind": "task", "instruction": "pick up the red cube"});
    let sc = sign_command(&key, 1, now(), &cmd);
    let (status, resp) = http(d.port, "POST", "/v1/robot/command", Some(&serde_json::to_string(&sc).unwrap()));
    assert_eq!(status, 200, "command rejected: {resp}");
    assert_eq!(resp["accepted"], true);
    assert_eq!(resp["receipt"]["kind"], "task");
    assert_eq!(resp["receipt"]["cost_qug"], "0.050000");

    // The state actually changed — the instruction is live.
    let (_, state) = http(d.port, "GET", "/v1/robot/state", None);
    assert_eq!(state["task"], "pick up the red cube");
    assert_eq!(state["commands_accepted"], 1);
    assert_eq!(state["qug_owed"], "0.050000");

    // Ledger hook agrees.
    assert_eq!(d.ledger_totals(), (1, 50_000));
    d.stop();
}

#[test]
fn joints_command_moves_exactly_31_dof() {
    let d = spawn_with_operator();
    let key = operator_key();

    // Wrong arity is refused.
    let bad = sign_command(&key, 1, now(), &serde_json::json!({"kind":"joints","targets":[0.1, 0.2]}));
    let (status, resp) = http(d.port, "POST", "/v1/robot/command", Some(&serde_json::to_string(&bad).unwrap()));
    assert_eq!(status, 403);
    assert!(resp["error"].as_str().unwrap().contains("31"), "{resp}");

    // Correct arity lands, and the state vector reflects it.
    let targets: Vec<f64> = (0..31).map(|i| i as f64 / 10.0).collect();
    let good = sign_command(&key, 2, now(), &serde_json::json!({"kind":"joints","targets":targets}));
    let (status, _) = http(d.port, "POST", "/v1/robot/command", Some(&serde_json::to_string(&good).unwrap()));
    assert_eq!(status, 200);
    let (_, state) = http(d.port, "GET", "/v1/robot/state", None);
    assert_eq!(state["body_joints"][30], 3.0);
    d.stop();
}

#[test]
fn unauthorized_wallet_is_refused_even_with_valid_signature() {
    let d = spawn_with_operator();
    // A DIFFERENT key — valid crypto, not on the allowlist.
    let stranger = SigningKey::from_bytes(&[13u8; 32]);
    let sc = sign_command(&stranger, 1, now(), &serde_json::json!({"kind":"task","instruction":"obey me"}));
    let (status, resp) = http(d.port, "POST", "/v1/robot/command", Some(&serde_json::to_string(&sc).unwrap()));
    assert_eq!(status, 403);
    assert!(resp["error"].as_str().unwrap().contains("not an authorized operator"), "{resp}");
    d.stop();
}

#[test]
fn wire_replay_is_rejected() {
    let d = spawn_with_operator();
    let key = operator_key();
    let sc = sign_command(&key, 1, now(), &serde_json::json!({"kind":"task","instruction":"wave"}));
    let body = serde_json::to_string(&sc).unwrap();

    let (s1, _) = http(d.port, "POST", "/v1/robot/command", Some(&body));
    assert_eq!(s1, 200);
    // Byte-identical resend — a captured envelope must be worthless.
    let (s2, resp) = http(d.port, "POST", "/v1/robot/command", Some(&body));
    assert_eq!(s2, 403);
    assert!(resp["error"].as_str().unwrap().contains("replay"), "{resp}");
    // Only ONE receipt was paid.
    assert_eq!(d.ledger_totals().0, 1);
    d.stop();
}

#[test]
fn estop_is_unsigned_latches_and_blocks_motion_until_signed_clear() {
    let d = spawn_with_operator();
    let key = operator_key();

    // Anyone can estop — no signature, no body.
    let (status, resp) = http(d.port, "POST", "/v1/robot/estop", None);
    assert_eq!(status, 200);
    assert_eq!(resp["estopped"], true);

    // Motion is now refused even for the authorized operator.
    let mv = sign_command(&key, 1, now(), &serde_json::json!({"kind":"task","instruction":"keep going"}));
    let (status, resp) = http(d.port, "POST", "/v1/robot/command", Some(&serde_json::to_string(&mv).unwrap()));
    assert_eq!(status, 403);
    assert!(resp["error"].as_str().unwrap().contains("estop"), "{resp}");

    // Only a SIGNED clear_estop un-latches.
    let clear = sign_command(&key, 2, now(), &serde_json::json!({"kind":"clear_estop"}));
    let (status, _) = http(d.port, "POST", "/v1/robot/command", Some(&serde_json::to_string(&clear).unwrap()));
    assert_eq!(status, 200);
    let (_, state) = http(d.port, "GET", "/v1/robot/state", None);
    assert_eq!(state["estopped"], false);

    // And motion works again.
    let mv2 = sign_command(&key, 3, now(), &serde_json::json!({"kind":"task","instruction":"resume"}));
    let (status, _) = http(d.port, "POST", "/v1/robot/command", Some(&serde_json::to_string(&mv2).unwrap()));
    assert_eq!(status, 200);
    d.stop();
}

#[test]
fn receipts_paginate_with_cursor_until_exhausted() {
    let d = spawn_with_operator();
    let key = operator_key();

    // 7 accepted commands → 7 receipts; page size 3 → pages of 3/3/1.
    for n in 1..=7u64 {
        let sc = sign_command(&key, n, now(), &serde_json::json!({"kind":"task","instruction":format!("step {n}")}));
        let (status, _) = http(d.port, "POST", "/v1/robot/command", Some(&serde_json::to_string(&sc).unwrap()));
        assert_eq!(status, 200);
    }

    let mut cursor: Option<String> = None;
    let mut pages = 0;
    let mut seqs: Vec<u64> = vec![];
    loop {
        let path = match &cursor {
            Some(c) => format!("/v1/robot/receipts?after={c}&limit=3"),
            None => "/v1/robot/receipts?limit=3".to_string(),
        };
        let (status, page) = http(d.port, "GET", &path, None);
        assert_eq!(status, 200);
        pages += 1;
        for r in page["data"].as_array().unwrap() {
            seqs.push(r["seq"].as_u64().unwrap());
        }
        match page["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
        assert!(pages < 10, "cursor never terminated");
    }
    assert_eq!(pages, 3, "expected 3 pages of 3/3/1");
    assert_eq!(seqs, vec![1, 2, 3, 4, 5, 6, 7], "receipts lost or reordered across pages");
    d.stop();
}

#[test]
fn telemetry_streams_sse_frames() {
    let d = spawn_with_operator();
    let mut s = TcpStream::connect(("127.0.0.1", d.port)).unwrap();
    s.write_all(b"GET /v1/robot/telemetry HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    assert!(resp.contains("text/event-stream"));
    let frames: Vec<&str> = resp.matches("data: {").collect();
    assert_eq!(frames.len(), 3, "expected 3 telemetry frames:\n{resp}");
    assert!(resp.contains("data: [DONE]"));
    d.stop();
}
