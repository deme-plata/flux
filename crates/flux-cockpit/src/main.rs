// flux-cockpit — the fluxc-native ratatui terminal app.
//
// A "second terminal" you run beside your work: tabs for SIGIL chain status, the
// SIGIL wallet, and a 🏆 swarm scoreboard, with two ALWAYS-ON right-hand side
// panels — live flux MCP swarm comms (every agent message) and the wallet — so
// you can watch all communication at a glance.
//
// Built to cross-compile to a single self-contained Windows .exe and to update
// itself via fluxc: on launch it swaps in any staged `<exe>.new`, and it polls a
// version endpoint, showing an update banner; press `u` to download + stage.
//
//   Keys:  Tab / Shift-Tab switch tabs · ↑/↓ scroll comms · r refresh · u update · q quit
//
// Data sources (env-configurable so the same binary works on the build host with
// local files AND on a remote Windows box pointed at HTTP snapshots):
//   FLUX_COCKPIT_COMMS     swarm messages   (default /tmp/flux-swarm-messages.jsonl)
//   FLUX_COCKPIT_SWARM     swarm agents     (default /tmp/flux-swarm.json)
//   FLUX_COCKPIT_ACTIVITY  swarm activity   (default /tmp/flux-swarm-activity.jsonl)
//   FLUX_COCKPIT_WALLET    wallet snapshot  (optional; file path or http url)
//   FLUX_COCKPIT_UPDATE    version endpoint (optional; returns {"version":..,"url":..} or a bare version)
//   FLUX_COCKPIT_LLM_URL   ollama-compatible base url for the 🧠 LLM tab (default http://localhost:11434)
//   FLUX_COCKPIT_LLM_MODEL model tag for the LLM tab (default qwen2.5:14b)
// A value beginning with "http" is fetched over HTTP; otherwise it's read as a file.
// The 🧠 LLM tab: press `i`, type a prompt, Enter → the model answers in-panel with
// tok/s. Point FLUX_COCKPIT_LLM_URL at any served endpoint (e.g. an A100's ollama).

use std::io::{self, Write};
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Tabs, Wrap},
    Frame, Terminal,
};
use serde_json::Value;

const VERSION: &str = env!("CARGO_PKG_VERSION");

// ── SIGIL palette: obsidian + violet + provenance gold (matches fluxmux) ──
const C_PANEL: Color = Color::Rgb(26, 20, 40);
const C_ACCENT: Color = Color::Rgb(139, 92, 246); // violet
const C_BRIGHT: Color = Color::Rgb(192, 132, 252); // bright violet
const C_GOLD: Color = Color::Rgb(251, 191, 36); // provenance only
const C_OK: Color = Color::Rgb(74, 222, 128);
const C_WARN: Color = Color::Rgb(251, 146, 60);
const C_DIM: Color = Color::Rgb(120, 120, 140);
const C_TEXT: Color = Color::Rgb(226, 232, 240);

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Sigil,
    Wallet,
    Board,
    Build,
    Tasks,
    Llm,
}
impl Tab {
    fn all() -> [Tab; 6] {
        [Tab::Sigil, Tab::Wallet, Tab::Board, Tab::Build, Tab::Tasks, Tab::Llm]
    }
    fn title(&self) -> &'static str {
        match self {
            Tab::Sigil => "🌌 SIGIL",
            Tab::Wallet => "🪙 Wallet",
            Tab::Board => "🏆 Board",
            Tab::Build => "⚡ Build",
            Tab::Tasks => "📋 Tasks",
            Tab::Llm => "🧠 LLM",
        }
    }
}

// ── data models ──────────────────────────────────────────────────────────────
#[derive(Clone)]
struct Comm {
    from: String,
    to: String,
    payload: String,
    ts_ms: u64,
}

#[derive(Clone)]
struct Score {
    agent: String,
    qug: f64,
    tasks: u64,
}

#[derive(Clone, Default)]
struct Wallet {
    address: String,
    sigil: Option<f64>,
    qug: Option<f64>,
    tip_verified: Option<bool>,
    tip_ms: Option<f64>,
    height: Option<u64>,
    peers: Option<u64>,
    note: String, // non-empty when no feed is configured / fetch failed
}

#[derive(Default)]
struct Data {
    comms: Vec<Comm>,       // newest last
    scores: Vec<Score>,     // sorted desc by qug
    total_qug: f64,
    agents_active: usize,
    wallet: Wallet,
    completed_total: u64,
    update_version: Option<String>, // Some(v) if a newer version is advertised
    update_url: Option<String>,
    update_checked: bool, // true once the update endpoint was reached this refresh
    net_err: Option<String>, // first feed fetch error this refresh (shown in UI)
    feed_src: String, // where comms were fetched from (shown for transparency)
}

struct App {
    tab: usize,
    comms_scroll: u16,
    data: Data,
    status: String,
    last_refresh: Instant,
    feedback_mode: bool,   // 'f' opens the flux-eye snap input
    feedback_input: String,
    llm_mode: bool,        // 'i' on the LLM tab opens the prompt input
    llm_input: String,     // the prompt being typed
    llm_output: String,    // last model response
    llm_stat: String,      // tok/s + load stat line for the last response
}

