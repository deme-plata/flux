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
//!
//! POOLS (`--pools`, 2026-11-02). Quillon's DEX is not on chain: a swap or a liquidity add/remove
//! moves QUG between a wallet (`wallet_balance_*`, counted in the supply above) and a pool reserve
//! (NOT counted). So the wallet total alone is not conserved — Epsilon's 10-27/28 step read +4,012 QUG
//! over the headers and the next step −4,005, with 0 non-coinbase transactions, and nothing could
//! say whether that was a DEX round trip or money created. With `--pools` every sample also reads
//! `/api/v1/dex/pools` and sums the QUG side of every pool; a window whose wallets disagree but whose
//! wallets + pools agree is a `DexMove` (moved, not made). The pool reserves are served as f64 QUG,
//! so that sum is exact only to ~1e-11 QUG, and it is read a few ms after the supply — a swap in
//! between shows as one skewed window. QUG held anywhere else off the wallet table (bank, vaults,
//! credit locks) still reads as over/under-pay: named, not explained.

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
    /// QUG held in DEX pool reserves (base units), when read (`--pools`)
    pub pool_qug: Option<u128>,
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
    /// Wallets alone disagree, but wallets + DEX pool reserves agree: QUG moved between wallets
    /// and pools (swap / liquidity), none was created or destroyed.
    DexMove,
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
    /// Δ QUG in DEX pools (signed base units), when both samples read the pools
    pub pools_moved: Option<String>,
    /// (Δwallets + Δpools) / minted, when both samples read the pools
    pub total_ratio: Option<f64>,
    pub verdict: Verdict,
}

/// Pure judge over two samples and the headers' minted sum for (from, to].
pub fn judge(node: &str, a: &Sample, b: &Sample, minted: u128, transfers: u64, missing: Vec<u64>,
             tolerance: f64, min_blocks: u64) -> Judgement {
    let blocks = b.height.saturating_sub(a.height);
    let grew: i128 = b.supply as i128 - a.supply as i128;
    let ratio = if minted == 0 { f64::NAN } else { grew as f64 / minted as f64 };
    let pools: Option<i128> = match (a.pool_qug, b.pool_qug) {
        (Some(pa), Some(pb)) => Some(pb as i128 - pa as i128),
        _ => None,
    };
    let total_ratio = pools.map(|dp| if minted == 0 { f64::NAN } else { (grew + dp) as f64 / minted as f64 });
    let verdict = if !missing.is_empty() {
        Verdict::Unreadable
    } else if blocks < min_blocks || minted == 0 {
        Verdict::TooFew
    } else if (ratio - 1.0).abs() <= tolerance {
        Verdict::Agrees
    } else if total_ratio.map_or(false, |t| (t - 1.0).abs() <= tolerance) {
        Verdict::DexMove
    } else if total_ratio.unwrap_or(ratio) > 1.0 {
        // with pools read, judge the money that is really there (wallets + pools)
        Verdict::OverPays
    } else {
        Verdict::UnderPays
    };
    Judgement { node: node.to_string(), from_height: a.height, to_height: b.height, blocks, minted,
                grew: grew.to_string(), ratio, transfers, missing,
                pools_moved: pools.map(|p| p.to_string()), total_ratio, verdict }
}

/// QUG side of every pool in a `/api/v1/dex/pools` reply, summed, in base units (1 QUG = 1e24).
/// Reserves are served as JSON numbers (f64 QUG) or strings; a pool without QUG adds nothing.
pub fn qug_in_pools(json: &str) -> Result<u128, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("pools: {e}"))?;
    let list = v.get("data").unwrap_or(&v);
    let list = list.get("pools").unwrap_or(list);
    let pools = list.as_array().ok_or("pools: no list")?;
    let is_qug = |t: &serde_json::Value| matches!(t.as_str().map(|s| s.to_ascii_uppercase()).as_deref(),
                                                  Some("QUG") | Some("NATIVE-QUG"));
    let num = |r: &serde_json::Value| r.as_f64().or_else(|| r.as_str().and_then(|s| s.parse().ok()));
    let mut sum = 0u128;
    for p in pools {
        let side = if is_qug(&p["token0"]) { &p["reserve0"] } else if is_qug(&p["token1"]) { &p["reserve1"] } else { continue };
        let q = num(side).ok_or("pools: unreadable reserve")?;
        if q.is_sign_negative() || !q.is_finite() {
            return Err(format!("pools: impossible reserve {q}"));
        }
        sum += (q * 1e24) as u128;
    }
    Ok(sum)
}

/// One reading of `<node>/api/v1/dex/pools` → QUG in pools.
pub fn sample_pools(client: &reqwest::blocking::Client, node: &str) -> Result<u128, String> {
    let url = format!("{}/api/v1/dex/pools", node.trim_end_matches('/'));
    let body = client.get(&url).send().and_then(|r| r.text()).map_err(|e| format!("{url}: {e}"))?;
    qug_in_pools(&body)
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
        pool_qug: None,
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
    /// also read `/api/v1/dex/pools` so a wallet↔pool move is told apart from created money
    pub pools: bool,
}

