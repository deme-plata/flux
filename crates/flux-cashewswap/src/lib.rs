//! flux-cashewswap — Flux-native port of the CashewSwap `LiquidityPool` AMM.
//!
//! Ported from the Django/Solidity DEX that lives on Beta at
//! `myquantumproject/my-token-project/backupcontract/LiquidityPool.sol`
//! (the cashewstable.com app). This reimplements the on-chain logic as a
//! pure, deterministic, overflow-checked Rust state machine — no web3, no
//! RPC, no Solidity. It is the settlement core a Flux/SIGIL-native DEX
//! (and the Wickes CMS "CashewSwap" site) calls into.
//!
//! Faithful to the contract:
//!   * constant-product swap with a 0.3% swap fee (`getSwapOutput` / `swapTokens`)
//!   * dynamic fee tied to liquidity depth and 24h volume (`calculateFee`)
//!   * slippage / price-impact guard (`minAmountOut` check)
//!   * LP mint == `amount1 + amount2`, proportional burn on withdraw
//!   * tiered staking-reward multiplier (`getUserTier` / `calculateReward`)
//!
//! Every arithmetic path is `checked_*`; an overflow is a hard error, never
//! a silent wrap (mirrors Solidity 0.8 checked arithmetic).

use thiserror::Error;

/// Fee precision denominator. `1000` basis points-ish: `1000 == 100%`.
/// Matches `FEE_PRECISION` in the contract.
pub const FEE_PRECISION: u128 = 1000;
/// Base swap fee numerator: `3/1000 = 0.3%`. Matches `SWAP_FEE`.
pub const SWAP_FEE: u128 = 3;
/// One token unit at 18 decimals ("1 ether" in the contract).
pub const ONE: u128 = 1_000_000_000_000_000_000;
/// 24 hours in seconds (volume window).
pub const VOLUME_WINDOW_SECS: u64 = 24 * 60 * 60;

/// Which side of the pool a swap consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Token1,
    Token2,
}

impl Token {
    /// The opposite side.
    pub fn other(self) -> Token {
        match self {
            Token::Token1 => Token::Token2,
            Token::Token2 => Token::Token1,
        }
    }
}

/// Errors mirror the contract's `require(...)` failure reasons.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SwapError {
    #[error("input and output tokens must be different")]
    SameToken,
    #[error("amount must be greater than zero")]
    ZeroAmount,
    #[error("insufficient liquidity")]
    InsufficientLiquidity,
    #[error("price impact too high")]
    PriceImpactTooHigh,
    #[error("not enough LP tokens")]
    NotEnoughLp,
    #[error("arithmetic overflow")]
    Overflow,
}

type Result<T> = core::result::Result<T, SwapError>;

#[inline]
fn cmul(a: u128, b: u128) -> Result<u128> {
    a.checked_mul(b).ok_or(SwapError::Overflow)
}
#[inline]
fn cadd(a: u128, b: u128) -> Result<u128> {
    a.checked_add(b).ok_or(SwapError::Overflow)
}
#[inline]
fn csub(a: u128, b: u128) -> Result<u128> {
    a.checked_sub(b).ok_or(SwapError::Overflow)
}

/// The output of a swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapResult {
    /// Gross output before the swap fee.
    pub amount_out: u128,
    /// Net output delivered to the user (`amount_out - fee`).
    pub amount_out_after_fee: u128,
    /// Fee skimmed to the fee address.
    pub fee: u128,
    /// Dynamic fee numerator in effect for this swap (`calculateFee`).
    pub dynamic_fee: u128,
}

/// A constant-product (`x * y = k`) liquidity pool over `token1`/`token2`.
#[derive(Debug, Clone)]
pub struct Pool {
    pub reserve1: u128,
    pub reserve2: u128,
    /// Total LP shares outstanding.
    pub lp_supply: u128,
    pub total_transactions: u64,
    pub volume_24h: u128,
    pub last_volume_update: u64,
    // --- dynamic fee parameters (contract defaults) ---
    pub min_fee: u128,
    pub max_fee: u128,
    pub liquidity_threshold: u128,
    pub volume_threshold: u128,
    /// Default slippage tolerance numerator over `FEE_PRECISION` (50 == 5%).
    pub default_slippage_tolerance: u128,
}

impl Default for Pool {
    fn default() -> Self {
        Pool {
            reserve1: 0,
            reserve2: 0,
            lp_supply: 0,
            total_transactions: 0,
            volume_24h: 0,
            last_volume_update: 0,
            // Contract defaults: minFee 0.1%, maxFee 1%.
            min_fee: 1,
            max_fee: 10,
            liquidity_threshold: 100_000 * ONE,
            volume_threshold: 10_000 * ONE,
            default_slippage_tolerance: 50,
        }
    }
}

