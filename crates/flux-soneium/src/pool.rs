//! Seeding the wSIGIL pool on a verified V2 router.

use crate::dex::{IERC20, IUniswapV2Factory, IUniswapV2Router02};
use alloy::primitives::{Address, B256, U256};
use alloy::providers::DynProvider;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Uniswap V2 burns this many LP units to `address(0)` on first mint.
pub const MINIMUM_LIQUIDITY: u128 = 1000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Quote {
    /// Pair against WETH via `addLiquidityETH` — funding needs ETH only (one asset to bridge).
    Weth,
    /// Pair against bridged USDC.e (6 dp) — matches Polygon's quote asset exactly.
    UsdcE,
}

impl Quote {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() { "weth" | "eth" => Some(Quote::Weth), "usdc" | "usdc.e" | "usdce" => Some(Quote::UsdcE), _ => None }
    }
    pub fn label(&self) -> &'static str { match self { Quote::Weth => "weth", Quote::UsdcE => "usdc.e" } }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedPlan {
    pub quote: Quote,
    pub seed_wsigil_wei: U256,
    /// Wei when `quote == Weth`, micro-USDC when `quote == UsdcE`.
    pub seed_quote_units: U256,
    pub target_usd_per_sigil: f64,
    pub eth_usd: Option<f64>,
    /// `sqrt(a·b)` — must exceed [`MINIMUM_LIQUIDITY`] or `mint()` reverts.
    pub lp_estimate: U256,
}

/// Opening ratio for `seed_wsigil` at `target_usd_per_sigil`. The ratio IS the price.
pub fn seed_plan(quote: Quote, seed_wsigil_wei: U256, target_usd_per_sigil: f64, eth_usd: Option<f64>) -> Result<SeedPlan> {
    if seed_wsigil_wei.is_zero() { bail!("seed wSIGIL must be > 0"); }
    if !(target_usd_per_sigil.is_finite() && target_usd_per_sigil > 0.0) { bail!("price must be > 0"); }
    let sigil = seed_wsigil_wei.to::<u128>() as f64 / 1e18;
    let usd = sigil * target_usd_per_sigil;
    let units = match quote {
        Quote::Weth => {
            let px = eth_usd.filter(|p| *p > 0.0).context("ETH/USD unknown — cannot size a WETH seed")?;
            (usd / px * 1e18).round() as u128
        }
        Quote::UsdcE => (usd * 1e6).round() as u128,
    };
    if units == 0 { bail!("quote side rounds to zero units — raise the seed"); }
    let q = U256::from(units);
    let lp = (seed_wsigil_wei * q).root(2);
    if lp <= U256::from(MINIMUM_LIQUIDITY) { bail!("sqrt(a·b) = {lp} ≤ MINIMUM_LIQUIDITY — pool mint would revert"); }
    Ok(SeedPlan { quote, seed_wsigil_wei, seed_quote_units: q, target_usd_per_sigil, eth_usd, lp_estimate: lp })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolReceipt {
    pub pair: Address,
    pub approve_txs: Vec<B256>,
    pub add_tx: B256,
    pub gas_used: u64,
    pub gas_limit: u64,
    pub amount_token: U256,
    pub amount_quote: U256,
    pub liquidity: U256,
}

fn deadline(secs: u64) -> U256 {
    U256::from(SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) + secs)
}

async fn ensure_allowance(p: &DynProvider, token: Address, owner: Address, spender: Address, amount: U256, txs: &mut Vec<B256>) -> Result<()> {
    let t = IERC20::new(token, p.clone());
    if t.allowance(owner, spender).call().await? >= amount { return Ok(()); }
    let call = t.approve(spender, amount);
    let est = call.estimate_gas().await.context("estimate approve")?;
    // Wait for it to MINE before addLiquidity, or the router's transferFrom reverts.
    let r = call.gas(est * 112 / 100).send().await.context("send approve")?.get_receipt().await.context("approve receipt")?;
    if !r.status() { bail!("approve {} reverted", r.transaction_hash); }
    txs.push(r.transaction_hash);
    Ok(())
}

