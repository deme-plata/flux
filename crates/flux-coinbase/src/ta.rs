//! Technical-analysis measurements — pure functions over candles, book and tape, and the report +
//! prompt that the TUI hands to `claude -p`. Nothing here decides anything; it measures, and the
//! numbers travel with the question so the answer can be checked against them.

use crate::dca::{rsi, sma};
use crate::{Book, Candle, Gate, Position, ProductInfo, Tape};
use serde_json::{json, Value};

pub fn ema_series(closes: &[f64], n: usize) -> Vec<f64> {
    if n == 0 || closes.is_empty() { return vec![]; }
    let k = 2.0 / (n as f64 + 1.0);
    let mut out = Vec::with_capacity(closes.len());
    let mut e = closes[0];
    for (i, c) in closes.iter().enumerate() {
        e = if i == 0 { *c } else { c * k + e * (1.0 - k) };
        out.push(e);
    }
    out
}
pub fn ema(closes: &[f64], n: usize) -> Option<f64> {
    if closes.len() < n { return None; }
    ema_series(closes, n).last().copied()
}

/// MACD(12,26,9) → (macd, signal, histogram).
pub fn macd(closes: &[f64]) -> Option<(f64, f64, f64)> {
    if closes.len() < 35 { return None; }
    let (e12, e26) = (ema_series(closes, 12), ema_series(closes, 26));
    let line: Vec<f64> = e12.iter().zip(e26.iter()).map(|(a, b)| a - b).collect();
    let sig = ema_series(&line[26..], 9);
    let m = *line.last()?; let s = *sig.last()?;
    Some((m, s, m - s))
}

/// Bollinger(n, k) → (lower, mid, upper, %b) where %b = (price − lower)/(upper − lower).
pub fn bollinger(closes: &[f64], n: usize, k: f64) -> Option<(f64, f64, f64, f64)> {
    if closes.len() < n || n == 0 { return None; }
    let w = &closes[closes.len() - n..];
    let mid = w.iter().sum::<f64>() / n as f64;
    let var = w.iter().map(|x| (x - mid).powi(2)).sum::<f64>() / n as f64;
    let sd = var.sqrt();
    let (lo, hi) = (mid - k * sd, mid + k * sd);
    let last = *w.last()?;
    let pct_b = if hi > lo { (last - lo) / (hi - lo) } else { 0.5 };
    Some((lo, mid, hi, pct_b))
}

/// Average true range over the last n candles (Wilder).
pub fn atr(c: &[Candle], n: usize) -> Option<f64> {
    if c.len() < n + 1 || n == 0 { return None; }
    let trs: Vec<f64> = c.windows(2).map(|w| {
        let (p, k) = (&w[0], &w[1]);
        (k.high - k.low).max((k.high - p.close).abs()).max((k.low - p.close).abs())
    }).collect();
    Some(trs[trs.len() - n..].iter().sum::<f64>() / n as f64)
}

/// Volume-weighted average price of a set of trades.
pub fn vwap(prices_sizes: &[(f64, f64)]) -> Option<f64> {
    let v: f64 = prices_sizes.iter().map(|(_, s)| s).sum();
    if v <= 0.0 { return None; }
    Some(prices_sizes.iter().map(|(p, s)| p * s).sum::<f64>() / v)
}

/// Swing high / low of the window and where price sits inside it (0 = at low, 1 = at high).
pub fn range_position(c: &[Candle]) -> Option<(f64, f64, f64)> {
    if c.is_empty() { return None; }
    let hi = c.iter().map(|k| k.high).fold(f64::MIN, f64::max);
    let lo = c.iter().map(|k| k.low).fold(f64::MAX, f64::min);
    let last = c.last()?.close;
    Some((hi, lo, if hi > lo { (last - lo) / (hi - lo) } else { 0.5 }))
}

