//! flux-buzz CLI — relay server + agent-friendly client (JSON in, JSON out).
//!
//! ```text
//! flux-buzz keygen                         # create/show identity
//! flux-buzz serve --listen 127.0.0.1:9950  # run the relay
//! flux-buzz send --channel general --content "hello"
//! flux-buzz tail --channel general         # follow events as JSON lines
//! flux-buzz git-post --repo /path/to/repo  # announce the latest commit
//! ```
//!
//! Client subcommands default to the local relay and print raw JSON so LLM
//! agents can consume the output directly.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use flux_buzz::event::{Identity, KIND_CHAT, KIND_COMMIT, KIND_PROVENANCE};
use flux_buzz::rev::flux_rev_snapshot;
use flux_buzz::relay::{run_plain, run_tls, RelayState};
use flux_buzz::store::EventStore;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "flux-buzz", version, about = "Flux-native Buzz relay + client")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Generate (or show) this participant's Ed25519 identity.
    Keygen {
        /// Identity file path.
        #[arg(long, default_value_os_t = default_key_path())]
        key: PathBuf,
    },
    /// Run the relay server.
    Serve {
        /// Plain-HTTP listen address (keep loopback in production; q-flux or
        /// the TLS listener is the public face).
        #[arg(long, default_value = "127.0.0.1:9950")]
        listen: String,
        /// Data directory for the event log.
        #[arg(long, default_value_os_t = default_data_dir())]
        data: PathBuf,
        /// Optional TLS listen address (e.g. 0.0.0.0:9951).
        #[arg(long)]
        tls_listen: Option<String>,
        /// TLS certificate chain (PEM).
        #[arg(long)]
        tls_cert: Option<String>,
        /// TLS private key (PEM).
        #[arg(long)]
        tls_key: Option<String>,
    },
    /// Sign and publish a chat event.
    Send {
        #[arg(long, default_value = "http://127.0.0.1:9950")]
        relay: String,
        #[arg(long, default_value_os_t = default_key_path())]
        key: PathBuf,
        #[arg(long, default_value = "general")]
        channel: String,
        #[arg(long)]
        content: String,
        /// Event kind (1 = chat, 20 = agent action).
        #[arg(long, default_value_t = KIND_CHAT)]
        kind: u32,
    },
    /// Follow events as JSON lines (poll-based).
    Tail {
        #[arg(long, default_value = "http://127.0.0.1:9950")]
        relay: String,
        #[arg(long)]
        channel: Option<String>,
        /// Start cursor in unix ms (default: now — only new events).
        #[arg(long)]
        since: Option<u64>,
        /// Print the backlog first, then follow.
        #[arg(long, default_value_t = false)]
        backlog: bool,
    },
    /// Snapshot a directory with flux-rev and publish the content-address
    /// (`full:` stamp) as a signed provenance event.
    RevPost {
        /// Directory to snapshot (must be flux-rev-initialized; run
        /// `flux-rev genesis <dir>` once if not).
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:9950")]
        relay: String,
        #[arg(long, default_value_os_t = default_key_path())]
        key: PathBuf,
        #[arg(long, default_value = "provenance")]
        channel: String,
        /// Human-readable note for the event body (default: auto-generated).
        #[arg(long)]
        note: Option<String>,
    },
    /// Announce the latest commit of a git repository as a signed event.
    GitPost {
        #[arg(long)]
        repo: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:9950")]
        relay: String,
        #[arg(long, default_value_os_t = default_key_path())]
        key: PathBuf,
        #[arg(long, default_value = "commits")]
        channel: String,
    },
}

fn default_home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".flux-buzz")
}

fn default_key_path() -> PathBuf {
    default_home().join("identity.json")
}

