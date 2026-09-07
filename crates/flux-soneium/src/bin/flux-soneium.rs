//! flux-soneium CLI — SIGIL on Soneium (chain 1868), Alchemy-first RPC.
//!
//!   flux-soneium check   [--net mainnet|minato] [--router 0x…]            verify RPC, routers, wallet, ETH/USD
//!   flux-soneium plan    [--quote weth|usdc] [--seed N] [--seed-supply N] [--price P]   funding plan (no tx)
//!   flux-soneium deploy  [--operator 0x…] [--seed-supply N] [--yes]        deploy SigilBridgeWrappedG3
//!   flux-soneium pool    [--quote weth|usdc] [--seed N] [--price P] [--to 0x…] [--yes]   seed the V2 pool
//!   flux-soneium all     … both, refusing to start unless the wallet is funded
//!   flux-soneium quote   <amount> [--quote weth|usdc]                       how much wSIGIL <amount> quote buys
//!   flux-soneium relay                                                      write + print the relay descriptor
//!   flux-soneium status                                                     live token + pool read via the descriptor
//!
//! Money moves only on `deploy`, `pool`, `all` — and only with `--yes`. Everything else is read-only.

use alloy::network::EthereumWallet;
use alloy::primitives::{Address, U256};
use anyhow::{bail, Context, Result};
use flux_soneium::chain::Network;
use flux_soneium::dex::{pick_router, quote as dex_quote};
use flux_soneium::plan::{estimate, fmt_eth};
use flux_soneium::pool::{seed_plan, seed_pool, Quote};
use flux_soneium::relay::{live_status, RelayDescriptor};
use flux_soneium::rpc::{connect, connect_with, load_signer};
use flux_soneium::wsigil::{deploy, sigil_to_wei, token_status, Artifact, Deployment};
use flux_soneium::ANCHOR_USD_PER_SIGIL;
use serde_json::json;
use std::path::Path;

/// LP tokens land here on Polygon; same default here.
const DEFAULT_LP_RECIPIENT: &str = "0xD7cAb8075188DF9A50Dc494E9bb827f96dF93936";

struct Args { cmd: String, pos: Vec<String>, flags: std::collections::HashMap<String, String> }

fn parse_args() -> Args {
    let mut it = std::env::args().skip(1);
    let cmd = it.next().unwrap_or_else(|| "help".into());
    let (mut pos, mut flags) = (Vec::new(), std::collections::HashMap::new());
    let rest: Vec<String> = it.collect();
    let mut i = 0;
    while i < rest.len() {
        if let Some(k) = rest[i].strip_prefix("--") {
            if k == "yes" || k == "json" { flags.insert(k.into(), "1".into()); i += 1; continue; }
            flags.insert(k.into(), rest.get(i + 1).cloned().unwrap_or_default()); i += 2;
        } else { pos.push(rest[i].clone()); i += 1; }
    }
    Args { cmd, pos, flags }
}

impl Args {
    fn s(&self, k: &str) -> Option<&str> { self.flags.get(k).map(String::as_str) }
    fn f(&self, k: &str, d: f64) -> Result<f64> { self.s(k).map(|v| v.parse::<f64>().with_context(|| format!("--{k} must be a number"))).transpose().map(|v| v.unwrap_or(d)) }
    fn addr(&self, k: &str) -> Result<Option<Address>> { self.s(k).map(|v| v.parse::<Address>().with_context(|| format!("--{k} is not an address"))).transpose() }
    fn yes(&self) -> bool { self.s("yes").is_some() }
}

fn net_of(a: &Args) -> Result<Network> {
    let n = a.s("net").unwrap_or("mainnet");
    Network::by_name(n).with_context(|| format!("unknown network {n} (mainnet|minato)"))
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await { eprintln!("error: {e:#}"); std::process::exit(1); }
}

async fn run() -> Result<()> {
    let a = parse_args();
    match a.cmd.as_str() {
        "check" => cmd_check(&a).await,
        "plan" => cmd_plan(&a).await,
        "deploy" => cmd_deploy(&a).await,
        "pool" => cmd_pool(&a).await,
        "all" => { cmd_deploy(&a).await?; cmd_pool(&a).await }
        "quote" => cmd_quote(&a).await,
        "relay" => cmd_relay(&a).await,
        "status" => cmd_status(&a).await,
        _ => { println!("{}", include_str!("flux-soneium.rs").lines().take_while(|l| l.starts_with("//!")).map(|l| l.trim_start_matches("//!")).collect::<Vec<_>>().join("\n")); Ok(()) }
    }
}

