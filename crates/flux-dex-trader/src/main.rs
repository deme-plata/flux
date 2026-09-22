//! flux-dex-trader — a user-directed Uniswap-V2-style trade executor.
//!
//! # Why this exists, and what it deliberately does NOT do
//!
//! This replaces a Django/Celery bot (`myapp/tasks.py`, `process_buy`) that
//! ran on a fixed schedule, ALWAYS bought (never sold), randomized the
//! amount specifically to look organic, and registered N `Wallet` rows that
//! all pointed at the SAME master private key to fake the appearance of
//! independent buyers. That combination — one-directional, scheduled,
//! camouflaged, fake-diversified — is the shape of a pump scheme, not a
//! trading tool, regardless of how small each individual buy is.
//!
//! This tool keeps the legitimate plumbing (router calls, balance/allowance
//! checks, EIP-1559 gas handling, retries) and removes every piece above:
//!
//! 1. **`buy` and `sell` are separate subcommands**, not a `--direction`
//!    flag on one `trade` command. There is no code path that can default
//!    to buying — the caller names the direction every single time.
//! 2. **No scheduler, no loop, no "run forever" mode.** Every invocation is
//!    one explicit trade. If you want periodic trading, that's an OS-level
//!    cron entry YOU own and can inspect/kill — not a hidden loop baked
//!    into this binary that a wallet name in a database toggles on/off.
//! 3. **No amount jitter/randomization.** Nothing here tries to disguise
//!    automated activity as organic human trading.
//! 4. **One key, one wallet, per invocation.** No multi-`Wallet`-row fan-out
//!    pretending to be independent actors.
//! 5. **Fails closed on real spend.** Without an explicit `--yes`, `buy`
//!    and `sell` ALWAYS behave as if `--dry-run` were passed: compute and
//!    print exactly what would happen, sign nothing, send nothing.
//! 6. **The private key is never a CLI arg or inline value** — only a path
//!    to a keyfile (plain 64-hex, no `0x`), read once. Put that file at
//!    `600` permissions, same convention as this session's other burner
//!    keyfiles.
//!
//! `quote` and `status` are pure reads and need no key at all.

use std::path::PathBuf;

use alloy::{
    network::EthereumWallet,
    primitives::{Address, U256},
    providers::{Provider, ProviderBuilder},
    signers::local::PrivateKeySigner,
    sol,
};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

sol! {
    #[sol(rpc)]
    interface IERC20 {
        function balanceOf(address owner) external view returns (uint256);
        function decimals() external view returns (uint8);
        function symbol() external view returns (string);
        function allowance(address owner, address spender) external view returns (uint256);
        function approve(address spender, uint256 amount) external returns (bool);
    }

    #[sol(rpc)]
    interface IUniswapV2Router {
        function getAmountsOut(uint256 amountIn, address[] calldata path) external view returns (uint256[] memory amounts);
        function swapExactTokensForTokens(
            uint256 amountIn,
            uint256 amountOutMin,
            address[] calldata path,
            address to,
            uint256 deadline
        ) external returns (uint256[] memory amounts);
    }
}

#[derive(Parser)]
#[command(name = "flux-dex-trader", about = "User-directed DEX trades — no autonomous buy/pump loop. See module docs.")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Read-only: what would swapping `amount_in` of `in_token` for `out_token` yield right now?
    Quote {
        #[arg(long)]
        rpc_url: String,
        #[arg(long)]
        router: Address,
        #[arg(long)]
        in_token: Address,
        #[arg(long)]
        out_token: Address,
        #[arg(long)]
        amount_in: String,
    },
    /// Read-only: token balances + allowances for a keyfile's address.
    Status {
        #[arg(long)]
        rpc_url: String,
        #[arg(long)]
        keyfile: PathBuf,
        #[arg(long, value_delimiter = ',')]
        tokens: Vec<Address>,
        #[arg(long)]
        router: Option<Address>,
    },
    /// Explicit BUY: spend `quote_token` to acquire `target_token`.
    Buy(TradeArgs),
    /// Explicit SELL: spend `target_token` to acquire `quote_token`.
    Sell(TradeArgs),
}

