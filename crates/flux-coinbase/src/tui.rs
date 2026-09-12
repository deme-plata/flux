//! flux-coinbase TUI — a real trading terminal in ratatui: live order book with depth bars,
//! candlestick chart with SMA overlay, trade tape, balances, perp positions with liquidation
//! distance, open orders, and the leveraged-DCA panel. Every write goes through the same Gate +
//! confirm discipline as the CLI and the MCP tools: in PAPER mode (default) Enter on a proposal
//! logs what *would* be sent; only `--live` lets Enter place an order, and only after the preview
//! from Coinbase has been shown.
//!
//! Keys: `1-5`/Tab tabs · `←/→` product · `g` granularity · `b`/`s` buy/sell form · `d` DCA
//! proposal · `c` cancel-all · `r` refresh · `q` quit. Inside a form: digits/`.` amount, `+`/`-`
//! leverage, `m` market/limit, `Tab` field, `Enter` preview → place, `Esc` close.

use crate::dca::{self, DcaConfig, DcaState};
use crate::{base_from_quote, build_order, is_perp, Balance, Book, Candle, Coinbase, Gate, OpenOrder,
            OrderKind, Position, ProductInfo, Tape, Verdict};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::prelude::*;
use ratatui::widgets::canvas::{Canvas, Line as CLine};
use ratatui::widgets::{Block, BorderType, Cell, Clear, Paragraph, Row, Table, Tabs, Wrap};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const GRANS: [&str; 6] = ["1m", "5m", "15m", "1h", "6h", "1d"];
const TABS: [&str; 5] = ["Trade", "DCA", "Orders", "Account", "Log"];

#[derive(Clone)]
pub struct TuiOpts {
    pub product: String,
    pub products: Vec<String>,
    pub live: bool,
    pub gran: String,
    pub dca: DcaConfig,
}

impl Default for TuiOpts {
    fn default() -> Self {
        TuiOpts { product: "BTC-USD".into(),
                  products: vec!["BTC-USD".into(), "ETH-USD".into(), "SOL-USD".into(), "BTC-PERP-INTX".into(), "ETH-PERP-INTX".into()],
                  live: false, gran: "1h".into(), dca: DcaConfig::default() }
    }
}

