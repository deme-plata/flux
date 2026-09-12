//! Leveraged DCA — dollar-cost averaging with an optional leverage multiplier, built so that every
//! number a human is asked to approve was derived in the open.
//!
//! The picture: a DCA is a metronome. Every `every_hours` it wants to buy `base_usd` of the
//! product. Two things bend the ticket size — the **dip tilt** (buy more when price sits below
//! its long moving average and RSI is washed out, less when it is euphoric) and the **position
//! cap** (stop adding once the accumulated notional reaches `max_position_usd`). Leverage does not
//! change WHAT we buy, it changes how much margin the same notional costs — and where liquidation
//! sits. That last number is printed on every plan; if it is closer than `min_liq_distance_pct`,
//! the plan refuses.
//!
//! Everything here is pure except `market_view` (reads candles) and `tick` (may place an order).
//! The pure parts are unit-tested with no network in sight.

use crate::{base_from_quote, build_order, is_perp, now_s, record, round_down, Coinbase, Gate, OrderKind, Verdict};
use serde_json::{json, Value};

pub const STATE_DIR: &str = "/home/storage/claude-code/flux-coinbase";

#[derive(Debug, Clone)]
pub struct DcaConfig {
    pub product_id: String,
    /// Ticket per tick in USD. With leverage this is MARGIN; notional = base_usd × leverage.
    pub base_usd: f64,
    pub every_hours: f64,
    pub leverage: u32,
    pub margin_type: String,
    pub dip_tilt: bool,
    pub max_multiplier: f64,
    /// Stop adding once accumulated notional reaches this.
    pub max_position_usd: f64,
    /// Refuse a leveraged buy whose estimated liquidation is closer than this (percent of price).
    pub min_liq_distance_pct: f64,
    pub sma_len: usize,
    pub rsi_len: usize,
    pub candle_gran: String,
    pub gate: Gate,
}

impl Default for DcaConfig {
    fn default() -> Self {
        DcaConfig {
            product_id: "BTC-USD".into(), base_usd: 25.0, every_hours: 24.0, leverage: 1,
            margin_type: "CROSS".into(), dip_tilt: true, max_multiplier: 3.0, max_position_usd: 1_000.0,
            min_liq_distance_pct: 15.0, sma_len: 200, rsi_len: 14, candle_gran: "1h".into(),
            gate: Gate::default(),
        }
    }
}

impl DcaConfig {
    pub fn from_json(a: &Value) -> DcaConfig {
        let d = DcaConfig::default();
        let s = |k: &str, dv: &str| a.get(k).and_then(|v| v.as_str()).filter(|x| !x.is_empty()).unwrap_or(dv).to_string();
        let f = |k: &str, dv: f64| a.get(k).and_then(|v| v.as_f64()).unwrap_or(dv);
        let mut gate = Gate::default();
        if let Some(w) = a.get("whitelist").and_then(|v| v.as_array()) {
            let l: Vec<String> = w.iter().filter_map(|x| x.as_str()).map(|s| s.to_uppercase()).collect();
            if !l.is_empty() { gate.whitelist = l; }
        }
        if let Some(m) = a.get("max_notional_usd").and_then(|v| v.as_f64()) { gate.max_notional_usd = m; }
        if let Some(l) = a.get("max_leverage").and_then(|v| v.as_u64()) { gate.max_leverage = l as u32; }
        let product_id = s("product_id", &d.product_id).to_uppercase();
        // a DCA product must be tradeable by the gate, so put it on the whitelist explicitly
        if !gate.whitelist.iter().any(|w| w.eq_ignore_ascii_case(&product_id)) { gate.whitelist.push(product_id.clone()); }
        DcaConfig {
            product_id,
            base_usd: f("base_usd", d.base_usd),
            every_hours: f("every_hours", d.every_hours),
            leverage: a.get("leverage").and_then(|v| v.as_u64()).unwrap_or(d.leverage as u64) as u32,
            margin_type: s("margin_type", &d.margin_type).to_uppercase(),
            dip_tilt: a.get("dip_tilt").and_then(|v| v.as_bool()).unwrap_or(d.dip_tilt),
            max_multiplier: f("max_multiplier", d.max_multiplier),
            max_position_usd: f("max_position_usd", d.max_position_usd),
            min_liq_distance_pct: f("min_liq_distance_pct", d.min_liq_distance_pct),
            sma_len: a.get("sma_len").and_then(|v| v.as_u64()).unwrap_or(d.sma_len as u64) as usize,
            rsi_len: a.get("rsi_len").and_then(|v| v.as_u64()).unwrap_or(d.rsi_len as u64) as usize,
            candle_gran: s("candle_gran", &d.candle_gran),
            gate,
        }
    }
    pub fn to_json(&self) -> Value {
        json!({"product_id": self.product_id, "base_usd": self.base_usd, "every_hours": self.every_hours,
               "leverage": self.leverage, "margin_type": self.margin_type, "dip_tilt": self.dip_tilt,
               "max_multiplier": self.max_multiplier, "max_position_usd": self.max_position_usd,
               "min_liq_distance_pct": self.min_liq_distance_pct, "sma_len": self.sma_len, "rsi_len": self.rsi_len,
               "candle_gran": self.candle_gran, "gate": self.gate.to_json()})
    }
    pub fn state_path(&self) -> String {
        format!("{}/dca-{}.json", std::env::var("FLUX_COINBASE_STATE_DIR").unwrap_or_else(|_| STATE_DIR.into()),
                self.product_id.to_lowercase())
    }
}

