//! referee.rs — flux-legacy WARD: **does a node's money match its block headers?** (live)
//!
//! A Quillon block header carries `total_coinbase_reward` — the exact integer sum of the block's
//! Coinbase transactions. A node that applies every block exactly once grows its total supply by
//! exactly that sum (a transfer moves money; only a coinbase makes it). So over any window of
//! blocks:  Δsupply / Σ header.total_coinbase_reward  must read 1.
//!
//! Measured 2026-10-30 (10-minute windows): the producer Epsilon 1.0000; the Stockholm follower
//! 1.7133, then 1.9870 after a restart — every block paid twice, invisible to every other check
//! because Quillon's own balance-root comparison runs in SHADOW mode (logs, never refuses).
//! This is SIGIL's rule ("a node whose state disagrees with the header stops") in the form a
//! legacy chain can have TODAY: no consensus change, read-only, two HTTP routes per node.
//!
//! Known limits (printed in every report): `/integrity/compare` reads height and supply a few ms
//! apart, so one window can be off by ±1 block at each end → judge only windows of ≥ `min_blocks`;
//! a fee that is burned (not re-paid) would read as ratio < 1; blocks the blocks-node cannot serve
//! are listed and the window is marked unreadable rather than guessed.

use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Sample {
    pub at_ms: u64,
    pub height: u64,
    #[serde(serialize_with = "ser_u128")]
    pub supply: u128,
    pub wallets: u64,
}

fn ser_u128<S: serde::Serializer>(v: &u128, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&v.to_string())
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub enum Verdict {
    /// |ratio − 1| ≤ tolerance over ≥ min_blocks blocks.
    Agrees,
    /// Supply grew more than the headers minted (double payment shape).
    OverPays,
    /// Supply grew less than the headers minted (lost update / skipped block / burned fee).
    UnderPays,
    /// Window too short to judge (skew of ±1 block dominates).
    TooFew,
    /// Some block in the window could not be read — no guess.
    Unreadable,
}

#[derive(Debug, Clone, Serialize)]
pub struct Judgement {
    pub node: String,
    pub from_height: u64,
    pub to_height: u64,
    pub blocks: u64,
    #[serde(serialize_with = "ser_u128")]
    pub minted: u128,
    pub grew: String, // signed, base units
    pub ratio: f64,
    pub transfers: u64,
    pub missing: Vec<u64>,
    pub verdict: Verdict,
}

/// Pure judge over two samples and the headers' minted sum for (from, to].
pub fn judge(node: &str, a: &Sample, b: &Sample, minted: u128, transfers: u64, missing: Vec<u64>,
             tolerance: f64, min_blocks: u64) -> Judgement {
    let blocks = b.height.saturating_sub(a.height);
    let grew: i128 = b.supply as i128 - a.supply as i128;
    let ratio = if minted == 0 { f64::NAN } else { grew as f64 / minted as f64 };
    let verdict = if !missing.is_empty() {
        Verdict::Unreadable
    } else if blocks < min_blocks || minted == 0 {
        Verdict::TooFew
    } else if (ratio - 1.0).abs() <= tolerance {
        Verdict::Agrees
    } else if ratio > 1.0 {
        Verdict::OverPays
    } else {
        Verdict::UnderPays
    };
    Judgement { node: node.to_string(), from_height: a.height, to_height: b.height, blocks, minted,
                grew: grew.to_string(), ratio, transfers, missing, verdict }
}

/// Exact integer after `"field":` in raw JSON (a bare number or a quoted string of digits).
/// serde_json without arbitrary_precision would round a 24-decimal QUG amount through f64.
pub fn raw_u128(json: &str, field: &str) -> Option<u128> {
    let pat = format!("\"{}\"", field);
    let at = json.find(&pat)? + pat.len();
    let rest = json[at..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"').unwrap_or(rest);
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn http() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder().timeout(Duration::from_secs(30)).build().expect("http client")
}

/// One reading of `<node>/api/v1/integrity/compare`.
pub fn sample(client: &reqwest::blocking::Client, node: &str) -> Result<Sample, String> {
    let url = format!("{}/api/v1/integrity/compare", node.trim_end_matches('/'));
    let body = client.get(&url).send().and_then(|r| r.text()).map_err(|e| format!("{url}: {e}"))?;
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| format!("{url}: {e}"))?;
    let d = &v["data"];
    Ok(Sample {
        at_ms: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
        height: d["height"].as_u64().ok_or("no height")?,
        supply: raw_u128(&body, "total_supply_base_units").ok_or("no total_supply_base_units")?,
        wallets: d["wallet_count"].as_u64().unwrap_or(0),
    })
}

/// header.total_coinbase_reward and the transfer count of one block, exact.
pub fn block_minted(client: &reqwest::blocking::Client, blocks_node: &str, h: u64) -> Result<(u128, u64), String> {
    let url = format!("{}/api/v1/blocks/{}", blocks_node.trim_end_matches('/'), h);
    let body = client.get(&url).send().and_then(|r| r.text()).map_err(|e| format!("{url}: {e}"))?;
    let minted = raw_u128(&body, "total_coinbase_reward").ok_or_else(|| format!("{url}: no total_coinbase_reward"))?;
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| format!("{url}: {e}"))?;
    let d = &v["data"];
    let txs = d["transactions"].as_array().map(|a| a.len() as u64).unwrap_or(0);
    let cb = d["header"]["coinbase_count"].as_u64().unwrap_or(txs);
    Ok((minted, txs.saturating_sub(cb)))
}