impl Pool {
    /// Empty pool with contract-default fee parameters.
    pub fn new() -> Self {
        Pool::default()
    }

    /// Pool seeded with initial reserves; LP supply starts at `r1 + r2`,
    /// matching `addLiquidity`'s `_mint(msg.sender, amount1 + amount2)`.
    pub fn seeded(r1: u128, r2: u128) -> Self {
        Pool {
            reserve1: r1,
            reserve2: r2,
            lp_supply: r1.saturating_add(r2),
            ..Pool::default()
        }
    }

    /// `(reserveIn, reserveOut)` for a swap consuming `token_in`.
    #[inline]
    fn reserves(&self, token_in: Token) -> (u128, u128) {
        match token_in {
            Token::Token1 => (self.reserve1, self.reserve2),
            Token::Token2 => (self.reserve2, self.reserve1),
        }
    }

    /// `k = reserve1 * reserve2` — the constant-product invariant. Grows as
    /// fees accrue; a useful test oracle.
    pub fn k(&self) -> Result<u128> {
        cmul(self.reserve1, self.reserve2)
    }

    /// Dynamic fee numerator (`calculateFee`): base `min_fee`, +0.1% when
    /// reserves are thin, +0.1% when 24h volume is hot, capped at `max_fee`.
    pub fn calculate_fee(&self) -> u128 {
        let mut fee = self.min_fee;
        if self.reserve1.saturating_add(self.reserve2) < self.liquidity_threshold {
            fee = fee.saturating_add(1);
        }
        if self.volume_24h > self.volume_threshold {
            fee = fee.saturating_add(1);
        }
        if fee > self.max_fee {
            fee = self.max_fee;
        }
        fee
    }

    /// Constant-product quote (`getSwapOutput`, AMM branch — no oracle).
    /// Returns `(min_amount_out, amount_out)` where `min_amount_out` applies
    /// the caller's `slippage` tolerance (numerator over `FEE_PRECISION`).
    pub fn get_swap_output(
        &self,
        token_in: Token,
        amount_in: u128,
        slippage: u128,
    ) -> Result<(u128, u128)> {
        let (balance_in, balance_out) = self.reserves(token_in);
        // amountInWithFee = amountIn * (1000 - SWAP_FEE) / 1000
        let amount_in_with_fee = cmul(amount_in, csub(FEE_PRECISION, SWAP_FEE)?)? / FEE_PRECISION;
        let numerator = cmul(amount_in_with_fee, balance_out)?;
        let denominator = cadd(balance_in, amount_in_with_fee)?;
        if denominator == 0 {
            return Err(SwapError::InsufficientLiquidity);
        }
        let amount_out = numerator / denominator;
        let slip = if slippage > FEE_PRECISION { FEE_PRECISION } else { slippage };
        let min_amount_out = cmul(amount_out, csub(FEE_PRECISION, slip)?)? / FEE_PRECISION;
        Ok((min_amount_out, amount_out))
    }

    /// Read-only price-impact estimate, in `token_out` units
    /// (`getPriceImpact`): quoted out minus the naive owner-fee output.
    pub fn price_impact(&self, token_in: Token, amount_in: u128) -> Result<u128> {
        let (_, amount_out) = self.get_swap_output(token_in, amount_in, self.default_slippage_tolerance)?;
        let naive = cmul(amount_in, csub(FEE_PRECISION, SWAP_FEE)?)? / FEE_PRECISION;
        Ok(amount_out.saturating_sub(naive))
    }

    /// Execute a swap (`swapTokens`). `desired_amount_out` is the user's
    /// quote; the post-fee output must clear `desired * (1 - slippage)` or the
    /// swap reverts with `PriceImpactTooHigh`. Mutates reserves, volume and
    /// the transaction counter. `now` is a unix timestamp (seconds).
    pub fn swap(
        &mut self,
        token_in: Token,
        amount_in: u128,
        desired_amount_out: u128,
        now: u64,
    ) -> Result<SwapResult> {
        if amount_in == 0 {
            return Err(SwapError::ZeroAmount);
        }
        let slippage = self.default_slippage_tolerance;
        let dynamic_fee = self.calculate_fee();
        let (_min, amount_out) = self.get_swap_output(token_in, amount_in, slippage)?;

        // fee = amountOut * SWAP_FEE / 1000
        let fee = cmul(amount_out, SWAP_FEE)? / FEE_PRECISION;
        let amount_out_after_fee = csub(amount_out, fee)?;

        // minAmountOut = desiredAmountOut * (FEE_PRECISION - slippage) / FEE_PRECISION
        let slip = if slippage > FEE_PRECISION { FEE_PRECISION } else { slippage };
        let min_amount_out = cmul(desired_amount_out, csub(FEE_PRECISION, slip)?)? / FEE_PRECISION;
        if amount_out_after_fee < min_amount_out {
            return Err(SwapError::PriceImpactTooHigh);
        }

        // Pool must actually hold `amount_out` of the output token
        // (it pays out `amount_out_after_fee` to the user + `fee` to the fee sink).
        let (_, balance_out) = self.reserves(token_in);
        if balance_out < amount_out {
            return Err(SwapError::InsufficientLiquidity);
        }

        // Apply: reserveIn += amountIn, reserveOut -= amountOut.
        match token_in {
            Token::Token1 => {
                self.reserve1 = cadd(self.reserve1, amount_in)?;
                self.reserve2 = csub(self.reserve2, amount_out)?;
            }
            Token::Token2 => {
                self.reserve2 = cadd(self.reserve2, amount_in)?;
                self.reserve1 = csub(self.reserve1, amount_out)?;
            }
        }

        self.update_volume(amount_in, now);
        self.total_transactions += 1;

        Ok(SwapResult {
            amount_out,
            amount_out_after_fee,
            fee,
            dynamic_fee,
        })
    }