// ───────────────────────────────────── indicators (pure) ─────────────────────────────────────

pub fn sma(closes: &[f64], n: usize) -> Option<f64> {
    if n == 0 || closes.len() < n { return None; }
    Some(closes[closes.len() - n..].iter().sum::<f64>() / n as f64)
}

/// Wilder RSI over the last `n` changes.
pub fn rsi(closes: &[f64], n: usize) -> Option<f64> {
    if n == 0 || closes.len() < n + 1 { return None; }
    let (mut g, mut l) = (0.0, 0.0);
    for w in closes[closes.len() - n - 1..].windows(2) {
        let d = w[1] - w[0];
        if d >= 0.0 { g += d } else { l -= d }
    }
    let (ag, al) = (g / n as f64, l / n as f64);
    if al == 0.0 { return Some(100.0); }
    Some(100.0 - 100.0 / (1.0 + ag / al))
}

/// The dip tilt. Returns the multiplier and the plain-language reasons behind it.
///   • price below SMA: +10× the drawdown fraction (5 % under → 1.5×, 10 % → 2×, 20 % → 3×)
///   • price above SMA: −5× the premium (10 % over → 0.5×)
///   • RSI < 30 → ×1.5 · RSI > 70 → ×0.5
/// clamped to [0.25, max]. Never zero — a DCA that stops buying is a market-timer.
pub fn tilt(price: f64, sma: Option<f64>, rsi: Option<f64>, max: f64) -> (f64, Vec<String>) {
    let mut m = 1.0;
    let mut why = vec![];
    match sma {
        Some(s) if s > 0.0 && price > 0.0 => {
            let dev = (price - s) / s;
            if dev < 0.0 {
                m += -dev * 10.0;
                why.push(format!("price {:.1}% below SMA → tilt up", -dev * 100.0));
            } else {
                m -= dev * 5.0;
                why.push(format!("price {:.1}% above SMA → tilt down", dev * 100.0));
            }
        }
        _ => why.push("no SMA (not enough candles) → no trend tilt".into()),
    }
    match rsi {
        Some(r) if r < 30.0 => { m *= 1.5; why.push(format!("RSI {r:.0} oversold → ×1.5")); }
        Some(r) if r > 70.0 => { m *= 0.5; why.push(format!("RSI {r:.0} overbought → ×0.5")); }
        Some(r) => why.push(format!("RSI {r:.0} neutral")),
        None => why.push("no RSI".into()),
    }
    let m = m.clamp(0.25, max.max(0.25));
    why.push(format!("multiplier {m:.2}× (clamped to [0.25, {max:.1}])"));
    (m, why)
}

/// Where a position gets liquidated, ignoring fees. `mmr` = maintenance margin rate (Coinbase
/// perps: roughly 0.5–1 %; we assume 1 % to err on the near side).
///   long : entry × (1 − 1/lev + mmr)      short: entry × (1 + 1/lev − mmr)
pub fn est_liquidation(entry: f64, leverage: u32, long: bool, mmr: f64) -> Option<f64> {
    if leverage <= 1 || entry <= 0.0 { return None; }
    let inv = 1.0 / leverage as f64;
    Some(if long { entry * (1.0 - inv + mmr) } else { entry * (1.0 + inv - mmr) })
}