enum Msg {
    Book(Book), Tape(Tape), Candles(Vec<Candle>), Product(ProductInfo),
    Accounts(Vec<Balance>), Positions(Vec<Position>, Option<String>), Orders(Vec<OpenOrder>),
    Latency(u64), Err(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Field { Usd, Price }

struct OrderForm {
    side: String, usd: String, price: String, leverage: u32, limit: bool, field: Field,
    proposal: Option<Value>, preview: Option<Value>, error: Option<String>,
}

enum Pending { CancelAll, Place(Value), Dca(Value) }

struct Shared { product: String, gran: String, refresh: bool }

struct App {
    opts: TuiOpts,
    shared: Arc<Mutex<Shared>>,
    client: Arc<Coinbase>,
    tab: usize,
    book: Book, tape: Tape, candles: Vec<Candle>, product: ProductInfo,
    accounts: Vec<Balance>, positions: Vec<Position>, positions_note: Option<String>, orders: Vec<OpenOrder>,
    log: VecDeque<String>, latency_ms: u64, last_data: Instant, errors: u32,
    form: Option<OrderForm>, pending: Option<Pending>,
    dca_state: DcaState, dca_plan: Option<Value>,
    gate: Gate, quit: bool,
}

impl App {
    fn log(&mut self, s: impl Into<String>) {
        let t = chrono_hms();
        self.log.push_front(format!("{t} {}", s.into()));
        if self.log.len() > 200 { self.log.pop_back(); }
    }
    fn product_id(&self) -> String { self.shared.lock().map(|s| s.product.clone()).unwrap_or_default() }
    fn gran(&self) -> String { self.shared.lock().map(|s| s.gran.clone()).unwrap_or_else(|_| "1h".into()) }
    fn switch_product(&mut self, delta: i32) {
        let pid = self.product_id();
        let n = self.opts.products.len() as i32;
        let i = self.opts.products.iter().position(|p| *p == pid).unwrap_or(0) as i32;
        let next = ((i + delta) % n + n) % n;
        let np = self.opts.products[next as usize].clone();
        if let Ok(mut s) = self.shared.lock() { s.product = np.clone(); s.refresh = true; }
        self.book = Book::default(); self.tape = Tape::default(); self.candles.clear(); self.product = ProductInfo::default();
        self.dca_plan = None;
        self.dca_state = DcaState::load(&self.dca_cfg().state_path());
        self.log(format!("→ {np}"));
    }
    fn cycle_gran(&mut self) {
        let g = self.gran();
        let i = GRANS.iter().position(|x| *x == g).unwrap_or(3);
        let ng = GRANS[(i + 1) % GRANS.len()].to_string();
        if let Ok(mut s) = self.shared.lock() { s.gran = ng.clone(); s.refresh = true; }
        self.candles.clear();
        self.log(format!("candles {ng}"));
    }
    fn dca_cfg(&self) -> DcaConfig {
        let mut c = self.opts.dca.clone();
        c.product_id = self.product_id();
        if !c.gate.whitelist.iter().any(|w| w.eq_ignore_ascii_case(&c.product_id)) { c.gate.whitelist.push(c.product_id.clone()); }
        c
    }
    fn mark(&self) -> f64 {
        self.book.mid().filter(|m| *m > 0.0).unwrap_or(self.product.price)
    }
}

fn chrono_hms() -> String {
    let s = crate::now_s() % 86_400;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}
fn hms_of(rfc3339: &str) -> String { rfc3339.get(11..19).unwrap_or(rfc3339).to_string() }

fn fmt_px(p: f64) -> String {
    if p >= 1000.0 { format!("{p:>12.2}") } else if p >= 1.0 { format!("{p:>12.4}") } else { format!("{p:>12.6}") }
}
/// Sizes span 8 orders of magnitude (0.00000063 BTC … 120000 PEPE); show what is there.
fn fmt_size(x: f64) -> String {
    if x >= 100.0 { format!("{x:.2}") } else if x >= 1.0 { format!("{x:.4}") } else if x >= 0.01 { format!("{x:.5}") } else { format!("{x:.8}") }
}
fn fmt_usd(x: f64) -> String {
    let neg = x < 0.0; let x = x.abs();
    let int = x.trunc() as u64; let frac = ((x - x.trunc()) * 100.0).round() as u64;
    let s = int.to_string(); let mut out = String::new();
    for (i, ch) in s.chars().enumerate() { if i > 0 && (s.len() - i) % 3 == 0 { out.push(','); } out.push(ch); }
    format!("{}{out}.{frac:02}", if neg { "-" } else { "" })
}

// ────────────────────────────────────────── data thread ──────────────────────────────────────────

fn spawn_fetcher(client: Arc<Coinbase>, shared: Arc<Mutex<Shared>>, tx: mpsc::Sender<Msg>) {
    std::thread::spawn(move || {
        let mut n: u64 = 0;
        loop {
            let (pid, gran, refresh) = match shared.lock() {
                Ok(mut s) => { let r = s.refresh; s.refresh = false; (s.product.clone(), s.gran.clone(), r) }
                Err(_) => break,
            };
            let t0 = Instant::now();
            match client.book(&pid, 25) { Ok(b) => { let _ = tx.send(Msg::Book(b)); }, Err(e) => { let _ = tx.send(Msg::Err(format!("book: {e}"))); } }
            let _ = tx.send(Msg::Latency(t0.elapsed().as_millis() as u64));
            if n % 2 == 0 || refresh {
                match client.tape(&pid, 40) { Ok(t) => { let _ = tx.send(Msg::Tape(t)); }, Err(e) => { let _ = tx.send(Msg::Err(format!("tape: {e}"))); } }
            }
            if n % 5 == 0 || refresh {
                match client.product(&pid) { Ok(p) => { let _ = tx.send(Msg::Product(p)); }, Err(e) => { let _ = tx.send(Msg::Err(format!("product: {e}"))); } }
            }
            if n % 20 == 0 || refresh {
                match client.candles(&pid, &gran, 120) { Ok(c) => { let _ = tx.send(Msg::Candles(c)); }, Err(e) => { let _ = tx.send(Msg::Err(format!("candles: {e}"))); } }
            }
            if client.is_signed() && (n % 8 == 0 || refresh) {
                match client.accounts() { Ok(a) => { let _ = tx.send(Msg::Accounts(a)); }, Err(e) => { let _ = tx.send(Msg::Err(format!("accounts: {e}"))); } }
                match client.positions() { Ok((p, w)) => { let _ = tx.send(Msg::Positions(p, w)); }, Err(e) => { let _ = tx.send(Msg::Err(format!("positions: {e}"))); } }
                match client.open_orders("") { Ok(o) => { let _ = tx.send(Msg::Orders(o)); }, Err(e) => { let _ = tx.send(Msg::Err(format!("orders: {e}"))); } }
            }
            n += 1;
            std::thread::sleep(Duration::from_millis(900));
        }
    });
}

// ──────────────────────────────────────────── entry ────────────────────────────────────────────

pub fn run(opts: TuiOpts) -> io::Result<()> {
    let client = Arc::new(Coinbase::best_effort().map_err(|e| io::Error::new(io::ErrorKind::Other, e))?);
    let shared = Arc::new(Mutex::new(Shared { product: opts.product.to_uppercase(), gran: opts.gran.clone(), refresh: true }));
    let (tx, rx) = mpsc::channel();
    spawn_fetcher(client.clone(), shared.clone(), tx);

    let mut app = App {
        opts: opts.clone(), shared, client: client.clone(), tab: 0,
        book: Book::default(), tape: Tape::default(), candles: vec![], product: ProductInfo::default(),
        accounts: vec![], positions: vec![], positions_note: None, orders: vec![],
        log: VecDeque::new(), latency_ms: 0, last_data: Instant::now(), errors: 0,
        form: None, pending: None, dca_state: DcaState::default(), dca_plan: None,
        gate: Gate::default(), quit: false,
    };
    app.dca_state = DcaState::load(&app.dca_cfg().state_path());
    if !app.opts.products.iter().any(|p| p.eq_ignore_ascii_case(&app.opts.product)) { app.opts.products.insert(0, app.opts.product.to_uppercase()); }
    app.log(format!("flux-coinbase · key {} · {}", client.key_fingerprint(), if opts.live { "LIVE — Enter can place orders" } else { "PAPER — nothing is sent" }));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let res = event_loop(&mut terminal, &mut app, rx);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    res
}

fn event_loop<B: Backend>(terminal: &mut Terminal<B>, app: &mut App, rx: mpsc::Receiver<Msg>) -> io::Result<()> {
    loop {
        while let Ok(m) = rx.try_recv() {
            app.last_data = Instant::now();
            match m {
                Msg::Book(b) => app.book = b, Msg::Tape(t) => app.tape = t, Msg::Candles(c) => app.candles = c,
                Msg::Product(p) => app.product = p, Msg::Accounts(a) => app.accounts = a,
                Msg::Positions(p, w) => { app.positions = p; app.positions_note = w; }
                Msg::Orders(o) => app.orders = o, Msg::Latency(l) => app.latency_ms = l,
                Msg::Err(e) => { app.errors += 1; app.log(format!("⚠ {e}")); }
            }
        }
        terminal.draw(|f| draw(f, app))?;
        if event::poll(Duration::from_millis(120))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press { on_key(app, k.code, k.modifiers); }
            }
        }
        if app.quit { return Ok(()); }
    }
}

// ───────────────────────────────────────────── keys ─────────────────────────────────────────────

fn on_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    if mods.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') { app.quit = true; return; }
    if app.pending.is_some() { return on_key_pending(app, code); }
    if app.form.is_some() { return on_key_form(app, code); }
    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.quit = true,
        KeyCode::Tab => app.tab = (app.tab + 1) % TABS.len(),
        KeyCode::BackTab => app.tab = (app.tab + TABS.len() - 1) % TABS.len(),
        KeyCode::Char(c @ '1'..='5') => app.tab = (c as u8 - b'1') as usize,
        KeyCode::Left | KeyCode::Char('[') => app.switch_product(-1),
        KeyCode::Right | KeyCode::Char(']') => app.switch_product(1),
        KeyCode::Char('g') => app.cycle_gran(),
        KeyCode::Char('r') => { if let Ok(mut s) = app.shared.lock() { s.refresh = true; } app.log("refresh"); }
        KeyCode::Char('b') | KeyCode::Char('s') => {
            let side = if code == KeyCode::Char('b') { "BUY" } else { "SELL" };
            let perp = is_perp(&app.product_id());
            app.form = Some(OrderForm { side: side.into(), usd: "25".into(), price: String::new(), leverage: if perp { 2 } else { 1 },
                                        limit: false, field: Field::Usd, proposal: None, preview: None, error: None });
        }
        KeyCode::Char('c') => {
            if !app.client.is_signed() { app.log("no key — nothing to cancel"); return; }
            app.pending = Some(Pending::CancelAll);
        }
        KeyCode::Char('d') => dca_propose(app),
        _ => {}
    }
}

