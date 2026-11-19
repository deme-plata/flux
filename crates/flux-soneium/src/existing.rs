//! Working with a Uniswap V2 pair that ALREADY exists — written for Polygon's wSIGIL3/USDC
//! pair `0x7e9C5d73…` (2026-11-19, Viktor: "make wsigil usdc pair on uniswap polygon pool").
//!
//! [`crate::pool::seed_pool`] assumes it creates the pair, so it can trust its own opening
//! ratio. An existing pair has a price someone else set (here $9.57/SIGIL after a sniper bot
//! bought 0.8991 of the 1.0 seed), and `addLiquidity` always deposits at the pair's CURRENT
//! ratio. So putting liquidity in at a chosen price takes three explicit steps:
//!
//! 1. [`mint_seed`] — operator-only `mint()`, with a lock id that is readable text
//!    ([`seed_lock_id`]) so nobody can mistake the supply for a bridge-backed lock.
//! 2. [`execute_reprice`] — ONE exact swap, sized by [`reprice_plan`] with Uniswap's own integer
//!    formula ([`amount_out`]), that moves the pair to the target price.
//! 3. [`add_existing`] — `addLiquidity` after checking the pair now sits within tolerance of the
//!    target, so the deposit lands at the price the operator chose.
//!
//! Everything that decides an amount is a pure function, tested against swaps that really
//! happened on that pair. Nothing here writes the crate's shared `deployment.json`.

