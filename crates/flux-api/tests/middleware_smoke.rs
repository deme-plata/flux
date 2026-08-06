// v0.15-D: behavioural smoke tests for generated middleware.
//
// The generator unit tests assert on *strings* — that the emitted source
// contains a retry loop, a cursor param, a stream reader. That is necessary but
// nowhere near sufficient: a generator can emit beautifully-shaped code that
// pages the wrong URL, drops the last page, or silently loses SSE frames that
// straddle a TCP read. (flux-db passed 105/105 unit tests for its whole life
// while destroying terabytes, because the probes timed `get()` without ever
// asserting the value came back.)
//
// So these tests run the generated TypeScript for real: `tsc` transpiles it,
// `node` executes it against a hand-rolled mock HTTP server in this process,
// and we assert on what actually crossed the socket.
//
// Toolchain discovery mirrors `sdk_compile_each.rs` — env override first
// (`FLUX_TSC`, `FLUX_NODE`), then PATH. Epsilon keeps node off the global PATH
// on purpose, so without the override these soft-skip rather than fail.

use flux_api::{
    generate_typescript_sdk, ApiEndpoint, ApiResponse, HttpMethod, MiddlewareSpec,
    PaginationStyle, RetryPolicy, StreamKind,
};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------- mock server

/// A one-connection-per-request HTTP/1.1 mock. Every response closes the
/// connection, which keeps undici (node's fetch) from holding a keep-alive
/// socket open past the end of the test.
struct Mock {
    port: u16,
    /// Request heads in arrival order, so tests can assert on attempt counts,
    /// query strings and headers.
    requests: Arc<Mutex<Vec<String>>>,
}

impl Mock {
    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("mock request log poisoned").clone()
    }
    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

/// Spawn the mock. `responder` receives the zero-based request index and the
/// request head, and writes the raw response itself — taking the stream rather
/// than returning bytes so a test can deliberately split a write to simulate
/// chunk boundaries.
fn spawn_mock<F>(responder: F) -> Mock
where
    F: Fn(usize, &str, &mut TcpStream) + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
    let port = listener.local_addr().expect("mock addr").port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&requests);
    let counter = AtomicUsize::new(0);

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let head = match read_head(&stream) {
                Some(h) => h,
                None => continue,
            };
            let idx = counter.fetch_add(1, Ordering::SeqCst);
            log.lock().expect("mock log poisoned").push(head.clone());
            responder(idx, &head, &mut stream);
            let _ = stream.flush();
        }
    });

    Mock { port, requests }
}

/// Read request headers up to the blank line. We never need the body — every
/// generated surface under test is a GET.
fn read_head(stream: &TcpStream) -> Option<String> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut head = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if line == "\r\n" || line == "\n" {
            return Some(head);
        }
        head.push_str(&line);
    }
}

fn json_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn status_response(code: u16, reason: &str) -> String {
    format!("HTTP/1.1 {code} {reason}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}")
}

// ------------------------------------------------------------- node toolchain

fn which(bin: &str) -> Option<String> {
    let out = std::process::Command::new("which").arg(bin).output().ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    })
}

fn resolve_tool(bin: &str, env_var: &str) -> Option<String> {
    if let Ok(p) = std::env::var(env_var) {
        if !p.is_empty() && std::path::Path::new(&p).exists() {
            return Some(p);
        }
    }
    which(bin)
}

/// Both tools, or `None` so the caller can soft-skip with one message.
fn node_toolchain() -> Option<(String, String)> {
    Some((resolve_tool("tsc", "FLUX_TSC")?, resolve_tool("node", "FLUX_NODE")?))
}

