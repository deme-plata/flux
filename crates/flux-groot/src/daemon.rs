// The reference GR00T control daemon.
//
// Implements exactly the endpoints `spec.rs` declares, over a dependency-free
// std HTTP/1.1 loop (one connection per request, Connection: close — the same
// shape flux-api's middleware smoke tests already prove clients against).
//
// This is the TRUST BOUNDARY, not the robot: envelope verification, nonce
// burn, operator allowlist, estop latch, and the pay-per-command receipt
// ledger all live here. Actuation is a mock 75-DoF state vector; swapping it
// for ROS 2 / Isaac middleware calls changes nothing on the wire.

use crate::envelope::{ReplayGuard, SignedCommand};
use crate::spec::{BODY_DOF, HAND_DOF};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Cost per accepted command, in micro-QUG (1e-6 QUG). Settlement happens
/// out-of-band — the daemon only keeps the ledger.
fn cost_micro_qug(kind: &str) -> u64 {
    match kind {
        "task" => 50_000,   // 0.05 QUG — a policy invocation is the expensive path
        "joints" => 10_000, // 0.01 QUG
        "hand" => 10_000,   // 0.01 QUG
        _ => 0,             // halt / clear_estop — never charge for stopping
    }
}

#[derive(Clone)]
pub struct DaemonConfig {
    /// 0 = ephemeral port (tests); the bound port is reported on `Daemon`.
    pub port: u16,
    /// Wallets allowed to MOVE the robot. Estop needs no entry here.
    pub operators: HashSet<[u8; 32]>,
    /// SSE frames served per telemetry connection (bounded so clients and
    /// tests terminate; a production build would stream until disconnect).
    pub telemetry_frames: usize,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self { port: 0, operators: HashSet::new(), telemetry_frames: 5 }
    }
}

#[derive(serde::Serialize, Clone)]
struct Receipt {
    seq: u64,
    wallet: String,
    kind: String,
    nonce: u64,
    timestamp: u64,
    command_hash: String,
    cost_qug: String,
}

struct RobotState {
    body_joints: [f64; BODY_DOF],
    left_hand: [f64; HAND_DOF],
    right_hand: [f64; HAND_DOF],
    estopped: bool,
    task: Option<String>,
    receipts: Vec<Receipt>,
    guard: ReplayGuard,
    commands_accepted: u64,
    owed_micro_qug: u64,
}

impl RobotState {
    fn new() -> Self {
        Self {
            body_joints: [0.0; BODY_DOF],
            left_hand: [0.0; HAND_DOF],
            right_hand: [0.0; HAND_DOF],
            estopped: false,
            task: None,
            receipts: Vec::new(),
            guard: ReplayGuard::default(),
            commands_accepted: 0,
            owed_micro_qug: 0,
        }
    }
}

pub struct Daemon {
    pub port: u16,
    state: Arc<Mutex<RobotState>>,
    stop: Arc<AtomicBool>,
}

impl Daemon {
    /// Bind, spawn the accept loop, return immediately.
    pub fn spawn(cfg: DaemonConfig) -> std::io::Result<Daemon> {
        let listener = TcpListener::bind(("127.0.0.1", cfg.port))?;
        let port = listener.local_addr()?.port();
        let state = Arc::new(Mutex::new(RobotState::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let st = Arc::clone(&state);
        let sp = Arc::clone(&stop);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if sp.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let st = Arc::clone(&st);
                let cfg = cfg.clone();
                std::thread::spawn(move || handle(stream, st, cfg));
            }
        });

        Ok(Daemon { port, state, stop })
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        // Kick the accept loop awake so the flag is observed.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }

    /// Test/ops hook: total accepted commands + micro-QUG owed.
    pub fn ledger_totals(&self) -> (u64, u64) {
        let s = self.state.lock().expect("robot state poisoned");
        (s.commands_accepted, s.owed_micro_qug)
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn handle(mut stream: TcpStream, state: Arc<Mutex<RobotState>>, cfg: DaemonConfig) {
    let Some((method, path, body)) = read_request(&mut stream) else { return };
    let (route, query) = match path.split_once('?') {
        Some((r, q)) => (r, q),
        None => (path.as_str(), ""),
    };

    match (method.as_str(), route) {
        ("GET", "/v1/robot/state") => {
            let s = state.lock().expect("robot state poisoned");
            let json = serde_json::json!({
                "body_joints": s.body_joints.to_vec(),
                "left_hand": s.left_hand.to_vec(),
                "right_hand": s.right_hand.to_vec(),
                "estopped": s.estopped,
                "task": s.task,
                "commands_accepted": s.commands_accepted,
                "qug_owed": format_qug(s.owed_micro_qug),
            });
            respond_json(&mut stream, 200, &json.to_string());
        }
        ("POST", "/v1/robot/command") => {
            let json = handle_command(&body, &state, &cfg);
            let status = if json.get("error").is_some() { 403 } else { 200 };
            respond_json(&mut stream, status, &json.to_string());
        }
        ("GET", "/v1/robot/receipts") => {
            let after: u64 = query_param(query, "after").and_then(|v| v.parse().ok()).unwrap_or(0);
            let limit: usize = query_param(query, "limit").and_then(|v| v.parse().ok()).unwrap_or(3);
            let s = state.lock().expect("robot state poisoned");
            let page: Vec<&Receipt> =
                s.receipts.iter().filter(|r| r.seq > after).take(limit.max(1)).collect();
            let next = match page.last() {
                Some(last) if (last.seq as usize) < s.receipts.len() => {
                    Some(last.seq.to_string())
                }
                _ => None,
            };
            let json = serde_json::json!({ "data": page, "next_cursor": next });
            respond_json(&mut stream, 200, &json.to_string());
        }
        ("GET", "/v1/robot/telemetry") => {
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(head.as_bytes());
            for i in 0..cfg.telemetry_frames {
                let frame = {
                    let s = state.lock().expect("robot state poisoned");
                    serde_json::json!({
                        "frame": i,
                        "estopped": s.estopped,
                        "task": s.task,
                        "body_joints": s.body_joints.to_vec(),
                    })
                };
                let _ = stream.write_all(format!("data: {frame}\n\n").as_bytes());
                let _ = stream.flush();
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let _ = stream.write_all(b"data: [DONE]\n\n");
        }
        ("POST", "/v1/robot/estop") => {
            // Unsigned BY DESIGN — see spec.rs. Latch and acknowledge.
            let mut s = state.lock().expect("robot state poisoned");
            s.estopped = true;
            s.task = None;
            respond_json(&mut stream, 200, r#"{"estopped":true}"#);
        }
        _ => respond_json(&mut stream, 404, r#"{"error":"no such route"}"#),
    }
}

fn handle_command(
    body: &str,
    state: &Arc<Mutex<RobotState>>,
    cfg: &DaemonConfig,
) -> serde_json::Value {
    let cmd: SignedCommand = match serde_json::from_str(body) {
        Ok(c) => c,
        Err(e) => return serde_json::json!({"error": format!("bad envelope JSON: {e}")}),
    };

    let mut s = state.lock().expect("robot state poisoned");

    // 1. Cryptographic verification + nonce burn (signature checked first).
    if let Err(e) = s.guard.verify_and_burn(&cmd, now_secs()) {
        return serde_json::json!({"error": e.to_string()});
    }

    // 2. Operator allowlist — motion is permissioned even with a valid key.
    let wallet = crate::envelope::wallet_bytes(&cmd.wallet).expect("verified wallet re-parses");
    if !cfg.operators.contains(&wallet) {
        return serde_json::json!({"error": format!("wallet {} is not an authorized operator", cmd.wallet)});
    }

    let kind = cmd.command.get("kind").and_then(|k| k.as_str()).unwrap_or("").to_string();

    // 3. Estop latch: while latched, only clear_estop/halt are accepted.
    if s.estopped && kind != "clear_estop" && kind != "halt" {
        return serde_json::json!({"error": "estop latched: motion refused until signed clear_estop"});
    }

    // 4. Dispatch (mock actuation).
    match kind.as_str() {
        "task" => {
            let Some(instruction) = cmd.command.get("instruction").and_then(|v| v.as_str()) else {
                return serde_json::json!({"error": "task command requires string field `instruction`"});
            };
            s.task = Some(instruction.to_string());
        }
        "joints" => {
            let Some(targets) = cmd.command.get("targets").and_then(|v| v.as_array()) else {
                return serde_json::json!({"error": "joints command requires array field `targets`"});
            };
            if targets.len() != BODY_DOF {
                return serde_json::json!({"error": format!("joints requires exactly {BODY_DOF} targets, got {}", targets.len())});
            }
            for (i, t) in targets.iter().enumerate() {
                s.body_joints[i] = t.as_f64().unwrap_or(0.0);
            }
        }
        "hand" => {
            let side = cmd.command.get("side").and_then(|v| v.as_str()).unwrap_or("");
            let Some(targets) = cmd.command.get("targets").and_then(|v| v.as_array()) else {
                return serde_json::json!({"error": "hand command requires array field `targets`"});
            };
            if targets.len() != HAND_DOF {
                return serde_json::json!({"error": format!("hand requires exactly {HAND_DOF} targets, got {}", targets.len())});
            }
            let hand = match side {
                "left" => &mut s.left_hand,
                "right" => &mut s.right_hand,
                _ => return serde_json::json!({"error": "hand command requires side = left|right"}),
            };
            for (i, t) in targets.iter().enumerate() {
                hand[i] = t.as_f64().unwrap_or(0.0);
            }
        }
        "halt" => {
            s.task = None;
        }
        "clear_estop" => {
            s.estopped = false;
        }
        other => return serde_json::json!({"error": format!("unknown command kind {other:?}")}),
    }

    // 5. Receipt — the settlement ledger row.
    let cost = cost_micro_qug(&kind);
    s.commands_accepted += 1;
    s.owed_micro_qug += cost;
    let seq = s.receipts.len() as u64 + 1;
    let receipt = Receipt {
        seq,
        wallet: cmd.wallet.clone(),
        kind: kind.clone(),
        nonce: cmd.nonce,
        timestamp: cmd.timestamp,
        command_hash: blake3::hash(cmd.command.to_string().as_bytes()).to_hex().to_string(),
        cost_qug: format_qug(cost),
    };
    s.receipts.push(receipt.clone());

    serde_json::json!({"accepted": true, "receipt": receipt})
}

fn format_qug(micro: u64) -> String {
    format!("{}.{:06}", micro / 1_000_000, micro % 1_000_000)
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then_some(v)
    })
}

/// Read one HTTP/1.1 request: (method, path, body). Body read honours
/// Content-Length; no chunked support (none of our clients emit it).
fn read_request(stream: &mut TcpStream) -> Option<(String, String, String)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end;
    loop {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_header_end(&buf) {
            header_end = pos;
            break;
        }
        if buf.len() > 1_048_576 {
            return None; // header flood
        }
    }
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let mut req = lines.next()?.split_whitespace();
    let method = req.next()?.to_string();
    let path = req.next()?.to_string();
    let content_length: usize = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse().ok())
        .unwrap_or(0);

    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);
    Some((method, path, String::from_utf8_lossy(&body).to_string()))
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn respond_json(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Error",
    };
    let _ = stream.write_all(
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    );
    let _ = stream.flush();
}
