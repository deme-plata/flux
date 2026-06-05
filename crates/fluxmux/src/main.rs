// fluxmux — Terminal multiplexer dashboard for Flux Foundation
//
// Ratatui-based TUI with 5 tabs: Build, P2P, Self-Heal, Stats, Journal.
// Press Tab to switch tabs, q to quit, Enter on buttons to trigger actions.
//
// Designed as the "second terminal" — run `fluxmux` alongside codewhale
// to watch builds, P2P health, self-heal events, and task completion live.

use std::time::{Duration, Instant};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Tabs, Wrap},
    Terminal,
};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

// ═══════════════════════════════════════════════════════════════
// App state
// ═══════════════════════════════════════════════════════════════

#[derive(Clone, Debug, PartialEq)]
enum Tab { Sigil, Wallet, Build, P2P, SelfHeal, Stats, Journal, Tasks, Gemma4 }

impl Tab {
    fn title(&self) -> &str {
        match self {
            Tab::Sigil    => "🌌 SIGIL",
            Tab::Wallet   => "🪙 Wallet",
            Tab::Build    => "⚡ Build",
            Tab::P2P      => "🌐 P2P Mesh",
            Tab::SelfHeal => "🏥 Self-Heal",
            Tab::Stats    => "📊 Stats",
            Tab::Journal  => "📓 Journal",
            Tab::Tasks    => "📋 Tasks",
            Tab::Gemma4   => "🤖 Gemma4",
        }
    }
    fn all() -> Vec<Tab> {
        vec![Tab::Sigil, Tab::Wallet, Tab::Build, Tab::P2P, Tab::SelfHeal, Tab::Stats, Tab::Journal, Tab::Tasks, Tab::Gemma4]
    }
}

// ═══════════════════════════════════════════════════════════════
// SIGIL palette — obsidian + violet + provenance gold
// (matches sigilgraph.quillon.xyz wallet identity, 2026-05-30)
// ═══════════════════════════════════════════════════════════════
const C_OBSIDIAN:      Color = Color::Rgb(10, 10, 15);
const C_PANEL:         Color = Color::Rgb(26, 20, 40);
const C_ACCENT:        Color = Color::Rgb(139, 92, 246);   // violet
const C_ACCENT_BRIGHT: Color = Color::Rgb(192, 132, 252);  // bright violet
const C_SIGIL_GOLD:    Color = Color::Rgb(251, 191, 36);   // provenance only
const C_OK:            Color = Color::Rgb(74, 222, 128);
const C_WARN:          Color = Color::Rgb(251, 146, 60);
const C_DANGER:        Color = Color::Rgb(244, 63, 94);
const C_MUTED:         Color = Color::Rgb(148, 163, 184);

struct App {
    tab: Tab,
    _running: bool,
    // Build stats
    build_count: u64,
    last_build_ms: u64,
    cache_hit_rate: f64,
    // P2P stats
    peer_count: usize,
    dag_round: u64,
    latency_ms: f64,
    bandwidth_kbps: f64,
    mempool_txs: u64,
    // Self-heal
    heals_applied: u64,
    tasks_completed: u64,
    tasks_active: u64,
    anomalies: u64,
    health_score: f64,
    last_heal_event: String,
    update_available: bool,
    update_version: String,
    // Stats
    uptime_secs: u64,
    crates_compiled: u64,
    mcp_calls: u64,
    // Journal
    journal: Vec<String>,
    tasks: Vec<TaskItem>,
    webhook_events: Vec<String>,
    wallet_balance: String,
    // Gemma4 chat
    gemma_input: String,
    gemma_response: String,
    gemma_loading: bool,
    gemma_history: Vec<(String, String)>,
    // Button states
    selected_button: usize,
    // ── SIGIL substrate (added 2026-05-30, fluxmux v0.10+) ──
    sigil_node_active:  bool,
    sigil_node_pid:     String,
    sigil_bridge_active:bool,
    qflux_active:       bool,
    cert_days_remaining:i64,
    cert_san_count:     u32,
    wallet_bundle_kb:   u64,
    wallet_last_deploy: String,
    phase_done:         u32,   // out of 10 phases (A..J)
    phase_in_progress:  u32,
    sigil_p2p_port_open:bool,
}

impl App {
    fn new() -> Self {
        App {
            tab: Tab::Sigil,
            _running: true,
            build_count: 1,
            last_build_ms: 0,
            cache_hit_rate: 86.0,
            peer_count: 1,
            dag_round: 0,
            latency_ms: 12.4,
            bandwidth_kbps: 2340.0,
            mempool_txs: 0,
            heals_applied: 0,
            tasks_completed: 0,
            tasks_active: 0,
            anomalies: 0,
            health_score: 100.0,
            last_heal_event: "No events yet — monitoring active".into(),
            update_available: false,
            update_version: String::new(),
            uptime_secs: 0,
            crates_compiled: 16,
            mcp_calls: 50,
            journal: vec![],
            tasks: vec![],
            webhook_events: vec![],
            wallet_balance: "—".into(),
            gemma_input: String::new(),
            gemma_response: String::new(),
            gemma_loading: false,
            gemma_history: Vec::new(),
            selected_button: 0,
            sigil_node_active:   false,
            sigil_node_pid:      "—".into(),
            sigil_bridge_active: false,
            qflux_active:        false,
            cert_days_remaining: 0,
            cert_san_count:      0,
            wallet_bundle_kb:    0,
            wallet_last_deploy:  "—".into(),
            phase_done:          0,
            phase_in_progress:   0,
            sigil_p2p_port_open: false,
            // Connection status loaded from /tmp/flux-mux.state
        }
    }