fn on_key_pending(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('y') | KeyCode::Enter => {
            let p = app.pending.take();
            match p {
                Some(Pending::CancelAll) => {
                    if !app.opts.live { app.log("PAPER: would cancel all open orders (run with --live)"); return; }
                    match app.client.cancel_all("", true) { Ok(v) => app.log(format!("cancel-all → {}", compact(&v))), Err(e) => app.log(format!("✗ cancel: {e}")) }
                }
                Some(Pending::Place(order)) => {
                    if !app.opts.live { app.log(format!("PAPER: would send {}", compact(&order))); app.form = None; return; }
                    match app.client.place_order(&order, true) {
                        Ok(v) => { app.log(format!("✅ ORDER SENT → {}", compact(&v["result"]))); app.form = None; }
                        Err(e) => app.log(format!("✗ order: {e}")),
                    }
                    if let Ok(mut s) = app.shared.lock() { s.refresh = true; }
                }
                Some(Pending::Dca(_)) => {
                    if !app.opts.live { app.log("PAPER: would place the DCA ticket (run with --live)"); return; }
                    let cfg = app.dca_cfg();
                    match dca::tick(&cfg, &app.client, true, true) {
                        Ok(v) => { app.log(format!("✅ DCA ticket sent → {}", compact(&v["result"]))); app.dca_state = DcaState::load(&cfg.state_path()); app.dca_plan = Some(v); }
                        Err(e) => app.log(format!("✗ dca: {e}")),
                    }
                }
                None => {}
            }
        }
        _ => { app.pending = None; app.log("cancelled"); }
    }
}

fn on_key_form(app: &mut App, code: KeyCode) {
    let mark = app.mark();
    let pid = app.product_id();
    let signed = app.client.is_signed();
    let f = app.form.as_mut().unwrap();
    match code {
        KeyCode::Esc => app.form = None,
        KeyCode::Tab => f.field = if f.field == Field::Usd { Field::Price } else { Field::Usd },
        KeyCode::Char('m') => { f.limit = !f.limit; if f.limit && f.price.is_empty() && mark > 0.0 { f.price = format!("{mark:.2}"); } f.proposal = None; f.preview = None; }
        KeyCode::Char('+') | KeyCode::Char('=') => { f.leverage = (f.leverage + 1).min(20); f.proposal = None; }
        KeyCode::Char('-') => { f.leverage = f.leverage.saturating_sub(1).max(1); f.proposal = None; }
        KeyCode::Backspace => { match f.field { Field::Usd => { f.usd.pop(); }, Field::Price => { f.price.pop(); } } f.proposal = None; }
        KeyCode::Char(c) if c.is_ascii_digit() || c == '.' => {
            match f.field { Field::Usd => f.usd.push(c), Field::Price => f.price.push(c) }
            f.proposal = None; f.preview = None;
        }
        KeyCode::Enter => {
            if let Some(p) = f.proposal.clone() {
                if p["gate"] == json!("PASS") { app.pending = Some(Pending::Place(p["would_send"].clone())); }
                return;
            }
            // build proposal: gate → order body → preview
            let usd: f64 = f.usd.parse().unwrap_or(0.0);
            let price: Option<f64> = if f.limit { f.price.parse().ok() } else { None };
            let lev = if is_perp(&pid) { f.leverage } else { 1 };
            let notional = usd;
            let verdict = app.gate.check(&pid, &f.side, notional, lev, price, Some(mark));
            let inc = if app.product.base_increment.is_empty() { "0.00000001".to_string() } else { app.product.base_increment.clone() };
            match verdict {
                Verdict::Reject(r) => { f.error = Some(r); f.proposal = None; }
                Verdict::Pass => {
                    let kind = if f.limit {
                        match base_from_quote(notional, price.unwrap_or(mark), &inc) {
                            Ok(b) => OrderKind::Limit { base_size: b, limit_price: format!("{:.2}", price.unwrap_or(mark)), post_only: false },
                            Err(e) => { f.error = Some(e); return; }
                        }
                    } else if is_perp(&pid) || f.side == "SELL" {
                        match base_from_quote(notional, mark, &inc) { Ok(b) => OrderKind::MarketBase { base_size: b }, Err(e) => { f.error = Some(e); return; } }
                    } else { OrderKind::MarketQuote { quote_size: format!("{notional:.2}") } };
                    let order = build_order(&pid, &f.side, &kind, Some(lev), Some("CROSS"), None);
                    f.error = None;
                    f.preview = if signed { Some(app.client.preview(&order).unwrap_or_else(|e| json!({"error": e}))) } else { None };
                    f.proposal = Some(json!({"gate": "PASS", "would_send": order, "mark": mark, "notional_usd": notional, "leverage": lev}));
                }
            }
        }
        _ => {}
    }
}

fn dca_propose(app: &mut App) {
    let cfg = app.dca_cfg();
    match dca::tick(&cfg, &app.client, false, true) {
        Ok(v) => {
            let gate = v["plan"]["gate"].as_str().unwrap_or("?").to_string();
            app.log(format!("DCA plan: {} — {}", gate, v["plan"]["reasons"].as_array().map(|a| a.len()).unwrap_or(0)));
            if gate == "PASS" { app.pending = Some(Pending::Dca(v.clone())); }
            app.dca_plan = Some(v);
            app.tab = 1;
        }
        Err(e) => app.log(format!("✗ dca: {e}")),
    }
}

/// Char-safe prefix — a byte-index `truncate` panics on the first `·` or `→` it lands inside.
fn take_chars(s: &str, n: usize) -> String { s.chars().take(n).collect() }

fn compact(v: &Value) -> String {
    let s = serde_json::to_string(v).unwrap_or_default();
    if s.chars().count() > 160 { format!("{}…", take_chars(&s, 160)) } else { s }
}

// ──────────────────────────────────────────── drawing ────────────────────────────────────────────

const C_BG: Color = Color::Rgb(12, 14, 20);
const C_DIM: Color = Color::Rgb(110, 118, 140);
const C_GOLD: Color = Color::Rgb(240, 190, 80);
const C_CYAN: Color = Color::Rgb(90, 200, 230);
const C_GREEN: Color = Color::Rgb(70, 200, 120);
const C_RED: Color = Color::Rgb(230, 80, 90);
const C_TXT: Color = Color::Rgb(220, 224, 232);