/// flux-eye: percent-encode a comment for the sentinel-GET query string.
fn pct(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

/// flux-eye SNAP: fire-and-forget a GET to /flux-eye with the live state summary
/// + comment. q-flux logs the URL; an Epsilon tailer turns it into a feedback
/// record. The "snapshot" of a data TUI is its visible state — tab, counts,
/// champion, comms — captured here alongside the comment.
fn send_feedback(app: &App) -> Result<(), String> {
    // The sentinel path q-flux logs; default is the site-root /flux-eye.
    let base = std::env::var("FLUX_COCKPIT_FEEDBACK")
        .unwrap_or_else(|_| "https://quillon.xyz/flux-eye".to_string());
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "windows".to_string());
    let champ = app.data.scores.first().map(|s| s.agent.as_str()).unwrap_or("-");
    let tab = Tab::all()[app.tab].title();
    let url = format!(
        "{base}?ts={ts}&v={v}&host={host}&tab={tab}&comms={comms}&agents={agents}&tasks={tasks}&qug={qug}&champion={champ}&comment={comment}",
        ts = now_secs(),
        v = VERSION,
        host = pct(&host),
        tab = pct(tab),
        comms = app.data.comms.len(),
        agents = app.data.scores.len(),
        tasks = app.data.completed_total,
        qug = pct(&format!("{:.2}", app.data.total_qug)),
        champ = pct(champ),
        comment = pct(&app.feedback_input),
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(6))
        .user_agent(concat!("flux-cockpit/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    client.get(&url).send().map_err(|e| e.to_string())?;
    Ok(())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── source resolution: http url vs local file ─────────────────────────────────
// Returns Ok(body) or Err(reason). Errors are surfaced in the UI (never swallowed)
// so a remote box tells us exactly why a feed is empty instead of silently blank.
fn fetch(source: &str) -> Result<String, String> {
    if source.starts_with("http://") || source.starts_with("https://") {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(8))
            .user_agent(concat!("flux-cockpit/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| format!("client init: {e}"))?;
        let resp = client.get(source).send().map_err(|e| format!("connect: {e}"))?;
        let code = resp.status();
        if !code.is_success() {
            return Err(format!("http {}", code.as_u16()));
        }
        resp.text().map_err(|e| format!("body: {e}"))
    } else {
        std::fs::read_to_string(source).map_err(|e| format!("file: {e}"))
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

// ── flux-moe LLM: talk to any ollama-compatible endpoint ──────────────────────
//   FLUX_COCKPIT_LLM_URL    base url (default http://localhost:11434)
//   FLUX_COCKPIT_LLM_MODEL  model tag (default qwen2.5:14b)
// Point URL at a remote box (e.g. the A100's ollama) and the same binary chats it.
fn llm_endpoint() -> (String, String) {
    (
        env_or("FLUX_COCKPIT_LLM_URL", "http://localhost:11434"),
        env_or("FLUX_COCKPIT_LLM_MODEL", "qwen2.5:14b"),
    )
}

/// Send a prompt to the ollama `/api/generate` endpoint (non-streaming) and
/// return (response_text, stat_line). Built with a manual JSON body + text parse
/// so we don't need reqwest's `json` feature. Errors are surfaced, never swallowed.
fn llm_generate(prompt: &str) -> Result<(String, String), String> {
    let (base, model) = llm_endpoint();
    let url = format!("{}/api/generate", base.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        "options": { "num_predict": 256, "temperature": 0.3 }
    })
    .to_string();
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(180))
        .user_agent(concat!("flux-cockpit/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("client init: {e}"))?;
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .map_err(|e| format!("connect {url}: {e}"))?;
    let code = resp.status();
    if !code.is_success() {
        return Err(format!("http {} from {url}", code.as_u16()));
    }
    let raw = resp.text().map_err(|e| format!("body: {e}"))?;
    let v: Value = serde_json::from_str(&raw).map_err(|e| format!("parse: {e}"))?;
    let text = v.get("response").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    let ec = v.get("eval_count").and_then(|x| x.as_u64()).unwrap_or(0);
    let ed = v.get("eval_duration").and_then(|x| x.as_u64()).unwrap_or(1) as f64 / 1e9;
    let ld = v.get("load_duration").and_then(|x| x.as_u64()).unwrap_or(0) as f64 / 1e9;
    let stat = format!(
        "{model} · {ec} tok in {ed:.2}s = {tps:.1} tok/s · load {ld:.2}s",
        tps = ec as f64 / ed.max(1e-6)
    );
    Ok((text, stat))
}

/// Resolve a data source: explicit env override wins; else the local file if it
/// exists (build-host / Epsilon); else the published HTTPS feed. This is what
/// makes the same binary "just work" on a remote Windows box with zero config —
/// the local /tmp files don't exist there, so it auto-uses the quillon.xyz feed.
fn source_for(key: &str, local: &str, url: &str) -> String {
    if let Ok(v) = std::env::var(key) {
        return v;
    }
    if std::path::Path::new(local).exists() {
        return local.to_string();
    }
    url.to_string()
}

const FEED_BASE: &str = "https://quillon.xyz/downloads";

fn is_test_agent(id: &str, wallet: &str) -> bool {
    id.starts_with("test_") || wallet.starts_with("qnk_test") || wallet == "qnk_a" || wallet == "qnk_b"
}

// ── loaders ────────────────────────────────────────────────────────────────
fn load_comms(n: usize) -> (Vec<Comm>, String, Option<String>) {
    let src = source_for(
        "FLUX_COCKPIT_COMMS",
        "/tmp/flux-swarm-messages.jsonl",
        &format!("{FEED_BASE}/flux-swarm-messages.jsonl"),
    );
    let raw = match fetch(&src) {
        Ok(r) => r,
        Err(e) => return (Vec::new(), src, Some(e)),
    };
    let mut out: Vec<Comm> = raw
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|v| Comm {
            from: v.get("from").and_then(|x| x.as_str()).unwrap_or("?").to_string(),
            to: v.get("to").and_then(|x| x.as_str()).unwrap_or("?").to_string(),
            payload: v.get("payload").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            ts_ms: v.get("ts_ms").and_then(|x| x.as_u64()).unwrap_or(0),
        })
        .collect();
    let len = out.len();
    if len > n {
        out.drain(0..len - n);
    }
    (out, src, None)
}

fn load_scores() -> (Vec<Score>, f64, usize, u64, Option<String>) {
    let swarm_src = source_for("FLUX_COCKPIT_SWARM", "/tmp/flux-swarm.json", &format!("{FEED_BASE}/flux-swarm.json"));
    let act_src = source_for("FLUX_COCKPIT_ACTIVITY", "/tmp/flux-swarm-activity.jsonl", &format!("{FEED_BASE}/flux-swarm-activity.jsonl"));
    let mut err: Option<String> = None;

    // task counts per agent from the activity log (kind == "completed").
    let mut tasks: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    let mut completed_total = 0u64;
    if let Ok(raw) = fetch(&act_src) {
        for l in raw.lines() {
            if let Ok(v) = serde_json::from_str::<Value>(l) {
                if v.get("kind").and_then(|x| x.as_str()) == Some("completed") {
                    let a = v.get("agent").and_then(|x| x.as_str()).unwrap_or("?").to_string();
                    *tasks.entry(a).or_insert(0) += 1;
                    completed_total += 1;
                }
            }
        }
    }

    // per-agent settled QUG (the swarm's own "chosen" metric) from swarm.json.
    let mut scores: Vec<Score> = Vec::new();
    let mut total = 0.0f64;
    let mut active = 0usize;
    match fetch(&swarm_src) {
        Ok(raw) => {
            if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                if let Some(agents) = v.get("agents").and_then(|a| a.as_object()) {
                    for (id, a) in agents {
                        let wallet = a.get("wallet_address").and_then(|x| x.as_str()).unwrap_or("");
                        if is_test_agent(id, wallet) {
                            continue;
                        }
                        let qug = a.get("total_earned_qug").and_then(|x| x.as_f64()).unwrap_or(0.0);
                        total += qug;
                        if a.get("status").and_then(|x| x.as_str()) == Some("Working") {
                            active += 1;
                        }
                        scores.push(Score { agent: id.clone(), qug, tasks: *tasks.get(id).unwrap_or(&0) });
                    }
                }
            }
        }
        Err(e) => err = Some(e),
    }
    scores.sort_by(|a, b| {
        b.qug
            .partial_cmp(&a.qug)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.tasks.cmp(&a.tasks))
    });
    (scores, total, active, completed_total, err)
}

fn load_wallet() -> Wallet {
    let src = match std::env::var("FLUX_COCKPIT_WALLET") {
        Ok(s) => s,
        Err(_) => {
            return Wallet {
                note: "no wallet feed — set FLUX_COCKPIT_WALLET to a file/URL".into(),
                ..Default::default()
            }
        }
    };
    let raw = match fetch(&src) {
        Ok(r) => r,
        Err(e) => return Wallet { note: format!("wallet feed: {e}"), ..Default::default() },
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(v) => Wallet {
            address: v.get("address").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            sigil: v.get("sigil").and_then(|x| x.as_f64()),
            qug: v.get("qug").and_then(|x| x.as_f64()),
            tip_verified: v.get("tip_verified").and_then(|x| x.as_bool()),
            tip_ms: v.get("tip_ms").and_then(|x| x.as_f64()),
            height: v.get("height").and_then(|x| x.as_u64()),
            peers: v.get("peers").and_then(|x| x.as_u64()),
            note: String::new(),
        },
        Err(_) => Wallet { note: "wallet feed is not valid JSON".into(), ..Default::default() },
    }
}

/// Returns Some((newer_version, download_url)) if the update endpoint advertises
/// a version different from ours.
/// Returns (newer_version, url, reached) — `reached` is true iff the endpoint
/// was actually fetched (so the UI can show "check failed" vs a real "up to date").
fn check_update() -> (Option<String>, Option<String>, bool) {
    // Default to the published manifest so auto-update works with zero config.
    let src = env_or("FLUX_COCKPIT_UPDATE", &format!("{FEED_BASE}/flux-cockpit-version.json"));
    let raw = match fetch(&src) {
        Ok(r) => r,
        Err(_) => return (None, None, false),
    };
    // Accept either {"version":"x","url":"y"} or a bare version string.
    let (ver, url) = match serde_json::from_str::<Value>(&raw) {
        Ok(v) => (
            v.get("version").and_then(|x| x.as_str()).map(|s| s.to_string()),
            v.get("url").and_then(|x| x.as_str()).map(|s| s.to_string()),
        ),
        Err(_) => (Some(raw.trim().to_string()), None),
    };
    match ver {
        Some(v) if v != VERSION && !v.is_empty() => (Some(v), url, true),
        _ => (None, None, true),
    }
}

/// Download the advertised new binary to `<current_exe>.new` (staged; swapped in
/// on next launch by `swap_staged_update`).
fn stage_update(url: &str) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let staged = exe.with_extension("new");
    let bytes = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?
        .get(url)
        .send()
        .map_err(|e| e.to_string())?
        .bytes()
        .map_err(|e| e.to_string())?;
    std::fs::write(&staged, &bytes).map_err(|e| e.to_string())?;
    Ok(staged.display().to_string())
}

/// On startup: if `<exe>.new` exists, replace the running binary with it. Works
/// on Windows (a running exe can be renamed) and on Linux. Best-effort.
fn swap_staged_update() {
    if let Ok(exe) = std::env::current_exe() {
        let staged = exe.with_extension("new");
        if staged.exists() {
            let old = exe.with_extension("old");
            let _ = std::fs::remove_file(&old);
            if std::fs::rename(&exe, &old).is_ok() && std::fs::rename(&staged, &exe).is_ok() {
                let _ = std::fs::remove_file(&old);
            }
        }
    }
}

impl App {
    fn refresh(&mut self) {
        let (comms, src, comms_err) = load_comms(400);
        self.data.comms = comms;
        self.data.feed_src = src;
        let (scores, total, active, completed, score_err) = load_scores();
        self.data.scores = scores;
        self.data.total_qug = total;
        self.data.agents_active = active;
        self.data.completed_total = completed;
        self.data.wallet = load_wallet();
        let (uv, url, reached) = check_update();
        self.data.update_version = uv;
        self.data.update_url = url;
        self.data.update_checked = reached;
        self.data.net_err = comms_err.or(score_err);
        self.last_refresh = Instant::now();
    }
}

fn main() -> io::Result<()> {
    // Headless verification / snapshot mode: load every data source and print a
    // text summary, no TTY. `flux-cockpit --dump`
    if std::env::args().any(|a| a == "--dump") {
        let mut app = App {
            tab: 0,
            comms_scroll: 0,
            data: Data::default(),
            status: String::new(),
            last_refresh: Instant::now() - Duration::from_secs(10),
            feedback_mode: false,
            feedback_input: String::new(),
            llm_mode: false,
            llm_input: String::new(),
            llm_output: String::new(),
            llm_stat: String::new(),
        };
        app.refresh();
        println!("flux-cockpit v{VERSION} — data dump");
        println!("feed source: {}", app.data.feed_src);
        if let Some(e) = &app.data.net_err {
            println!("⚠ FEED ERROR: {e}");
        }
        println!("comms: {} messages loaded", app.data.comms.len());
        if let Some(c) = app.data.comms.last() {
            println!("  latest: {} → {} : {}", c.from, c.to, c.payload.replace('\n', " ").chars().take(70).collect::<String>());
        }
        println!("agents: {} (working {}) · tasks ✓ {} · total {:.2} QUG", app.data.scores.len(), app.data.agents_active, app.data.completed_total, app.data.total_qug);
        println!("🏆 scoreboard (top 10, chosen by settled QUG):");
        for (i, s) in app.data.scores.iter().take(10).enumerate() {
            println!("  {:>2}. {:<18} {:>7.2} QUG · {} tasks", i + 1, s.agent, s.qug, s.tasks);
        }
        let w = &app.data.wallet;
        if w.note.is_empty() {
            println!("wallet: {} · SIGIL {:?} · height {:?} · peers {:?} · tip {:?}", w.address, w.sigil, w.height, w.peers, w.tip_verified);
        } else {
            println!("wallet: {}", w.note);
        }
        match (&app.data.update_version, app.data.update_checked) {
            (Some(v), _) => println!("update: v{v} available ({:?})", app.data.update_url),
            (None, true) => println!("update: up to date (v{VERSION})"),
            (None, false) => println!("update: CHECK FAILED (endpoint unreachable) — v{VERSION}"),
        }
        return Ok(());
    }

    // Headless LLM probe: `flux-cockpit --ask "your prompt"` — verifies the LLM
    // panel's generate path end-to-end against the configured endpoint, no TTY.
    if let Some(pos) = std::env::args().position(|a| a == "--ask") {
        let prompt = std::env::args().nth(pos + 1).unwrap_or_else(|| "Say hello in one short sentence.".to_string());
        let (base, model) = llm_endpoint();
        println!("flux-cockpit v{VERSION} — LLM probe → {base} ({model})");
        match llm_generate(&prompt) {
            Ok((resp, stat)) => println!("{stat}\n--- answer ---\n{resp}"),
            Err(e) => {
                eprintln!("ask failed: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    swap_staged_update();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App {
        tab: 0,
        comms_scroll: 0,
        data: Data::default(),
        status: "loading…".into(),
        last_refresh: Instant::now() - Duration::from_secs(10),
        feedback_mode: false,
        feedback_input: String::new(),
        llm_mode: false,
        llm_input: String::new(),
        llm_output: String::new(),
        llm_stat: String::new(),
    };
    app.refresh();
    app.status = format!("ready · flux-cockpit v{VERSION}");

    let res = run(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;
    if let Err(e) = res {
        let _ = writeln!(io::stderr(), "flux-cockpit error: {e}");
    }
    Ok(())
}

fn run<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>, app: &mut App) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;

        if event::poll(Duration::from_millis(400))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    if app.feedback_mode {
                        // flux-eye input mode: every key edits the comment.
                        match k.code {
                            KeyCode::Esc => {
                                app.feedback_mode = false;
                                app.feedback_input.clear();
                                app.status = "snap cancelled".into();
                            }
                            KeyCode::Enter => {
                                app.feedback_mode = false;
                                if app.feedback_input.trim().is_empty() {
                                    app.status = "snap cancelled (empty comment)".into();
                                } else {
                                    app.status = "📸 sending flux-eye snap…".into();
                                    let _ = terminal.draw(|f| ui(f, app));
                                    match send_feedback(app) {
                                        Ok(()) => app.status = "📸 flux-eye snap sent ✓".into(),
                                        Err(e) => app.status = format!("snap failed: {e}"),
                                    }
                                }
                                app.feedback_input.clear();
                            }
                            KeyCode::Backspace => {
                                app.feedback_input.pop();
                            }
                            KeyCode::Char(c) => app.feedback_input.push(c),
                            _ => {}
                        }
                    } else if app.llm_mode {
                        // LLM prompt input mode (only entered from the LLM tab).
                        match k.code {
                            KeyCode::Esc => {
                                app.llm_mode = false;
                                app.llm_input.clear();
                                app.status = "ask cancelled".into();
                            }
                            KeyCode::Enter => {
                                app.llm_mode = false;
                                let prompt = app.llm_input.trim().to_string();
                                if prompt.is_empty() {
                                    app.status = "ask cancelled (empty)".into();
                                } else {
                                    app.status = "🧠 thinking…".into();
                                    let _ = terminal.draw(|f| ui(f, app));
                                    match llm_generate(&prompt) {
                                        Ok((resp, stat)) => {
                                            app.llm_output = resp;
                                            app.llm_stat = stat;
                                            app.status = "🧠 answer ready ✓".into();
                                        }
                                        Err(e) => {
                                            app.llm_output = format!("⚠ {e}");
                                            app.llm_stat = String::new();
                                            app.status = format!("ask failed: {e}");
                                        }
                                    }
                                }
                                app.llm_input.clear();
                            }
                            KeyCode::Backspace => {
                                app.llm_input.pop();
                            }
                            KeyCode::Char(c) => app.llm_input.push(c),
                            _ => {}
                        }
                    } else {
                        match k.code {
                            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                            KeyCode::Char('i') if Tab::all()[app.tab] == Tab::Llm => {
                                app.llm_mode = true;
                                app.llm_input.clear();
                                app.status = "🧠 ask the brain: type · Enter=send · Esc=cancel".into();
                            }
                            KeyCode::Char('f') => {
                                app.feedback_mode = true;
                                app.feedback_input.clear();
                                app.status = "flux-eye snap: type a comment · Enter=send · Esc=cancel".into();
                            }
                            KeyCode::Tab | KeyCode::Right => app.tab = (app.tab + 1) % Tab::all().len(),
                            KeyCode::BackTab | KeyCode::Left => {
                                app.tab = (app.tab + Tab::all().len() - 1) % Tab::all().len()
                            }
                            KeyCode::Up => app.comms_scroll = app.comms_scroll.saturating_sub(1),
                            KeyCode::Down => app.comms_scroll = app.comms_scroll.saturating_add(1),
                            KeyCode::Char('r') => {
                                app.refresh();
                                app.status = "refreshed".into();
                            }
                            KeyCode::Char('u') => match (app.data.update_version.clone(), app.data.update_url.clone()) {
                                (Some(v), Some(url)) => {
                                    app.status = format!("downloading v{v}…");
                                    let _ = terminal.draw(|f| ui(f, app));
                                    match stage_update(&url) {
                                        Ok(p) => app.status = format!("v{v} staged → {p} · restart to apply"),
                                        Err(e) => app.status = format!("update failed: {e}"),
                                    }
                                }
                                (Some(v), None) => app.status = format!("v{v} available but no download url advertised"),
                                _ => app.status = "already up to date".into(),
                            },
                            _ => {}
                        }
                    }
                }
            }
        }

        if app.last_refresh.elapsed() >= Duration::from_secs(2) {
            app.refresh();
        }
    }
}