/// Everything measurable about one timeframe, as JSON.
pub fn frame_report(label: &str, c: &[Candle]) -> Value {
    let closes: Vec<f64> = c.iter().map(|k| k.close).collect();
    let last = closes.last().copied().unwrap_or(0.0);
    let pct = |x: Option<f64>| x.map(|v| if last > 0.0 { (last - v) / v * 100.0 } else { 0.0 });
    let (s20, s50, s200) = (sma(&closes, 20), sma(&closes, 50), sma(&closes, 200));
    let vol_recent: f64 = c.iter().rev().take(10).map(|k| k.volume).sum();
    let vol_prior: f64 = c.iter().rev().skip(10).take(10).map(|k| k.volume).sum();
    let change = if c.len() >= 2 && c[0].open > 0.0 { (last - c[0].open) / c[0].open * 100.0 } else { 0.0 };
    json!({
        "timeframe": label, "candles": c.len(), "last": last, "change_over_window_pct": change,
        "sma20": s20, "sma50": s50, "sma200": s200,
        "price_vs_sma20_pct": pct(s20), "price_vs_sma50_pct": pct(s50), "price_vs_sma200_pct": pct(s200),
        "ema12": ema(&closes, 12), "ema26": ema(&closes, 26),
        "rsi14": rsi(&closes, 14),
        "macd_12_26_9": macd(&closes).map(|(m, s, h)| json!({"macd": m, "signal": s, "hist": h})),
        "bollinger_20_2": bollinger(&closes, 20, 2.0).map(|(lo, mid, hi, b)| json!({"lower": lo, "mid": mid, "upper": hi, "pct_b": b})),
        "atr14": atr(c, 14), "atr14_pct": atr(c, 14).map(|a| if last > 0.0 { a / last * 100.0 } else { 0.0 }),
        "range": range_position(c).map(|(hi, lo, pos)| json!({"high": hi, "low": lo, "position_0_low_1_high": pos})),
        "volume_last10_vs_prior10": if vol_prior > 0.0 { Some(vol_recent / vol_prior) } else { None },
        "last_5_closes": closes.iter().rev().take(5).rev().collect::<Vec<_>>(),
    })
}

pub fn book_report(b: &Book, mark: f64) -> Value {
    let within = |lv: &[crate::Level], pct: f64, below: bool| -> f64 {
        lv.iter().filter(|l| if below { l.price >= mark * (1.0 - pct) } else { l.price <= mark * (1.0 + pct) })
          .map(|l| l.price * l.size).sum()
    };
    json!({
        "best_bid": b.best_bid(), "best_ask": b.best_ask(), "mid": b.mid(), "spread_bps": b.spread_bps(),
        "imbalance_top10": b.imbalance(10), "imbalance_top25": b.imbalance(25),
        "bid_depth_usd_within_0_5pct": within(&b.bids, 0.005, true),
        "ask_depth_usd_within_0_5pct": within(&b.asks, 0.005, false),
        "largest_bid": b.bids.iter().max_by(|x, y| x.size.partial_cmp(&y.size).unwrap()).map(|l| json!([l.price, l.size])),
        "largest_ask": b.asks.iter().max_by(|x, y| x.size.partial_cmp(&y.size).unwrap()).map(|l| json!([l.price, l.size])),
    })
}

pub fn tape_report(t: &Tape) -> Value {
    let buys: f64 = t.trades.iter().filter(|x| x.side.eq_ignore_ascii_case("BUY")).map(|x| x.size).sum();
    let sells: f64 = t.trades.iter().filter(|x| x.side.eq_ignore_ascii_case("SELL")).map(|x| x.size).sum();
    let ps: Vec<(f64, f64)> = t.trades.iter().map(|x| (x.price, x.size)).collect();
    json!({
        "trades": t.trades.len(), "buy_volume": buys, "sell_volume": sells,
        "buy_ratio": if buys + sells > 0.0 { Some(buys / (buys + sells)) } else { None },
        "vwap": vwap(&ps),
        "largest_trade": t.trades.iter().max_by(|x, y| x.size.partial_cmp(&y.size).unwrap()).map(|x| json!({"side": x.side, "price": x.price, "size": x.size})),
        "first_time": t.trades.last().map(|x| x.time.clone()), "last_time": t.trades.first().map(|x| x.time.clone()),
    })
}

/// What the chart in front of the operator is: zoom label, candle width, how much time it spans.
#[derive(Debug, Clone)]
pub struct View { pub zoom: String, pub candle_secs: u64, pub candles: usize }