fn panel(title: &str, accent: Color) -> Block<'static> {
    Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(accent))
        .title(Line::from(vec![Span::styled(format!(" {title} "), Style::new().fg(accent).bold())]))
        .style(Style::new().bg(C_BG))
}

/// ratatui buffers are u16-indexed; a 4K terminal at a tiny font exceeds 65,535 cells and panics
/// every frame. Clamp the drawable area so the app survives any terminal size.
fn safe_area(a: Rect) -> Rect {
    let cells = a.width as u32 * a.height as u32;
    if cells <= 60_000 { return a; }
    let h = (60_000 / a.width.max(1) as u32) as u16;
    Rect { x: a.x, y: a.y, width: a.width, height: h.min(a.height) }
}

fn draw(f: &mut Frame, app: &App) {
    let area = safe_area(f.area());
    f.render_widget(Block::new().style(Style::new().bg(C_BG)), area);
    let rows = Layout::vertical([Constraint::Length(3), Constraint::Length(1), Constraint::Min(8), Constraint::Length(1)]).split(area);
    draw_header(f, app, rows[0]);
    let tabs = Tabs::new(TABS.iter().enumerate().map(|(i, t)| Line::from(format!(" {} {t} ", i + 1))).collect::<Vec<_>>())
        .select(app.tab).style(Style::new().fg(C_DIM)).highlight_style(Style::new().fg(C_GOLD).bold().underlined())
        .divider("│");
    f.render_widget(tabs, rows[1]);
    match app.tab {
        0 => draw_trade(f, app, rows[2]),
        1 => draw_dca(f, app, rows[2]),
        2 => draw_orders(f, app, rows[2]),
        3 => draw_account(f, app, rows[2]),
        _ => draw_log(f, app, rows[2]),
    }
    draw_footer(f, app, rows[3]);
    if let Some(form) = &app.form { draw_form(f, app, form, area); }
    if let Some(p) = &app.pending { draw_confirm(f, app, p, area); }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let pid = app.product_id();
    let p = &app.product;
    let chg = p.change_24h_pct;
    let chg_style = if chg >= 0.0 { Style::new().fg(C_GREEN) } else { Style::new().fg(C_RED) };
    let price = app.mark();
    let stale = app.last_data.elapsed() > Duration::from_secs(6);
    let mode = if app.opts.live { Span::styled(" LIVE ", Style::new().fg(Color::Black).bg(C_RED).bold()) }
               else { Span::styled(" PAPER ", Style::new().fg(Color::Black).bg(C_GOLD).bold()) };
    let line1 = Line::from(vec![
        Span::styled(" ◆ FLUX COINBASE ", Style::new().fg(C_GOLD).bold()),
        Span::styled(format!(" {pid} "), Style::new().fg(C_CYAN).bold()),
        if p.perp { Span::styled(" PERP ", Style::new().fg(Color::Black).bg(C_CYAN).bold()) } else { Span::styled(" SPOT ", Style::new().fg(Color::Black).bg(C_DIM)) },
        Span::raw("  "),
        Span::styled(if price > 0.0 { format!("${}", fmt_usd(price)) } else { "—".into() }, Style::new().fg(C_TXT).bold()),
        Span::raw("  "),
        Span::styled(format!("{}{:.2}% 24h", if chg >= 0.0 { "▲ +" } else { "▼ " }, chg), chg_style),
        Span::raw("   "), mode,
    ]);
    let (bb, ba) = (app.book.best_bid().unwrap_or(0.0), app.book.best_ask().unwrap_or(0.0));
    let line2 = Line::from(vec![
        Span::styled(format!(" bid {} ", fmt_px(bb).trim()), Style::new().fg(C_GREEN)),
        Span::styled(format!("ask {} ", fmt_px(ba).trim()), Style::new().fg(C_RED)),
        Span::styled(format!("spread {:.1} bps ", app.book.spread_bps().unwrap_or(0.0)), Style::new().fg(C_DIM)),
        Span::styled(format!("imb {:+.2} ", app.book.imbalance(10)), Style::new().fg(if app.book.imbalance(10) >= 0.0 { C_GREEN } else { C_RED })),
        Span::styled(format!("vol24h {:.1} ", p.volume_24h), Style::new().fg(C_DIM)),
        if p.perp { Span::styled(format!("funding {:+.4}% ", p.funding_rate * 100.0), Style::new().fg(C_CYAN)) } else { Span::raw("") },
        Span::styled(format!("│ key {} ", app.client.key_fingerprint()), Style::new().fg(C_DIM)),
        Span::styled(format!("│ {} ms ", app.latency_ms), Style::new().fg(if app.latency_ms < 400 { C_GREEN } else { C_GOLD })),
        Span::styled(if stale { "● STALE " } else { "● live " }, Style::new().fg(if stale { C_RED } else { C_GREEN })),
        Span::styled(chrono_hms(), Style::new().fg(C_DIM)),
    ]);
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(C_GOLD)).style(Style::new().bg(C_BG));
    f.render_widget(Paragraph::new(vec![line1, line2]).block(block), area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let last = app.log.front().cloned().unwrap_or_default();
    let keys = " q quit │ 1-5 tabs │ ←→ product │ g candles │ b/s order │ d DCA │ c cancel-all │ r refresh ";
    let w = area.width as usize;
    let klen = keys.chars().count();
    let mut text = last.clone();
    if text.chars().count() + klen > w { text = take_chars(&text, w.saturating_sub(klen + 2)); }
    let pad = w.saturating_sub(text.chars().count() + klen);
    let line = Line::from(vec![
        Span::styled(text, Style::new().fg(if last.contains('⚠') || last.contains('✗') { C_RED } else { C_TXT })),
        Span::raw(" ".repeat(pad)), Span::styled(keys, Style::new().fg(C_DIM)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_trade(f: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::horizontal([Constraint::Length(38), Constraint::Min(40), Constraint::Length(36)]).split(area);
    draw_book(f, app, cols[0]);
    let mid = Layout::vertical([Constraint::Min(10), Constraint::Length(9)]).split(cols[1]);
    draw_chart(f, app, mid[0]);
    draw_tape(f, app, mid[1]);
    let right = Layout::vertical([Constraint::Percentage(34), Constraint::Percentage(33), Constraint::Percentage(33)]).split(cols[2]);
    draw_balances(f, app, right[0]);
    draw_positions(f, app, right[1]);
    draw_open_orders_small(f, app, right[2]);
}

fn draw_book(f: &mut Frame, app: &App, area: Rect) {
    let b = &app.book;
    let inner_h = area.height.saturating_sub(2) as usize;
    let per_side = inner_h.saturating_sub(1) / 2;
    let asks: Vec<_> = b.asks.iter().take(per_side).collect();
    let bids: Vec<_> = b.bids.iter().take(per_side).collect();
    let cum = |lv: &[&crate::Level]| -> Vec<f64> { let mut c = 0.0; lv.iter().map(|l| { c += l.size; c }).collect() };
    let (ca, cb) = (cum(&asks), cum(&bids));
    let maxc = ca.last().copied().unwrap_or(0.0).max(cb.last().copied().unwrap_or(0.0)).max(1e-12);
    let barw = 10usize;
    let mut lines: Vec<Line> = vec![];
    for (i, l) in asks.iter().enumerate().rev() {
        let n = ((ca[i] / maxc) * barw as f64).round() as usize;
        lines.push(Line::from(vec![
            Span::styled(fmt_px(l.price), Style::new().fg(C_RED)),
            Span::styled(format!(" {:>10} ", fmt_size(l.size)), Style::new().fg(C_TXT)),
            Span::styled(format!("{}{}", "█".repeat(n), "░".repeat(barw - n)), Style::new().fg(C_RED)),
        ]));
    }
    let spread = b.spread_bps().map(|s| format!("{s:.1} bps")).unwrap_or("—".into());
    lines.push(Line::from(Span::styled(format!("{:^36}", format!("── mid {} · {spread} ──", b.mid().map(|m| fmt_px(m).trim().to_string()).unwrap_or("—".into()))), Style::new().fg(C_GOLD))));
    for (i, l) in bids.iter().enumerate() {
        let n = ((cb[i] / maxc) * barw as f64).round() as usize;
        lines.push(Line::from(vec![
            Span::styled(fmt_px(l.price), Style::new().fg(C_GREEN)),
            Span::styled(format!(" {:>10} ", fmt_size(l.size)), Style::new().fg(C_TXT)),
            Span::styled(format!("{}{}", "█".repeat(n), "░".repeat(barw - n)), Style::new().fg(C_GREEN)),
        ]));
    }
    f.render_widget(Paragraph::new(lines).block(panel("ORDER BOOK", C_CYAN)), area);
}

fn draw_chart(f: &mut Frame, app: &App, area: Rect) {
    let c = &app.candles;
    let title = format!("CANDLES {} · {}", app.product_id(), app.gran());
    if c.len() < 2 {
        f.render_widget(Paragraph::new(Span::styled("loading candles…", Style::new().fg(C_DIM))).block(panel(&title, C_GOLD)), area);
        return;
    }
    let inner_w = area.width.saturating_sub(2) as usize;
    let n = c.len().min(inner_w.max(10) / 2);
    let show = &c[c.len() - n..];
    let lo = show.iter().map(|x| x.low).fold(f64::MAX, f64::min);
    let hi = show.iter().map(|x| x.high).fold(f64::MIN, f64::max);
    let pad = (hi - lo).max(hi * 0.001) * 0.05;
    let (ylo, yhi) = (lo - pad, hi + pad);
    let closes: Vec<f64> = c.iter().map(|x| x.close).collect();
    let sma_n = 20usize;
    let last = show.last().copied().unwrap_or_default();
    let last_style = if last.close >= last.open { C_GREEN } else { C_RED };
    let hdr = format!("{title}  hi {}  lo {}  last {}  sma20 {}", fmt_px(hi).trim(), fmt_px(lo).trim(), fmt_px(last.close).trim(),
                      dca::sma(&closes, sma_n).map(|s| fmt_px(s).trim().to_string()).unwrap_or("—".into()));
    let canvas = Canvas::default()
        .block(panel(&hdr, C_GOLD))
        .marker(symbols::Marker::Braille)
        .x_bounds([0.0, n as f64])
        .y_bounds([ylo, yhi])
        .paint(move |ctx| {
            for (i, k) in show.iter().enumerate() {
                let x = i as f64 + 0.5;
                let col = if k.close >= k.open { C_GREEN } else { C_RED };
                ctx.draw(&CLine { x1: x, y1: k.low, x2: x, y2: k.high, color: col });
                let (b0, b1) = (k.open.min(k.close), k.open.max(k.close));
                let b1 = if (b1 - b0).abs() < (yhi - ylo) * 0.002 { b0 + (yhi - ylo) * 0.002 } else { b1 };
                for dx in [-0.25, 0.0, 0.25] { ctx.draw(&CLine { x1: x + dx, y1: b0, x2: x + dx, y2: b1, color: col }); }
            }
            // SMA-20 overlay over the visible window
            let off = c.len() - n;
            let mut prev: Option<(f64, f64)> = None;
            for i in 0..n {
                let end = off + i + 1;
                if end >= sma_n {
                    let s = closes[end - sma_n..end].iter().sum::<f64>() / sma_n as f64;
                    let pt = (i as f64 + 0.5, s);
                    if let Some(p) = prev { ctx.draw(&CLine { x1: p.0, y1: p.1, x2: pt.0, y2: pt.1, color: C_GOLD }); }
                    prev = Some(pt);
                }
            }
            ctx.print(n as f64 - 0.5, last.close, Line::from(Span::styled(format!("◀ {}", fmt_px(last.close).trim()), Style::new().fg(last_style).bold())));
        });
    f.render_widget(canvas, area);
}

fn draw_tape(f: &mut Frame, app: &App, area: Rect) {
    let rows: Vec<Row> = app.tape.trades.iter().take(area.height.saturating_sub(3) as usize).map(|t| {
        let col = if t.side.eq_ignore_ascii_case("BUY") { C_GREEN } else { C_RED };
        Row::new(vec![
            Cell::from(hms_of(&t.time)).style(Style::new().fg(C_DIM)),
            Cell::from(t.side.clone()).style(Style::new().fg(col).bold()),
            Cell::from(fmt_px(t.price).trim().to_string()).style(Style::new().fg(col)),
            Cell::from(fmt_size(t.size)).style(Style::new().fg(C_TXT)),
            Cell::from(format!("${}", fmt_usd(t.price * t.size))).style(Style::new().fg(C_DIM)),
        ])
    }).collect();
    let t = Table::new(rows, [Constraint::Length(9), Constraint::Length(5), Constraint::Length(13), Constraint::Length(10), Constraint::Min(10)])
        .header(Row::new(vec!["time", "side", "price", "size", "notional"]).style(Style::new().fg(C_DIM).underlined()))
        .block(panel("TAPE", C_DIM));
    f.render_widget(t, area);
}

fn draw_balances(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = vec![];
    if !app.client.is_signed() {
        lines.push(Line::from(Span::styled("no key — run `flux-coinbase setup`", Style::new().fg(C_GOLD))));
    } else {
        let mut bal: Vec<&Balance> = app.accounts.iter().filter(|b| b.available > 0.0 || b.hold > 0.0).collect();
        bal.sort_by(|a, b| b.available.partial_cmp(&a.available).unwrap_or(std::cmp::Ordering::Equal));
        if bal.is_empty() { lines.push(Line::from(Span::styled("no balances yet", Style::new().fg(C_DIM)))); }
        for b in bal.iter().take(area.height.saturating_sub(2) as usize) {
            lines.push(Line::from(vec![
                Span::styled(format!("{:<6}", b.currency), Style::new().fg(C_CYAN).bold()),
                Span::styled(format!("{:>14.6}", b.available), Style::new().fg(C_TXT)),
                if b.hold > 0.0 { Span::styled(format!(" hold {:.4}", b.hold), Style::new().fg(C_DIM)) } else { Span::raw("") },
            ]));
        }
    }
    f.render_widget(Paragraph::new(lines).block(panel("BALANCES", C_GREEN)), area);
}

fn draw_positions(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = vec![];
    if let Some(n) = &app.positions_note { lines.push(Line::from(Span::styled(n.clone(), Style::new().fg(C_DIM)))); }
    if app.positions.is_empty() && app.positions_note.is_none() { lines.push(Line::from(Span::styled("no open perp positions", Style::new().fg(C_DIM)))); }
    for p in app.positions.iter().take(6) {
        let pnl_c = if p.unrealized_pnl >= 0.0 { C_GREEN } else { C_RED };
        let dist = if p.mark > 0.0 && p.liq > 0.0 { (p.mark - p.liq).abs() / p.mark * 100.0 } else { 0.0 };
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", p.product_id), Style::new().fg(C_CYAN).bold()),
            Span::styled(format!("{} {:.4} {}x", p.side, p.size, p.leverage), Style::new().fg(C_TXT)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("  entry {} pnl {:+.2}", fmt_px(p.entry).trim(), p.unrealized_pnl), Style::new().fg(pnl_c)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("  liq {} ({dist:.1}% away)", fmt_px(p.liq).trim()), Style::new().fg(if dist < 15.0 { C_RED } else { C_DIM })),
        ]));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }).block(panel("POSITIONS", C_RED)), area);
}