// ── UI ───────────────────────────────────────────────────────────────────────
fn ui(f: &mut Frame, app: &App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0), Constraint::Length(1)])
        .split(f.area());

    title_bar(f, root[0], app);

    // body: left (tab content) | right column (comms over wallet)
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(root[1]);

    match Tab::all()[app.tab] {
        Tab::Sigil => sigil_tab(f, body[0], app),
        Tab::Wallet => wallet_tab(f, body[0], app),
        Tab::Board => board_tab(f, body[0], app),
        Tab::Build => build_tab(f, body[0], app),
        Tab::Tasks => tasks_tab(f, body[0], app),
        Tab::Llm => llm_tab(f, body[0], app),
    }

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(body[1]);
    comms_panel(f, right[0], app);
    wallet_panel(f, right[1], app);

    footer(f, root[2], app);
}

fn panel(title: &str, accent: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(format!(" {title} "), Style::default().fg(accent).add_modifier(Modifier::BOLD)))
        .border_style(Style::default().fg(C_ACCENT))
        .style(Style::default().bg(C_PANEL))
}

fn title_bar(f: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(38)])
        .split(area);

    let titles: Vec<Line> = Tab::all()
        .iter()
        .map(|t| Line::from(Span::styled(t.title(), Style::default().fg(C_TEXT))))
        .collect();
    let tabs = Tabs::new(titles)
        .select(app.tab)
        .highlight_style(Style::default().fg(C_GOLD).add_modifier(Modifier::BOLD | Modifier::UNDERLINED))
        .divider(Span::styled(" · ", Style::default().fg(C_DIM)))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(" FLUX COCKPIT ", Style::default().fg(C_BRIGHT).add_modifier(Modifier::BOLD)))
                .border_style(Style::default().fg(C_ACCENT))
                .style(Style::default().bg(C_PANEL)),
        );
    f.render_widget(tabs, cols[0]);

    let right = match (&app.data.update_version, app.data.update_checked) {
        (Some(v), _) => Line::from(vec![
            Span::styled("⬆ update ", Style::default().fg(C_GOLD).add_modifier(Modifier::BOLD)),
            Span::styled(format!("v{v} "), Style::default().fg(C_GOLD)),
            Span::styled("(press u)", Style::default().fg(C_DIM)),
        ]),
        (None, true) => Line::from(vec![
            Span::styled(format!("v{VERSION} "), Style::default().fg(C_ACCENT)),
            Span::styled("✓ up to date", Style::default().fg(C_OK)),
        ]),
        (None, false) => Line::from(vec![
            Span::styled(format!("v{VERSION} "), Style::default().fg(C_ACCENT)),
            Span::styled("⚠ offline", Style::default().fg(C_WARN)),
        ]),
    };
    f.render_widget(
        Paragraph::new(right).block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(C_ACCENT)).style(Style::default().bg(C_PANEL))),
        cols[1],
    );
}

