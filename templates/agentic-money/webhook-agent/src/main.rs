//! webhook-agent — an event-driven money agent. It listens for webhook POSTs
//! and, on each event, runs a gated money action. Non-blocking by design: the
//! agent sleeps at zero cost until something happens, then reacts.
//!
//! This is the std-only, self-contained twin of the `flux_webhook_register`
//! swarm pattern — a tiny HTTP listener you can point any event source at
//! (a price oracle, a swarm broadcast bridge, a cron curl). Every event still
//! goes through the Verified Execution Gate before it can move funds.
//!
//! Run:    webhook-agent [LISTEN_ADDR] [RPC_URL]
//!   e.g.  webhook-agent 127.0.0.1:8777 http://127.0.0.1:8099
//! Fire:   curl -s -XPOST 127.0.0.1:8777/event \
//!           -d '{"dir":"AtoB","amount_in":500}'

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use agentic_money_kit::gate::{evaluate, Decision, GateConfig, Verdict};
use agentic_money_kit::Rpc;

const TRADER: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const POOL: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const USDS: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const WQUG: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn main() {
    let mut args = std::env::args().skip(1);
    let listen = args.next().unwrap_or_else(|| "127.0.0.1:8777".into());
    let rpc_url = args.next().unwrap_or_else(|| "http://127.0.0.1:8099".into());

    let rpc = Rpc::new(&rpc_url);
    let cfg = GateConfig::default();

    let server = TcpListener::bind(&listen).unwrap_or_else(|e| {
        eprintln!("bind {listen} failed: {e}");
        std::process::exit(1);
    });
    println!("webhook-agent listening on http://{listen}  → chain {rpc_url}");
    println!("fire an event:  curl -XPOST {listen}/event -d '{{\"dir\":\"AtoB\",\"amount_in\":500}}'");

    for conn in server.incoming() {
        match conn {
            Ok(stream) => handle(stream, &rpc, &cfg),
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
}

fn handle(mut stream: TcpStream, rpc: &Rpc, cfg: &GateConfig) {
    let mut buf = [0u8; 8192];
    let n = match stream.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return,
    };
    let req = String::from_utf8_lossy(&buf[..n]);
    let body = req.splitn(2, "\r\n\r\n").nth(1).unwrap_or("").trim_end_matches('\0');

    let reply = match serde_json::from_str::<serde_json::Value>(body) {
        Ok(ev) => react(rpc, cfg, &ev),
        Err(_) => "{\"ok\":false,\"error\":\"bad json event\"}".to_string(),
    };
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.len(),
        reply
    );
    let _ = stream.write_all(resp.as_bytes());
}

/// Turn one webhook event into a gated money action.
fn react(rpc: &Rpc, cfg: &GateConfig, ev: &serde_json::Value) -> String {
    let dir = ev.get("dir").and_then(|v| v.as_str()).unwrap_or("AtoB").to_string();
    let amount = ev.get("amount_in").and_then(|v| v.as_u64()).unwrap_or(500) as u128;
    let (token_in, token_out) = if dir == "AtoB" { ("USDS", "WQUG") } else { ("WQUG", "USDS") };
    let decision = Decision { dir: dir.clone(), amount_in: amount, token_in: token_in.into(), token_out: token_out.into() };

    let (ra, rb) = rpc.pool_reserves(0).unwrap_or((0, 0));
    let (bal_in, res_in, res_out) = if dir == "AtoB" {
        (rpc.balance(TRADER, USDS), ra, rb)
    } else {
        (rpc.balance(TRADER, WQUG), rb, ra)
    };

    match evaluate(cfg, &decision, bal_in, res_in, res_out) {
        Verdict::Reject(reason) => {
            println!("🚫 event gated out: {reason}");
            format!("{{\"ok\":false,\"gated\":true,\"reason\":\"{reason}\"}}")
        }
        Verdict::Approve(d) => match rpc.swap(TRADER, POOL, &d.dir, d.amount_in, 1) {
            Ok(resp) => {
                println!("✅ event → {} {}  on-chain: {}", d.dir, d.amount_in, resp.trim());
                format!("{{\"ok\":true,\"dir\":\"{}\",\"amount_in\":{}}}", d.dir, d.amount_in)
            }
            Err(e) => format!("{{\"ok\":false,\"error\":\"transport {e}\"}}"),
        },
    }
}
