//! flux-vite-engine::eye — Chrome-extension bridge for live in-browser introspection.
//!
//! Architecture
//! ------------
//!
//!     ┌─────────────────────────┐         ┌─────────────────────────┐
//!     │   Chrome extension      │  poll   │  eye-server (Node)      │
//!     │   in operator's tab     │ ───────►│  127.0.0.1:9789         │
//!     │                         │ ◄─────── │                         │
//!     └─────────┬───────────────┘  result └──────────┬──────────────┘
//!               │ scripting.executeScript            │ queue + result JSON files
//!               ▼                                    │
//!     ┌─────────────────────────┐                    ▼
//!     │  Wallet DOM             │           ┌─────────────────────────┐
//!     │  (SIGIL panels)         │           │  fluxc-mcp tools        │
//!     └─────────────────────────┘           │  flux_eye_snapshot /    │
//!                                           │  flux_eye_query / click │
//!                                           └─────────────────────────┘
//!
//! Eye lives ENTIRELY inside flux-vite-engine because vite is the dev
//! substrate. SIGIL is just the first consumer; future Flux-sibling apps
//! get it for free.
//!
//! ## Public surface
//!
//! - [`EyeServer::spawn`] — start the Node bridge as a child process.
//! - [`EyeClient`] — sync HTTP client for queuing commands and reading results.
//! - [`PanelSnapshot`] — typed representation of the wallet's panel state.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Default bind address for the eye-server.
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 9789;

/// Public address the Chrome extension should be pointed at when the dev box
/// is reachable over the internet (Epsilon's public IP, port-forwarded).
pub const DEFAULT_PUBLIC_HOST: &str = "89.149.241.126";

/// Resolve the on-disk path to the eye-server's Node entry script.
///
/// Looks up the crate-relative path (`eye-server/eye-server.mjs`) from the
/// `CARGO_MANIFEST_DIR` of `flux-vite-engine`, falling back to common deploy
/// locations.
pub fn server_script_path() -> Option<PathBuf> {
    // 1) crate-relative (development)
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").ok();
    if let Some(m) = &manifest_dir {
        let p = PathBuf::from(m).join("eye-server/eye-server.mjs");
        if p.exists() {
            return Some(p);
        }
    }
    // 2) common deploy paths
    for candidate in [
        "/opt/flux/eye-server/eye-server.mjs",
        "/home/orobit/flux-vite-engine/eye-server/eye-server.mjs",
        "/home/storage/deepseek-codewhale/flux/crates/flux-vite-engine/eye-server/eye-server.mjs",
    ] {
        let p = PathBuf::from(candidate);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Resolve the on-disk path to the Chrome extension directory.
pub fn extension_dir_path() -> Option<PathBuf> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").ok();
    if let Some(m) = &manifest_dir {
        let p = PathBuf::from(m).join("eye-extension");
        if p.exists() {
            return Some(p);
        }
    }
    for candidate in [
        "/opt/flux/eye-extension",
        "/home/storage/deepseek-codewhale/flux/crates/flux-vite-engine/eye-extension",
    ] {
        let p = PathBuf::from(candidate);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Owns the Node child process that runs the eye-server.
pub struct EyeServer {
    child: Child,
    pub host: String,
    pub port: u16,
}

impl EyeServer {
    /// Spawn the eye-server on the given host/port. Returns immediately;
    /// the caller is responsible for polling [`EyeClient::is_alive`] before
    /// queuing commands.
    pub fn spawn(host: &str, port: u16) -> Result<Self> {
        let script = server_script_path()
            .context("eye-server script not found (looked in CARGO_MANIFEST_DIR and common deploy paths)")?;
        let child = Command::new("node")
            .arg(&script)
            .env("SIGIL_EYE_HOST", host)
            .env("SIGIL_EYE_PORT", port.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .context("failed to spawn `node eye-server.mjs`")?;
        Ok(Self {
            child,
            host: host.to_string(),
            port,
        })
    }

    /// Sugar for [`EyeServer::spawn`] with default host/port.
    pub fn spawn_default() -> Result<Self> {
        Self::spawn(DEFAULT_HOST, DEFAULT_PORT)
    }

    /// Hard-kill the child process.
    pub fn kill(mut self) -> Result<()> {
        self.child.kill().ok();
        Ok(())
    }
}

/// Sync HTTP client for the eye-server. Used by MCP tools to enqueue commands
/// and read results / snapshots.
pub struct EyeClient {
    base: String,
}

impl EyeClient {
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            base: format!("http://{host}:{port}"),
        }
    }

    pub fn local() -> Self {
        Self::new(DEFAULT_HOST, DEFAULT_PORT)
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// Returns `Ok(true)` if the server responds to `/health`.
    pub fn is_alive(&self) -> bool {
        ureq_get(&format!("{}/health", self.base)).is_ok()
    }

    /// Enqueue a command. Returns the command id assigned by the server.
    pub fn queue(&self, cmd: &EyeCommand) -> Result<String> {
        let body = serde_json::to_string(cmd)?;
        let resp = ureq_post(&format!("{}/cmd", self.base), &body)?;
        let v: serde_json::Value = serde_json::from_str(&resp)?;
        v.get("id")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("server returned no id"))
    }

    /// Block-wait for a queued command's result. Polls every `poll_ms`
    /// up to `timeout_ms`.
    pub fn wait_result(&self, id: &str, timeout_ms: u64, poll_ms: u64) -> Result<EyeResult> {
        let start = Instant::now();
        loop {
            let r = ureq_get(&format!("{}/result/{id}", self.base));
            if let Ok((status, body)) = r {
                if status == 200 {
                    let r: EyeResult = serde_json::from_str(&body)?;
                    return Ok(r);
                }
            }
            if start.elapsed().as_millis() as u64 >= timeout_ms {
                return Err(anyhow!("timed out waiting for result id={id}"));
            }
            std::thread::sleep(Duration::from_millis(poll_ms));
        }
    }

    /// Run a command and block-wait for its result in one call.
    pub fn run(&self, cmd: EyeCommand, timeout_ms: u64) -> Result<EyeResult> {
        let id = self.queue(&cmd)?;
        self.wait_result(&id, timeout_ms, 200)
    }

    /// Fetch the most recent auto-snapshot posted by the extension.
    pub fn latest_snapshot(&self) -> Result<PanelSnapshot> {
        let (status, body) = ureq_get(&format!("{}/snapshot", self.base))?;
        if status == 404 {
            return Err(anyhow!("no snapshot posted yet (is the wallet tab open in the operator's browser?)"));
        }
        if status != 200 {
            return Err(anyhow!("server returned status {status}"));
        }
        Ok(serde_json::from_str(&body)?)
    }
}

/// A command queued for the extension to execute.
/// NOTE: Serialize is hand-written below (custom `{fn:,args:}` wire format) — do
/// NOT add it to the derive (E0119 conflict). Deserialize stays derived.
#[derive(Debug, Clone, Deserialize)]
pub struct EyeCommand {
    pub fn_: String,
    pub args: serde_json::Value,
}

impl EyeCommand {
    pub fn snapshot() -> Self {
        Self { fn_: "snapshot".into(), args: serde_json::Value::Null }
    }
    pub fn click(selector: &str) -> Self {
        Self { fn_: "click".into(), args: serde_json::json!({ "selector": selector }) }
    }
    pub fn eval(expr: &str) -> Self {
        Self { fn_: "eval".into(), args: serde_json::json!({ "expr": expr }) }
    }
}

// Custom serde to match the extension's wire format ({ fn: ..., args: ... }).
impl Serialize for EyeCommand {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let mut m = std::collections::BTreeMap::new();
        m.insert("fn", serde_json::Value::String(self.fn_.clone()));
        m.insert("args", self.args.clone());
        serde::Serialize::serialize(&m, ser)
    }
}

/// The decoded result of a queued command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EyeResult {
    pub id: String,
    pub ts: u64,
    #[serde(default)]
    pub result: serde_json::Value,
    #[serde(default)]
    pub error: Option<String>,
}

