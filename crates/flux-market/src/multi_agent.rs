//! multi_agent.rs — a SWARM of flux-llm trader agents trading inside chronos.
//!
//! The vision: spin up many Qwen-flux-llm agents, network them over flux-p2p, and
//! let them trade against a shared market in chronos virtual time. This is the
//! deterministic, testable core of that: N [`Trader`]s with different policies
//! (contrarian / trend / balanced) each run the flux-moe decision loop over a
//! shared price + Fear&Greed series, paper-execute, and **gossip** every decision
//! into a shared feed (the flux-p2p stand-in). Many agents × many steps = many
//! trades, fully reproducible. Real deployment = `flux_nodeswarm_spawn` N served
//! Qwen nodes + flux-p2p gossip + `flux_chronos_run`; this proves the economics.

use crate::fear_greed::Sentiment;
use crate::sim::{execute, Decision, SimPortfolio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Buys the dip HARDER as fear rises (the horror-dip thesis).
    Contrarian,
    /// Momentum: buys strength (price above trend).
    Trend,
    /// Plain DCA on the dip.
    Balanced,
}

/// One flux-llm trader agent.
#[derive(Debug, Clone)]
pub struct Trader {
    pub id: String,
    pub p: SimPortfolio,
    pub dca_usds: f64,
    pub style: Style,
}
impl Trader {
    pub fn new(id: impl Into<String>, start_usds: f64, dca_usds: f64, style: Style) -> Self {
        Self { id: id.into(), p: SimPortfolio::new(start_usds), dca_usds, style }
    }
    /// The agent's decision for this step given price, trend, and the Fear&Greed value.
    pub fn decide(&self, price: f64, sma: f64, fng: u8) -> Decision {
        let sent = Sentiment::from_value(fng);
        match self.style {
            Style::Contrarian if price < sma && self.p.usds > 0.0 => Decision::DcaBuy {
                usds: (self.dca_usds * sent.dca_multiplier()).min(self.p.usds),
                reason: format!("contrarian: {} dip-buy ×{:.2}", sent.label(), sent.dca_multiplier()),
            },
            Style::Trend if price > sma && self.p.usds > 0.0 => Decision::DcaBuy {
                usds: self.dca_usds.min(self.p.usds),
                reason: "trend: momentum buy above SMA".into(),
            },
            Style::Balanced if price < sma && self.p.usds > 0.0 => Decision::DcaBuy {
                usds: self.dca_usds.min(self.p.usds),
                reason: "balanced: DCA the dip".into(),
            },
            _ => Decision::Hold { reason: "no signal".into() },
        }
    }
}

/// A gossiped trade (the flux-p2p feed entry).
#[derive(Debug, Clone)]
pub struct Gossip {
    pub step: usize,
    pub agent: String,
    pub action: String,
}

/// Final standing for one agent.
#[derive(Debug, Clone)]
pub struct AgentResult {
    pub id: String,
    pub style: Style,
    pub final_value: f64,
    pub pnl_pct: f64,
    pub btc: f64,
    pub trades: usize,
}

/// Run the swarm over a shared price + Fear&Greed series in chronos virtual time.
/// Returns the per-agent leaderboard (sorted best-first) + the full gossip feed.
pub fn run_swarm(
    mut traders: Vec<Trader>,
    prices: &[f64],
    fng: &[u8],
    sma_window: usize,
) -> (Vec<AgentResult>, Vec<Gossip>) {
    let starts: Vec<f64> = traders.iter().map(|t| t.p.value(prices.first().copied().unwrap_or(0.0))).collect();
    let mut gossip = vec![];
    let mut trades: Vec<usize> = vec![0; traders.len()];

    for (i, &price) in prices.iter().enumerate() {
        let lo = i.saturating_sub(sma_window.saturating_sub(1));
        let sma = prices[lo..=i].iter().sum::<f64>() / (i - lo + 1) as f64;
        let f = *fng.get(i).unwrap_or(&50);
        for (idx, t) in traders.iter_mut().enumerate() {
            let d = t.decide(price, sma, f);
            if let Decision::DcaBuy { usds, .. } = &d {
                trades[idx] += 1;
                gossip.push(Gossip { step: i, agent: t.id.clone(), action: format!("buy ${usds:.0} @ {price:.0} (fng {f})") });
            }
            execute(&mut t.p, &d, price);
        }
    }

    let final_price = prices.last().copied().unwrap_or(0.0);
    let mut board: Vec<AgentResult> = traders.iter().enumerate().map(|(idx, t)| {
        let fv = t.p.value(final_price);
        AgentResult {
            id: t.id.clone(), style: t.style, final_value: fv,
            pnl_pct: if starts[idx] == 0.0 { 0.0 } else { (fv - starts[idx]) / starts[idx] * 100.0 },
            btc: t.p.btc, trades: trades[idx],
        }
    }).collect();
    board.sort_by(|a, b| b.final_value.partial_cmp(&a.final_value).unwrap());
    (board, gossip)
}

#[cfg(test)]
mod tests {
    use super::*;

    // a fear-driven dip that recovers: fng low (fear) during the dip, high (greed) at the top
    const PRICES: [f64; 8] = [70_000.0, 66_000.0, 60_000.0, 57_000.0, 61_000.0, 67_000.0, 72_000.0, 75_000.0];
    const FNG: [u8; 8] = [55, 35, 18, 12, 30, 60, 78, 85];

    fn swarm() -> Vec<Trader> {
        vec![
            Trader::new("contrarian-1", 1000.0, 100.0, Style::Contrarian),
            Trader::new("trend-1", 1000.0, 100.0, Style::Trend),
            Trader::new("balanced-1", 1000.0, 100.0, Style::Balanced),
        ]
    }

    #[test]
    fn many_agents_make_many_trades_and_gossip() {
        let (board, gossip) = run_swarm(swarm(), &PRICES, &FNG, 3);
        assert_eq!(board.len(), 3);
        assert!(gossip.len() >= 4, "swarm should produce multiple gossiped trades, got {}", gossip.len());
        assert!(board.iter().all(|r| r.trades >= 1), "every agent trades at least once");
    }

    #[test]
    fn contrarian_beats_trend_on_the_fear_dip() {
        let (board, _) = run_swarm(swarm(), &PRICES, &FNG, 3);
        let c = board.iter().find(|r| r.style == Style::Contrarian).unwrap();
        let t = board.iter().find(|r| r.style == Style::Trend).unwrap();
        // buying the fear dip (cheap) beats buying momentum (expensive) into a recovery
        assert!(c.final_value > t.final_value, "contrarian {:.0} should beat trend {:.0}", c.final_value, t.final_value);
        assert!(c.btc > t.btc, "contrarian accumulates more BTC on the dip");
    }

    #[test]
    fn leaderboard_sorted_best_first() {
        let (board, _) = run_swarm(swarm(), &PRICES, &FNG, 3);
        for w in board.windows(2) {
            assert!(w[0].final_value >= w[1].final_value, "leaderboard must be sorted desc");
        }
    }
}