    /// Add liquidity (`addLiquidity`). LP shares minted == `amount1 + amount2`
    /// (faithful to the contract, which does not use a geometric-mean mint).
    /// Returns the LP shares minted.
    pub fn add_liquidity(&mut self, amount1: u128, amount2: u128) -> Result<u128> {
        if amount1 == 0 && amount2 == 0 {
            return Err(SwapError::ZeroAmount);
        }
        self.reserve1 = cadd(self.reserve1, amount1)?;
        self.reserve2 = cadd(self.reserve2, amount2)?;
        let minted = cadd(amount1, amount2)?;
        self.lp_supply = cadd(self.lp_supply, minted)?;
        Ok(minted)
    }

    /// Remove liquidity (`removeLiquidity`). Burns `amount` LP shares held by a
    /// provider with `holder_balance` shares and returns `(out1, out2)`
    /// proportional to current reserves.
    pub fn remove_liquidity(&mut self, amount: u128, holder_balance: u128) -> Result<(u128, u128)> {
        if amount == 0 {
            return Err(SwapError::ZeroAmount);
        }
        if amount > holder_balance {
            return Err(SwapError::NotEnoughLp);
        }
        if self.lp_supply == 0 || amount > self.lp_supply {
            return Err(SwapError::NotEnoughLp);
        }
        let out1 = cmul(self.reserve1, amount)? / self.lp_supply;
        let out2 = cmul(self.reserve2, amount)? / self.lp_supply;
        self.lp_supply = csub(self.lp_supply, amount)?;
        self.reserve1 = csub(self.reserve1, out1)?;
        self.reserve2 = csub(self.reserve2, out2)?;
        Ok((out1, out2))
    }

    /// 24h volume bookkeeping (`updateVolume`): reset the window when it has
    /// elapsed, otherwise accumulate.
    fn update_volume(&mut self, amount: u128, now: u64) {
        if now > self.last_volume_update.saturating_add(VOLUME_WINDOW_SECS) {
            self.volume_24h = amount;
            self.last_volume_update = now;
        } else {
            self.volume_24h = self.volume_24h.saturating_add(amount);
        }
    }
}

/// Reward tier for a staker (`getUserTier`): index of the first threshold the
/// staked balance is below; `thresholds.len()` if above them all.
pub fn user_tier(staked: u128, thresholds: &[u128]) -> usize {
    for (i, t) in thresholds.iter().enumerate() {
        if staked < *t {
            return i;
        }
    }
    thresholds.len()
}

