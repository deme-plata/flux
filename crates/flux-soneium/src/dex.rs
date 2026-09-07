//! Uniswap-V2-compatible DEX access, with on-chain verification of the router.

use crate::chain::{parse, Network};
use alloy::primitives::{Address, U256};
use alloy::providers::DynProvider;
use alloy::sol;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

sol! {
    #[sol(rpc)]
    interface IUniswapV2Router02 {
        function factory() external view returns (address);
        function WETH() external view returns (address);
        function getAmountsOut(uint256 amountIn, address[] calldata path) external view returns (uint256[] memory amounts);
        function addLiquidity(address tokenA, address tokenB, uint256 amountADesired, uint256 amountBDesired, uint256 amountAMin, uint256 amountBMin, address to, uint256 deadline) external returns (uint256 amountA, uint256 amountB, uint256 liquidity);
        function addLiquidityETH(address token, uint256 amountTokenDesired, uint256 amountTokenMin, uint256 amountETHMin, address to, uint256 deadline) external payable returns (uint256 amountToken, uint256 amountETH, uint256 liquidity);
        function swapExactTokensForTokens(uint256 amountIn, uint256 amountOutMin, address[] calldata path, address to, uint256 deadline) external returns (uint256[] memory amounts);
        function swapExactETHForTokens(uint256 amountOutMin, address[] calldata path, address to, uint256 deadline) external payable returns (uint256[] memory amounts);
    }

    #[sol(rpc)]
    interface IUniswapV2Factory {
        function allPairsLength() external view returns (uint256);
        function getPair(address tokenA, address tokenB) external view returns (address pair);
        event PairCreated(address indexed token0, address indexed token1, address pair, uint256);
    }

    #[sol(rpc)]
    interface IUniswapV2Pair {
        function token0() external view returns (address);
        function token1() external view returns (address);
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
        function totalSupply() external view returns (uint256);
        function balanceOf(address owner) external view returns (uint256);
    }

    #[sol(rpc)]
    interface IERC20 {
        function approve(address spender, uint256 amount) external returns (bool);
        function allowance(address owner, address spender) external view returns (uint256);
        function balanceOf(address owner) external view returns (uint256);
        function decimals() external view returns (uint8);
        function symbol() external view returns (string);
        function totalSupply() external view returns (uint256);
    }
}

/// Everything we could learn about a router without trusting anyone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterVerdict {
    pub name: String,
    pub router: Address,
    pub factory: Address,
    pub weth: Address,
    /// `WETH()` equals the OP-Stack predeploy — a router pointing elsewhere is not on this chain's ETH.
    pub weth_ok: bool,
    pub pairs: u64,
    pub weth_usdc_pair: Option<Address>,
    /// Raw reserves of the WETH/USDC.e pair (wei, micro-USDC), already ordered WETH first.
    pub weth_reserve: U256,
    pub usdc_reserve: U256,
    /// ETH/USD implied by that pair. `None` when there is no such pair.
    pub eth_usd: Option<f64>,
    pub ok: bool,
    pub reason: String,
}

/// ETH/USD from a WETH(18dp)/USDC(6dp) reserve pair. Pure, so it is testable against the
/// exact bytes the chain returned.
pub fn eth_usd_from_reserves(weth_wei: U256, usdc_micro: U256) -> Option<f64> {
    if weth_wei.is_zero() || usdc_micro.is_zero() { return None; }
    let w = weth_wei.to::<u128>() as f64 / 1e18;
    let u = usdc_micro.to::<u128>() as f64 / 1e6;
    Some(u / w)
}

/// Order a pair's reserves as (weth, other) given `token0`.
pub fn order_reserves(token0: Address, weth: Address, r0: U256, r1: U256) -> (U256, U256) {
    if token0 == weth { (r0, r1) } else { (r1, r0) }
}