fn draw_open_orders_small(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = vec![];
    if app.orders.is_empty() { lines.push(Line::from(Span::styled("no open orders", Style::new().fg(C_DIM)))); }
    for o in app.orders.iter().take(area.height.saturating_sub(2) as usize) {
        let c = if o.side == "BUY" { C_GREEN } else { C_RED };
        lines.push(Line::from(vec![
            Span::styled(format!("{:<4}", o.side), Style::new().fg(c).bold()),
            Span::styled(format!("{} ", o.product_id), Style::new().fg(C_CYAN)),
            Span::styled(format!("{} @ {}", o.size, fmt_px(o.price).trim()), Style::new().fg(C_TXT)),
        ]));
    }
    f.render_widget(Paragraph::new(lines).block(panel("OPEN ORDERS", C_GOLD)), area);
}

fn draw_orders(f: &mut Frame, app: &App, area: Rect) {
    let rows: Vec<Row> = app.orders.iter().map(|o| {
        let c = if o.side == "BUY" { C_GREEN } else { C_RED };
        Row::new(vec![
            Cell::from(hms_of(&o.created)).style(Style::new().fg(C_DIM)),
            Cell::from(o.product_id.clone()).style(Style::new().fg(C_CYAN)),
            Cell::from(o.side.clone()).style(Style::new().fg(c).bold()),
            Cell::from(o.kind.clone()), Cell::from(fmt_px(o.price).trim().to_string()),
            Cell::from(format!("{}", o.size)), Cell::from(format!("{}", o.filled)),
            Cell::from(o.status.clone()).style(Style::new().fg(C_GOLD)),
            Cell::from(o.order_id.clone()).style(Style::new().fg(C_DIM)),
        ])
    }).collect();
    let t = Table::new(rows, [Constraint::Length(9), Constraint::Length(15), Constraint::Length(5), Constraint::Length(18),
                              Constraint::Length(13), Constraint::Length(12), Constraint::Length(12), Constraint::Length(8), Constraint::Min(20)])
        .header(Row::new(vec!["created", "product", "side", "type", "price", "size", "filled", "status", "order id"]).style(Style::new().fg(C_DIM).underlined()))
        .block(panel(&format!("OPEN ORDERS ({}) · c = cancel all", app.orders.len()), C_GOLD));
    f.render_widget(t, area);
}