/// Typed representation of a wallet panel snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelSnapshot {
    pub ts: u64,
    pub href: String,
    pub panels: HashMap<String, PanelInfo>,
    #[serde(default)]
    pub ribbon: serde_json::Value,
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub errors: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelInfo {
    pub exists: bool,
    #[serde(default)]
    pub open: bool,
    #[serde(default)]
    pub bd: serde_json::Value,
    #[serde(default)]
    pub inner: serde_json::Value,
}

// ── minimal sync HTTP client (no extra deps) ──

fn ureq_get(url: &str) -> Result<(u16, String)> {
    let (host, port, path) = split_url(url)?;
    let mut s = std::net::TcpStream::connect((host.as_str(), port))?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.set_write_timeout(Some(Duration::from_secs(5)))?;
    use std::io::{Read, Write};
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\nAccept: application/json\r\n\r\n");
    s.write_all(req.as_bytes())?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf)?;
    let body = String::from_utf8_lossy(&buf).into_owned();
    let status = parse_status(&body).unwrap_or(0);
    let body = parse_body(&body);
    Ok((status, body))
}

fn ureq_post(url: &str, body: &str) -> Result<String> {
    let (host, port, path) = split_url(url)?;
    let mut s = std::net::TcpStream::connect((host.as_str(), port))?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.set_write_timeout(Some(Duration::from_secs(5)))?;
    use std::io::{Read, Write};
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes())?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf)?;
    let raw = String::from_utf8_lossy(&buf).into_owned();
    Ok(parse_body(&raw))
}

fn split_url(url: &str) -> Result<(String, u16, String)> {
    let rest = url.strip_prefix("http://").ok_or_else(|| anyhow!("only http:// supported, got {url}"))?;
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match hostport.find(':') {
        Some(i) => (&hostport[..i], hostport[i + 1..].parse::<u16>()?),
        None => (hostport, 80u16),
    };
    Ok((host.to_string(), port, path.to_string()))
}

fn parse_status(raw: &str) -> Option<u16> {
    raw.split_whitespace().nth(1).and_then(|s| s.parse().ok())
}

fn parse_body(raw: &str) -> String {
    if let Some(idx) = raw.find("\r\n\r\n") {
        raw[idx + 4..].to_string()
    } else {
        raw.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_serializes_to_extension_wire_format() {
        let c = EyeCommand::eval("1+1");
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.contains("\"fn\":\"eval\""), "got {s}");
        assert!(s.contains("\"args\""), "got {s}");
        assert!(s.contains("\"expr\":\"1+1\""), "got {s}");
    }

    #[test]
    fn snapshot_command_has_null_args() {
        let s = serde_json::to_string(&EyeCommand::snapshot()).unwrap();
        assert!(s.contains("\"fn\":\"snapshot\""), "got {s}");
    }

    #[test]
    fn split_url_basic() {
        let (h, p, path) = split_url("http://127.0.0.1:9789/cmd").unwrap();
        assert_eq!(h, "127.0.0.1");
        assert_eq!(p, 9789);
        assert_eq!(path, "/cmd");
    }

    #[test]
    fn split_url_root() {
        let (h, p, path) = split_url("http://10.0.0.1:80").unwrap();
        assert_eq!(h, "10.0.0.1");
        assert_eq!(p, 80);
        assert_eq!(path, "/");
    }
}