impl View {
    pub fn span_secs(&self) -> u64 { self.candle_secs * self.candles as u64 }
    /// The horizon a chart of this span is naturally about — the analysis must answer at THAT scale.
    pub fn horizon(&self) -> (&'static str, &'static str) {
        match self.span_secs() {
            0..=1800 => ("scalp", "the next few minutes — entry timing, tape and book pressure dominate; SMAs on this frame are noise"),
            1801..=21_600 => ("intraday", "the next few hours — session structure, VWAP, intraday levels"),
            21_601..=259_200 => ("swing", "the next days — daily levels, 1h trend, funding drift"),
            _ => ("position", "weeks to months — the daily trend, SMA200, the DCA horizon itself"),
        }
    }
    pub fn to_json(&self) -> Value {
        let (h, d) = self.horizon();
        json!({"zoom": self.zoom, "candle_secs": self.candle_secs, "candles": self.candles, "span_secs": self.span_secs(),
               "span_human": human_span(self.span_secs()), "horizon": h, "horizon_meaning": d})
    }
}

pub fn human_span(s: u64) -> String {
    if s < 120 { format!("{s} s") } else if s < 7200 { format!("{} min", s / 60) }
    else if s < 172_800 { format!("{} h", s / 3600) } else { format!("{} d", s / 86_400) }
}

/// The full measurement bundle the analysis is asked about. `frames[0]` is the chart on screen.
#[allow(clippy::too_many_arguments)]
pub fn report(product: &ProductInfo, book: &Book, tape: &Tape, view: &View, zoom_candles: &[Candle],
              c1h: &[Candle], c1d: &[Candle], positions: &[Position], dca_state: &Value, gate: &Gate) -> Value {
    let mark = book.mid().filter(|m| *m > 0.0).unwrap_or(product.price);
    json!({
        "venue": "coinbase-advanced-trade", "product": crate::product_json(product), "mark": mark,
        "measured_at_unix": crate::now_s(), "view": view.to_json(),
        "frames": [frame_report(&format!("{} (on screen: {} candles of {} s)", view.zoom, view.candles, view.candle_secs), zoom_candles),
                   frame_report("1h", c1h), frame_report("1d", c1d)],
        "book": book_report(book, mark), "tape": tape_report(tape),
        "perp": if product.perp { Some(json!({"funding_rate": product.funding_rate, "funding_rate_annualized_pct": product.funding_rate * 3.0 * 365.0 * 100.0,
                                             "open_interest": product.open_interest, "max_leverage": product.max_leverage})) } else { None },
        "positions": positions.iter().map(crate::position_json).collect::<Vec<_>>(),
        "risk": crate::dca::risk_report(positions, 15.0),
        "dca_state": dca_state, "gate": gate.to_json(),
    })
}