fn draw_account(f: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).split(area);
    let rows: Vec<Row> = app.accounts.iter().filter(|b| b.available > 0.0 || b.hold > 0.0).map(|b| Row::new(vec![
        Cell::from(b.currency.clone()).style(Style::new().fg(C_CYAN).bold()),
        Cell::from(format!("{:.8}", b.available)), Cell::from(format!("{:.8}", b.hold)),
        Cell::from(b.kind.clone()).style(Style::new().fg(C_DIM)),
    ])).collect();
    let t = Table::new(rows, [Constraint::Length(8), Constraint::Length(20), Constraint::Length(16), Constraint::Min(10)])
        .header(Row::new(vec!["asset", "available", "hold", "type"]).style(Style::new().fg(C_DIM).underlined()))
        .block(panel("BALANCES", C_GREEN));
    f.render_widget(t, cols[0]);
    let risk = dca::risk_report(&app.positions, 15.0);
    let mut lines = vec![
        Line::from(vec![Span::styled("gate ", Style::new().fg(C_DIM)), Span::styled(format!("{:?}", app.gate.whitelist), Style::new().fg(C_TXT))]),
        Line::from(vec![Span::styled("     ", Style::new()), Span::styled(format!("max ${:.0}/order · max {}x · limit dev {} bps", app.gate.max_notional_usd, app.gate.max_leverage, app.gate.max_price_deviation_bps), Style::new().fg(C_TXT))]),
        Line::from(""),
        Line::from(Span::styled(format!("risk: {}", risk["note"].as_str().unwrap_or("")), Style::new().fg(if risk["warnings"].as_u64().unwrap_or(0) > 0 { C_RED } else { C_GREEN }))),
    ];
    if let Some(n) = &app.positions_note { lines.push(Line::from(Span::styled(n.clone(), Style::new().fg(C_DIM)))); }
    for r in risk["positions"].as_array().cloned().unwrap_or_default() {
        lines.push(Line::from(Span::styled(format!("{} {} {} lev {} liq {} ({:.1}% away) pnl {:+.2}",
            r["product_id"].as_str().unwrap_or(""), r["side"].as_str().unwrap_or(""), r["size"], r["leverage"], r["liquidation"],
            r["liq_distance_pct"].as_f64().unwrap_or(0.0), r["unrealized_pnl"].as_f64().unwrap_or(0.0)),
            Style::new().fg(if r["warn"] == json!(true) { C_RED } else { C_TXT }))));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(format!("ledger {}", crate::ledger_path()), Style::new().fg(C_DIM))));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }).block(panel("GATE · RISK", C_RED)), cols[1]);
}