// ───────────────────────────────────────── state ─────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct DcaState {
    pub last_tick_ts: u64,
    pub ticks: u64,
    pub margin_spent_usd: f64,
    pub notional_usd: f64,
    pub base_acc: f64,
    pub avg_entry: f64,
    pub history: Vec<Value>,
}

impl DcaState {
    pub fn load(path: &str) -> DcaState {
        let v: Value = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
        let f = |k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
        DcaState {
            last_tick_ts: v.get("last_tick_ts").and_then(|x| x.as_u64()).unwrap_or(0),
            ticks: v.get("ticks").and_then(|x| x.as_u64()).unwrap_or(0),
            margin_spent_usd: f("margin_spent_usd"), notional_usd: f("notional_usd"),
            base_acc: f("base_acc"), avg_entry: f("avg_entry"),
            history: v.get("history").and_then(|x| x.as_array()).cloned().unwrap_or_default(),
        }
    }
    pub fn save(&self, path: &str) -> Result<(), String> {
        if let Some(dir) = std::path::Path::new(path).parent() { std::fs::create_dir_all(dir).map_err(|e| e.to_string())?; }
        std::fs::write(path, serde_json::to_string_pretty(&self.to_json()).unwrap_or_default()).map_err(|e| e.to_string())
    }
    pub fn to_json(&self) -> Value {
        json!({"last_tick_ts": self.last_tick_ts, "ticks": self.ticks, "margin_spent_usd": self.margin_spent_usd,
               "notional_usd": self.notional_usd, "base_acc": self.base_acc, "avg_entry": self.avg_entry,
               "history": self.history})
    }
    /// Fold one fill into the running average. Pure.
    pub fn apply_fill(&mut self, notional: f64, margin: f64, base: f64, price: f64, ts: u64) {
        let total_base = self.base_acc + base;
        if total_base > 0.0 { self.avg_entry = (self.avg_entry * self.base_acc + price * base) / total_base; }
        self.base_acc = total_base;
        self.notional_usd += notional;
        self.margin_spent_usd += margin;
        self.ticks += 1;
        self.last_tick_ts = ts;
    }
    /// `(due, seconds until next)`
    pub fn due(&self, every_hours: f64, now: u64) -> (bool, i64) {
        if self.last_tick_ts == 0 { return (true, 0); } // never ticked → the first beat is now
        let period = (every_hours * 3600.0) as u64;
        let next = self.last_tick_ts + period;
        (now >= next, next as i64 - now as i64)
    }
}

// ───────────────────────────────────────── the plan ─────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MarketView {
    pub price: f64,
    pub sma: Option<f64>,
    pub rsi: Option<f64>,
    pub candles: usize,
    pub funding_rate: f64,
    pub base_increment: String,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub multiplier: f64,
    pub margin_usd: f64,
    pub notional_usd: f64,
    pub base_size: String,
    pub leverage: u32,
    pub est_liq: Option<f64>,
    pub liq_distance_pct: Option<f64>,
    pub reasons: Vec<String>,
    pub verdict: Verdict,
    pub order: Value,
}

impl Plan {
    pub fn to_json(&self) -> Value {
        json!({"multiplier": self.multiplier, "margin_usd": self.margin_usd, "notional_usd": self.notional_usd,
               "base_size": self.base_size, "leverage": self.leverage, "est_liquidation": self.est_liq,
               "liq_distance_pct": self.liq_distance_pct, "reasons": self.reasons,
               "gate": match &self.verdict { Verdict::Pass => "PASS".to_string(), Verdict::Reject(r) => format!("REJECTED: {r}") },
               "would_send": self.order})
    }
}