#[derive(Parser)]
struct TradeArgs {
    #[arg(long)]
    rpc_url: String,
    #[arg(long)]
    router: Address,
    /// The stable/quote side of the pair (e.g. USDC on Polygon).
    #[arg(long)]
    quote_token: Address,
    /// The token being bought or sold against `quote_token`.
    #[arg(long)]
    target_token: Address,
    /// Human units of the token being SPENT (quote_token for buy, target_token for sell).
    #[arg(long)]
    amount: String,
    /// Path to a plain-hex private key file (no 0x, no other content). Never pass a key inline.
    #[arg(long)]
    keyfile: PathBuf,
    #[arg(long, default_value_t = 100)]
    slippage_bps: u32,
    /// Actually broadcast. Omit this and the tool only computes + prints — nothing is signed or sent.
    #[arg(long, default_value_t = false)]
    yes: bool,
    #[arg(long, default_value_t = 300)]
    deadline_secs: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Quote { rpc_url, router, in_token, out_token, amount_in } => {
            quote(&rpc_url, router, in_token, out_token, &amount_in).await
        }
        Cmd::Status { rpc_url, keyfile, tokens, router } => {
            status(&rpc_url, &keyfile, &tokens, router).await
        }
        Cmd::Buy(args) => trade(args, Direction::Buy).await,
        Cmd::Sell(args) => trade(args, Direction::Sell).await,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Direction { Buy, Sell }

fn read_keyfile(path: &PathBuf) -> Result<PrivateKeySigner> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading keyfile {}", path.display()))?;
    let hex = raw.trim().strip_prefix("0x").unwrap_or(raw.trim());
    let signer: PrivateKeySigner = hex.parse().context("keyfile does not contain a valid hex private key")?;
    Ok(signer)
}

async fn erc20_decimals<P: Provider + Clone>(provider: &P, token: Address) -> Result<u8> {
    let c = IERC20::new(token, provider.clone());
    Ok(c.decimals().call().await?)
}

async fn erc20_symbol<P: Provider + Clone>(provider: &P, token: Address) -> Result<String> {
    let c = IERC20::new(token, provider.clone());
    Ok(c.symbol().call().await.unwrap_or_else(|_| "?".to_string()))
}

fn to_base_units(human: &str, decimals: u8) -> Result<U256> {
    let parts: Vec<&str> = human.splitn(2, '.').collect();
    let whole = parts[0];
    let frac = parts.get(1).copied().unwrap_or("");
    if frac.len() > decimals as usize {
        bail!("amount has more decimal places ({}) than the token supports ({})", frac.len(), decimals);
    }
    let mut digits = String::new();
    digits.push_str(whole);
    digits.push_str(frac);
    digits.push_str(&"0".repeat(decimals as usize - frac.len()));
    digits.parse::<U256>().context("amount is not a valid number")
}

fn from_base_units(amount: U256, decimals: u8) -> String {
    let s = amount.to_string();
    let d = decimals as usize;
    if s.len() <= d {
        let pad = "0".repeat(d - s.len());
        format!("0.{pad}{s}").trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        let (whole, frac) = s.split_at(s.len() - d);
        let frac_trimmed = frac.trim_end_matches('0');
        if frac_trimmed.is_empty() { whole.to_string() } else { format!("{whole}.{frac_trimmed}") }
    }
}

async fn quote(rpc_url: &str, router: Address, in_token: Address, out_token: Address, amount_in_human: &str) -> Result<()> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let in_dec = erc20_decimals(&provider, in_token).await?;
    let out_dec = erc20_decimals(&provider, out_token).await?;
    let in_sym = erc20_symbol(&provider, in_token).await?;
    let out_sym = erc20_symbol(&provider, out_token).await?;
    let amount_in = to_base_units(amount_in_human, in_dec)?;

    let r = IUniswapV2Router::new(router, &provider);
    let amounts = r.getAmountsOut(amount_in, vec![in_token, out_token]).call().await
        .context("getAmountsOut failed — is there a live pool for this pair on this router?")?;
    let out = amounts.last().copied().unwrap_or_default();

    println!("{amount_in_human} {in_sym} -> ~{} {out_sym}  (read-only quote, nothing sent)", from_base_units(out, out_dec));
    Ok(())
}