fn draw_dca(f: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);
    let cfg = app.dca_cfg();
    let st = &app.dca_state;
    let (due, next) = st.due(cfg.every_hours, crate::now_s());
    let mut lines = vec![
        Line::from(vec![Span::styled("product   ", Style::new().fg(C_DIM)), Span::styled(cfg.product_id.clone(), Style::new().fg(C_CYAN).bold())]),
        Line::from(vec![Span::styled("ticket    ", Style::new().fg(C_DIM)), Span::styled(format!("${:.2} margin every {}h × {}x = ${:.2} notional", cfg.base_usd, cfg.every_hours, cfg.leverage, cfg.base_usd * cfg.leverage as f64), Style::new().fg(C_TXT))]),
        Line::from(vec![Span::styled("tilt      ", Style::new().fg(C_DIM)), Span::styled(format!("{} · max {:.1}× · SMA{} / RSI{} on {}", if cfg.dip_tilt { "dip-tilt ON" } else { "flat" }, cfg.max_multiplier, cfg.sma_len, cfg.rsi_len, cfg.candle_gran), Style::new().fg(C_TXT))]),
        Line::from(vec![Span::styled("caps      ", Style::new().fg(C_DIM)), Span::styled(format!("position ${:.0} · gate ${:.0}/order · {}x · liq floor {}%", cfg.max_position_usd, cfg.gate.max_notional_usd, cfg.gate.max_leverage, cfg.min_liq_distance_pct), Style::new().fg(C_TXT))]),
        Line::from(""),
        Line::from(vec![Span::styled("ticks     ", Style::new().fg(C_DIM)), Span::styled(format!("{} · spent ${} margin · ${} notional", st.ticks, fmt_usd(st.margin_spent_usd), fmt_usd(st.notional_usd)), Style::new().fg(C_TXT))]),
        Line::from(vec![Span::styled("holding   ", Style::new().fg(C_DIM)), Span::styled(format!("{:.6} @ avg {}", st.base_acc, fmt_px(st.avg_entry).trim()), Style::new().fg(C_GOLD))]),
        Line::from(vec![Span::styled("next      ", Style::new().fg(C_DIM)), Span::styled(if due { "DUE NOW".to_string() } else { format!("in {}h {}m", next / 3600, (next % 3600) / 60) }, Style::new().fg(if due { C_GREEN } else { C_TXT }))]),
    ];
    if st.avg_entry > 0.0 && app.mark() > 0.0 {
        let pnl = (app.mark() - st.avg_entry) / st.avg_entry * 100.0 * cfg.leverage as f64;
        lines.push(Line::from(vec![Span::styled("unreal.   ", Style::new().fg(C_DIM)), Span::styled(format!("{pnl:+.2}% on margin (at {}x)", cfg.leverage), Style::new().fg(if pnl >= 0.0 { C_GREEN } else { C_RED }))]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("press d for a plan · Enter/y in the confirm box places it (LIVE only)", Style::new().fg(C_DIM))));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }).block(panel("LEVERAGED DCA", C_GOLD)), cols[0]);

    let mut plan_lines: Vec<Line> = vec![];
    match &app.dca_plan {
        None => plan_lines.push(Line::from(Span::styled("no plan yet — press d", Style::new().fg(C_DIM)))),
        Some(v) => {
            let p = &v["plan"];
            let gate = p["gate"].as_str().unwrap_or("?");
            plan_lines.push(Line::from(Span::styled(format!("gate {gate}"), Style::new().fg(if gate == "PASS" { C_GREEN } else { C_RED }).bold())));
            let m = &v["market"];
            plan_lines.push(Line::from(Span::styled(format!("price {} · SMA {} · RSI {} · {} candles",
                m["price"].as_f64().map(|x| fmt_px(x).trim().to_string()).unwrap_or("—".into()),
                m["sma"].as_f64().map(|x| fmt_px(x).trim().to_string()).unwrap_or("—".into()),
                m["rsi"].as_f64().map(|x| format!("{x:.0}")).unwrap_or("—".into()), m["candles"]), Style::new().fg(C_TXT))));
            plan_lines.push(Line::from(Span::styled(format!("× {:.2} → ${:.2} margin · ${:.2} notional · {} base · {}x",
                p["multiplier"].as_f64().unwrap_or(1.0), p["margin_usd"].as_f64().unwrap_or(0.0), p["notional_usd"].as_f64().unwrap_or(0.0),
                p["base_size"].as_str().unwrap_or("?"), p["leverage"]), Style::new().fg(C_GOLD))));
            if let Some(l) = p["est_liquidation"].as_f64() {
                plan_lines.push(Line::from(Span::styled(format!("est. liquidation {} ({:.1}% below)", fmt_px(l).trim(), p["liq_distance_pct"].as_f64().unwrap_or(0.0)), Style::new().fg(C_RED))));
            }
            for r in p["reasons"].as_array().cloned().unwrap_or_default() {
                plan_lines.push(Line::from(Span::styled(format!("• {}", r.as_str().unwrap_or("")), Style::new().fg(C_DIM))));
            }
            if let Some(pv) = v.get("preview") {
                plan_lines.push(Line::from(""));
                plan_lines.push(Line::from(Span::styled(format!("coinbase preview: total {} fee {} {}",
                    pv["order_total"].as_str().unwrap_or("—"), pv["commission_total"].as_str().unwrap_or("—"),
                    pv["error"].as_str().map(|e| format!("✗ {e}")).unwrap_or_default()), Style::new().fg(C_CYAN))));
            }
            if v["confirmed"] == json!(true) { plan_lines.push(Line::from(Span::styled("✅ SENT", Style::new().fg(C_GREEN).bold()))); }
        }
    }
    plan_lines.push(Line::from(""));
    plan_lines.push(Line::from(Span::styled("history", Style::new().fg(C_DIM).underlined())));
    for h in st.history.iter().rev().take(8) {
        plan_lines.push(Line::from(Span::styled(format!("  ${:.2} × {:.2} @ {} ({}x)", h["margin_usd"].as_f64().unwrap_or(0.0), h["multiplier"].as_f64().unwrap_or(1.0),
            h["price"].as_f64().map(|x| fmt_px(x).trim().to_string()).unwrap_or("?".into()), h["leverage"]), Style::new().fg(C_TXT))));
    }
    f.render_widget(Paragraph::new(plan_lines).wrap(Wrap { trim: true }).block(panel("PLAN", C_CYAN)), cols[1]);
}