fn comms_panel(f: &mut Frame, area: Rect, app: &App) {
    let mut lines: Vec<Line> = Vec::new();
    for c in app.data.comms.iter().rev() {
        let arrow = if c.to == "*" { "→ all".to_string() } else { format!("→ {}", c.to) };
        let head = format!("{} {}", c.from, arrow);
        lines.push(Line::from(Span::styled(head, Style::default().fg(C_BRIGHT).add_modifier(Modifier::BOLD))));
        let snippet: String = c.payload.replace('\n', " ").chars().take(180).collect();
        lines.push(Line::from(Span::styled(snippet, Style::default().fg(C_TEXT))));
        lines.push(Line::from(Span::styled("─".repeat(3), Style::default().fg(C_DIM))));
    }
    if lines.is_empty() {
        match &app.data.net_err {
            Some(e) => {
                lines.push(Line::from(Span::styled("⚠ feed fetch failed:", Style::default().fg(C_WARN).add_modifier(Modifier::BOLD))));
                lines.push(Line::from(Span::styled(e.clone(), Style::default().fg(C_WARN))));
                lines.push(Line::from(Span::styled(format!("src: {}", app.data.feed_src), Style::default().fg(C_DIM))));
            }
            None => lines.push(Line::from(Span::styled("no swarm comms feed", Style::default().fg(C_DIM)))),
        }
    }
    let title = format!("📡 SWARM COMMS · {} msgs", app.data.comms.len());
    f.render_widget(
        Paragraph::new(lines).block(panel(&title, C_BRIGHT)).wrap(Wrap { trim: false }).scroll((app.comms_scroll, 0)),
        area,
    );
}