async fn status(rpc_url: &str, keyfile: &PathBuf, tokens: &[Address], router: Option<Address>) -> Result<()> {
    let signer = read_keyfile(keyfile)?;
    let addr = signer.address();
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    println!("wallet: {addr}");
    let native = provider.get_balance(addr).await?;
    println!("  native: {} wei ({})", native, from_base_units(native, 18));
    for &t in tokens {
        let c = IERC20::new(t, &provider);
        let sym = c.symbol().call().await.unwrap_or_else(|_| "?".to_string());
        let dec = c.decimals().call().await.unwrap_or(18);
        let bal = c.balanceOf(addr).call().await.unwrap_or_default();
        print!("  {sym} ({t}): {}", from_base_units(bal, dec));
        if let Some(r) = router {
            let allowance = c.allowance(addr, r).call().await.unwrap_or_default();
            print!("  [router allowance: {}]", from_base_units(allowance, dec));
        }
        println!();
    }
    Ok(())
}

async fn trade(args: TradeArgs, dir: Direction) -> Result<()> {
    let signer = read_keyfile(&args.keyfile)?;
    let addr = signer.address();
    let read_provider = ProviderBuilder::new().connect_http(args.rpc_url.parse()?);

    // BUY spends quote_token for target_token; SELL spends target_token for quote_token.
    // Same function either way — direction only decides which end of the pair is `in`.
    let (in_token, out_token) = match dir {
        Direction::Buy => (args.quote_token, args.target_token),
        Direction::Sell => (args.target_token, args.quote_token),
    };
    let dir_word = match dir { Direction::Buy => "BUY", Direction::Sell => "SELL" };

    let in_dec = erc20_decimals(&read_provider, in_token).await?;
    let out_dec = erc20_decimals(&read_provider, out_token).await?;
    let in_sym = erc20_symbol(&read_provider, in_token).await?;
    let out_sym = erc20_symbol(&read_provider, out_token).await?;
    let amount_in = to_base_units(&args.amount, in_dec)?;

    let in_c = IERC20::new(in_token, &read_provider);
    let balance = in_c.balanceOf(addr).call().await?;
    if balance < amount_in {
        bail!("insufficient {in_sym} balance: have {}, need {}", from_base_units(balance, in_dec), args.amount);
    }

    let router_c = IUniswapV2Router::new(args.router, &read_provider);
    let path = vec![in_token, out_token];
    let amounts = router_c.getAmountsOut(amount_in, path.clone()).call().await
        .context("getAmountsOut failed — no live pool for this pair on this router?")?;
    let expected_out = amounts.last().copied().unwrap_or_default();
    let min_out = expected_out - (expected_out * U256::from(args.slippage_bps) / U256::from(10_000u32));

    println!("{dir_word} plan: spend {} {in_sym} -> expect ~{} {out_sym} (min after {}bps slippage: {})",
        args.amount, from_base_units(expected_out, out_dec), args.slippage_bps, from_base_units(min_out, out_dec));
    println!("  wallet: {addr}");
    println!("  router: {}", args.router);

    if !args.yes {
        println!("  [dry-run — no --yes given, nothing signed or sent. Re-run with --yes to actually {dir_word}.]");
        return Ok(());
    }

    let wallet = EthereumWallet::from(signer);
    let signing_provider = ProviderBuilder::new().wallet(wallet).connect_http(args.rpc_url.parse()?);
    let in_c_signing = IERC20::new(in_token, &signing_provider);

    let allowance = in_c_signing.allowance(addr, args.router).call().await?;
    if allowance < amount_in {
        println!("  approving router for {in_sym}...");
        let pending = in_c_signing.approve(args.router, amount_in).send().await
            .context("approve transaction failed to send")?;
        let receipt = pending.get_receipt().await.context("approve transaction failed to confirm")?;
        println!("  approve tx: {:?} (status: {})", receipt.transaction_hash, receipt.status());
    }

    let deadline = U256::from(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() + args.deadline_secs,
    );
    let router_signing = IUniswapV2Router::new(args.router, &signing_provider);
    let pending = router_signing
        .swapExactTokensForTokens(amount_in, min_out, path, addr, deadline)
        .send().await
        .context("swap transaction failed to send")?;
    let receipt = pending.get_receipt().await.context("swap transaction failed to confirm")?;
    println!("{dir_word} tx: {:?} (status: {})", receipt.transaction_hash, receipt.status());
    Ok(())
}
