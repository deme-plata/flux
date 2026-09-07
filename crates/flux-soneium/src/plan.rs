//! Funding plan: what the wallet holds vs what deploy + pool will cost, with every gas limit
//! estimated (not padded from a table) and the OP-Stack L1 data fee included.

use crate::chain::{parse, GAS_PRICE_ORACLE};
use crate::pool::{Quote, SeedPlan};
use alloy::network::TransactionBuilder;
use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::{DynProvider, Provider};
use alloy::rpc::types::TransactionRequest;
use alloy::sol;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

sol! {
    #[sol(rpc)]
    interface IGasPriceOracle {
        function getL1Fee(bytes memory data) external view returns (uint256);
        function l1BaseFee() external view returns (uint256);
    }
}

/// Fallbacks measured on Polygon with the identical bytecode (2026-09-06), used only when
/// `estimate_gas` cannot run (e.g. the artifact is missing).
pub const FALLBACK_GAS_DEPLOY: u64 = 797_520;
pub const FALLBACK_GAS_APPROVE: u64 = 60_000;
pub const FALLBACK_GAS_ADD_LIQUIDITY: u64 = 2_900_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub label: String,
    pub gas: u64,
    pub estimated: bool,
    pub l2_fee_wei: U256,
    pub l1_fee_wei: U256,
    /// Value sent with the tx (the WETH seed on `addLiquidityETH`), else zero.
    pub value_wei: U256,
}

impl Step {
    pub fn total_wei(&self) -> U256 { self.l2_fee_wei + self.l1_fee_wei + self.value_wei }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingPlan {
    pub wallet: Address,
    pub gas_price_wei: U256,
    pub eth_usd: Option<f64>,
    pub eth_balance_wei: U256,
    pub eth_needed_wei: U256,
    pub eth_shortfall_wei: U256,
    pub recommended_topup_wei: U256,
    pub usdc_balance: U256,
    pub usdc_needed: U256,
    pub usdc_shortfall: U256,
    pub steps: Vec<Step>,
    pub funded: bool,
}

pub fn shortfall(balance: U256, needed: U256) -> U256 { needed.saturating_sub(balance) }

/// Shortfall × 1.25, rounded UP to the next 0.0005 ETH so the number is easy to type into a
/// bridge UI. Zero stays zero.
pub fn recommended_topup(shortfall_wei: U256) -> U256 {
    if shortfall_wei.is_zero() { return U256::ZERO; }
    let padded = shortfall_wei * U256::from(125) / U256::from(100);
    let step = U256::from(500_000_000_000_000u128); // 0.0005 ETH
    ((padded + step - U256::from(1)) / step) * step
}

pub fn sum_steps(steps: &[Step]) -> U256 { steps.iter().fold(U256::ZERO, |a, s| a + s.total_wei()) }

/// Gas price the plan is priced at: node's `eth_gasPrice` (already includes a tip on
/// OP-Stack) × 1.05.
pub async fn priced_gas(p: &DynProvider) -> Result<U256> {
    let g = p.get_gas_price().await.context("eth_gasPrice")?;
    Ok(U256::from(g) * U256::from(105) / U256::from(100))
}

async fn l1_fee(p: &DynProvider, data: &Bytes) -> U256 {
    IGasPriceOracle::new(parse(GAS_PRICE_ORACLE), p.clone()).getL1Fee(data.clone()).call().await.unwrap_or(U256::ZERO)
}

async fn step(p: &DynProvider, label: &str, tx: Option<TransactionRequest>, calldata_for_l1: Bytes, fallback_gas: u64, gas_price: U256, value: U256) -> Step {
    let (gas, estimated) = match tx {
        Some(t) => match p.estimate_gas(t).await { Ok(g) => (g * 112 / 100, true), Err(_) => (fallback_gas, false) },
        None => (fallback_gas, false),
    };
    Step { label: label.into(), gas, estimated, l2_fee_wei: U256::from(gas) * gas_price, l1_fee_wei: l1_fee(p, &calldata_for_l1).await, value_wei: value }
}

/// Build the plan. `deploy_code` is `None` when the token already exists (skips that step).
pub async fn estimate(p: &DynProvider, wallet: Address, eth_usd: Option<f64>, deploy_code: Option<Bytes>, seed: &SeedPlan, usdc_e: Option<Address>) -> Result<FundingPlan> {
    let gas_price = priced_gas(p).await?;
    let mut steps = Vec::new();
    if let Some(code) = deploy_code {
        let tx = TransactionRequest::default().with_from(wallet).with_deploy_code(code.clone());
        steps.push(step(p, "deploy wSIGIL (SigilBridgeWrappedG3)", Some(tx), code, FALLBACK_GAS_DEPLOY, gas_price, U256::ZERO).await);
    }
    // approve(spender, amount) = 4 + 64 bytes of calldata; addLiquidity ≈ 4 + 8·32 (+ETH variant 6·32).
    let approve_cd = Bytes::from(vec![0u8; 68]);
    steps.push(step(p, "approve wSIGIL → router", None, approve_cd.clone(), FALLBACK_GAS_APPROVE, gas_price, U256::ZERO).await);
    let (add_label, add_cd, value) = match seed.quote {
        Quote::Weth => ("addLiquidityETH (creates pair)", Bytes::from(vec![0u8; 4 + 6 * 32]), seed.seed_quote_units),
        Quote::UsdcE => {
            steps.push(step(p, "approve USDC.e → router", None, approve_cd, FALLBACK_GAS_APPROVE, gas_price, U256::ZERO).await);
            ("addLiquidity (creates pair)", Bytes::from(vec![0u8; 4 + 8 * 32]), U256::ZERO)
        }
    };
    steps.push(step(p, add_label, None, add_cd, FALLBACK_GAS_ADD_LIQUIDITY, gas_price, value).await);

    let eth_balance = p.get_balance(wallet).await.context("eth_getBalance")?;
    let eth_needed = sum_steps(&steps);
    let (usdc_balance, usdc_needed) = match (seed.quote, usdc_e) {
        (Quote::UsdcE, Some(u)) => (crate::dex::IERC20::new(u, p.clone()).balanceOf(wallet).call().await.unwrap_or(U256::ZERO), seed.seed_quote_units),
        _ => (U256::ZERO, U256::ZERO),
    };
    let eth_short = shortfall(eth_balance, eth_needed);
    let usdc_short = shortfall(usdc_balance, usdc_needed);
    Ok(FundingPlan { wallet, gas_price_wei: gas_price, eth_usd, eth_balance_wei: eth_balance, eth_needed_wei: eth_needed, eth_shortfall_wei: eth_short, recommended_topup_wei: recommended_topup(eth_short), usdc_balance, usdc_needed, usdc_shortfall: usdc_short, funded: eth_short.is_zero() && usdc_short.is_zero(), steps })
}

pub fn fmt_eth(wei: U256) -> String { format!("{:.6}", wei.to::<u128>() as f64 / 1e18) }

#[cfg(test)]
mod tests {
    use super::*;