fn wallet_panel(f: &mut Frame, area: Rect, app: &App) {
    let w = &app.data.wallet;
    let mut lines: Vec<Line> = Vec::new();
    if !w.note.is_empty() {
        lines.push(Line::from(Span::styled(&w.note, Style::default().fg(C_DIM))));
    } else {
        if !w.address.is_empty() {
            let short: String = w.address.chars().take(18).collect();
            lines.push(kv("addr", &format!("{short}…"), C_DIM));
        }
        if let Some(s) = w.sigil {
            lines.push(kv("SIGIL", &format!("{s:.4}"), C_GOLD));
        }
        if let Some(q) = w.qug {
            lines.push(kv("QUG", &format!("{q:.4}"), C_ACCENT));
        }
        if let Some(h) = w.height {
            lines.push(kv("height", &h.to_string(), C_TEXT));
        }
        if let Some(p) = w.peers {
            lines.push(kv("peers", &p.to_string(), if p > 0 { C_OK } else { C_WARN }));
        }
        if let Some(tv) = w.tip_verified {
            let ms = w.tip_ms.map(|m| format!(" {m:.1}ms")).unwrap_or_default();
            lines.push(kv("tip", &format!("{}{ms}", if tv { "✓ verified" } else { "✗ FAIL" }), if tv { C_OK } else { C_WARN }));
        }
    }
    f.render_widget(Paragraph::new(lines).block(panel("🪙 WALLET", C_GOLD)).wrap(Wrap { trim: false }), area);
}