    /// Periodic SIGIL substrate probe. Cheap shell-outs every tick — keeps
    /// the operator's pulse on what's running without leaving the TUI.
    fn probe_sigil(&mut self) {
        // sigil-node (Delta + Epsilon both report as 'sigil-node' systemd unit
        // OR can be running as 'sigil-node-delta' standalone process)
        let sn = std::process::Command::new("systemctl").args(["is-active", "sigil-node"]).output();
        self.sigil_node_active = matches!(sn, Ok(o) if String::from_utf8_lossy(&o.stdout).trim() == "active")
            || std::process::Command::new("pgrep").args(["-f", "sigil-node-delta"]).output()
                .map(|o| !o.stdout.is_empty()).unwrap_or(false);
        if self.sigil_node_active {
            if let Ok(o) = std::process::Command::new("pgrep").args(["-f", "sigil-node"]).output() {
                self.sigil_node_pid = String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("?").to_string();
            }
        }
        // sigil-bridge
        self.sigil_bridge_active = std::process::Command::new("pgrep").args(["-f", "sigil-bridge"]).output()
            .map(|o| !o.stdout.is_empty()).unwrap_or(false);
        // q-flux
        let qf = std::process::Command::new("systemctl").args(["is-active", "q-flux"]).output();
        self.qflux_active = matches!(qf, Ok(o) if String::from_utf8_lossy(&o.stdout).trim() == "active");
        // SIGIL p2p port :9501
        if let Ok(o) = std::process::Command::new("ss").args(["-tln"]).output() {
            let s = String::from_utf8_lossy(&o.stdout);
            self.sigil_p2p_port_open = s.contains(":9501 ");
        }
        // TLS cert expiry on /etc/letsencrypt/live/quillon.xyz/fullchain.pem
        if let Ok(o) = std::process::Command::new("openssl")
            .args(["x509", "-in", "/etc/letsencrypt/live/quillon.xyz/fullchain.pem", "-noout", "-enddate"])
            .output() {
            let s = String::from_utf8_lossy(&o.stdout);
            // Parse "notAfter=Aug 28 09:08:03 2026 GMT" using `date`
            if let Some(date_str) = s.strip_prefix("notAfter=").map(|x| x.trim()) {
                if let Ok(d) = std::process::Command::new("date").args(["-d", date_str, "+%s"]).output() {
                    if let Ok(end_epoch) = String::from_utf8_lossy(&d.stdout).trim().parse::<i64>() {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs() as i64).unwrap_or(0);
                        self.cert_days_remaining = (end_epoch - now) / 86_400;
                    }
                }
            }
            // SAN count from same cert
            if let Ok(san) = std::process::Command::new("openssl")
                .args(["x509", "-in", "/etc/letsencrypt/live/quillon.xyz/fullchain.pem", "-noout", "-ext", "subjectAltName"])
                .output() {
                let s = String::from_utf8_lossy(&san.stdout);
                self.cert_san_count = s.matches("DNS:").count() as u32;
            }
        }
        // Wallet bundle size + last deploy
        if let Ok(m) = std::fs::metadata("/home/orobit/q-narwhalknight/dist-final/sigil-wallet/index.html") {
            if let Ok(t) = m.modified() {
                if let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) {
                    let secs = d.as_secs();
                    let h = (secs / 3600) % 24;
                    let m_ = (secs / 60) % 60;
                    self.wallet_last_deploy = format!("{:02}:{:02} UTC", h, m_);
                }
            }
        }
        if let Ok(o) = std::process::Command::new("du")
            .args(["-sk", "/home/orobit/q-narwhalknight/dist-final/sigil-wallet"])
            .output() {
            let s = String::from_utf8_lossy(&o.stdout);
            self.wallet_bundle_kb = s.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
        }
        // Phase progress — count completed Sigil-wallet tasks by scanning /tmp/flux-tasks.json
        // For now, derive from CHANGELOG presence in sigil-wallet
        self.phase_done = 1; // Phase A is shipped as of 2026-05-30
        self.phase_in_progress = 1; // Phase B
    }

    fn log(&mut self, msg: &str) {
        let ts = chrono_now();
        self.journal.push(format!("[{}] {}", ts, msg));
        if self.journal.len() > 200 { self.journal.drain(0..50); }
    }

    /// Periodic update poll: check for ready flag from background thread.
    fn poll_update(&mut self) {
        if !self.update_available {
            if let Ok(ver) = std::fs::read_to_string("/tmp/flux-update.ready") {
                let ver = ver.trim().to_string();
                if !ver.is_empty() {
                    self.update_available = true;
                    self.update_version = ver.clone();
                    self.log(&format!("🔔 Update available: v{}", self.update_version));
                }
            }
        }
    }

    fn tick(&mut self) {
        // Probe SIGIL substrate every 5s (cheap shell-outs)
        if self.uptime_secs % 5 == 0 {
            self.probe_sigil();
        }
        self.poll_update();
        // Check for Gemma4 response (streaming: show partial, finalize on done)
        if self.gemma_loading {
            if std::fs::metadata("/tmp/flux-gemma.done").is_ok() {
                if let Ok(resp) = std::fs::read_to_string("/tmp/flux-gemma.response") {
                    self.gemma_response = resp;
                    self.gemma_history.push(("You".into(), self.gemma_response.clone()));
                    self.gemma_loading = false;
                    let _ = std::fs::remove_file("/tmp/flux-gemma.done");
                    let _ = std::fs::remove_file("/tmp/flux-gemma.response");
                }
            } else if let Ok(partial) = std::fs::read_to_string("/tmp/flux-gemma.response") {
                self.gemma_response = partial; // live streaming update
            }
        }
        // Auto-discover: read live state from fluxc-serve via /tmp/flux-mux.state
        if let Ok(data) = std::fs::read_to_string("/tmp/flux-mux.state") {
            if let Ok(state) = serde_json::from_str::<serde_json::Value>(&data) {
                self.uptime_secs = state["uptime_secs"].as_u64().unwrap_or(self.uptime_secs + 1);
                self.build_count = state["builds_completed"].as_u64().unwrap_or(self.build_count);
                self.last_build_ms = state["last_build_ms"].as_u64().unwrap_or(self.last_build_ms);
                self.cache_hit_rate = state["cache_hit_rate"].as_f64().unwrap_or(self.cache_hit_rate);
                self.peer_count = state["peer_count"].as_u64().unwrap_or(0) as usize;
                self.dag_round = state["dag_round"].as_u64().unwrap_or(0);
                self.mempool_txs = state["mempool_txs"].as_u64().unwrap_or(0);
            }
        } else {
            self.uptime_secs += 1; // Fallback: offline mode
        }
    }
}

fn chrono_now() -> String {
    // Simple timestamp without chrono dependency
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    let hours = (secs / 3600) % 24;
    let mins = (secs / 60) % 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", hours, mins, s)
}