async fn cmd_check(a: &Args) -> Result<()> {
    let net = net_of(a)?;
    let c = connect(&net).await?;
    let signer = load_signer(a.s("key").map(Path::new))?;
    let wallet = signer.address();
    let (best, all) = pick_router(&c.provider, &net, a.addr("router")?).await?;
    let bal = alloy::providers::Provider::get_balance(&c.provider, wallet).await?;
    let d = Deployment::load(None, net.chain_id);
    println!("{}", serde_json::to_string_pretty(&json!({
        "network": net.name, "chain_id": net.chain_id,
        "rpc": { "source": c.endpoint.source, "endpoint": c.endpoint.display, "skipped": c.skipped },
        "wallet": wallet, "eth_balance": fmt_eth(bal),
        "eth_usd": best.eth_usd,
        "router": { "name": best.name, "router": best.router, "factory": best.factory, "pairs": best.pairs, "weth_usdc_pair": best.weth_usdc_pair, "weth_reserve_eth": fmt_eth(best.weth_reserve), "usdc_reserve": best.usdc_reserve.to::<u128>() as f64 / 1e6 },
        "all_candidates": all.iter().map(|v| json!({"name": v.name, "ok": v.ok, "pairs": v.pairs, "reason": v.reason})).collect::<Vec<_>>(),
        "deployment": d,
    }))?);
    Ok(())
}

struct Ctx { net: Network, c: flux_soneium::rpc::Connected, wallet: Address, dep: Deployment }

async fn ctx(a: &Args, signing: bool) -> Result<Ctx> {
    let net = net_of(a)?;
    let signer = load_signer(a.s("key").map(Path::new))?;
    let wallet = signer.address();
    let c = if signing { connect_with(&net, Some(EthereumWallet::from(signer))).await? } else { connect(&net).await? };
    let dep = Deployment::load(None, net.chain_id);
    Ok(Ctx { net, c, wallet, dep })
}

async fn seed_of(a: &Args, x: &Ctx) -> Result<(flux_soneium::pool::SeedPlan, flux_soneium::dex::RouterVerdict)> {
    let (best, _) = pick_router(&x.c.provider, &x.net, a.addr("router")?.or(x.dep.router)).await?;
    let quote = Quote::parse(a.s("quote").unwrap_or("weth")).context("--quote must be weth|usdc")?;
    let seed = sigil_to_wei(a.f("seed", 100.0)?)?;
    let price = a.f("price", ANCHOR_USD_PER_SIGIL)?;
    Ok((seed_plan(quote, seed, price, best.eth_usd)?, best))
}

async fn cmd_plan(a: &Args) -> Result<()> {
    let x = ctx(a, false).await?;
    let (seed, best) = seed_of(a, &x).await?;
    let code = match x.dep.token {
        Some(_) => None,
        None => {
            let art = Artifact::load(a.s("artifact").map(Path::new))?;
            let op = a.addr("operator")?.unwrap_or(x.wallet);
            Some(art.deploy_code(op, sigil_to_wei(a.f("seed-supply", 100.0)?)?)?)
        }
    };
    let plan = estimate(&x.c.provider, x.wallet, best.eth_usd, code, &seed, x.net.usdc_e_addr()).await?;
    println!("{}", serde_json::to_string_pretty(&json!({
        "network": x.net.name, "rpc": x.c.endpoint.display, "router": best.name, "eth_usd": best.eth_usd,
        "seed": { "quote": seed.quote, "wsigil": fmt_eth(seed.seed_wsigil_wei), "quote_units": seed.seed_quote_units, "usd_per_sigil": seed.target_usd_per_sigil, "lp_estimate": seed.lp_estimate },
        "steps": plan.steps.iter().map(|s| json!({"label": s.label, "gas": s.gas, "estimated": s.estimated, "l2_fee_eth": fmt_eth(s.l2_fee_wei), "l1_fee_eth": fmt_eth(s.l1_fee_wei), "value_eth": fmt_eth(s.value_wei)})).collect::<Vec<_>>(),
        "eth": { "balance": fmt_eth(plan.eth_balance_wei), "needed": fmt_eth(plan.eth_needed_wei), "shortfall": fmt_eth(plan.eth_shortfall_wei), "recommended_topup": fmt_eth(plan.recommended_topup_wei) },
        "usdc_e": { "balance": plan.usdc_balance, "needed": plan.usdc_needed, "shortfall": plan.usdc_shortfall },
        "funded": plan.funded, "fund_this_address": x.wallet,
    }))?);
    Ok(())
}

async fn cmd_deploy(a: &Args) -> Result<()> {
    let mut x = ctx(a, true).await?;
    if let Some(t) = x.dep.token { println!("already deployed on {}: {t} (tx {:?})", x.net.name, x.dep.deploy_tx); return Ok(()); }
    let art = Artifact::load(a.s("artifact").map(Path::new))?;
    let op = a.addr("operator")?.unwrap_or(x.wallet);
    let seed_supply = sigil_to_wei(a.f("seed-supply", 100.0)?)?;
    let code = art.deploy_code(op, seed_supply)?;
    let (seed, best) = seed_of(a, &x).await?;
    let plan = estimate(&x.c.provider, x.wallet, best.eth_usd, Some(code.clone()), &seed, x.net.usdc_e_addr()).await?;
    if !plan.funded { bail!("wallet {} is short {} ETH (recommend sending {} ETH on {}) — nothing sent", x.wallet, fmt_eth(plan.eth_shortfall_wei), fmt_eth(plan.recommended_topup_wei), x.net.name); }
    if !a.yes() { bail!("would deploy wSIGIL on {} (operator {op}, seed supply {}) for ≈{} ETH — re-run with --yes", x.net.name, fmt_eth(seed_supply), fmt_eth(plan.eth_needed_wei)); }
    let r = deploy(&x.c.provider, x.wallet, code).await?;
    x.dep.token = Some(r.token); x.dep.deploy_tx = Some(r.tx); x.dep.operator = Some(op); x.dep.seed_supply_wei = Some(seed_supply);
    x.dep.deployed_at = Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs());
    x.dep.save(None)?;
    RelayDescriptor::from_deployment(&x.net, &x.dep).write(None)?;
    println!("{}", serde_json::to_string_pretty(&json!({"deployed": r, "explorer": x.net.addr_url(&r.token.to_string()), "tx": x.net.tx_url(&r.tx.to_string())}))?);
    Ok(())
}