/// Transpile the generated client, drop in a driver, run it under node and
/// return whatever the driver printed on stdout.
///
/// The driver is written as ESM against the emitted `client.js`; `type: module`
/// in package.json is what makes node treat tsc's `.js` output as ESM.
fn run_driver(sdk: &str, driver_ts: &str, tsc: &str, node: &str) -> String {
    let dir = std::env::temp_dir().join(format!(
        "flux_api_mw_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("create driver dir");
    std::fs::write(dir.join("client.ts"), sdk).expect("write client.ts");
    std::fs::write(dir.join("driver.ts"), driver_ts).expect("write driver.ts");
    std::fs::write(dir.join("package.json"), r#"{"type":"module"}"#).expect("write package.json");

    // `tsc` is a `#!/usr/bin/env node` shim, so it needs node reachable on PATH
    // even when we invoke it by absolute path. Epsilon deliberately keeps node
    // off the global PATH, so inject its directory rather than depend on
    // whatever the caller happened to export.
    let mut tsc_cmd = std::process::Command::new(tsc);
    if let Some(node_dir) = std::path::Path::new(node).parent() {
        let path = match std::env::var("PATH") {
            Ok(p) => format!("{}:{p}", node_dir.display()),
            Err(_) => node_dir.display().to_string(),
        };
        tsc_cmd.env("PATH", path);
    }
    let out = tsc_cmd
        .args([
            "--target", "ES2022",
            "--module", "ES2022",
            "--moduleResolution", "node",
            "--strict", "false",
            "--skipLibCheck",
        ])
        .arg(dir.join("client.ts"))
        .arg(dir.join("driver.ts"))
        .output()
        .expect("spawn tsc");
    assert!(
        out.status.success(),
        "tsc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let run = std::process::Command::new(node)
        .arg(dir.join("driver.js"))
        .output()
        .expect("spawn node");
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    assert!(
        run.status.success(),
        "node driver failed:\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
    stdout
}

// ------------------------------------------------------------------- fixtures

fn endpoint(path: &str, op: &str, mw: MiddlewareSpec) -> ApiEndpoint {
    ApiEndpoint {
        crate_name: "demo".into(),
        method: HttpMethod::GET,
        path: path.into(),
        operation_id: op.into(),
        summary: "demo".into(),
        parameters: vec![],
        request_body: None,
        responses: vec![ApiResponse { status: 200, description: String::new(), schema: None }],
        tags: vec!["demo".into()],
        middleware: Some(mw),
    }
}

// ---------------------------------------------------------------------- tests

#[test]
fn cursor_paginator_walks_every_page_and_sends_auth() {
    let Some((tsc, node)) = node_toolchain() else {
        println!("[skip] tsc/node unavailable — set FLUX_TSC and FLUX_NODE to run middleware smoke tests");
        return;
    };

    // Three pages, the last one terminating with a null cursor. If the
    // generated loop stops early we lose items 5; if it fails to terminate the
    // driver hangs and the test times out — both are real failures a
    // string-contains assertion cannot catch.
    let mock = spawn_mock(|_idx, head, stream| {
        let body = if head.contains("after=c2") {
            r#"{"data":[5],"next_cursor":null}"#
        } else if head.contains("after=c1") {
            r#"{"data":[3,4],"next_cursor":"c2"}"#
        } else {
            r#"{"data":[1,2],"next_cursor":"c1"}"#
        };
        let _ = stream.write_all(json_response(body).as_bytes());
    });

    let eps = vec![endpoint(
        "/cursor",
        "list_cursor",
        MiddlewareSpec::bearer_auth().with_pagination(PaginationStyle::Cursor {
            cursor_param: "after".into(),
            response_field: "next_cursor".into(),
        }),
    )];
    let sdk = generate_typescript_sdk(&eps, &mock.base_url());

    let driver = r#"
import { DemoClient } from "./client.js";
const c = new DemoClient(undefined, "secret-token");
const items: any[] = [];
for await (const it of c.listCursorIterItems()) items.push(it);
console.log(JSON.stringify(items));
"#;
    let out = run_driver(&sdk, driver, &tsc, &node);

    assert_eq!(
        out.trim(),
        "[1,2,3,4,5]",
        "paginator did not yield every item across 3 pages; got {out}"
    );

    let reqs = mock.requests();
    assert_eq!(reqs.len(), 3, "expected exactly 3 page fetches, saw {}:\n{reqs:#?}", reqs.len());
    // Auth must ride on every page request, not just the first.
    for (i, r) in reqs.iter().enumerate() {
        assert!(
            r.contains("Authorization: Bearer secret-token"),
            "page {i} went out unauthenticated:\n{r}"
        );
    }
    // The cursor from each response must actually be sent on the next request.
    assert!(reqs[1].contains("after=c1"), "2nd request lost the cursor:\n{}", reqs[1]);
    assert!(reqs[2].contains("after=c2"), "3rd request lost the cursor:\n{}", reqs[2]);
}

#[test]
fn retry_reattempts_on_429_streak_then_succeeds() {
    let Some((tsc, node)) = node_toolchain() else {
        println!("[skip] tsc/node unavailable — set FLUX_TSC and FLUX_NODE");
        return;
    };

    // 429, 429, then 200 — exactly exhausting a 3-attempt policy.
    let mock = spawn_mock(|idx, _head, stream| {
        let resp = if idx < 2 {
            status_response(429, "Too Many Requests")
        } else {
            json_response(r#"{"ok":true}"#)
        };
        let _ = stream.write_all(resp.as_bytes());
    });

    let eps = vec![endpoint(
        "/flaky",
        "get_flaky",
        MiddlewareSpec::default().with_retry(RetryPolicy::standard()),
    )];
    let sdk = generate_typescript_sdk(&eps, &mock.base_url());

    let driver = r#"
import { DemoClient } from "./client.js";
const c = new DemoClient();
console.log(JSON.stringify(await c.getFlaky()));
"#;
    let out = run_driver(&sdk, driver, &tsc, &node);

    assert_eq!(out.trim(), r#"{"ok":true}"#, "retry did not surface the eventual success: {out}");
    assert_eq!(
        mock.requests().len(),
        3,
        "expected 3 attempts (2 retries) on a 429 streak, saw {}",
        mock.requests().len()
    );
}

#[test]
fn retry_gives_up_after_max_attempts_and_returns_last_response() {
    let Some((tsc, node)) = node_toolchain() else {
        println!("[skip] tsc/node unavailable — set FLUX_TSC and FLUX_NODE");
        return;
    };

    // Never recovers. The client must stop at max_attempts rather than spin
    // forever, and must hand back the final failing response instead of hanging.
    let mock = spawn_mock(|_idx, _head, stream| {
        let _ = stream.write_all(status_response(503, "Service Unavailable").as_bytes());
    });

    let eps = vec![endpoint(
        "/down",
        "get_down",
        MiddlewareSpec::default().with_retry(RetryPolicy::standard()),
    )];
    let sdk = generate_typescript_sdk(&eps, &mock.base_url());

    let driver = r#"
import { DemoClient } from "./client.js";
const c = new DemoClient();
await c.getDown();
console.log("returned");
"#;
    let out = run_driver(&sdk, driver, &tsc, &node);

    assert_eq!(out.trim(), "returned", "client did not return after exhausting retries");
    assert_eq!(
        mock.requests().len(),
        3,
        "retry budget not honoured: expected 3 attempts, saw {}",
        mock.requests().len()
    );
}

#[test]
fn sse_stream_reassembles_frames_split_across_tcp_writes() {
    let Some((tsc, node)) = node_toolchain() else {
        println!("[skip] tsc/node unavailable — set FLUX_TSC and FLUX_NODE");
        return;
    };

    // The point of this test: deliberately cut a `data:` frame in half across
    // two TCP writes. A per-chunk parser drops the split frame silently. The
    // generated reader buffers, so all three events must arrive intact.
    let mock = spawn_mock(|_idx, _head, stream| {
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.flush();
        // First full frame, then the first half of the second.
        let _ = stream.write_all(b"data: {\"n\":1}\n\ndata: {\"n\"");
        let _ = stream.flush();
        std::thread::sleep(std::time::Duration::from_millis(60));
        // Completion of frame 2, a third frame, and the terminator.
        let _ = stream.write_all(b":2}\n\ndata: {\"n\":3}\n\ndata: [DONE]\n\n");
        let _ = stream.flush();
    });

    let eps = vec![endpoint(
        "/events",
        "watch_events",
        MiddlewareSpec::default().with_streaming(StreamKind::Sse),
    )];
    let sdk = generate_typescript_sdk(&eps, &mock.base_url());

    let driver = r#"
import { DemoClient } from "./client.js";
const c = new DemoClient();
const seen: any[] = [];
for await (const ev of c.watchEventsStream()) seen.push(ev.n);
console.log(JSON.stringify(seen));
"#;
    let out = run_driver(&sdk, driver, &tsc, &node);

    assert_eq!(
        out.trim(),
        "[1,2,3]",
        "SSE reader lost or mangled a frame split across TCP writes; got {out}"
    );
}