/// Size one tick. Pure — the gate, the cap and the liquidation distance all decide here.
pub fn plan(cfg: &DcaConfig, mv: &MarketView, state: &DcaState) -> Plan {
    let (mult, mut reasons) = if cfg.dip_tilt { tilt(mv.price, mv.sma, mv.rsi, cfg.max_multiplier) }
                              else { (1.0, vec!["dip tilt off → flat ticket".into()]) };
    let lev = cfg.leverage.max(1);
    let margin = (cfg.base_usd * mult * 100.0).round() / 100.0;
    let notional = margin * lev as f64;
    let base_size = base_from_quote(notional, mv.price, &mv.base_increment).unwrap_or_else(|_| "0".into());
    let est_liq = est_liquidation(mv.price, lev, true, 0.01);
    let liq_dist = est_liq.map(|l| (mv.price - l) / mv.price * 100.0);
    reasons.push(format!("ticket ${margin:.2} margin × {lev}x = ${notional:.2} notional = {base_size} @ {:.2}", mv.price));

    let mut verdict = cfg.gate.check(&cfg.product_id, "BUY", notional, lev, None, Some(mv.price));
    if verdict == Verdict::Pass && state.notional_usd + notional > cfg.max_position_usd {
        verdict = Verdict::Reject(format!("position cap: ${:.2} held + ${notional:.2} > ${:.2}", state.notional_usd, cfg.max_position_usd));
    }
    if verdict == Verdict::Pass {
        if let Some(d) = liq_dist {
            if d < cfg.min_liq_distance_pct {
                verdict = Verdict::Reject(format!("liquidation only {d:.1}% below entry (est. {:.2}); floor is {}%",
                                                  est_liq.unwrap_or(0.0), cfg.min_liq_distance_pct));
            }
        }
    }
    if verdict == Verdict::Pass && base_size.parse::<f64>().unwrap_or(0.0) <= 0.0 {
        verdict = Verdict::Reject("ticket rounds to zero base units".into());
    }
    if mv.funding_rate.abs() > 0.0005 {
        reasons.push(format!("funding {:.4}%/period — {}", mv.funding_rate * 100.0,
                             if mv.funding_rate > 0.0 { "longs PAY" } else { "longs RECEIVE" }));
    }
    let kind = if is_perp(&cfg.product_id) { OrderKind::MarketBase { base_size: base_size.clone() } }
               else { OrderKind::MarketQuote { quote_size: round_down(notional, "0.01") } };
    let order = build_order(&cfg.product_id, "BUY", &kind, Some(lev), Some(&cfg.margin_type), None);
    Plan { multiplier: mult, margin_usd: margin, notional_usd: notional, base_size, leverage: lev,
           est_liq, liq_distance_pct: liq_dist, reasons, verdict, order }
}

/// Read what the plan needs: candles → SMA/RSI, the mark price, and the product's increments.
pub fn market_view(client: &Coinbase, cfg: &DcaConfig) -> Result<MarketView, String> {
    let product = client.product(&cfg.product_id)?;
    let candles = client.candles(&cfg.product_id, &cfg.candle_gran, 350).unwrap_or_default();
    let closes: Vec<f64> = candles.iter().map(|c| c.close).collect();
    let price = client.mark(&cfg.product_id)?;
    Ok(MarketView { price, sma: sma(&closes, cfg.sma_len), rsi: rsi(&closes, cfg.rsi_len), candles: closes.len(),
                    funding_rate: product.funding_rate, base_increment: product.base_increment })
}

/// One tick of the metronome. Propose-only unless `confirm`. `force` ignores the schedule.
pub fn tick(cfg: &DcaConfig, client: &Coinbase, confirm: bool, force: bool) -> Result<Value, String> {
    let path = cfg.state_path();
    let mut state = DcaState::load(&path);
    let now = now_s();
    let (due, in_s) = state.due(cfg.every_hours, now);
    if !due && !force {
        return Ok(json!({"ok": true, "due": false, "next_in_s": in_s, "state": state.to_json(), "config": cfg.to_json()}));
    }
    let mv = market_view(client, cfg)?;
    let p = plan(cfg, &mv, &state);
    let mut out = json!({"ok": true, "due": true, "config": cfg.to_json(),
                         "market": {"price": mv.price, "sma": mv.sma, "rsi": mv.rsi, "candles": mv.candles, "funding_rate": mv.funding_rate},
                         "plan": p.to_json(), "state_before": state.to_json(), "confirmed": false});
    if let Verdict::Reject(r) = &p.verdict {
        out["sent"] = json!(false); out["reason"] = json!(r);
        record(&json!({"kind": "dca_reject", "product": cfg.product_id, "reason": r, "plan": p.to_json()}));
        return Ok(out);
    }
    if client.is_signed() {
        match client.preview(&p.order) {
            Ok(pv) => out["preview"] = pv,
            Err(e) => out["preview"] = json!({"error": e}),
        }
    }
    if !confirm {
        out["note"] = json!("PROPOSAL ONLY — nothing sent. Re-run with confirm=true to place this DCA ticket.");
        return Ok(out);
    }
    let res = client.place_order(&p.order, true)?;
    let fill_price = res.get("result").and_then(|r| r.get("success_response")).and_then(|_| Some(mv.price)).unwrap_or(mv.price);
    state.apply_fill(p.notional_usd, p.margin_usd, p.base_size.parse().unwrap_or(0.0), fill_price, now);
    state.history.push(json!({"ts": now, "price": mv.price, "margin_usd": p.margin_usd, "notional_usd": p.notional_usd,
                              "base_size": p.base_size, "multiplier": p.multiplier, "leverage": p.leverage,
                              "order_id": res.get("result").and_then(|r| r.get("success_response")).and_then(|s| s.get("order_id")).cloned()}));
    if state.history.len() > 500 { let n = state.history.len() - 500; state.history.drain(..n); }
    state.save(&path)?;
    record(&json!({"kind": "dca_fill", "product": cfg.product_id, "plan": p.to_json(), "result": res}));
    out["confirmed"] = json!(true); out["sent"] = json!(true); out["result"] = res; out["state_after"] = state.to_json();
    Ok(out)
}