use crate::dex::{IERC20, IUniswapV2Pair, IUniswapV2Router02};
use crate::wsigil::ISigilBridgeWrapped;
use alloy::primitives::{Address, B256, U256};
use alloy::providers::{DynProvider, Provider};
use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long to wait for one receipt before giving up (the tx may still land; the caller is
/// told the hash either way).
pub const RECEIPT_TIMEOUT: Duration = Duration::from_secs(300);
/// Polygon PoS refuses tips below its floor (`eth_maxPriorityFeePerGas` answered 30 gwei on
/// 2026-11-19; the 20th-percentile tip was 78). Never send below this on chain 137.
pub const POLYGON_MIN_TIP_WEI: u128 = 30_000_000_000;

/// `UniswapV2Library.getAmountOut`, exact integer form (0.3 % fee). Reproduces the two real
/// swaps on the Polygon wSIGIL3 pair to the wei (see tests).
pub fn amount_out(amount_in: U256, reserve_in: U256, reserve_out: U256) -> U256 {
    if amount_in.is_zero() || reserve_in.is_zero() || reserve_out.is_zero() { return U256::ZERO; }
    let a = amount_in * U256::from(997u64);
    a * reserve_out / (reserve_in * U256::from(1000u64) + a)
}

/// A price as an exact integer: quote BASE units per one whole token, × 10^12. For USDC
/// (6 dp) at $0.001 that is 1,000 micro-USDC × 10^12 = 10^15. The × 10^12 keeps sub-unit
/// prices representable without floats in the comparisons.
pub fn price_target(price: f64, quote_decimals: u8) -> Result<U256> {
    ensure!(price.is_finite() && price > 0.0, "price must be a finite number > 0");
    let scaled = price * 10f64.powi(quote_decimals as i32 + 12);
    ensure!(scaled >= 1.0 && scaled < 1e30, "price {price} is outside the representable range");
    Ok(U256::from(scaled.round() as u128))
}

/// Human price (quote per whole token) of a reserve pair. Display only — decisions use
/// [`cmp_price`].
pub fn price_f64(reserve_token: U256, reserve_quote: U256, token_decimals: u8, quote_decimals: u8) -> f64 {
    if reserve_token.is_zero() { return f64::INFINITY; }
    let t = reserve_token.to_string().parse::<f64>().unwrap_or(0.0) / 10f64.powi(token_decimals as i32);
    let q = reserve_quote.to_string().parse::<f64>().unwrap_or(0.0) / 10f64.powi(quote_decimals as i32);
    q / t
}

/// Compare the pair's price with a [`price_target`], exactly.
pub fn cmp_price(reserve_token: U256, reserve_quote: U256, token_decimals: u8, target_e12: U256) -> std::cmp::Ordering {
    let scale = U256::from(10u64).pow(U256::from(token_decimals as u64 + 12));
    (reserve_quote * scale).cmp(&(target_e12 * reserve_token))
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Price is above target: sell token into the pair, quote comes out.
    SellToken,
    /// Price is below target: buy token with quote.
    BuyToken,
    /// Already within tolerance — no swap.
    None,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepricePlan {
    pub direction: Direction,
    pub amount_in: U256,
    pub expected_out: U256,
    pub price_before: f64,
    pub price_after: f64,
    pub target_price: f64,
    pub reserve_token_after: U256,
    pub reserve_quote_after: U256,
}

fn reserves_after(dir: Direction, rt: U256, rq: U256, d: U256) -> (U256, U256, U256) {
    match dir {
        Direction::SellToken => { let o = amount_out(d, rt, rq); (rt + d, rq - o, o) }
        Direction::BuyToken => { let o = amount_out(d, rq, rt); (rt - o, rq + d, o) }
        Direction::None => (rt, rq, U256::ZERO),
    }
}

/// The SMALLEST single swap that brings the pair to (or just past) `target_e12`. Exact: binary
/// search over integer input amounts using [`amount_out`], so the plan is what the router will
/// compute. `tolerance_bps`: a pair already this close needs no swap.
pub fn reprice_plan(rt: U256, rq: U256, td: u8, qd: u8, target_e12: U256, tolerance_bps: u32) -> Result<RepricePlan> {
    ensure!(!rt.is_zero() && !rq.is_zero(), "pair has an empty reserve");
    let target_price = target_e12.to_string().parse::<f64>().unwrap_or(0.0) / 10f64.powi(qd as i32 + 12);
    let before = price_f64(rt, rq, td, qd);
    let dir = if (before / target_price - 1.0).abs() * 10_000.0 <= tolerance_bps as f64 { Direction::None }
        else if cmp_price(rt, rq, td, target_e12).is_gt() { Direction::SellToken } else { Direction::BuyToken };
    if dir == Direction::None {
        return Ok(RepricePlan { direction: dir, amount_in: U256::ZERO, expected_out: U256::ZERO, price_before: before, price_after: before, target_price, reserve_token_after: rt, reserve_quote_after: rq });
    }
    // done(d): the swap of size d reaches the target from the starting side.
    let done = |d: U256| { let (t, q, _) = reserves_after(dir, rt, rq, d); match dir {
        Direction::SellToken => !cmp_price(t, q, td, target_e12).is_gt(),
        _ => !cmp_price(t, q, td, target_e12).is_lt(),
    } };
    let (mut lo, mut hi) = (U256::ZERO, U256::from(1u64));
    let cap = U256::from(1u64) << 200;
    while !done(hi) { lo = hi; hi <<= 1; ensure!(hi < cap, "no swap reaches the target — reserves too skewed"); }
    while hi - lo > U256::from(1u64) { let mid = (lo + hi) >> 1; if done(mid) { hi = mid } else { lo = mid } }
    let (t, q, out) = reserves_after(dir, rt, rq, hi);
    ensure!(!out.is_zero(), "the reprice swap would receive nothing");
    Ok(RepricePlan { direction: dir, amount_in: hi, expected_out: out, price_before: before, price_after: price_f64(t, q, td, qd), target_price, reserve_token_after: t, reserve_quote_after: q })
}

/// Lock id for a pool-seed mint that can never be mistaken for a SIGIL lock: the memo's ASCII
/// bytes, left-aligned in 32 bytes (a real lock id is a BLAKE3 tx hash, i.e. random bytes).
/// Polygonscan shows the id in hex; decoding it reads the memo.
pub fn seed_lock_id(memo: &str) -> Result<U256> {
    ensure!(!memo.is_empty() && memo.len() <= 32, "memo must be 1..=32 bytes (it IS the 32-byte lock id)");
    ensure!(memo.bytes().all(|b| b.is_ascii_graphic() || b == b' '), "memo must be printable ASCII");
    let mut b = [0u8; 32];
    b[..memo.len()].copy_from_slice(memo.as_bytes());
    Ok(U256::from_be_bytes(b))
}

/// Inverse of [`seed_lock_id`]: `Some(text)` when a lock id is a printable memo.
pub fn lock_id_text(id: U256) -> Option<String> {
    let b: [u8; 32] = id.to_be_bytes();
    let end = b.iter().position(|&x| x == 0).unwrap_or(32);
    if end == 0 || b[end..].iter().any(|&x| x != 0) { return None; }
    let s = &b[..end];
    s.iter().all(|&x| x.is_ascii_graphic() || x == b' ').then(|| String::from_utf8_lossy(s).into_owned())
}

/// What one read of the pair says.
#[derive(Debug, Clone, Serialize)]
pub struct PairView {
    pub pair: Address,
    pub token: Address,
    pub quote: Address,
    pub token_symbol: String,
    pub quote_symbol: String,
    pub token_decimals: u8,
    pub quote_decimals: u8,
    pub reserve_token: U256,
    pub reserve_quote: U256,
    pub lp_supply: U256,
    pub price: f64,
}

/// Read a pair and refuse unless it is exactly {token, quote}.
pub async fn read_pair(p: &DynProvider, pair: Address, token: Address, quote: Address) -> Result<PairView> {
    let pc = IUniswapV2Pair::new(pair, p.clone());
    let (t0, t1) = (pc.token0().call().await.context("pair.token0()")?, pc.token1().call().await.context("pair.token1()")?);
    let token_is0 = match (t0 == token && t1 == quote, t0 == quote && t1 == token) {
        (true, _) => true, (_, true) => false,
        _ => bail!("pair {pair} holds {t0}/{t1}, not {token}/{quote}"),
    };
    let r = pc.getReserves().call().await.context("pair.getReserves()")?;
    let (r0, r1) = (U256::from(r.reserve0), U256::from(r.reserve1));
    let (reserve_token, reserve_quote) = if token_is0 { (r0, r1) } else { (r1, r0) };
    let (ti, qi) = (IERC20::new(token, p.clone()), IERC20::new(quote, p.clone()));
    let (token_decimals, quote_decimals) = (ti.decimals().call().await?, qi.decimals().call().await?);
    let (token_symbol, quote_symbol) = (ti.symbol().call().await.unwrap_or_default(), qi.symbol().call().await.unwrap_or_default());
    let lp_supply = pc.totalSupply().call().await?;
    let price = price_f64(reserve_token, reserve_quote, token_decimals, quote_decimals);
    Ok(PairView { pair, token, quote, token_symbol, quote_symbol, token_decimals, quote_decimals, reserve_token, reserve_quote, lp_supply, price })
}

/// EIP-1559 fees with the Polygon tip floor applied. (max_fee, tip) in wei.
pub async fn fees(p: &DynProvider, chain_id: u64) -> Result<(u128, u128)> {
    let est = p.estimate_eip1559_fees().await.context("eth_feeHistory")?;
    let floor = if chain_id == 137 { POLYGON_MIN_TIP_WEI } else { 0 };
    let tip = est.max_priority_fee_per_gas.max(floor);
    Ok((est.max_fee_per_gas + (tip - est.max_priority_fee_per_gas), tip))
}

fn deadline(secs: u64) -> U256 {
    U256::from(SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) + secs)
}

/// Send a prepared call: simulate, estimate × 1.12, send with explicit fees, wait. One macro
/// because alloy's CallBuilder types differ per function.
macro_rules! send_call {
    ($call:expr, $label:expr, $fees:expr) => {{
        let call = $call;
        call.call().await.with_context(|| format!("simulate {}", $label))?;
        let est = call.estimate_gas().await.with_context(|| format!("estimate {}", $label))?;
        let pending = call.gas(est * 112 / 100).max_fee_per_gas($fees.0).max_priority_fee_per_gas($fees.1)
            .send().await.with_context(|| format!("send {}", $label))?;
        let hash = *pending.tx_hash();
        eprintln!("  sent {} {hash} — waiting…", $label);
        let r = pending.with_timeout(Some(RECEIPT_TIMEOUT)).get_receipt().await
            .with_context(|| format!("{} {hash}: no receipt within {}s (it may still land — check the explorer)", $label, RECEIPT_TIMEOUT.as_secs()))?;
        if !r.status() { bail!("{} {hash} REVERTED", $label); }
        eprintln!("  ✓ {} in block {:?}, gas {}", $label, r.block_number, r.gas_used);
        TxDone { label: $label.to_string(), hash, block: r.block_number, gas_used: r.gas_used }
    }};
}

#[derive(Debug, Clone, Serialize)]
pub struct TxDone { pub label: String, pub hash: B256, pub block: Option<u64>, pub gas_used: u64 }

/// Approve EXACTLY `amount` (never unlimited) unless the allowance already covers it.
pub async fn approve_exact(p: &DynProvider, chain_id: u64, token: Address, owner: Address, spender: Address, amount: U256, label: &str) -> Result<Option<TxDone>> {
    let t = IERC20::new(token, p.clone());
    if t.allowance(owner, spender).call().await? >= amount { return Ok(None); }
    let f = fees(p, chain_id).await?;
    Ok(Some(send_call!(t.approve(spender, amount), label, f)))
}

/// Operator mint of pool-seed supply. Refuses unless `from` IS the operator and the lock id is
/// unused (the contract would revert anyway; this says why first).
pub async fn mint_seed(p: &DynProvider, chain_id: u64, from: Address, token: Address, to: Address, amount: U256, lock_id: U256) -> Result<TxDone> {
    let c = ISigilBridgeWrapped::new(token, p.clone());
    let op = c.operator().call().await.context("operator()")?;
    ensure!(op == from, "signing wallet {from} is not the token's operator ({op})");
    ensure!(!c.usedLock(lock_id).call().await?, "lock id already used — pick a new memo");
    let f = fees(p, chain_id).await?;
    Ok(send_call!(c.mint(to, amount, lock_id), "mint (pool seed)", f))
}

#[derive(Debug, Clone, Serialize)]
pub struct RepriceDone { pub approve: Option<TxDone>, pub swap: TxDone, pub plan: RepricePlan, pub price_now: f64 }

/// Run a [`RepricePlan`]: approve the exact input, re-check the router's own quote equals the
/// plan (reserves unchanged), then swap with `slippage_bps` of protection. Output → `from`.
pub async fn execute_reprice(p: &DynProvider, chain_id: u64, from: Address, router: Address, view: &PairView, plan: &RepricePlan, slippage_bps: u32) -> Result<RepriceDone> {
    let path = match plan.direction {
        Direction::SellToken => vec![view.token, view.quote],
        Direction::BuyToken => vec![view.quote, view.token],
        Direction::None => bail!("no reprice needed"),
    };
    let bal = IERC20::new(path[0], p.clone()).balanceOf(from).call().await?;
    ensure!(bal >= plan.amount_in, "wallet holds {bal} of the input token, the swap needs {}", plan.amount_in);
    let r = IUniswapV2Router02::new(router, p.clone());
    let q = r.getAmountsOut(plan.amount_in, path.clone()).call().await.context("router.getAmountsOut")?;
    ensure!(q.last() == Some(&plan.expected_out), "pair moved since the plan: router quotes {:?}, plan expected {} — re-run", q.last(), plan.expected_out);
    let approve = approve_exact(p, chain_id, path[0], from, router, plan.amount_in, "approve (reprice input)").await?;
    let min_out = plan.expected_out * U256::from(10_000 - slippage_bps.min(10_000)) / U256::from(10_000u64);
    let f = fees(p, chain_id).await?;
    let swap = send_call!(r.swapExactTokensForTokens(plan.amount_in, min_out, path, from, deadline(20 * 60)), "swap (reprice)", f);
    let now = read_pair(p, view.pair, view.token, view.quote).await?;
    Ok(RepriceDone { approve, swap, plan: plan.clone(), price_now: now.price })
}

#[derive(Debug, Clone, Serialize)]
pub struct AddDone { pub approvals: Vec<TxDone>, pub add: TxDone, pub amount_token: U256, pub amount_quote: U256, pub liquidity: U256, pub price_after: f64, pub lp_supply_after: U256 }

/// `addLiquidity` into an existing pair, only if its price is within `tolerance_bps` of the
/// target — otherwise the deposit would land at someone else's price. LP → `recipient`.
pub async fn add_existing(p: &DynProvider, chain_id: u64, from: Address, router: Address, view: &PairView, amount_token: U256, amount_quote: U256, target_e12: U256, tolerance_bps: u32, recipient: Address) -> Result<AddDone> {
    let target = target_e12.to_string().parse::<f64>().unwrap_or(0.0) / 10f64.powi(view.quote_decimals as i32 + 12);
    let off = (view.price / target - 1.0).abs() * 10_000.0;
    ensure!(off <= tolerance_bps as f64, "pair price {} is {off:.0} bps from the target {target} — run `reprice` first", view.price);
    for (t, a, n) in [(view.token, amount_token, &view.token_symbol), (view.quote, amount_quote, &view.quote_symbol)] {
        let b = IERC20::new(t, p.clone()).balanceOf(from).call().await?;
        ensure!(b >= a, "wallet holds {b} {n} base units, the deposit needs {a}");
    }
    let mut approvals = Vec::new();
    approvals.extend(approve_exact(p, chain_id, view.token, from, router, amount_token, "approve (token → router)").await?);
    approvals.extend(approve_exact(p, chain_id, view.quote, from, router, amount_quote, "approve (quote → router)").await?);
    let r = IUniswapV2Router02::new(router, p.clone());
    // The router deposits at the pair's ratio; mins = (1 − tolerance) of what we offer.
    let keep = U256::from(10_000 - tolerance_bps.min(10_000));
    let (min_t, min_q) = (amount_token * keep / U256::from(10_000u64), amount_quote * keep / U256::from(10_000u64));
    let call = r.addLiquidity(view.token, view.quote, amount_token, amount_quote, min_t, min_q, recipient, deadline(20 * 60));
    let sim = call.call().await.context("simulate addLiquidity")?;
    let f = fees(p, chain_id).await?;
    let add = send_call!(call, "addLiquidity", f);
    let now = read_pair(p, view.pair, view.token, view.quote).await?;
    Ok(AddDone { approvals, add, amount_token: sim.amountA, amount_quote: sim.amountB, liquidity: sim.liquidity, price_after: now.price, lp_supply_after: now.lp_supply })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(x: u128) -> U256 { U256::from(x) }

    #[test]
    fn amount_out_reproduces_the_real_polygon_swaps_to_the_wei() {
        // 2026-09-06 14:49, tx 4ac3b986…: 100 micro-USDC in against 1000 micro-USDC / 1.0 wSIGIL3.
        assert_eq!(amount_out(u(100), u(1000), u(10u128.pow(18))), u(90_661_089_388_014_913));
        // 2026-09-06 16:18, tx f0bf1bca… (the sniper bot): 96,883 micro-USDC in against
        // 1,100 / 0.909338910611985087 wSIGIL3 → the 0.899099902220497163 it holds today.
        assert_eq!(amount_out(u(96_883), u(1_100), u(909_338_910_611_985_087)), u(899_099_902_220_497_163));
        assert_eq!(amount_out(U256::ZERO, u(1), u(1)), U256::ZERO);
    }

    #[test]
    fn price_target_and_exact_compare() {
        let t = price_target(0.001, 6).unwrap();
        assert_eq!(t, u(10u128.pow(15)));
        // 1 USDC vs 1,000 wSIGIL3 is exactly $0.001.
        assert!(cmp_price(u(1000 * 10u128.pow(18)), u(1_000_000), 18, t).is_eq());
        // Today's pair (2026-11-19): 97,983 micro-USDC vs 0.010239008391487924 wSIGIL3 → $9.57, above.
        assert!(cmp_price(u(10_239_008_391_487_924), u(97_983), 18, t).is_gt());
        assert!((price_f64(u(10_239_008_391_487_924), u(97_983), 18, 6) - 9.5696).abs() < 1e-3);
        assert!(price_target(0.0, 6).is_err() && price_target(f64::NAN, 6).is_err() && price_target(-1.0, 6).is_err());
    }

    #[test]
    fn reprice_from_todays_pair_to_a_tenth_of_a_cent() {
        let (rt, rq) = (u(10_239_008_391_487_924), u(97_983));
        let t = price_target(0.001, 6).unwrap();
        let p = reprice_plan(rt, rq, 18, 6, t, 20).unwrap();
        assert_eq!(p.direction, Direction::SellToken);
        // Reaches the target: at or below it after the swap…
        assert!(!cmp_price(p.reserve_token_after, p.reserve_quote_after, 18, t).is_gt());
        // …and one wei less does not (smallest swap).
        let o = amount_out(p.amount_in - u(1), rt, rq);
        assert!(cmp_price(rt + p.amount_in - u(1), rq - o, 18, t).is_gt());
        // The plan's numbers are the router's numbers.
        assert_eq!(p.expected_out, amount_out(p.amount_in, rt, rq));
        assert_eq!(p.reserve_quote_after, rq - p.expected_out);
        // Lands within 0.2 % (USDC has 6 dp; ~1,000 micro-USDC remain, so 1 unit ≈ 0.1 %).
        assert!((p.price_after / 0.001 - 1.0).abs() < 0.002, "{}", p.price_after);
        // ~0.99 wSIGIL3 in, ~0.097 USDC out.
        let sold = p.amount_in.to::<u128>() as f64 / 1e18;
        assert!(sold > 0.9 && sold < 1.1, "{sold}");
    }

    #[test]
    fn reprice_upwards_buys_the_token() {
        let (rt, rq) = (u(1000 * 10u128.pow(18)), u(1_000_000)); // $0.001
        let t = price_target(0.002, 6).unwrap();
        let p = reprice_plan(rt, rq, 18, 6, t, 20).unwrap();
        assert_eq!(p.direction, Direction::BuyToken);
        assert!(!cmp_price(p.reserve_token_after, p.reserve_quote_after, 18, t).is_lt());
        assert!((p.price_after / 0.002 - 1.0).abs() < 0.001);
    }

    #[test]
    fn reprice_within_tolerance_is_a_no_op() {
        let p = reprice_plan(u(1000 * 10u128.pow(18)), u(1_001_000), 18, 6, price_target(0.001, 6).unwrap(), 20).unwrap();
        assert_eq!(p.direction, Direction::None);
        assert!(p.amount_in.is_zero());
    }

    #[test]
    fn seed_lock_ids_read_as_text_and_never_as_a_hash() {
        let id = seed_lock_id("pool-seed 2026-11-19 not-bridged").unwrap();
        assert_eq!(lock_id_text(id).as_deref(), Some("pool-seed 2026-11-19 not-bridged"));
        // A real lock id (the 09-06 deploy tx hash as a number) is not text.
        let h: B256 = "0x53473362f7f2117cbdd153a06843f69953107821e84b3e5bbf5fc06e997a327a".parse().unwrap();
        assert_eq!(lock_id_text(U256::from_be_bytes(h.0)), None);
        assert!(seed_lock_id("").is_err());
        assert!(seed_lock_id(&"x".repeat(33)).is_err());
        assert!(seed_lock_id("tab\there").is_err());
        assert_eq!(lock_id_text(U256::ZERO), None);
    }
}