fn default_data_dir() -> PathBuf {
    default_home().join("data")
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Keygen { key } => {
            let existed = key.exists();
            let id = Identity::load_or_generate(&key)?;
            println!(
                "{}",
                serde_json::json!({
                    "pubkey": id.pubkey_hex(),
                    "key_file": key.display().to_string(),
                    "created": !existed,
                })
            );
            Ok(())
        }
        Cmd::Serve { listen, data, tls_listen, tls_cert, tls_key } => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(async move {
                let store = EventStore::open(&data)?;
                tracing::info!("event log: {} events loaded from {}", store.len(), data.display());
                let state = RelayState::new(store);
                let plain = tokio::net::TcpListener::bind(&listen)
                    .await
                    .with_context(|| format!("binding {listen}"))?;
                if let Some(tls_addr) = tls_listen {
                    let cert = tls_cert.context("--tls-cert required with --tls-listen")?;
                    let key = tls_key.context("--tls-key required with --tls-listen")?;
                    let tls = tokio::net::TcpListener::bind(&tls_addr)
                        .await
                        .with_context(|| format!("binding {tls_addr}"))?;
                    let state2 = state.clone();
                    tokio::select! {
                        r = run_plain(plain, state) => r,
                        r = run_tls(tls, &cert, &key, state2) => r,
                    }
                } else {
                    run_plain(plain, state).await
                }
            })
        }
        Cmd::Send { relay, key, channel, content, kind } => {
            let id = Identity::load_or_generate(&key)?;
            let ev = id.sign_event(kind, vec![vec!["c".into(), channel]], content);
            let resp: serde_json::Value = reqwest::blocking::Client::new()
                .post(format!("{relay}/v1/event"))
                .json(&ev)
                .send()
                .context("relay unreachable")?
                .json()?;
            println!("{resp}");
            if resp["ok"] != serde_json::json!(true) {
                bail!("relay rejected event");
            }
            Ok(())
        }
        Cmd::Tail { relay, channel, since, backlog } => {
            let client = reqwest::blocking::Client::new();
            let mut cursor = match (since, backlog) {
                (Some(s), _) => s,
                (None, true) => 0,
                (None, false) => flux_buzz::event::now_ms(),
            };
            loop {
                let mut url = format!("{relay}/v1/events?since={cursor}&limit=500");
                if let Some(ref c) = channel {
                    url.push_str(&format!("&channel={c}"));
                }
                let resp: serde_json::Value =
                    client.get(&url).send().context("relay unreachable")?.json()?;
                if let Some(events) = resp["events"].as_array() {
                    for ev in events {
                        println!("{ev}");
                        if let Some(ts) = ev["created_at"].as_u64() {
                            cursor = cursor.max(ts);
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        }
        Cmd::RevPost { dir, relay, key, channel, note } => {
            let stamp = flux_rev_snapshot(&dir)?;
            let dir_name = dir
                .canonicalize()
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                .unwrap_or_else(|| dir.display().to_string());
            let content = note.unwrap_or_else(|| {
                format!("📦 provenance snapshot of {dir_name} — full:{}", &stamp[..16])
            });
            let id = Identity::load_or_generate(&key)?;
            let ev = id.sign_event(
                KIND_PROVENANCE,
                vec![
                    vec!["c".into(), channel],
                    vec!["rev".into(), stamp.clone()],
                    vec!["dir".into(), dir_name],
                ],
                content,
            );
            let resp: serde_json::Value = reqwest::blocking::Client::new()
                .post(format!("{relay}/v1/event"))
                .json(&ev)
                .send()
                .context("relay unreachable")?
                .json()?;
            println!("{}", serde_json::json!({"relay": resp, "rev": stamp}));
            if resp["ok"] != serde_json::json!(true) {
                bail!("relay rejected provenance event");
            }
            Ok(())
        }
        Cmd::GitPost { repo, relay, key, channel } => {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["log", "-1", "--format=%H%x1f%an%x1f%ct%x1f%s"])
                .output()
                .context("running git")?;
            if !out.status.success() {
                bail!("git log failed: {}", String::from_utf8_lossy(&out.stderr));
            }
            let line = String::from_utf8_lossy(&out.stdout);
            let parts: Vec<&str> = line.trim().split('\u{1f}').collect();
            if parts.len() < 4 {
                bail!("unexpected git log output: {line}");
            }
            let (hash, author, subject) = (parts[0], parts[1], parts[3]);
            let repo_name = repo
                .canonicalize()
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                .unwrap_or_else(|| repo.display().to_string());
            let id = Identity::load_or_generate(&key)?;
            let ev = id.sign_event(
                KIND_COMMIT,
                vec![
                    vec!["c".into(), channel],
                    vec!["repo".into(), repo_name],
                    vec!["commit".into(), hash.to_string()],
                    vec!["author".into(), author.to_string()],
                ],
                subject.to_string(),
            );
            let resp: serde_json::Value = reqwest::blocking::Client::new()
                .post(format!("{relay}/v1/event"))
                .json(&ev)
                .send()
                .context("relay unreachable")?
                .json()?;
            println!("{resp}");
            if resp["ok"] != serde_json::json!(true) {
                bail!("relay rejected commit event");
            }
            Ok(())
        }
    }
}