/// Risk read on live perp positions: distance to liquidation, flagged when under `warn_pct`.
pub fn risk_report(positions: &[crate::Position], warn_pct: f64) -> Value {
    let rows: Vec<Value> = positions.iter().map(|p| {
        let dist = if p.mark > 0.0 && p.liq > 0.0 { Some((p.mark - p.liq).abs() / p.mark * 100.0) } else { None };
        json!({"product_id": p.product_id, "side": p.side, "size": p.size, "entry": p.entry, "mark": p.mark,
               "liquidation": p.liq, "leverage": p.leverage, "unrealized_pnl": p.unrealized_pnl,
               "liq_distance_pct": dist, "warn": dist.map(|d| d < warn_pct).unwrap_or(false)})
    }).collect();
    let warn = rows.iter().filter(|r| r["warn"] == json!(true)).count();
    json!({"positions": rows, "warnings": warn, "warn_below_pct": warn_pct,
           "note": if warn > 0 { "at least one position is inside the warning band — add margin or reduce" } else { "no position inside the warning band" }})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mv(price: f64, sma_v: Option<f64>, rsi_v: Option<f64>) -> MarketView {
        MarketView { price, sma: sma_v, rsi: rsi_v, candles: 300, funding_rate: 0.0, base_increment: "0.00000001".into() }
    }

    #[test]
    fn sma_and_rsi_basics() {
        let c: Vec<f64> = (1..=10).map(|x| x as f64).collect();
        assert_eq!(sma(&c, 5), Some(8.0));
        assert_eq!(sma(&c, 11), None);
        assert_eq!(rsi(&c, 5), Some(100.0));
        let down: Vec<f64> = (1..=10).rev().map(|x| x as f64).collect();
        assert!(rsi(&down, 5).unwrap() < 1.0);
        let flat = vec![5.0; 20];
        assert_eq!(rsi(&flat, 14), Some(100.0));
    }

    #[test]
    fn tilt_buys_more_on_dips_less_on_rips() {
        let (m_dip, _) = tilt(90.0, Some(100.0), Some(50.0), 3.0);
        assert!((m_dip - 2.0).abs() < 1e-9, "10% under SMA → 2×, got {m_dip}");
        let (m_rip, _) = tilt(110.0, Some(100.0), Some(50.0), 3.0);
        assert!((m_rip - 0.5).abs() < 1e-9);
        let (m_cap, _) = tilt(50.0, Some(100.0), Some(20.0), 3.0);
        assert_eq!(m_cap, 3.0, "clamped at max");
        let (m_floor, _) = tilt(200.0, Some(100.0), Some(90.0), 3.0);
        assert_eq!(m_floor, 0.25, "never zero");
        let (m_none, why) = tilt(100.0, None, None, 3.0);
        assert_eq!(m_none, 1.0);
        assert!(why.iter().any(|w| w.contains("no SMA")));
    }

    #[test]
    fn liquidation_estimate() {
        assert_eq!(est_liquidation(100.0, 1, true, 0.01), None);
        let l = est_liquidation(100.0, 5, true, 0.01).unwrap();
        assert!((l - 81.0).abs() < 1e-9, "5x long liq at entry×(1−0.2+0.01)");
        let s = est_liquidation(100.0, 2, false, 0.01).unwrap();
        assert!((s - 149.0).abs() < 1e-9);
    }

    #[test]
    fn plan_spot_flat() {
        let cfg = DcaConfig { product_id: "BTC-USD".into(), base_usd: 25.0, dip_tilt: false, ..Default::default() };
        let p = plan(&cfg, &mv(100_000.0, None, None), &DcaState::default());
        assert_eq!(p.verdict, Verdict::Pass);
        assert_eq!(p.notional_usd, 25.0);
        assert_eq!(p.order["order_configuration"]["market_market_ioc"]["quote_size"], "25.00");
        assert!(p.order.get("leverage").is_none());
        assert_eq!(p.est_liq, None);
    }

    #[test]
    fn plan_perp_with_leverage_and_liq_floor() {
        let cfg = DcaConfig { product_id: "BTC-PERP-INTX".into(), base_usd: 20.0, leverage: 3, dip_tilt: false, ..Default::default() };
        let p = plan(&cfg, &mv(100_000.0, None, None), &DcaState::default());
        assert_eq!(p.verdict, Verdict::Pass);
        assert_eq!(p.notional_usd, 60.0);
        assert_eq!(p.order["leverage"], "3");
        assert_eq!(p.order["order_configuration"]["market_market_ioc"]["base_size"], "0.00060000");
        assert!(p.liq_distance_pct.unwrap() > 30.0);
        // 3x is the default gate cap; 4x must be refused by the gate, not silently clamped
        let hot = DcaConfig { leverage: 4, ..cfg.clone() };
        assert!(matches!(plan(&hot, &mv(100_000.0, None, None), &DcaState::default()).verdict, Verdict::Reject(_)));
        // a liq floor tighter than 3x's ~32% distance refuses too
        let tight = DcaConfig { min_liq_distance_pct: 40.0, ..cfg.clone() };
        let r = plan(&tight, &mv(100_000.0, None, None), &DcaState::default());
        assert!(matches!(&r.verdict, Verdict::Reject(m) if m.contains("liquidation")));
    }

    #[test]
    fn plan_respects_position_cap_and_gate_notional() {
        let cfg = DcaConfig { base_usd: 25.0, dip_tilt: false, max_position_usd: 100.0, ..Default::default() };
        let mut st = DcaState::default();
        st.notional_usd = 90.0;
        assert!(matches!(plan(&cfg, &mv(100_000.0, None, None), &st).verdict, Verdict::Reject(_)));
        let big = DcaConfig { base_usd: 500.0, dip_tilt: false, ..Default::default() };
        assert!(matches!(plan(&big, &mv(100_000.0, None, None), &DcaState::default()).verdict, Verdict::Reject(_)));
    }

    #[test]
    fn state_fold_and_schedule() {
        let mut s = DcaState::default();
        s.apply_fill(100.0, 100.0, 0.001, 100_000.0, 1000);
        s.apply_fill(100.0, 100.0, 0.002, 50_000.0, 2000);
        assert!((s.base_acc - 0.003).abs() < 1e-12);
        assert!((s.avg_entry - 66_666.666).abs() < 1.0);
        assert_eq!(s.ticks, 2);
        assert_eq!(s.due(1.0, 2000 + 3599).0, false);
        assert_eq!(s.due(1.0, 2000 + 3600).0, true);
        let fresh = DcaState::default();
        assert!(fresh.due(24.0, 5).0, "never ticked → due now");
    }

    #[test]
    fn config_from_json_whitelists_its_own_product() {
        let c = DcaConfig::from_json(&json!({"product_id": "sol-usd", "base_usd": 10, "leverage": 2, "max_leverage": 5}));
        assert_eq!(c.product_id, "SOL-USD");
        assert!(c.gate.whitelist.contains(&"SOL-USD".to_string()));
        assert_eq!(c.gate.max_leverage, 5);
        assert_eq!(c.leverage, 2);
    }

    #[test]
    fn risk_report_flags_near_liq() {
        let p = crate::Position { product_id: "BTC-PERP-INTX".into(), side: "LONG".into(), size: 0.01, entry: 100.0,
                                  mark: 100.0, liq: 95.0, leverage: 10.0, unrealized_pnl: 0.0, margin: 10.0 };
        let r = risk_report(&[p], 10.0);
        assert_eq!(r["warnings"], 1);
    }
}