/// Verify one router candidate on chain.
pub async fn verify_router(p: &DynProvider, net: &Network, name: &str, router: Address) -> RouterVerdict {
    let weth = net.weth_addr();
    let mut v = RouterVerdict { name: name.into(), router, factory: Address::ZERO, weth: Address::ZERO, weth_ok: false, pairs: 0, weth_usdc_pair: None, weth_reserve: U256::ZERO, usdc_reserve: U256::ZERO, eth_usd: None, ok: false, reason: String::new() };
    let r = IUniswapV2Router02::new(router, p.clone());
    v.factory = match r.factory().call().await { Ok(a) => a, Err(e) => { v.reason = format!("factory() failed: {e}"); return v; } };
    v.weth = match r.WETH().call().await { Ok(a) => a, Err(e) => { v.reason = format!("WETH() failed: {e}"); return v; } };
    v.weth_ok = v.weth == weth;
    let f = IUniswapV2Factory::new(v.factory, p.clone());
    v.pairs = match f.allPairsLength().call().await { Ok(n) => n.to::<u64>(), Err(e) => { v.reason = format!("allPairsLength() failed: {e}"); return v; } };
    if let Some(usdc) = net.usdc_e_addr() {
        if let Ok(pair) = f.getPair(weth, usdc).call().await {
            if pair != Address::ZERO {
                v.weth_usdc_pair = Some(pair);
                let pc = IUniswapV2Pair::new(pair, p.clone());
                if let (Ok(t0), Ok(res)) = (pc.token0().call().await, pc.getReserves().call().await) {
                    let (w, u) = order_reserves(t0, weth, U256::from(res.reserve0), U256::from(res.reserve1));
                    v.weth_reserve = w; v.usdc_reserve = u; v.eth_usd = eth_usd_from_reserves(w, u);
                }
            }
        }
    }
    v.ok = v.weth_ok && v.pairs > 0;
    v.reason = if !v.weth_ok { format!("WETH() = {} ≠ predeploy {}", v.weth, weth) }
        else if v.pairs == 0 { "factory has no pairs".into() }
        else if v.eth_usd.is_none() { "no WETH/USDC.e pair (cannot derive ETH/USD from this DEX)".into() }
        else { "verified".into() };
    v
}

/// Verify all candidates (plus an optional operator override) and pick the deepest
/// WETH/USDC.e pool among the ones that pass. Depth, not name, decides.
pub async fn pick_router(p: &DynProvider, net: &Network, override_router: Option<Address>) -> Result<(RouterVerdict, Vec<RouterVerdict>)> {
    let mut all = Vec::new();
    if let Some(r) = override_router {
        all.push(verify_router(p, net, "override", r).await);
    } else {
        for c in &net.routers { all.push(verify_router(p, net, c.name, parse(c.router)).await); }
    }
    if all.is_empty() { bail!("no router candidates for {} — pass --router 0x…", net.name); }
    let best = pick_best(&all).context("no router passed verification")?;
    Ok((best, all))
}