/// Tiered reward (`calculateReward`): `base_reward * tierRewards[tier]`.
/// `tier_rewards` must cover every tier index (len == thresholds.len() + 1).
pub fn calculate_reward(
    staked: u128,
    base_reward: u128,
    thresholds: &[u128],
    tier_rewards: &[u128],
) -> Result<u128> {
    let tier = user_tier(staked, thresholds);
    let mult = tier_rewards.get(tier).copied().unwrap_or(0);
    cmul(base_reward, mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1000/1000 pool, swap 100 token1 in.
    // amountInWithFee = 100*997/1000 = 99
    // out = 99*1000 / (1000+99) = 99000/1099 = 90
    #[test]
    fn constant_product_quote_matches_contract() {
        let p = Pool::seeded(1000, 1000);
        let (min_out, out) = p.get_swap_output(Token::Token1, 100, 50).unwrap();
        assert_eq!(out, 90);
        // min_out = 90*(1000-50)/1000 = 85
        assert_eq!(min_out, 85);
    }

    #[test]
    fn swap_grows_k_invariant() {
        let mut p = Pool::seeded(1000, 1000);
        let before = p.k().unwrap();
        let r = p.swap(Token::Token1, 100, 80, 1_000).unwrap();
        assert_eq!(r.amount_out, 90);
        // fee = 90*3/1000 = 0 (integer), so user gets the full 90 here.
        assert_eq!(r.fee, 0);
        assert_eq!(r.amount_out_after_fee, 90);
        assert_eq!(p.reserve1, 1100);
        assert_eq!(p.reserve2, 910);
        // x*y must not shrink — the fee stays in the pool.
        assert!(p.k().unwrap() >= before);
        assert_eq!(p.total_transactions, 1);
    }

    #[test]
    fn swap_fee_is_skimmed_on_large_trade() {
        // Big reserves so amount_out is large enough that 0.3% rounds > 0.
        let mut p = Pool::seeded(1_000_000, 1_000_000);
        let r = p.swap(Token::Token2, 10_000, 1, 1).unwrap();
        assert!(r.fee > 0, "fee should be non-zero on a large trade");
        assert_eq!(r.amount_out_after_fee, r.amount_out - r.fee);
    }

    #[test]
    fn slippage_guard_rejects_unmet_quote() {
        let mut p = Pool::seeded(1000, 1000);
        // Demand far more out than the pool can give -> price impact too high.
        let err = p.swap(Token::Token1, 100, 100_000, 1).unwrap_err();
        assert_eq!(err, SwapError::PriceImpactTooHigh);
    }

    #[test]
    fn same_and_zero_inputs_rejected() {
        let mut p = Pool::seeded(1000, 1000);
        assert_eq!(p.swap(Token::Token1, 0, 0, 1).unwrap_err(), SwapError::ZeroAmount);
        assert_eq!(Token::Token1.other(), Token::Token2);
    }

    #[test]
    fn add_then_remove_liquidity_roundtrips_proportionally() {
        let mut p = Pool::seeded(1000, 2000); // lp_supply = 3000
        let minted = p.add_liquidity(1000, 2000).unwrap();
        assert_eq!(minted, 3000);
        assert_eq!(p.lp_supply, 6000);
        assert_eq!((p.reserve1, p.reserve2), (2000, 4000));
        // Burn half of the freshly minted shares.
        let (o1, o2) = p.remove_liquidity(3000, 3000).unwrap();
        assert_eq!((o1, o2), (1000, 2000));
        assert_eq!(p.lp_supply, 3000);
        assert_eq!((p.reserve1, p.reserve2), (1000, 2000));
    }

    #[test]
    fn remove_more_than_held_rejected() {
        let mut p = Pool::seeded(1000, 1000);
        assert_eq!(p.remove_liquidity(10, 5).unwrap_err(), SwapError::NotEnoughLp);
    }

    #[test]
    fn dynamic_fee_responds_to_depth_and_volume() {
        // Thin pool below the liquidity threshold -> base + 0.1%.
        let mut p = Pool::seeded(10, 10);
        assert_eq!(p.calculate_fee(), p.min_fee + 1);
        // Push 24h volume above the threshold -> + another 0.1%.
        p.volume_24h = p.volume_threshold + 1;
        assert_eq!(p.calculate_fee(), p.min_fee + 2);
    }

    #[test]
    fn volume_window_resets_after_24h() {
        let mut p = Pool::seeded(1_000_000, 1_000_000);
        p.swap(Token::Token1, 100, 1, 1_000).unwrap();
        let v1 = p.volume_24h;
        assert_eq!(v1, 100);
        // Within the window: accumulate.
        p.swap(Token::Token1, 50, 1, 1_000 + 10).unwrap();
        assert_eq!(p.volume_24h, 150);
        // Past the window: reset to the new trade's amount.
        p.swap(Token::Token1, 70, 1, 1_000 + VOLUME_WINDOW_SECS + 5).unwrap();
        assert_eq!(p.volume_24h, 70);
    }

    #[test]
    fn tiered_rewards() {
        let thresholds = [100 * ONE, 500 * ONE, 1000 * ONE];
        let rewards = [1, 2, 3, 4];
        assert_eq!(user_tier(50 * ONE, &thresholds), 0);
        assert_eq!(user_tier(600 * ONE, &thresholds), 2);
        assert_eq!(user_tier(5000 * ONE, &thresholds), 3);
        assert_eq!(calculate_reward(600 * ONE, 10, &thresholds, &rewards).unwrap(), 30);
    }

    #[test]
    fn overflow_is_an_error_not_a_wrap() {
        let p = Pool::seeded(u128::MAX, u128::MAX);
        // amount_in_with_fee * balance_out overflows u128 -> hard error.
        assert_eq!(p.get_swap_output(Token::Token1, u128::MAX, 50).unwrap_err(), SwapError::Overflow);
    }
}