/// Approve → `addLiquidity{ETH}` → read the pair back from the factory. `recipient` gets
/// the LP tokens (Viktor's address on Polygon; keep it the same here unless told otherwise).
pub async fn seed_pool(p: &DynProvider, from: Address, router: Address, factory: Address, weth: Address, usdc_e: Option<Address>, token: Address, plan: &SeedPlan, recipient: Address) -> Result<PoolReceipt> {
    let mut approve_txs = Vec::new();
    ensure_allowance(p, token, from, router, plan.seed_wsigil_wei, &mut approve_txs).await?;
    let r = IUniswapV2Router02::new(router, p.clone());
    let min = |x: U256| x * U256::from(995) / U256::from(1000);
    let dl = deadline(20 * 60);
    let (quote_token, rcpt, amount_token, amount_quote, liquidity, limit) = match plan.quote {
        Quote::Weth => {
            let call = r.addLiquidityETH(token, plan.seed_wsigil_wei, min(plan.seed_wsigil_wei), min(plan.seed_quote_units), recipient, dl).value(plan.seed_quote_units);
            let sim = call.call().await.context("simulate addLiquidityETH")?;
            let est = call.estimate_gas().await.context("estimate addLiquidityETH")?;
            let limit = est * 112 / 100;
            let rcpt = call.gas(limit).send().await.context("send addLiquidityETH")?.get_receipt().await.context("addLiquidityETH receipt")?;
            (weth, rcpt, sim.amountToken, sim.amountETH, sim.liquidity, limit)
        }
        Quote::UsdcE => {
            let usdc = usdc_e.context("this network has no USDC.e pinned")?;
            ensure_allowance(p, usdc, from, router, plan.seed_quote_units, &mut approve_txs).await?;
            let call = r.addLiquidity(token, usdc, plan.seed_wsigil_wei, plan.seed_quote_units, min(plan.seed_wsigil_wei), min(plan.seed_quote_units), recipient, dl);
            let sim = call.call().await.context("simulate addLiquidity")?;
            let est = call.estimate_gas().await.context("estimate addLiquidity")?;
            let limit = est * 112 / 100;
            let rcpt = call.gas(limit).send().await.context("send addLiquidity")?.get_receipt().await.context("addLiquidity receipt")?;
            (usdc, rcpt, sim.amountA, sim.amountB, sim.liquidity, limit)
        }
    };
    if !rcpt.status() { bail!("addLiquidity tx {} reverted", rcpt.transaction_hash); }
    let pair = IUniswapV2Factory::new(factory, p.clone()).getPair(token, quote_token).call().await.context("getPair after seed")?;
    if pair == Address::ZERO { bail!("factory reports no pair after a successful addLiquidity — wrong factory?"); }
    Ok(PoolReceipt { pair, approve_txs, add_tx: rcpt.transaction_hash, gas_used: rcpt.gas_used, gas_limit: limit, amount_token, amount_quote, liquidity })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wei(sigil: u128) -> U256 { U256::from(sigil * 10u128.pow(18)) }

    #[test]
    fn weth_seed_sizes_from_eth_usd() {
        // 100 wSIGIL at $0.001 = $0.10; at ETH $2,488.34 that is 4.0187e-5 ETH.
        let p = seed_plan(Quote::Weth, wei(100), 0.001, Some(2488.34)).unwrap();
        let eth = p.seed_quote_units.to::<u128>() as f64 / 1e18;
        assert!((eth - 0.10 / 2488.34).abs() < 1e-12, "{eth}");
        assert!(p.lp_estimate > U256::from(MINIMUM_LIQUIDITY));
    }

    #[test]
    fn usdc_seed_matches_polygon_anchor() {
        // Polygon seeded 1.0 wSIGIL3 against 0.001 USDC (1000 micro) → $0.001/SIGIL.
        let p = seed_plan(Quote::UsdcE, wei(1), 0.001, None).unwrap();
        assert_eq!(p.seed_quote_units, U256::from(1000));
        assert_eq!(p.lp_estimate, U256::from(31_622_776_601u128)); // sqrt(1e18 · 1e3), matches the LP Polygon minted (31,622,775,601 + 1000 burned)
    }

    #[test]
    fn weth_without_price_is_refused() {
        assert!(seed_plan(Quote::Weth, wei(1), 0.001, None).is_err());
        assert!(seed_plan(Quote::Weth, wei(1), 0.001, Some(0.0)).is_err());
    }

    #[test]
    fn dust_seeds_are_refused_before_they_revert_on_chain() {
        // 1 wei of wSIGIL against 1 micro-USDC: sqrt(1) = 1 ≤ 1000.
        assert!(seed_plan(Quote::UsdcE, U256::from(1), 0.001, None).is_err());
        assert!(seed_plan(Quote::UsdcE, U256::ZERO, 0.001, None).is_err());
        assert!(seed_plan(Quote::UsdcE, wei(1), 0.0, None).is_err());
    }

    #[test]
    fn quote_parsing() {
        assert_eq!(Quote::parse("ETH"), Some(Quote::Weth));
        assert_eq!(Quote::parse("usdc.e"), Some(Quote::UsdcE));
        assert_eq!(Quote::parse("usdt"), None);
    }
}