fn kv(k: &str, v: &str, vc: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{k:<8}"), Style::default().fg(C_DIM)),
        Span::styled(v.to_string(), Style::default().fg(vc).add_modifier(Modifier::BOLD)),
    ])
}

fn board_tab(f: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    // champion banner
    let champ = match app.data.scores.first() {
        Some(s) => Line::from(vec![
            Span::styled("🏆 Swarm Champion: ", Style::default().fg(C_GOLD).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{} ", s.agent), Style::default().fg(C_BRIGHT).add_modifier(Modifier::BOLD)),
            Span::styled(format!("· {:.2} QUG · {} tasks", s.qug, s.tasks), Style::default().fg(C_TEXT)),
        ]),
        None => Line::from(Span::styled("no scores yet", Style::default().fg(C_DIM))),
    };
    f.render_widget(Paragraph::new(champ).block(panel("TOP ACHIEVEMENT", C_GOLD)), rows[0]);

    let max = app.data.scores.first().map(|s| s.qug).unwrap_or(1.0).max(0.001);
    let medals = ["🥇", "🥈", "🥉"];
    let items: Vec<ListItem> = app
        .data
        .scores
        .iter()
        .take(20)
        .enumerate()
        .map(|(i, s)| {
            let medal = medals.get(i).copied().unwrap_or("  ");
            let bar_w = ((s.qug / max) * 18.0).round() as usize;
            let bar = format!("{}{}", "█".repeat(bar_w), "░".repeat(18usize.saturating_sub(bar_w)));
            ListItem::new(Line::from(vec![
                Span::styled(format!("{medal} {:>2}. ", i + 1), Style::default().fg(C_GOLD)),
                Span::styled(format!("{:<16}", trunc(&s.agent, 16)), Style::default().fg(C_BRIGHT).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{bar} "), Style::default().fg(C_ACCENT)),
                Span::styled(format!("{:>7.2} QUG ", s.qug), Style::default().fg(C_GOLD)),
                Span::styled(format!("· {} tasks", s.tasks), Style::default().fg(C_DIM)),
            ]))
        })
        .collect();
    let title = format!("🏆 SWARM SCOREBOARD · chosen by settled QUG · {:.2} total", app.data.total_qug);
    f.render_widget(List::new(items).block(panel(&title, C_GOLD)), rows[1]);
}

fn sigil_tab(f: &mut Frame, area: Rect, app: &App) {
    let mut lines = vec![
        Line::from(Span::styled("SIGIL — DagKnight-on-Flux chain", Style::default().fg(C_BRIGHT).add_modifier(Modifier::BOLD))),
        Line::from(""),
        kv("agents", &app.data.scores.len().to_string(), C_TEXT),
        kv("working", &app.data.agents_active.to_string(), C_OK),
        kv("tasks ✓", &app.data.completed_total.to_string(), C_ACCENT),
        kv("QUG", &format!("{:.2}", app.data.total_qug), C_GOLD),
    ];
    let w = &app.data.wallet;
    if let Some(h) = w.height {
        lines.push(kv("height", &h.to_string(), C_TEXT));
    }
    if let Some(p) = w.peers {
        lines.push(kv("peers", &p.to_string(), if p > 0 { C_OK } else { C_WARN }));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "side panels (right) stream live swarm comms + wallet — always on",
        Style::default().fg(C_DIM),
    )));
    f.render_widget(Paragraph::new(lines).block(panel("🌌 SIGIL", C_ACCENT)).wrap(Wrap { trim: false }), area);
}