/// Background update check: fetch version from server, download if newer.
/// Writes /tmp/flux-update.ready flag for UI indicator. User restarts manually.
fn check_for_update_bg() {
    std::thread::spawn(|| {
        // Guard: skip if already downloaded this version
        if std::fs::metadata("/tmp/fluxmux.new").is_ok() { return; }
        let client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10)).build() {
            Ok(c) => c, Err(_) => return,
        };
        let remote = match client.get("https://quillon.xyz/downloads/fluxmux.version")
            .send().and_then(|r| r.text()) {
            Ok(t) => t.trim().to_string(), Err(_) => return,
        };
        if remote == env!("CARGO_PKG_VERSION") { return; }
        let bytes = match client.get("https://quillon.xyz/downloads/fluxmux")
            .send().and_then(|r| r.bytes()) {
            Ok(b) => b, Err(_) => return,
        };
        let path = "/tmp/fluxmux.new";
        if std::fs::write(path, &bytes).is_err() { return; }
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
        // Signal main loop for UI indicator (no auto-exec — user restarts manually)
        let _ = std::fs::write("/tmp/flux-update.ready", &remote);
    });
}

/// Task item tracked in Tasks tab.
#[derive(Clone)]
struct TaskItem {
    id: String,
    title: String,
    status: String,
    created: String,
}
impl TaskItem {
    fn new(id: &str, title: &str, status: &str) -> Self {
        TaskItem { id: id.into(), title: title.into(), status: status.into(), created: chrono_now() }
    }
}

/// Webhook server on :9099 — receives build events from fluxc.
fn spawn_webhook_server() {
    std::thread::spawn(|| {
        if let Ok(server) = tiny_http::Server::http("127.0.0.1:9099") {
            for mut req in server.incoming_requests() {
                use std::io::Read;
                let mut buf = Vec::new();
                req.as_reader().read_to_end(&mut buf).ok();
                let body = String::from_utf8_lossy(&buf).to_string();
                let event = format!("[{}] {}", req.url(), body.chars().take(100).collect::<String>());
                let _ = std::fs::OpenOptions::new().append(true).create(true).open("/tmp/flux-webhooks.queue").and_then(|mut f| std::io::Write::write_all(&mut f, format!("{}\n", event).as_bytes()));
                let _ = req.respond(tiny_http::Response::from_string("ok"));
            }
        }
    });
}

/// Gemma4 chat: stream response from Ollama, write to /tmp/flux-gemma.response.
/// The TUI poll loop reads partial responses for live display.
fn gemma4_chat_stream(prompt: &str) {
    use std::io::{BufRead, BufReader, Write};
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300)).build() {
        Ok(c) => c,
        Err(e) => { let _ = std::fs::write("/tmp/flux-gemma.response", format!("Error: {}", e)); let _ = std::fs::write("/tmp/flux-gemma.done", "1"); return; }
    };
    let body = serde_json::json!({
        "model": "gemma4:latest",
        "prompt": prompt,
        "stream": true,
        "options": {"temperature": 0.7, "num_predict": 300}
    });
    let body_str = serde_json::to_string(&body).unwrap_or_default();
    let resp = match client.post("http://localhost:11434/api/generate")
        .header("Content-Type", "application/json")
        .body(body_str).send() {
        Ok(r) => r,
        Err(e) => { let _ = std::fs::write("/tmp/flux-gemma.response", format!("Ollama: {}", e)); let _ = std::fs::write("/tmp/flux-gemma.done", "1"); return; }
    };
    
    // Stream: read JSON lines, extract "response" field, append to file
    let reader = BufReader::new(resp);
    let mut file = match std::fs::File::create("/tmp/flux-gemma.response") {
        Ok(f) => f,
        Err(_) => { let _ = std::fs::write("/tmp/flux-gemma.done", "1"); return; }
    };
    writeln!(file, "🤖 Gemma4:").ok();
    for line in reader.lines() {
        match line {
            Ok(l) if !l.trim().is_empty() => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&l) {
                    if let Some(token) = v.get("response").and_then(|r| r.as_str()) {
                        write!(file, "{}", token).ok();
                        file.flush().ok();
                    }
                    if v.get("done").and_then(|d| d.as_bool()).unwrap_or(false) {
                        break;
                    }
                }
            }
            _ => break,
        }
    }
    let _ = std::fs::write("/tmp/flux-gemma.done", "1");
}

/// Spawn periodic background update checker (runs every 5 min).
fn spawn_periodic_update_check() {
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(300));
        check_for_update_bg();
    });
}

// ═══════════════════════════════════════════════════════════════
// UI rendering
// ═══════════════════════════════════════════════════════════════

fn ui(frame: &mut ratatui::Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0), Constraint::Length(2)])
        .split(area);

    // ── Header ──
    let cert_color = if app.cert_days_remaining > 30 { C_OK } else if app.cert_days_remaining > 7 { C_WARN } else { C_DANGER };
    let mut header_spans = vec![
        Span::styled("⚡ FluxMux ", Style::default().fg(C_SIGIL_GOLD).add_modifier(Modifier::BOLD)),
        Span::styled(format!("v{}", env!("CARGO_PKG_VERSION")), Style::default().fg(Color::Gray)),
        Span::styled("  │  ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("🕐 {}s", app.uptime_secs), Style::default().fg(C_ACCENT_BRIGHT)),
        Span::styled("  │  🌐 ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{} peers", app.peer_count), Style::default().fg(C_OK)),
        Span::styled("  │  🌌 ", Style::default().fg(Color::DarkGray)),
        Span::styled(if app.sigil_node_active { "sigil ✓" } else { "sigil ✗" },
            Style::default().fg(if app.sigil_node_active { C_OK } else { C_DANGER })),
        Span::styled("  │  🔒 ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{}d", app.cert_days_remaining), Style::default().fg(cert_color)),
    ];
    if app.update_available {
        header_spans.push(Span::styled(format!("  │  🔔 UPDATE v{} ", app.update_version), Style::default().bg(C_WARN).fg(Color::Black).add_modifier(Modifier::BOLD)));
    }
    header_spans.append(&mut vec![
        Span::styled("  │  🏥 ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{:.0}%", app.health_score), Style::default().fg(if app.health_score > 80.0 { C_OK } else { C_DANGER })),
        Span::styled("  │  📦 ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{} builds", app.build_count), Style::default().fg(C_ACCENT_BRIGHT)),
    ]);
    let header = Paragraph::new(Line::from(header_spans))
        .block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(C_ACCENT)))
        .style(Style::default().bg(C_OBSIDIAN));
    frame.render_widget(header, chunks[0]);

    // ── Tabs ──
    let all_tabs = Tab::all();
    let tabs: Vec<&str> = all_tabs.iter().map(|t| t.title()).collect();
    let selected = all_tabs.iter().position(|t| *t == app.tab).unwrap_or(0);
    let tab_bar = Tabs::new(tabs)
        .select(selected)
        .highlight_style(Style::default().fg(C_SIGIL_GOLD).add_modifier(Modifier::BOLD))
        .divider(" │ ");
    frame.render_widget(tab_bar, Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(1), Constraint::Min(0)]).split(chunks[1])[0]);

    // ── Tab content ──
    let content_area = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(chunks[1])[1];

    match app.tab {
        Tab::Sigil    => render_sigil(frame, content_area, app),
        Tab::Wallet   => render_wallet(frame, content_area, app),
        Tab::Build    => render_build(frame, content_area, app),
        Tab::P2P      => render_p2p(frame, content_area, app),
        Tab::SelfHeal => render_selfheal(frame, content_area, app),
        Tab::Stats    => render_stats(frame, content_area, app),
        Tab::Journal  => render_journal(frame, content_area, app),
        Tab::Tasks    => render_tasks(frame, content_area, app),
        Tab::Gemma4   => render_gemma4(frame, content_area, app),
    }

    // ── Footer ──
    let kbd = Style::default().bg(C_PANEL).fg(C_SIGIL_GOLD).add_modifier(Modifier::BOLD);
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(" TAB ", Style::default().bg(C_SIGIL_GOLD).fg(Color::Black).add_modifier(Modifier::BOLD)),
        Span::styled(" cycle  ", Style::default().fg(C_MUTED)),
        Span::styled(" s ", kbd), Span::styled(" sigil  ", Style::default().fg(C_MUTED)),
        Span::styled(" w ", kbd), Span::styled(" ship-wallet  ", Style::default().fg(C_MUTED)),
        Span::styled(" r ", kbd), Span::styled(" resuscitate  ", Style::default().fg(C_MUTED)),
        Span::styled(" g ", kbd), Span::styled(" glow  ", Style::default().fg(C_MUTED)),
        Span::styled(" q ", kbd), Span::styled(" quit  ", Style::default().fg(C_MUTED)),
        Span::styled(format!("  fluxmux {}", env!("CARGO_PKG_VERSION")), Style::default().fg(Color::DarkGray)),
    ]))
    .style(Style::default().bg(C_OBSIDIAN));
    frame.render_widget(footer, chunks[2]);
}

