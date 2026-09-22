// SDK-generation proof: the TypeScript client that flux-api EMITS from
// `groot_endpoints()` is compiled by a real tsc and executed by a real node
// against the LIVE daemon. The generated cursor-paginator walks the receipt
// ledger and the generated SSE reader consumes telemetry — so the v0.15-B
// emitters are proven against a genuine server, not a hand-built mock.
//
// Toolchain: FLUX_TSC / FLUX_NODE env overrides (Epsilon keeps node off the
// global PATH); soft-skips when absent, same convention as flux-api's own
// middleware_smoke tests.

use ed25519_dalek::SigningKey;
use flux_groot::{groot_endpoints, groot_schemas, sign_command, Daemon, DaemonConfig};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::TcpStream;

fn resolve_tool(bin: &str, env_var: &str) -> Option<String> {
    if let Ok(p) = std::env::var(env_var) {
        if !p.is_empty() && std::path::Path::new(&p).exists() {
            return Some(p);
        }
    }
    let out = std::process::Command::new("which").arg(bin).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

fn post_command(port: u16, body: &str) -> u16 {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let req = format!(
        "POST /v1/robot/command HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    resp.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0)
}

#[test]
fn generated_ts_client_walks_receipts_and_reads_telemetry_live() {
    let Some(tsc) = resolve_tool("tsc", "FLUX_TSC") else {
        println!("[skip] tsc unavailable — set FLUX_TSC (generated-client live test skipped)");
        return;
    };
    let Some(node) = resolve_tool("node", "FLUX_NODE") else {
        println!("[skip] node unavailable — set FLUX_NODE");
        return;
    };

    // Live daemon with 5 accepted commands on the ledger and 3 SSE frames.
    let key = SigningKey::from_bytes(&[42u8; 32]);
    let mut operators = HashSet::new();
    operators.insert(key.verifying_key().to_bytes());
    let d = Daemon::spawn(DaemonConfig { port: 0, operators, telemetry_frames: 3 }).unwrap();
    for n in 1..=5u64 {
        let sc = sign_command(&key, n, now(), &serde_json::json!({"kind":"task","instruction":format!("step {n}")}));
        assert_eq!(post_command(d.port, &serde_json::to_string(&sc).unwrap()), 200);
    }

    // Emit the SDK from the spec — the exact artifact a consumer would get.
    let sdk = flux_api::generate_typescript_sdk_with_types(
        &groot_endpoints(),
        &d.base_url(),
        &groot_schemas(),
    );
    assert!(sdk.contains("export class GrootClient"), "missing client class:\n{sdk}");

    let dir = std::env::temp_dir().join(format!("flux_groot_sdk_{}_{}", std::process::id(), now()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("client.ts"), &sdk).unwrap();
    std::fs::write(
        dir.join("driver.ts"),
        r#"
import { GrootClient } from "./client.js";
const c = new GrootClient();
// 1. Generated cursor-paginator walks the live ledger (pages of 3 → 3+2).
const seqs: number[] = [];
for await (const r of c.listRobotReceiptsIterItems()) seqs.push(r.seq);
// 2. Generated SSE reader consumes live telemetry frames.
const frames: number[] = [];
for await (const ev of c.streamRobotTelemetryStream()) frames.push(ev.frame);
// 3. Plain generated method reads state.
const state = await c.getRobotState();
console.log(JSON.stringify({ seqs, frames, task: state.task, accepted: state.commands_accepted }));
"#,
    )
    .unwrap();
    std::fs::write(dir.join("package.json"), r#"{"type":"module"}"#).unwrap();

    // tsc shim needs node on PATH.
    let node_dir = std::path::Path::new(&node).parent().unwrap().display().to_string();
    let path_env = match std::env::var("PATH") {
        Ok(p) => format!("{node_dir}:{p}"),
        Err(_) => node_dir,
    };
    let out = std::process::Command::new(&tsc)
        .env("PATH", &path_env)
        .args(["--target", "ES2022", "--module", "ES2022", "--moduleResolution", "node", "--strict", "false", "--skipLibCheck"])
        .arg(dir.join("client.ts"))
        .arg(dir.join("driver.ts"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "tsc rejected the generated SDK:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let run = std::process::Command::new(&node).arg(dir.join("driver.js")).output().unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(run.status.success(), "driver failed:\n{stdout}\n{}", String::from_utf8_lossy(&run.stderr));

    let got: serde_json::Value = serde_json::from_str(stdout.trim()).expect("driver JSON");
    assert_eq!(got["seqs"], serde_json::json!([1, 2, 3, 4, 5]), "paginator lost receipts: {got}");
    assert_eq!(got["frames"], serde_json::json!([0, 1, 2]), "SSE reader lost frames: {got}");
    assert_eq!(got["task"], "step 5");
    assert_eq!(got["accepted"], 5);

    let _ = std::fs::remove_dir_all(&dir);
    d.stop();
}

#[test]
fn openapi_and_all_five_sdks_generate_nonempty() {
    let eps = groot_endpoints();
    let defs = groot_schemas();
    let spec = flux_api::generate_openapi_with_schemas("GR00T", "0.1", &eps, &defs);
    assert!(spec["paths"]["/v1/robot/command"]["post"].is_object());
    assert!(spec["components"]["schemas"]["SignedCommand"].is_object());

    let ts = flux_api::generate_typescript_sdk_with_types(&eps, "http://r", &defs);
    assert!(ts.contains("listRobotReceiptsIterPages") && ts.contains("streamRobotTelemetryStream"));
    let py = flux_api::generate_python_sdk_with_types(&eps, "http://r", &defs);
    assert!(py.contains("list_robot_receipts_iter_pages") && py.contains("stream_robot_telemetry_stream"));
    assert!(!flux_api::generate_go_sdk(&eps, "http://r", "groot").is_empty());
    assert!(!flux_api::generate_rust_client_sdk(&eps, "http://r").is_empty());
    assert!(!flux_api::generate_kotlin_sdk(&eps, "http://r", "groot").is_empty());
}