fn wallet_tab(f: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)])
        .split(area);
    wallet_panel(f, rows[0], app); // reuse the detailed renderer in the main pane
    // a little supply gauge if we know sigil balance
    let ratio = app.data.wallet.sigil.map(|s| (s / 21_000_000.0).clamp(0.0, 1.0)).unwrap_or(0.0);
    let g = Gauge::default()
        .block(panel("share of 21M cap", C_GOLD))
        .gauge_style(Style::default().fg(C_GOLD).bg(C_PANEL))
        .ratio(ratio);
    f.render_widget(g, rows[1]);
}

fn build_tab(f: &mut Frame, area: Rect, app: &App) {
    // surface the build-flavored swarm comms (shipped/tests/build events).
    let mut lines: Vec<Line> = Vec::new();
    for c in app.data.comms.iter().rev() {
        let p = &c.payload;
        if p.contains("SHIPPED") || p.contains("tests") || p.contains("build") || p.contains("✅") || p.contains("settled") {
            let snippet: String = p.replace('\n', " ").chars().take(110).collect();
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", trunc(&c.from, 12)), Style::default().fg(C_BRIGHT)),
                Span::styled(snippet, Style::default().fg(C_TEXT)),
            ]));
        }
        if lines.len() >= 30 {
            break;
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled("no build events in the comms feed yet", Style::default().fg(C_DIM))));
    }
    f.render_widget(Paragraph::new(lines).block(panel("⚡ BUILD FEED (from swarm comms)", C_ACCENT)).wrap(Wrap { trim: false }), area);
}