pub struct RefereeConfig {
    pub node: String,
    /// node that serves the blocks (default = `node`; point it away from a production node to spare it)
    pub blocks_node: String,
    pub every: Duration,
    /// judge the window from the sample `window` steps back to now
    pub window: usize,
    pub tolerance: f64,
    pub min_blocks: u64,
    /// stop after this many judgements (0 = run forever)
    pub rounds: usize,
}

/// The live loop: sample every `every`, judge the sliding window, hand each judgement to `emit`.
pub fn run(cfg: &RefereeConfig, mut emit: impl FnMut(&Judgement)) -> Result<(), String> {
    let client = http();
    let mut ring: Vec<Sample> = vec![sample(&client, &cfg.node)?];
    let mut cache: HashMap<u64, (u128, u64)> = HashMap::new();
    let mut done = 0usize;
    loop {
        std::thread::sleep(cfg.every);
        let s = match sample(&client, &cfg.node) {
            Ok(s) => s,
            Err(e) => { eprintln!("referee: sample failed: {e}"); continue; }
        };
        ring.push(s);
        if ring.len() > cfg.window + 1 {
            ring.remove(0);
        }
        let a = ring[0];
        let (mut minted, mut transfers, mut missing) = (0u128, 0u64, Vec::new());
        for h in a.height + 1..=s.height {
            let v = match cache.get(&h) {
                Some(v) => Some(*v),
                None => block_minted(&client, &cfg.blocks_node, h).ok().map(|v| { cache.insert(h, v); v }),
            };
            match v {
                Some((m, t)) => { minted += m; transfers += t; }
                None => missing.push(h),
            }
        }
        cache.retain(|h, _| *h > a.height);
        emit(&judge(&cfg.node, &a, &s, minted, transfers, missing, cfg.tolerance, cfg.min_blocks));
        done += 1;
        if cfg.rounds > 0 && done >= cfg.rounds {
            return Ok(());
        }
    }
}

pub fn render_line(j: &Judgement) -> String {
    let mark = match j.verdict {
        Verdict::Agrees => "✅ agrees",
        Verdict::OverPays => "🔥 OVER-PAYS",
        Verdict::UnderPays => "⚠️  under-pays",
        Verdict::TooFew => "… too few blocks",
        Verdict::Unreadable => "❓ unreadable",
    };
    let grew: f64 = j.grew.parse::<f64>().unwrap_or(f64::NAN) / 1e24;
    format!("{} ({}..{}] {} blocks: supply {:+.6} QUG vs headers {:.6} QUG → ratio {:.4} · transfers {} — {}{}",
            j.node, j.from_height, j.to_height, j.blocks, grew, j.minted as f64 / 1e24, j.ratio, j.transfers, mark,
            if j.missing.is_empty() { String::new() } else { format!(" (missing {:?})", &j.missing[..j.missing.len().min(5)]) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(h: u64, supply: u128) -> Sample { Sample { at_ms: 0, height: h, supply, wallets: 1 } }

    #[test]
    fn exact_u128_from_raw_json_number_and_string() {
        let j = r#"{"data":{"header":{"total_coinbase_reward":440602133898843654193152,"x":1}}}"#;
        assert_eq!(raw_u128(j, "total_coinbase_reward"), Some(440_602_133_898_843_654_193_152));
        let j = r#"{"data":{"total_supply_base_units": "5551110446159512504059190238346"}}"#;
        assert_eq!(raw_u128(j, "total_supply_base_units"), Some(5_551_110_446_159_512_504_059_190_238_346));
        assert_eq!(raw_u128(j, "absent"), None);
    }

    #[test]
    fn the_two_measured_nodes_are_judged_right() {
        let q = 1_000_000_000_000_000_000_000_000u128; // 1 QUG
        // Epsilon 2026-10-30: grew 46.806904 = headers 46.806904 over 112 blocks
        let e = judge("eps", &s(100, 0), &s(212, 46_806_904 * q / 1_000_000), 46_806_904 * q / 1_000_000, 0, vec![], 0.05, 50);
        assert_eq!(e.verdict, Verdict::Agrees);
        // Stockholm after the restart: grew 90.806450 vs headers 45.700103 over 105 blocks
        let st = judge("sto", &s(100, 0), &s(205, 90_806_450 * q / 1_000_000), 45_700_103 * q / 1_000_000, 0, vec![], 0.05, 50);
        assert_eq!(st.verdict, Verdict::OverPays);
        assert!((st.ratio - 1.9870).abs() < 1e-3, "{}", st.ratio);
    }

    #[test]
    fn short_or_holey_windows_are_not_judged() {
        assert_eq!(judge("n", &s(1, 0), &s(4, 3), 3, 0, vec![], 0.05, 50).verdict, Verdict::TooFew);
        assert_eq!(judge("n", &s(1, 0), &s(90, 3), 3, 0, vec![42], 0.05, 50).verdict, Verdict::Unreadable);
        assert_eq!(judge("n", &s(1, 10), &s(90, 5), 100, 0, vec![], 0.05, 50).verdict, Verdict::UnderPays);
    }
}