    fn s(gas: u64, gp: u128, l1: u128, value: u128) -> Step {
        Step { label: "x".into(), gas, estimated: true, l2_fee_wei: U256::from(gas as u128 * gp), l1_fee_wei: U256::from(l1), value_wei: U256::from(value) }
    }

    #[test]
    fn steps_sum_gas_l1_and_value() {
        let steps = vec![s(100, 10, 5, 0), s(200, 10, 0, 7)];
        assert_eq!(sum_steps(&steps), U256::from(1000 + 5 + 2000 + 7));
    }

    #[test]
    fn shortfall_never_goes_negative() {
        assert_eq!(shortfall(U256::from(10), U256::from(3)), U256::ZERO);
        assert_eq!(shortfall(U256::from(3), U256::from(10)), U256::from(7));
    }

    #[test]
    fn topup_pads_and_rounds_to_half_a_milli_eth() {
        assert_eq!(recommended_topup(U256::ZERO), U256::ZERO);
        // 0.001 ETH short → ×1.25 = 0.00125 → rounds up to 0.0015.
        let one_milli = U256::from(1_000_000_000_000_000u128);
        assert_eq!(recommended_topup(one_milli), U256::from(1_500_000_000_000_000u128));
        // 1 wei short → 0.0005 ETH.
        assert_eq!(recommended_topup(U256::from(1)), U256::from(500_000_000_000_000u128));
    }

    #[test]
    fn soneium_gas_is_dust_compared_to_polygon() {
        // Measured 2026-09-08: eth_gasPrice = 0xf435e wei ≈ 0.001 gwei. A 2.9M-gas pool tx is
        // ~3e-6 ETH on L2 — the L1 data fee, not L2 gas, dominates on this chain.
        let gp = U256::from(0xf435eu128) * U256::from(105) / U256::from(100);
        let l2 = U256::from(FALLBACK_GAS_ADD_LIQUIDITY) * gp;
        assert!(l2 < U256::from(5_000_000_000_000u128), "{l2} wei");
    }
}