fn tasks_tab(f: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app
        .data
        .scores
        .iter()
        .take(20)
        .map(|s| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<16}", trunc(&s.agent, 16)), Style::default().fg(C_BRIGHT)),
                Span::styled(format!("{:>3} tasks  ", s.tasks), Style::default().fg(C_ACCENT)),
                Span::styled(format!("{:>7.2} QUG", s.qug), Style::default().fg(C_GOLD)),
            ]))
        })
        .collect();
    f.render_widget(List::new(items).block(panel("📋 AGENTS · tasks & earnings", C_ACCENT)), area);
}

fn llm_tab(f: &mut Frame, area: Rect, app: &App) {
    let (base, model) = llm_endpoint();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(0)])
        .split(area);

    // header: endpoint + model + the live prompt being typed (or the hint)
    let mut head: Vec<Line> = vec![
        Line::from(Span::styled("🧠 flux-moe — agentic-money brain", Style::default().fg(C_BRIGHT).add_modifier(Modifier::BOLD))),
        kv("endpoint", &base, C_DIM),
        kv("model", &model, C_ACCENT),
    ];
    if app.llm_mode {
        head.push(Line::from(vec![
            Span::styled("ask ▸ ", Style::default().fg(C_GOLD).add_modifier(Modifier::BOLD)),
            Span::styled(&app.llm_input, Style::default().fg(C_TEXT).add_modifier(Modifier::BOLD)),
            Span::styled("▏", Style::default().fg(C_BRIGHT)),
        ]));
    } else {
        head.push(Line::from(Span::styled("press  i  to ask · Enter sends · Esc cancels", Style::default().fg(C_DIM))));
    }
    if !app.llm_stat.is_empty() {
        head.push(Line::from(Span::styled(&app.llm_stat, Style::default().fg(C_GOLD))));
    }
    f.render_widget(Paragraph::new(head).block(panel("🧠 LLM", C_ACCENT)).wrap(Wrap { trim: false }), rows[0]);

    // body: the model's last answer
    let answer = if app.llm_output.is_empty() {
        "no answer yet — press i and ask anything (try an agentic-money decision)".to_string()
    } else {
        app.llm_output.clone()
    };
    let color = if app.llm_output.starts_with("⚠") { C_WARN } else { C_TEXT };
    f.render_widget(
        Paragraph::new(Span::styled(answer, Style::default().fg(color)))
            .block(panel("💬 ANSWER", C_BRIGHT))
            .wrap(Wrap { trim: false }),
        rows[1],
    );
}

fn footer(f: &mut Frame, area: Rect, app: &App) {
    // flux-eye input mode takes over the footer as a comment prompt.
    if app.feedback_mode {
        let line = Line::from(vec![
            Span::styled(" 📸 flux-eye ▸ ", Style::default().fg(C_GOLD).add_modifier(Modifier::BOLD)),
            Span::styled(&app.feedback_input, Style::default().fg(C_TEXT).add_modifier(Modifier::BOLD)),
            Span::styled("▏", Style::default().fg(C_BRIGHT)), // cursor
            Span::styled("   (Enter=send · Esc=cancel)", Style::default().fg(C_DIM)),
        ]);
        f.render_widget(Paragraph::new(line).style(Style::default().bg(C_PANEL)), area);
        return;
    }
    let line = Line::from(vec![
        Span::styled(" Tab", Style::default().fg(C_GOLD)),
        Span::styled(" switch · ", Style::default().fg(C_DIM)),
        Span::styled("↑↓", Style::default().fg(C_GOLD)),
        Span::styled(" comms · ", Style::default().fg(C_DIM)),
        Span::styled("r", Style::default().fg(C_GOLD)),
        Span::styled(" refresh · ", Style::default().fg(C_DIM)),
        Span::styled("f", Style::default().fg(C_GOLD)),
        Span::styled(" 📸 snap · ", Style::default().fg(C_DIM)),
        Span::styled("i", Style::default().fg(C_GOLD)),
        Span::styled(" 🧠 ask · ", Style::default().fg(C_DIM)),
        Span::styled("u", Style::default().fg(C_GOLD)),
        Span::styled(" update · ", Style::default().fg(C_DIM)),
        Span::styled("q", Style::default().fg(C_GOLD)),
        Span::styled(" quit   ", Style::default().fg(C_DIM)),
        Span::styled(&app.status, Style::default().fg(C_OK)),
    ]);
    f.render_widget(Paragraph::new(line).style(Style::default().bg(C_PANEL)), area);
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}