/// One full reading: supply (+ pools when asked). A failed pool read leaves `pool_qug` None, and a
/// window with a None end is judged on wallets alone — never on a guessed pool figure.
fn read(client: &reqwest::blocking::Client, cfg: &RefereeConfig) -> Result<Sample, String> {
    let mut s = sample(client, &cfg.node)?;
    if cfg.pools {
        match sample_pools(client, &cfg.node) {
            Ok(q) => s.pool_qug = Some(q),
            Err(e) => eprintln!("referee: pool read failed: {e}"),
        }
    }
    Ok(s)
}

/// The live loop: sample every `every`, judge the sliding window, hand each judgement to `emit`.
pub fn run(cfg: &RefereeConfig, mut emit: impl FnMut(&Judgement)) -> Result<(), String> {
    let client = http();
    let mut ring: Vec<Sample> = vec![read(&client, cfg)?];
    let mut cache: HashMap<u64, (u128, u64)> = HashMap::new();
    let mut done = 0usize;
    loop {
        std::thread::sleep(cfg.every);
        let s = match read(&client, cfg) {
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
        Verdict::DexMove => "🔁 DEX move (wallets↔pools, nothing created)",
        Verdict::TooFew => "… too few blocks",
        Verdict::Unreadable => "❓ unreadable",
    };
    let grew: f64 = j.grew.parse::<f64>().unwrap_or(f64::NAN) / 1e24;
    let pools = match (&j.pools_moved, j.total_ratio) {
        (Some(p), Some(t)) => format!(" · pools {:+.6} QUG → with pools {:.4}", p.parse::<f64>().unwrap_or(f64::NAN) / 1e24, t),
        _ => String::new(),
    };
    format!("{} ({}..{}] {} blocks: supply {:+.6} QUG vs headers {:.6} QUG → ratio {:.4}{} · transfers {} — {}{}",
            j.node, j.from_height, j.to_height, j.blocks, grew, j.minted as f64 / 1e24, j.ratio, pools, j.transfers, mark,
            if j.missing.is_empty() { String::new() } else { format!(" (missing {:?})", &j.missing[..j.missing.len().min(5)]) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(h: u64, supply: u128) -> Sample { Sample { at_ms: 0, height: h, supply, wallets: 1, pool_qug: None } }
    fn sp(h: u64, supply: u128, pools: u128) -> Sample { Sample { pool_qug: Some(pools), ..s(h, supply) } }
    const MICRO: u128 = 1_000_000_000_000_000_000; // 1e-6 QUG in base units

    #[test]
    fn epsilon_10_27_step_is_a_dex_move_when_pools_paid_it() {
        // the measured step 24,320,000→24,330,000: wallets +11,541.966155, headers 7,529.720159.
        // If 4,012.245996 QUG left the pools in the same window, nothing was created.
        let (grew, minted, out) = (11_541_966_155 * MICRO, 7_529_720_159 * MICRO, 4_012_245_996 * MICRO);
        let pools0 = 40_000_000_000 * MICRO;
        let j = judge("eps", &sp(24_320_000, 0, pools0), &sp(24_330_000, grew, pools0 - out), minted, 0, vec![], 0.05, 50);
        assert_eq!(j.verdict, Verdict::DexMove);
        assert!((j.total_ratio.unwrap() - 1.0).abs() < 1e-9, "{:?}", j.total_ratio);
        // the same wallet numbers with the pools untouched: money was made
        let k = judge("eps", &sp(24_320_000, 0, pools0), &sp(24_330_000, grew, pools0), minted, 0, vec![], 0.05, 50);
        assert_eq!(k.verdict, Verdict::OverPays);
        // and without a pool reading the old verdict stands
        assert_eq!(judge("eps", &s(24_320_000, 0), &s(24_330_000, grew), minted, 0, vec![], 0.05, 50).verdict, Verdict::OverPays);
    }

    #[test]
    fn qug_side_of_every_pool_is_summed_either_way_round() {
        let j = r#"{"success":true,"data":[
            {"token0":"QUG","token1":"qnkabc","reserve0":408.5,"reserve1":735892.5},
            {"token0":"qnkdef","token1":"QUG","reserve0":1312.8,"reserve1":"13446.25"},
            {"token0":"QUGUSD","token1":"wZEC","reserve0":0,"reserve1":0},
            {"token0":"native-qug","token1":"QUGUSD","reserve0":1,"reserve1":2}]}"#;
        let q = qug_in_pools(j).unwrap();
        let want = 408.5 + 13446.25 + 1.0;
        assert!(((q as f64) / 1e24 - want).abs() < 1e-9, "{}", q as f64 / 1e24);
        assert!(qug_in_pools(r#"{"data":[{"token0":"QUG","token1":"x","reserve0":-1,"reserve1":1}]}"#).is_err());
    }

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