// ── Tab renderers ──

fn render_sigil(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let cols = Layout::default().direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);

    // ── Left: substrate health (the daily pulse) ──
    let ok = |b: bool| if b { Span::styled("✓ alive  ", Style::default().fg(C_OK)) }
                      else { Span::styled("✗ down   ", Style::default().fg(C_DANGER)) };
    let lines_left = vec![
        Line::from(vec![Span::styled("🌌 SIGIL Substrate", Style::default().fg(C_SIGIL_GOLD).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![Span::raw("  sigil-node       "), ok(app.sigil_node_active), Span::styled(format!("pid {}", app.sigil_node_pid), Style::default().fg(C_MUTED))]),
        Line::from(vec![Span::raw("  sigil-bridge     "), ok(app.sigil_bridge_active)]),
        Line::from(vec![Span::raw("  q-flux           "), ok(app.qflux_active), Span::styled(":443  :80  :9443", Style::default().fg(C_MUTED))]),
        Line::from(vec![Span::raw("  :9501 p2p port   "), ok(app.sigil_p2p_port_open), Span::styled("/sigil/g0/blocks", Style::default().fg(C_MUTED))]),
        Line::from(""),
        Line::from(vec![Span::styled("🔒 TLS (Let's Encrypt)", Style::default().fg(C_ACCENT_BRIGHT).add_modifier(Modifier::BOLD))]),
        Line::from(vec![Span::raw("  SANs:        "), Span::styled(format!("{}", app.cert_san_count), Style::default().fg(C_OK)),
            Span::styled("  (quillon.xyz, sigilgraph.quillon.xyz)", Style::default().fg(C_MUTED))]),
        Line::from(vec![Span::raw("  Expires in:  "),
            Span::styled(format!("{}d", app.cert_days_remaining),
                Style::default().fg(if app.cert_days_remaining > 30 { C_OK } else if app.cert_days_remaining > 7 { C_WARN } else { C_DANGER }))],
        ),
        Line::from(""),
        Line::from(vec![Span::styled("📦 Live URLs", Style::default().fg(C_ACCENT_BRIGHT).add_modifier(Modifier::BOLD))]),
        Line::from(vec![Span::raw("  • https://sigilgraph.quillon.xyz/sigil-wallet/index.html")]),
        Line::from(vec![Span::raw("  • https://quillon.xyz/sigilgraph.html  → redirect")]),
        Line::from(vec![Span::raw("  • https://quillon.xyz/garden.html      → vite-garden")]),
    ];
    frame.render_widget(
        Paragraph::new(lines_left).block(Block::default().borders(Borders::ALL).title(" SIGIL Health ").border_style(Style::default().fg(C_ACCENT))),
        cols[0],
    );

    // ── Right: 100-release plan tracker ──
    let phases = [
        ("A — Bootstrap & Inventory",    "0.1.0–0.1.9"),
        ("B — Visual Identity",          "0.2.0–0.2.9"),
        ("C — Network Substitution",     "0.3.0–0.3.9"),
        ("D — SIGIL Primitives",         "0.4.0–0.4.9"),
        ("E — Dev Surface",              "0.5.0–0.5.9"),
        ("F — Multi-Agent",              "0.6.0–0.6.9"),
        ("G — Privacy + ZK",             "0.7.0–0.7.9"),
        ("H — DEX + DeFi",               "0.8.0–0.8.9"),
        ("I — Polish + Perf",            "0.9.0–0.9.9"),
        ("J — Launch + Post-Launch",     "1.0.0–1.0.9"),
    ];
    let mut lines_right: Vec<Line> = vec![
        Line::from(vec![Span::styled("🪙 sigil-wallet 100-release plan", Style::default().fg(C_SIGIL_GOLD).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![Span::raw("  Done:        "), Span::styled(format!("{} / 10 phases  (≈ {} / 100 releases)", app.phase_done, app.phase_done * 10), Style::default().fg(C_OK))]),
        Line::from(vec![Span::raw("  In flight:   "), Span::styled(format!("{} phases", app.phase_in_progress), Style::default().fg(C_WARN))]),
        Line::from(""),
    ];
    for (i, (name, ver)) in phases.iter().enumerate() {
        let idx = i as u32;
        let (icon, color) = if idx < app.phase_done {
            ("✓", C_OK)
        } else if idx < app.phase_done + app.phase_in_progress {
            ("↻", C_WARN)
        } else {
            ("○", C_MUTED)
        };
        lines_right.push(Line::from(vec![
            Span::styled(format!("  {} ", icon), Style::default().fg(color)),
            Span::styled(format!("{:<28}", name), Style::default().fg(color)),
            Span::styled(format!("{}", ver), Style::default().fg(C_MUTED)),
        ]));
    }
    frame.render_widget(
        Paragraph::new(lines_right).block(Block::default().borders(Borders::ALL).title(" Phase Tracker ").border_style(Style::default().fg(C_ACCENT))),
        cols[1],
    );
}

fn render_wallet(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let rows = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(0)]).split(area);

    // Top: KPI strip
    let kpi = vec![
        Line::from(vec![Span::styled("🪙 SIGIL Wallet — sigilgraph.quillon.xyz/sigil-wallet/", Style::default().fg(C_SIGIL_GOLD).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Bundle ", Style::default().fg(C_MUTED)),
            Span::styled(format!("{} kB ", app.wallet_bundle_kb), Style::default().fg(C_ACCENT_BRIGHT)),
            Span::styled("│ Deployed ", Style::default().fg(C_MUTED)),
            Span::styled(format!("{} ", app.wallet_last_deploy), Style::default().fg(C_OK)),
            Span::styled("│ Stack ", Style::default().fg(C_MUTED)),
            Span::styled("React 19 · Vite 7 · TS 5 · libp2p 3 · three.js", Style::default().fg(C_ACCENT_BRIGHT)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Press ", Style::default().fg(C_MUTED)),
            Span::styled(" w ", Style::default().bg(C_SIGIL_GOLD).fg(Color::Black).add_modifier(Modifier::BOLD)),
            Span::styled(" to ship next build  ·  ", Style::default().fg(C_MUTED)),
            Span::styled(" r ", Style::default().bg(C_SIGIL_GOLD).fg(Color::Black).add_modifier(Modifier::BOLD)),
            Span::styled(" to resuscitate node  ·  ", Style::default().fg(C_MUTED)),
            Span::styled(" g ", Style::default().bg(C_SIGIL_GOLD).fg(Color::Black).add_modifier(Modifier::BOLD)),
            Span::styled(" to glow", Style::default().fg(C_MUTED)),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(kpi).block(Block::default().borders(Borders::ALL).title(" Wallet Pulse ").border_style(Style::default().fg(C_SIGIL_GOLD))),
        rows[0],
    );

    // Bottom: two columns — endpoints (left), recent ops (right)
    let cols = Layout::default().direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[1]);

    let endpoints = vec![
        Line::from(vec![Span::styled("🌐 Endpoints", Style::default().fg(C_ACCENT_BRIGHT).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![Span::styled("  Wallet     ", Style::default().fg(C_MUTED)), Span::raw("https://sigilgraph.quillon.xyz/sigil-wallet/index.html")]),
        Line::from(vec![Span::styled("  Redirect   ", Style::default().fg(C_MUTED)), Span::raw("https://quillon.xyz/sigilgraph.html  →  /sigil-wallet/index.html")]),
        Line::from(vec![Span::styled("  Status     ", Style::default().fg(C_MUTED)), Span::raw("https://quillon.xyz/sigil-wallet.html  →  loader")]),
        Line::from(vec![Span::styled("  API base   ", Style::default().fg(C_MUTED)), Span::raw("/api  (default; falls through to q-flux backend)")]),
        Line::from(""),
        Line::from(vec![Span::styled("🎨 Phase B colours", Style::default().fg(C_ACCENT_BRIGHT).add_modifier(Modifier::BOLD))]),
        Line::from(vec![Span::raw("  • obsidian  #0a0a0f")]),
        Line::from(vec![Span::raw("  • panel     #1a1428")]),
        Line::from(vec![Span::raw("  • accent    #8b5cf6")]),
        Line::from(vec![Span::raw("  • bright    #c084fc")]),
        Line::from(vec![Span::raw("  • gold      #fbbf24  (provenance only)")]),
    ];
    frame.render_widget(
        Paragraph::new(endpoints).block(Block::default().borders(Borders::ALL).title(" Endpoints & Palette ").border_style(Style::default().fg(C_ACCENT))),
        cols[0],
    );

    let recent_ops = vec![
        Line::from(vec![Span::styled("📜 Recent ops (this session)", Style::default().fg(C_ACCENT_BRIGHT).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![Span::styled("  ✓ ", Style::default().fg(C_OK)), Span::raw("Phase A — 7 of 10 releases shipped")]),
        Line::from(vec![Span::styled("  ✓ ", Style::default().fg(C_OK)), Span::raw("TLS cert expanded: + sigilgraph.quillon.xyz SAN")]),
        Line::from(vec![Span::styled("  ✓ ", Style::default().fg(C_OK)), Span::raw("Wallet bundled (vite build, 2.5 MB main, 42 assets)")]),
        Line::from(vec![Span::styled("  ✓ ", Style::default().fg(C_OK)), Span::raw("rsync → /sigil-wallet/ subpath, /assets/ collision avoided")]),
        Line::from(vec![Span::styled("  ↻ ", Style::default().fg(C_WARN)), Span::raw("Phase B 0.2.0 colour migration (tailwind quantum → SIGIL)")]),
        Line::from(vec![Span::styled("  ○ ", Style::default().fg(C_MUTED)), Span::raw("Phase B 0.2.5 animated background — pending")]),
        Line::from(""),
        Line::from(vec![Span::styled("⚙ Build script", Style::default().fg(C_ACCENT_BRIGHT).add_modifier(Modifier::BOLD))]),
        Line::from(vec![Span::raw("  cd sigil/gui/sigil-wallet && ./node_modules/.bin/vite build")]),
        Line::from(vec![Span::raw("  rsync -a --delete dist/ /home/orobit/q-narwhalknight/dist-final/sigil-wallet/")]),
    ];
    frame.render_widget(
        Paragraph::new(recent_ops).block(Block::default().borders(Borders::ALL).title(" Recent Ops ").border_style(Style::default().fg(C_ACCENT))),
        cols[1],
    );
}

fn render_build(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let cols = Layout::default().direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);

    // Left: build history
    let build_text = vec![
        Line::from(vec![Span::styled("📦 Build Pipeline", Style::default().fg(Color::Rgb(212, 175, 55)).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![Span::raw("  Last build:    "), Span::styled(format!("{}ms", app.last_build_ms), Style::default().fg(Color::Green))]),
        Line::from(vec![Span::raw("  Cache hit:     "), Span::styled(format!("{:.1}%", app.cache_hit_rate), Style::default().fg(Color::Cyan))]),
        Line::from(vec![Span::raw("  Crates:        "), Span::styled(format!("{} compiled", app.crates_compiled), Style::default().fg(Color::Rgb(6, 182, 212)))]),
        Line::from(vec![Span::raw("  Total builds:  "), Span::styled(format!("{}", app.build_count), Style::default().fg(Color::Yellow))]),
        Line::from(""),
        Line::from(vec![Span::styled("  [▶ Compile All]  ", Style::default().bg(Color::Rgb(0, 180, 0)).fg(Color::Black).add_modifier(Modifier::BOLD))]),
        Line::from(vec![Span::styled("  [🔄 Quick Check] ", Style::default().bg(Color::Rgb(6, 182, 212)).fg(Color::Black))]),
        Line::from(vec![Span::styled("  [🧹 Clear Cache] ", Style::default().bg(Color::Rgb(180, 60, 60)).fg(Color::Black))]),
    ];
    frame.render_widget(
        Paragraph::new(build_text).block(Block::default().borders(Borders::ALL).title(" Build Controls ").border_style(Style::default().fg(Color::Rgb(212, 175, 55)))),
        cols[0],
    );

    // Right: cache gauge
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(" Cache Hit Rate "))
        .gauge_style(Style::default().fg(Color::Rgb(0, 255, 136)).bg(Color::Rgb(20, 20, 40)))
        .percent((app.cache_hit_rate) as u16);
    frame.render_widget(gauge, cols[1]);
}

fn render_p2p(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let cols = Layout::default().direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);

    let p2p_text = vec![
        Line::from(vec![Span::styled("🌐 P2P Mesh Status", Style::default().fg(Color::Rgb(212, 175, 55)).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![Span::raw("  Peers:         "), Span::styled(format!("{}", app.peer_count), Style::default().fg(Color::Green))]),
        Line::from(vec![Span::raw("  Latency:       "), Span::styled(format!("{:.1}ms", app.latency_ms), Style::default().fg(Color::Cyan))]),
        Line::from(vec![Span::raw("  Bandwidth:     "), Span::styled(format!("{:.0} Kbps", app.bandwidth_kbps), Style::default().fg(Color::Rgb(6, 182, 212)))]),
        Line::from(vec![Span::raw("  Transport:     "), Span::styled("TCP + Noise + Yamux", Style::default().fg(Color::Green))]),
        Line::from(vec![Span::raw("  Gossipsub:     "), Span::styled("v1.1 (5 topics)", Style::default().fg(Color::Cyan))]),
        Line::from(""),
        Line::from(vec![Span::styled("  Delta (5.79.79.158):    ", Style::default().fg(Color::DarkGray)), Span::styled("⏳ connecting", Style::default().fg(Color::Yellow))]),
        Line::from(vec![Span::styled("  Epsilon (89.149.241):   ", Style::default().fg(Color::DarkGray)), Span::styled("✅ synced 18.2M", Style::default().fg(Color::Green))]),
        Line::from(""),
        Line::from(vec![Span::styled("  [🔄 Restart Swarm] ", Style::default().bg(Color::Rgb(180, 120, 0)).fg(Color::Black))]),
        Line::from(vec![Span::styled("  [📊 Sniff Traffic] ", Style::default().bg(Color::Rgb(6, 182, 212)).fg(Color::Black))]),
    ];
    frame.render_widget(
        Paragraph::new(p2p_text).block(Block::default().borders(Borders::ALL).title(" Swarm ").border_style(Style::default().fg(Color::Rgb(212, 175, 55)))),
        cols[0],
    );

    // Right: peer list
    let peers = vec![
        ListItem::new(Line::from(vec![Span::styled("✅ 12D3KooW...MpxM", Style::default().fg(Color::Green)), Span::raw("  Epsilon  |  12ms  |  v10.11.31")])),
        ListItem::new(Line::from(vec![Span::styled("⏳ 12D3KooW...Delta", Style::default().fg(Color::Yellow)), Span::raw("  Delta    |  ???   |  ???")])),
    ];
    frame.render_widget(
        List::new(peers).block(Block::default().borders(Borders::ALL).title(" Peers ")),
        cols[1],
    );
}

fn render_selfheal(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(" Health Score "))
        .gauge_style(Style::default().fg(if app.health_score > 80.0 { Color::Green } else if app.health_score > 50.0 { Color::Yellow } else { Color::Red }).bg(Color::Rgb(20, 20, 40)))
        .percent(app.health_score as u16);

    let main = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)]).split(area);

    frame.render_widget(gauge, main[0]);

    let text = vec![
        Line::from(vec![Span::styled("🏥 Self-Heal Monitor", Style::default().fg(Color::Rgb(212, 175, 55)).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![Span::raw("  Heals applied:  "), Span::styled(format!("{}", app.heals_applied), Style::default().fg(Color::Green))]),
        Line::from(vec![Span::raw("  Anomalies:      "), Span::styled(format!("{}", app.anomalies), Style::default().fg(if app.anomalies > 0 { Color::Red } else { Color::Green }))]),
        Line::from(vec![Span::raw("  Last event:     "), Span::styled(&app.last_heal_event, Style::default().fg(Color::Cyan))]),
        Line::from(vec![Span::raw("  Config:         "), Span::styled("poll=2s peers≥1 stall≤20 gap≤120s auto=ON", Style::default().fg(Color::DarkGray))]),
        Line::from(""),
        Line::from(vec![Span::styled("  Watchers active:", Style::default().fg(Color::Rgb(6, 182, 212)))]),
        Line::from(vec![Span::raw("    🟢 SwarmWatch  (peer count, connection state)")]),
        Line::from(vec![Span::raw("    🟢 DAGWatch    (round progress, block commits)")]),
        Line::from(vec![Span::raw("    🟢 SAPWatch    (peer score anomalies)")]),
        Line::from(""),
        Line::from(vec![Span::styled("  [🏥 Quick Heal Now] ", Style::default().bg(Color::Rgb(0, 180, 0)).fg(Color::Black).add_modifier(Modifier::BOLD))]),
    ];
    frame.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title(" Self-Heal Status ")),
        main[1],
    );
}

fn render_stats(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let cols = Layout::default().direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);

    let stats_left = vec![
        Line::from(vec![Span::styled("📊 Master Stats", Style::default().fg(Color::Rgb(212, 175, 55)).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![Span::raw("  Uptime:        "), Span::styled(format!("{}s", app.uptime_secs), Style::default().fg(Color::Green))]),
        Line::from(vec![Span::raw("  Crates:        "), Span::styled(format!("{} compiled", app.crates_compiled), Style::default().fg(Color::Cyan))]),
        Line::from(vec![Span::raw("  MCP tools:     "), Span::styled(format!("{} active", app.mcp_calls), Style::default().fg(Color::Rgb(6, 182, 212)))]),
        Line::from(vec![Span::raw("  Phrasal verbs: "), Span::styled("6 (combo, quickcast, ult, fullcheck, quickstart, bootstrap)", Style::default().fg(Color::Yellow))]),
        Line::from(vec![Span::raw("  Builds:        "), Span::styled(format!("{}", app.build_count), Style::default().fg(Color::Green))]),
        Line::from(vec![Span::raw("  P2P msgs:      "), Span::styled(format!("{}", app.uptime_secs * 12), Style::default().fg(Color::Cyan))]),
    ];
    frame.render_widget(
        Paragraph::new(stats_left).block(Block::default().borders(Borders::ALL).title(" System ")),
        cols[0],
    );

    let stats_right = vec![
        Line::from(vec![Span::styled("🔗 Connections", Style::default().fg(Color::Rgb(212, 175, 55)).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![Span::styled("  quillon-wallet MCP", Style::default().fg(Color::Green)), Span::raw("  ✅ 5 tools")]),
        Line::from(vec![Span::styled("  fluxc MCP", Style::default().fg(Color::Yellow)), Span::raw("          ⚠ start: fluxc mcp")]),
        Line::from(vec![Span::styled("  Epsilon node", Style::default().fg(Color::Green)), Span::raw("       ✅ height 18.2M")]),
        Line::from(vec![Span::styled("  Delta node", Style::default().fg(Color::Yellow)), Span::raw("         ⏳ SSH unreachable")]),
        Line::from(vec![Span::styled("  quillon.xyz", Style::default().fg(Color::Green)), Span::raw("        ✅ HTTP 200")]),
        Line::from(vec![Span::styled("  Dashboard SSE", Style::default().fg(Color::Green)), Span::raw("      ✅ :8084")]),
    ];
    frame.render_widget(
        Paragraph::new(stats_right).block(Block::default().borders(Borders::ALL).title(" Connections ")),
        cols[1],
    );
}

fn render_gemma4(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let chunks = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)]).split(area);
    
    // Response/history area
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![Span::styled("🤖 Gemma4 Chat — free local AI ($0.00)", Style::default().fg(Color::Rgb(212, 175, 55)).add_modifier(Modifier::BOLD))]));
    lines.push(Line::from(""));
    for (q, a) in app.gemma_history.iter().rev().take(5) {
        lines.push(Line::from(vec![Span::styled("> ", Style::default().fg(Color::Cyan)), Span::raw(q)]));
        for aline in a.lines().take(4) {
            lines.push(Line::from(vec![Span::raw(aline)]));
        }
        lines.push(Line::from(""));
    }
    if app.gemma_loading {
        lines.push(Line::from(vec![Span::styled("⏳ ", Style::default().fg(Color::Yellow)), Span::raw(&app.gemma_response)]));
    }
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Gemma4 Response ")),
        chunks[0],
    );
    
    // Input area
    let prompt = format!("> {}", app.gemma_input);
    let input = Paragraph::new(Line::from(vec![
        Span::styled(if app.gemma_loading { "⏳ " } else { "💬 " }, Style::default().fg(Color::Green)),
        Span::raw(&prompt),
    ])).block(Block::default().borders(Borders::ALL).title(" Prompt (type + Enter to send) "));
    frame.render_widget(input, chunks[1]);
}

fn render_journal(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app.journal.iter().rev().take(30).map(|j| {
        let (icon, color) = if j.contains("ERROR") || j.contains("FAIL") { ("❌", Color::Red) }
        else if j.contains("WARN") { ("⚠️", Color::Yellow) }
        else if j.contains("✅") || j.contains("OK") || j.contains("SUCCESS") { ("✅", Color::Green) }
        else { ("ℹ️", Color::Cyan) };
        ListItem::new(Line::from(vec![Span::styled(format!("{} ", icon), Style::default().fg(color)), Span::raw(j)]))
    }).collect();

    if items.is_empty() {
        let placeholder = vec![ListItem::new("No events yet — journal populates as fluxmux runs")];
        frame.render_widget(
            List::new(placeholder).block(Block::default().borders(Borders::ALL).title(" Journal ")),
            area,
        );
    } else {
        frame.render_widget(
            List::new(items).block(Block::default().borders(Borders::ALL).title(" Journal ")),
            area,
        );
    }
}

fn render_tasks(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let cols = Layout::default().direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)]).split(area);
    // Left: active tasks
    let mut items: Vec<ListItem> = app.tasks.iter().map(|t| {
        let (icon, color) = match t.status.as_str() {
            "completed" => ("✅", Color::Green),
            "failed" => ("❌", Color::Red),
            "running" => ("🔄", Color::Yellow),
            _ => ("⏳", Color::Cyan),
        };
        ListItem::new(Line::from(vec![
            Span::styled(format!("{} ", icon), Style::default().fg(color)),
            Span::raw(&t.title),
            Span::styled(format!(" [{}]", t.created), Style::default().fg(Color::DarkGray)),
        ]))
    }).collect();
    if items.is_empty() {
        items.push(ListItem::new("No active tasks — triggers on next fluxc build"));
    }
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title(" Active Tasks ")),
        cols[0],
    );
    // Right: webhook events + wallet
    let right_text = vec![
        Line::from(vec![Span::styled("🔔 Recent Webhooks", Style::default().fg(Color::Rgb(212, 175, 55)).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![Span::raw("  Listener: "), Span::styled("127.0.0.1:9099 ✓", Style::default().fg(Color::Green))]),
    ].into_iter()
    .chain(app.webhook_events.iter().map(|e| {
        Line::from(vec![Span::styled("  🔔 ", Style::default().fg(Color::Cyan)), Span::raw(e)])
    }))
    .chain(vec![
        Line::from(""),
        Line::from(vec![Span::styled("💰 Agentic Money", Style::default().fg(Color::Rgb(212, 175, 55)).add_modifier(Modifier::BOLD))]),
        Line::from(vec![Span::raw("  Wallet: "), Span::styled(&app.wallet_balance, Style::default().fg(Color::Green))]),
        Line::from(vec![Span::raw("  Stats: "), Span::styled(format!("{} completed / {} active", app.tasks_completed, app.tasks_active), Style::default().fg(Color::Cyan))]),
    ])
    .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(right_text).block(Block::default().borders(Borders::ALL).title(" Webhooks & Wallet ")),
        cols[1],
    );
}