fn draw_log(f: &mut Frame, app: &App, area: Rect) {
    let lines: Vec<Line> = app.log.iter().take(area.height.saturating_sub(2) as usize)
        .map(|l| Line::from(Span::styled(l.clone(), Style::new().fg(if l.contains('⚠') || l.contains('✗') { C_RED } else if l.contains('✅') { C_GREEN } else { C_TXT })))).collect();
    f.render_widget(Paragraph::new(lines).block(panel(&format!("LOG · {} errors", app.errors), C_DIM)), area);
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width); let h = h.min(area.height);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

fn draw_form(f: &mut Frame, app: &App, form: &OrderForm, area: Rect) {
    let r = centered(area, 72, 20);
    f.render_widget(Clear, r);
    let side_c = if form.side == "BUY" { C_GREEN } else { C_RED };
    let pid = app.product_id();
    let perp = is_perp(&pid);
    let fld = |sel: bool| if sel { Style::new().fg(Color::Black).bg(C_GOLD) } else { Style::new().fg(C_TXT) };
    let mut lines = vec![
        Line::from(vec![Span::styled(format!(" {} {pid} ", form.side), Style::new().fg(Color::Black).bg(side_c).bold()),
                        Span::styled(format!("  mark {}", fmt_px(app.mark()).trim()), Style::new().fg(C_DIM))]),
        Line::from(""),
        Line::from(vec![Span::styled("  notional USD  ", Style::new().fg(C_DIM)), Span::styled(format!(" {} ", form.usd), fld(form.field == Field::Usd))]),
        Line::from(vec![Span::styled("  type          ", Style::new().fg(C_DIM)), Span::styled(if form.limit { "LIMIT (m to toggle)" } else { "MARKET (m to toggle)" }, Style::new().fg(C_TXT))]),
        Line::from(vec![Span::styled("  limit price   ", Style::new().fg(C_DIM)),
                        if form.limit { Span::styled(format!(" {} ", form.price), fld(form.field == Field::Price)) } else { Span::styled("—", Style::new().fg(C_DIM)) }]),
        Line::from(vec![Span::styled("  leverage      ", Style::new().fg(C_DIM)),
                        if perp { Span::styled(format!("{}x  (+/-)", form.leverage), Style::new().fg(C_CYAN)) } else { Span::styled("1x (spot — no leverage)", Style::new().fg(C_DIM)) }]),
        Line::from(""),
    ];
    if let Some(e) = &form.error { lines.push(Line::from(Span::styled(format!("  ✗ gate: {e}"), Style::new().fg(C_RED)))); }
    if let Some(p) = &form.proposal {
        lines.push(Line::from(Span::styled("  gate PASS — would send:", Style::new().fg(C_GREEN).bold())));
        lines.push(Line::from(Span::styled(format!("  {}", compact(&p["would_send"])), Style::new().fg(C_TXT))));
        match &form.preview {
            Some(pv) if pv.get("error").is_some() => lines.push(Line::from(Span::styled(format!("  preview ✗ {}", pv["error"].as_str().unwrap_or("")), Style::new().fg(C_RED)))),
            Some(pv) => lines.push(Line::from(Span::styled(format!("  coinbase preview: total {} · fee {} · base {} · quote {}",
                pv["order_total"].as_str().unwrap_or("—"), pv["commission_total"].as_str().unwrap_or("—"),
                pv["base_size"].as_str().unwrap_or("—"), pv["quote_size"].as_str().unwrap_or("—")), Style::new().fg(C_CYAN)))),
            None => lines.push(Line::from(Span::styled("  (no key → no Coinbase preview)", Style::new().fg(C_DIM)))),
        }
        lines.push(Line::from(Span::styled(if app.opts.live { "  Enter = PLACE THIS ORDER (real money) · Esc = close" } else { "  Enter = paper-place (nothing sent; use --live) · Esc = close" },
                                            Style::new().fg(if app.opts.live { C_RED } else { C_GOLD }).bold())));
    } else {
        lines.push(Line::from(Span::styled("  Enter = run gate + preview · Tab = next field · Esc = close", Style::new().fg(C_DIM))));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(panel("NEW ORDER", side_c)), r);
}

fn draw_confirm(f: &mut Frame, app: &App, p: &Pending, area: Rect) {
    let r = centered(area, 70, 9);
    f.render_widget(Clear, r);
    let what = match p {
        Pending::CancelAll => format!("cancel ALL {} open orders", app.orders.len()),
        Pending::Place(o) => format!("PLACE {}", compact(o)),
        Pending::Dca(v) => format!("place DCA ticket: ${:.2} margin · {}x · {}", v["plan"]["margin_usd"].as_f64().unwrap_or(0.0), v["plan"]["leverage"], v["plan"]["base_size"].as_str().unwrap_or("?")),
    };
    let lines = vec![
        Line::from(Span::styled(what, Style::new().fg(C_TXT))),
        Line::from(""),
        Line::from(Span::styled(if app.opts.live { "This is REAL MONEY. y / Enter = yes · any other key = no" } else { "PAPER mode — y logs what would happen. Run with --live to trade." },
                                Style::new().fg(if app.opts.live { C_RED } else { C_GOLD }).bold())),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }).block(panel("CONFIRM", C_RED)), r);
}