/// The question. The numbers are in the prompt verbatim so the answer can be audited against them.
pub fn prompt(report: &Value) -> String {
    let pid = report["product"]["product_id"].as_str().unwrap_or("?");
    let v = &report["view"];
    let (zoom, span, horizon, meaning) = (v["zoom"].as_str().unwrap_or("?"), v["span_human"].as_str().unwrap_or("?"),
                                         v["horizon"].as_str().unwrap_or("?"), v["horizon_meaning"].as_str().unwrap_or(""));
    format!(
"You are a market analyst inside a trading terminal. Below is a live measurement bundle for {pid} on Coinbase Advanced Trade (JSON). Use ONLY these numbers.

THE OPERATOR IS LOOKING AT THE {zoom} CHART — {span} of history, horizon = {horizon} ({meaning}). frames[0] is that chart; frames[1] (1h) and frames[2] (1d) are context. Answer at the {horizon} horizon: a 1-second chart wants a read on the next minutes and the tape, a daily chart wants a read on the coming weeks and the SMA200. Do not give a swing-trade answer to a scalping chart or vice versa; say explicitly which frame you are weighting and why.

Write a tight analysis (≤ 350 words, plain text, no markdown headers, no disclaimers) with these sections, each a short paragraph, each citing the specific figures it rests on:
1. REGIME — trend and momentum at the {horizon} horizon first (frames[0]: price vs SMA20/50/200, EMA12/26, MACD histogram, RSI14, Bollinger %b, ATR%), then whether the 1h and 1d frames agree or disagree.
2. LEVELS — the concrete prices that matter at this horizon: frames[0] window high/low, Bollinger bands, SMAs, largest resting bids/asks, VWAP; add the 1d levels only if they are within reach of this span.
3. FLOW — what the order book and tape say right now (spread, imbalance, depth within 0.5%, buy ratio, largest trade), and for a perp the funding rate. At scalp horizon this section leads; at position horizon it is a footnote.
4. POSITIONING — given the gate (max notional, max leverage), the DCA state and any open position with its liquidation distance: what a leveraged-DCA operator should do at THIS horizon (scalp/intraday: when in the next minutes/hours to fire the ticket; swing/position: size and leverage of the next tick — add / hold / reduce), and why. Name the risk that would invalidate the read.
End with one line: VERDICT ({horizon}, {span}): <BULLISH|NEUTRAL|BEARISH> <confidence 0-100>% — <one clause>.

MEASUREMENTS:
{}",
        serde_json::to_string_pretty(report).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candles(closes: &[f64]) -> Vec<Candle> {
        closes.iter().enumerate().map(|(i, c)| Candle { start: i as u64 * 60, open: *c, high: c + 1.0, low: c - 1.0, close: *c, volume: 1.0 }).collect()
    }

    #[test]
    fn ema_macd_boll_atr() {
        let up: Vec<f64> = (1..=60).map(|x| x as f64).collect();
        assert!(ema(&up, 12).unwrap() > sma(&up, 12).unwrap() - 6.0);
        let (m, s, h) = macd(&up).unwrap();
        assert!(m > 0.0 && s > 0.0 && (m - s - h).abs() < 1e-12, "steady uptrend → positive macd");
        let (lo, mid, hi, b) = bollinger(&up, 20, 2.0).unwrap();
        assert!(lo < mid && mid < hi && b > 0.9, "last close in an uptrend sits near the upper band");
        let a = atr(&candles(&up), 14).unwrap();
        assert!((a - 2.0).abs() < 1e-9, "each candle spans 2.0 → ATR 2.0 (tr = max(2, |h-pc|=2, |l-pc|=0))");
        assert_eq!(macd(&up[..10]), None);
    }

    #[test]
    fn vwap_and_range() {
        assert!((vwap(&[(100.0, 1.0), (200.0, 3.0)]).unwrap() - 175.0).abs() < 1e-9);
        assert_eq!(vwap(&[]), None);
        let (hi, lo, pos) = range_position(&candles(&[10.0, 20.0, 15.0])).unwrap();
        assert_eq!((hi, lo), (21.0, 9.0));
        assert!((pos - 0.5).abs() < 1e-9);
    }

    #[test]
    fn report_and_prompt_carry_the_numbers() {
        let c = candles(&(1..=40).map(|x| 100.0 + x as f64).collect::<Vec<_>>());
        let p = ProductInfo { product_id: "BTC-USD".into(), price: 140.0, ..Default::default() };
        let b = Book { product_id: "BTC-USD".into(), bids: vec![crate::Level { price: 139.0, size: 2.0 }], asks: vec![crate::Level { price: 141.0, size: 1.0 }], time: String::new() };
        let t = Tape { trades: vec![crate::Trade { trade_id: "1".into(), price: 140.0, size: 0.5, side: "BUY".into(), time: "t".into() }], best_bid: 139.0, best_ask: 141.0 };
        let view = View { zoom: "1m".into(), candle_secs: 60, candles: c.len() };
        let r = report(&p, &b, &t, &view, &c, &c, &c, &[], &json!({"ticks": 0}), &Gate::default());
        assert_eq!(r["frames"].as_array().unwrap().len(), 3);
        assert_eq!(r["view"]["horizon"], "intraday");
        assert_eq!(r["view"]["span_secs"], 2400);
        assert!(r["frames"][0]["rsi14"].as_f64().unwrap() > 90.0);
        assert!((r["book"]["imbalance_top10"].as_f64().unwrap() - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(r["tape"]["buy_ratio"], 1.0);
        let q = prompt(&r);
        assert!(q.contains("BTC-USD") && q.contains("VERDICT") && q.contains("\"rsi14\""));
        assert!(q.contains("LOOKING AT THE 1m CHART") && q.contains("horizon = intraday"));
    }

    #[test]
    fn horizon_follows_span() {
        let h = |z: &str, cs: u64, n: usize| View { zoom: z.into(), candle_secs: cs, candles: n }.horizon().0;
        assert_eq!(h("1s", 1, 180), "scalp");
        assert_eq!(h("5s", 5, 180), "scalp");
        assert_eq!(h("1h", 60, 60), "intraday");
        assert_eq!(h("24h", 900, 96), "swing");
        assert_eq!(h("D", 86_400, 120), "position");
        assert_eq!(human_span(180), "3 min"); assert_eq!(human_span(86_400), "24 h"); assert_eq!(human_span(86_400 * 120), "120 d");
    }
}