/// Pure selection: passing candidates ranked by USDC.e depth of their WETH pool, then pair count.
pub fn pick_best(all: &[RouterVerdict]) -> Option<RouterVerdict> {
    all.iter().filter(|v| v.ok).max_by(|a, b| a.usdc_reserve.cmp(&b.usdc_reserve).then(a.pairs.cmp(&b.pairs))).cloned()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuoteOut {
    pub amount_in: U256,
    pub path: Vec<Address>,
    pub amounts: Vec<U256>,
    pub amount_out: U256,
}

/// `getAmountsOut` along `path`.
pub async fn quote(p: &DynProvider, router: Address, amount_in: U256, path: Vec<Address>) -> Result<QuoteOut> {
    let r = IUniswapV2Router02::new(router, p.clone());
    let amounts = r.getAmountsOut(amount_in, path.clone()).call().await.context("getAmountsOut")?;
    let amount_out = *amounts.last().context("empty amounts")?;
    Ok(QuoteOut { amount_in, path, amounts, amount_out })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairState {
    pub pair: Address,
    pub token0: Address,
    pub token1: Address,
    pub reserve0: U256,
    pub reserve1: U256,
    pub lp_total: U256,
}

pub async fn pair_state(p: &DynProvider, pair: Address) -> Result<PairState> {
    let pc = IUniswapV2Pair::new(pair, p.clone());
    let res = pc.getReserves().call().await.context("getReserves")?;
    Ok(PairState { pair, token0: pc.token0().call().await?, token1: pc.token1().call().await?, reserve0: U256::from(res.reserve0), reserve1: U256::from(res.reserve1), lp_total: pc.totalSupply().call().await? })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(name: &str, ok: bool, usdc: u128, pairs: u64) -> RouterVerdict {
        RouterVerdict { name: name.into(), router: Address::ZERO, factory: Address::ZERO, weth: Address::ZERO, weth_ok: ok, pairs, weth_usdc_pair: None, weth_reserve: U256::from(1), usdc_reserve: U256::from(usdc), eth_usd: None, ok, reason: String::new() }
    }

    #[test]
    fn eth_usd_from_the_live_sonus_reserves() {
        // Exact `getReserves()` bytes from pair 0x6de2b8f2… on 2026-09-08:
        // reserve0 (WETH) = 0x0ced27ffcd37849c, reserve1 (USDC.e) = 0x8a261b68.
        let w = U256::from(0x0ced27ffcd37849cu128);
        let u = U256::from(0x8a261b68u128);
        let px = eth_usd_from_reserves(w, u).unwrap();
        assert!((px - 2488.34).abs() < 0.5, "got {px}");
    }

    #[test]
    fn zero_reserves_give_no_price() {
        assert!(eth_usd_from_reserves(U256::ZERO, U256::from(5)).is_none());
        assert!(eth_usd_from_reserves(U256::from(5), U256::ZERO).is_none());
    }

    #[test]
    fn reserves_are_ordered_weth_first() {
        let weth: Address = "0x4200000000000000000000000000000000000006".parse().unwrap();
        let other: Address = "0xbA9986D2381edf1DA03B0B9c1f8b00dc4AacC369".parse().unwrap();
        assert_eq!(order_reserves(weth, weth, U256::from(1), U256::from(2)), (U256::from(1), U256::from(2)));
        assert_eq!(order_reserves(other, weth, U256::from(1), U256::from(2)), (U256::from(2), U256::from(1)));
    }

    #[test]
    fn deepest_passing_router_wins() {
        let all = vec![v("shallow", true, 1_056_000_000, 17), v("dead", false, 999_999_999_999, 1), v("deep", true, 2_317_556_584, 49)];
        assert_eq!(pick_best(&all).unwrap().name, "deep");
        assert!(pick_best(&[v("dead", false, 1, 1)]).is_none());
    }

    #[test]
    fn selectors_match_the_hand_encoded_js() {
        use alloy::sol_types::SolCall;
        // sigil-metamask.js hand-encodes these; a mismatch here would mean the browser and
        // this crate speak to different functions.
        assert_eq!(hex::encode(IUniswapV2Router02::getAmountsOutCall::SELECTOR), "d06ca61f");
        assert_eq!(hex::encode(IUniswapV2Router02::swapExactTokensForTokensCall::SELECTOR), "38ed1739");
        assert_eq!(hex::encode(IUniswapV2Router02::factoryCall::SELECTOR), "c45a0155");
        assert_eq!(hex::encode(IUniswapV2Router02::WETHCall::SELECTOR), "ad5c4648");
        assert_eq!(hex::encode(IUniswapV2Factory::allPairsLengthCall::SELECTOR), "574f2ba3");
        assert_eq!(hex::encode(IUniswapV2Pair::getReservesCall::SELECTOR), "0902f1ac");
    }
}