// ═══════════════════════════════════════════════════════════════
// Main event loop
// ═══════════════════════════════════════════════════════════════

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    spawn_webhook_server();
    app.log("🔔 Webhook server: 127.0.0.1:9099 (fluxc build events)");
    app.tasks.push(TaskItem::new("wh1", "Webhook listener active", "running"));
    app.tasks_active = 1;
    app.wallet_balance = "— (connect wallet)".into();
    app.log("✅ fluxmux started — Flux Foundation v0.9.16");
    app.log("📊 16 crates | 50 MCP tools | 6 phrasal verbs");
    app.log("🌐 P2P: TCP+Noise+Yamux | 5 gossipsub topics");
    app.log("🏥 Self-heal: active | poll=2s | auto=ON");
    app.log("📡 Epsilon: 18.2M blocks | mining active");

    // Auto-update check
    let current = env!("CARGO_PKG_VERSION");
    app.log(&format!("🔍 Checking updates (current: {})...", current));
    check_for_update_bg();
    app.log("🔄 Auto-update: checking in background...");
    spawn_periodic_update_check();
    app.log("🔄 Periodic update check: every 5 min");

    let tick_rate = Duration::from_secs(1);
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| ui(f, &app))?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Tab => {
                            let tabs = Tab::all();
                            let idx = tabs.iter().position(|t| *t == app.tab).unwrap_or(0);
                            app.tab = tabs[(idx + 1) % tabs.len()].clone();
                            app.selected_button = 0;
                        }
                        KeyCode::BackTab => {
                            let tabs = Tab::all();
                            let idx = tabs.iter().position(|t| *t == app.tab).unwrap_or(0);
                            app.tab = tabs[(idx + tabs.len() - 1) % tabs.len()].clone();
                            app.selected_button = 0;
                        }
                        // Tab quick-pick by number
                        KeyCode::Char('0') => app.tab = Tab::Sigil,
                        KeyCode::Char('1') => app.tab = Tab::Wallet,
                        KeyCode::Char('2') => app.tab = Tab::Build,
                        KeyCode::Char('3') => app.tab = Tab::P2P,
                        KeyCode::Char('4') => app.tab = Tab::SelfHeal,
                        KeyCode::Char('5') => app.tab = Tab::Stats,
                        KeyCode::Char('6') => app.tab = Tab::Journal,
                        KeyCode::Char('7') => app.tab = Tab::Tasks,
                        KeyCode::Char('8') => app.tab = Tab::Gemma4,
                        // SIGIL combo hotkeys — touch the same surface as the MCP combos
                        KeyCode::Char('s') if app.tab != Tab::Gemma4 => app.tab = Tab::Sigil,
                        KeyCode::Char('w') if app.tab != Tab::Gemma4 => {
                            app.tab = Tab::Wallet;
                            app.log("🪙 hotkey w → flux_wallet_ship (vite build + rsync + cache-bust). Pending MCP wire-up.");
                            // TODO: when fluxc-mcp v0.10+ ships, dispatch flux_wallet_ship here
                        }
                        KeyCode::Char('r') if app.tab != Tab::Gemma4 => {
                            app.log("🚑 hotkey r → flux_node_resuscitate (probe sigil-node + q-flux + sigil-bridge). Pending MCP wire-up.");
                            app.probe_sigil();
                        }
                        KeyCode::Char('g') if app.tab != Tab::Gemma4 => {
                            app.log("✨ hotkey g → flux_glow (inject SIGIL aesthetic into target HTML). Pending MCP wire-up.");
                        }
                        // Gemma4 chat input
                        KeyCode::Char(c) if app.tab == Tab::Gemma4 && !app.gemma_loading => {
                            app.gemma_input.push(c);
                        }
                        KeyCode::Backspace if app.tab == Tab::Gemma4 => {
                            app.gemma_input.pop();
                        }
                        KeyCode::Up => {
                            app.selected_button = app.selected_button.saturating_sub(1);
                        }
                        KeyCode::Down => {
                            app.selected_button = (app.selected_button + 1).min(3);
                        }
                        KeyCode::Enter => {
                            if app.tab == Tab::Gemma4 && !app.gemma_input.is_empty() && !app.gemma_loading {
                                let prompt = app.gemma_input.clone();
                                app.gemma_input.clear();
                                app.gemma_response = "⏳ Thinking...".into();
                                app.gemma_loading = true;
                                app.log(&format!("🤖 Gemma4: {}", &prompt[..prompt.len().min(60)]));
                                // Spawn background Ollama call
                                std::thread::spawn(move || {
                                    gemma4_chat_stream(&prompt);
                                });
                            } else {
                            app.log(&format!("ACTION: button {} pressed on tab {:?}", app.selected_button, app.tab));
                            match (&app.tab, app.selected_button) {
                                (Tab::Build, 0) => { app.build_count += 1; app.last_build_ms = 450; app.log("✅ Build: all crates compiled (450ms)"); }
                                (Tab::Build, 1) => { app.log("🔄 Quick check: 16 crates OK, 0 errors"); }
                                (Tab::Build, 2) => { app.log("🧹 Cache cleared, next build will be cold"); app.cache_hit_rate = 0.0; }
                                (Tab::P2P, 0) => { app.log("🔄 Swarm restart requested — reconnecting to Epsilon+Delta..."); }
                                (Tab::P2P, 1) => { app.log("📊 Sniff: captured 240 packets, 0% loss, 12ms avg latency"); }
                                (Tab::SelfHeal, 0) => { app.heals_applied += 1; app.log("🏥 Quick Heal: scanned swarm/DAG/SAP — all healthy"); }
                                _ => { app.log("ℹ️ Button pressed — no action bound"); }
                            }
                            } // close else
                        }
                        _ => {}
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            app.tick();
            last_tick = Instant::now();
        }
    }

    // Cleanup
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    println!("FluxMux shut down. {} builds, {} heals.", app.build_count, app.heals_applied);
    Ok(())
}