async fn cmd_pool(a: &Args) -> Result<()> {
    let mut x = ctx(a, true).await?;
    let token = x.dep.token.context("no token deployed yet — run `deploy` first")?;
    if let Some(p) = x.dep.pair { println!("pool already seeded on {}: pair {p} (tx {:?})", x.net.name, x.dep.pool_tx); return Ok(()); }
    let (seed, best) = seed_of(a, &x).await?;
    let ts = token_status(&x.c.provider, token, x.wallet).await?;
    if ts.holder_balance < seed.seed_wsigil_wei { bail!("wallet holds {} wSIGIL, seed needs {}", fmt_eth(ts.holder_balance), fmt_eth(seed.seed_wsigil_wei)); }
    let plan = estimate(&x.c.provider, x.wallet, best.eth_usd, None, &seed, x.net.usdc_e_addr()).await?;
    if !plan.funded { bail!("wallet {} short {} ETH / {} USDC.e — nothing sent", x.wallet, fmt_eth(plan.eth_shortfall_wei), plan.usdc_shortfall); }
    let to = a.addr("to")?.unwrap_or(DEFAULT_LP_RECIPIENT.parse()?);
    if !a.yes() { bail!("would seed {} wSIGIL vs {} {} units on {} ({}) at ${}/SIGIL, LP → {to} — re-run with --yes", fmt_eth(seed.seed_wsigil_wei), seed.seed_quote_units, seed.quote.label(), best.name, x.net.name, seed.target_usd_per_sigil); }
    let r = seed_pool(&x.c.provider, x.wallet, best.router, best.factory, x.net.weth_addr(), x.net.usdc_e_addr(), token, &seed, to).await?;
    x.dep.router = Some(best.router); x.dep.factory = Some(best.factory); x.dep.pair = Some(r.pair); x.dep.pool_tx = Some(r.add_tx);
    x.dep.quote = Some(seed.quote.label().into()); x.dep.seed_wsigil_wei = Some(seed.seed_wsigil_wei); x.dep.seed_quote_units = Some(seed.seed_quote_units);
    x.dep.save(None)?;
    let path = RelayDescriptor::from_deployment(&x.net, &x.dep).write(None)?;
    println!("{}", serde_json::to_string_pretty(&json!({"pool": r, "pair_url": x.net.addr_url(&r.pair.to_string()), "tx": x.net.tx_url(&r.add_tx.to_string()), "relay_descriptor": path}))?);
    Ok(())
}

async fn cmd_quote(a: &Args) -> Result<()> {
    let x = ctx(a, false).await?;
    let token = x.dep.token.context("no token deployed yet")?;
    let router = x.dep.router.context("no pool yet")?;
    let amt: f64 = a.pos.first().context("usage: quote <amount>")?.parse()?;
    let quote = Quote::parse(a.s("quote").unwrap_or(x.dep.quote.as_deref().unwrap_or("weth"))).context("--quote")?;
    let (amount_in, from) = match quote {
        Quote::Weth => (U256::from((amt * 1e18) as u128), x.net.weth_addr()),
        Quote::UsdcE => (U256::from((amt * 1e6) as u128), x.net.usdc_e_addr().context("no USDC.e")?),
    };
    let q = dex_quote(&x.c.provider, router, amount_in, vec![from, token]).await?;
    println!("{}", serde_json::to_string_pretty(&json!({"in": amt, "quote": quote, "wsigil_out": fmt_eth(q.amount_out), "amounts": q.amounts}))?);
    Ok(())
}

async fn cmd_relay(a: &Args) -> Result<()> {
    let net = net_of(a)?;
    let dep = Deployment::load(None, net.chain_id);
    let desc = RelayDescriptor::from_deployment(&net, &dep);
    let path = desc.write(None)?;
    eprintln!("wrote {}", path.display());
    println!("{}", serde_json::to_string_pretty(&desc)?);
    Ok(())
}

async fn cmd_status(a: &Args) -> Result<()> {
    let x = ctx(a, false).await?;
    let desc = RelayDescriptor::from_deployment(&x.net, &x.dep);
    let (best, _) = pick_router(&x.c.provider, &x.net, x.dep.router).await?;
    let st = live_status(&x.c.provider, &desc, best.eth_usd).await?;
    println!("{}", serde_json::to_string_pretty(&json!({"rpc": x.c.endpoint.display, "status": st, "polygon_sibling": desc.sibling}))?);
    Ok(())
}
