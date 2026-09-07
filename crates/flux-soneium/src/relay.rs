//! The relay descriptor — the one JSON a relayer, a wallet, or a game MCP reads to know
//! how SIGIL lives on Soneium. Selectors and the decimal shift are mirrored from
//! `sigil-relayer` (Polygon) so the same relayer code can drive both legs by swapping
//! this descriptor in.

use crate::chain::Network;
use crate::dex::{eth_usd_from_reserves, order_reserves, pair_state};
use crate::wsigil::{Deployment, ISigilBridgeWrapped, DECIMAL_SHIFT, SIGIL_DECIMALS, WRAPPED_DECIMALS};
use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::providers::DynProvider;
use alloy::sol_types::{SolCall, SolEvent};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_RELAY_PATH: &str = "/root/.config/sigil/soneium/relay.json";
/// Polygon sibling, for cross-reference (memory project_sigil_wsigil3_pool_live_2026_09_06).
pub const POLYGON_WSIGIL3: &str = "0x3FCED760b0DE6d57F96835C6110b1227941Ec2e9";
pub const POLYGON_PAIR: &str = "0x7e9C5d73104Ed2fC2bc55139ac0999036EAf41d7";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Selectors {
    pub mint: String,
    pub burn: String,
    pub balance_of: String,
    pub burned_to_topic: String,
    pub operator_minted_topic: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Sibling { pub chain: String, pub chain_id: u64, pub token: String, pub pair: String }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayDescriptor {
    pub chain: String,
    pub chain_id: u64,
    pub explorer: String,
    pub rpc_public: String,
    pub rpc_alchemy_slug: String,
    pub weth: Address,
    pub usdc_e: Option<Address>,
    pub token: Option<Address>,
    pub router: Option<Address>,
    pub factory: Option<Address>,
    pub pair: Option<Address>,
    pub quote: Option<String>,
    pub anchor_usd_per_sigil: f64,
    pub sigil_decimals: u32,
    pub wrapped_decimals: u32,
    pub decimal_shift: u128,
    pub selectors: Selectors,
    /// `masked` until the operator un-masks `sigil-bridge-relayer`. A token with a pool and
    /// a masked relayer is BUILT, not BRIDGED.
    pub relayer: String,
    pub sibling: Sibling,
    pub updated_at: u64,
}

pub fn selectors() -> Selectors {
    Selectors {
        mint: format!("0x{}", hex::encode(ISigilBridgeWrapped::mintCall::SELECTOR)),
        burn: format!("0x{}", hex::encode(ISigilBridgeWrapped::burnCall::SELECTOR)),
        balance_of: format!("0x{}", hex::encode(ISigilBridgeWrapped::balanceOfCall::SELECTOR)),
        burned_to_topic: format!("0x{}", hex::encode(ISigilBridgeWrapped::BurnedTo::SIGNATURE_HASH)),
        operator_minted_topic: format!("0x{}", hex::encode(ISigilBridgeWrapped::OperatorMinted::SIGNATURE_HASH)),
    }
}

impl RelayDescriptor {
    pub fn from_deployment(net: &Network, d: &Deployment) -> Self {
        RelayDescriptor {
            chain: net.name.into(), chain_id: net.chain_id, explorer: net.explorer.into(),
            rpc_public: net.public_rpcs.first().copied().unwrap_or("").into(), rpc_alchemy_slug: net.alchemy_slug.into(),
            weth: net.weth_addr(), usdc_e: net.usdc_e_addr(),
            token: d.token, router: d.router, factory: d.factory, pair: d.pair, quote: d.quote.clone(),
            anchor_usd_per_sigil: crate::ANCHOR_USD_PER_SIGIL,
            sigil_decimals: SIGIL_DECIMALS, wrapped_decimals: WRAPPED_DECIMALS, decimal_shift: DECIMAL_SHIFT,
            selectors: selectors(), relayer: "masked".into(),
            sibling: Sibling { chain: "polygon".into(), chain_id: 137, token: POLYGON_WSIGIL3.into(), pair: POLYGON_PAIR.into() },
            updated_at: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        }
    }
    pub fn path(p: Option<&Path>) -> PathBuf { p.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(DEFAULT_RELAY_PATH)) }
    pub fn write(&self, p: Option<&Path>) -> Result<PathBuf> {
        let path = Self::path(p);
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
        std::fs::write(&path, serde_json::to_string_pretty(self)?).with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }
    pub fn read(p: Option<&Path>) -> Result<Self> {
        let path = Self::path(p);
        let s = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&s).context("relay.json is not a RelayDescriptor")
    }
}

/// `mint(dest, glyphs·10^8, lockId)` — what a Soneium relayer sends per SIGIL lock. Mirrors
/// `sigil-relayer`'s conversion exactly (10 dp → 18 dp, lockId = SIGIL lock tx hash).
pub fn mint_calldata(dest: Address, glyphs: u128, lock_tx: B256) -> Bytes {
    let amount = U256::from(glyphs) * U256::from(DECIMAL_SHIFT);
    Bytes::from(ISigilBridgeWrapped::mintCall { to: dest, amount, lockId: U256::from_be_bytes(lock_tx.0) }.abi_encode())
}

/// `burn(amountWei, destSigilPubkey)` — what a holder sends to go back to SIGIL L1.
pub fn burn_calldata(amount_wei: U256, dest_sigil: B256) -> Bytes {
    Bytes::from(ISigilBridgeWrapped::burnCall { amount: amount_wei, destSigilAddress: dest_sigil }.abi_encode())
}

/// Wei → glyphs, floor. A non-zero remainder is dust the relayer must LOG, never drop silently.
pub fn wei_to_glyphs(wei: U256) -> (u128, u128) {
    let shift = U256::from(DECIMAL_SHIFT);
    ((wei / shift).to::<u128>(), (wei % shift).to::<u128>())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayStatus {
    pub token: Address,
    pub symbol: String,
    pub total_supply_wei: U256,
    pub operator: Address,
    pub pair: Option<Address>,
    pub reserve_wsigil_wei: U256,
    pub reserve_quote_units: U256,
    pub eth_usd: Option<f64>,
    pub usd_per_sigil: Option<f64>,
    pub relayer: String,
}

/// Live read of what the descriptor points at.
pub async fn live_status(p: &DynProvider, desc: &RelayDescriptor, eth_usd: Option<f64>) -> Result<RelayStatus> {
    let token = desc.token.context("descriptor has no token yet — run `deploy`")?;
    let c = ISigilBridgeWrapped::new(token, p.clone());
    let mut st = RelayStatus { token, symbol: c.symbol().call().await?, total_supply_wei: c.totalSupply().call().await?, operator: c.operator().call().await?, pair: desc.pair, reserve_wsigil_wei: U256::ZERO, reserve_quote_units: U256::ZERO, eth_usd, usd_per_sigil: None, relayer: desc.relayer.clone() };
    if let Some(pair) = desc.pair {
        let ps = pair_state(p, pair).await?;
        let (w, q) = order_reserves(ps.token0, token, ps.reserve0, ps.reserve1);
        st.reserve_wsigil_wei = w; st.reserve_quote_units = q;
        st.usd_per_sigil = match desc.quote.as_deref() {
            Some("weth") => eth_usd_from_reserves(w, q).and_then(|eth_per_sigil| eth_usd.map(|px| eth_per_sigil * px)),
            Some(_) => usd_per_sigil_from_usdc(w, q),
            None => None,
        };
    }
    Ok(st)
}

/// USDC(6dp) per wSIGIL(18dp) from reserves.
pub fn usd_per_sigil_from_usdc(wsigil_wei: U256, usdc_micro: U256) -> Option<f64> {
    if wsigil_wei.is_zero() { return None; }
    Some((usdc_micro.to::<u128>() as f64 / 1e6) / (wsigil_wei.to::<u128>() as f64 / 1e18))
}

pub fn ensure_ready(desc: &RelayDescriptor) -> Result<()> {
    if desc.token.is_none() { bail!("no token on {} yet", desc.chain); }
    if desc.pair.is_none() { bail!("token exists but no pool on {} yet", desc.chain); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_selectors_match_polygon_and_the_js() {
        let s = selectors();
        assert_eq!(s.mint, "0x156e29f6");
        assert_eq!(s.burn, "0xbcf64e05");
        assert_eq!(s.balance_of, "0x70a08231");
        assert!(s.burned_to_topic.starts_with("0x95d8284568"));
    }

    #[test]
    fn mint_calldata_shifts_ten_to_eighteen_decimals() {
        let dest: Address = "0xD7cAb8075188DF9A50Dc494E9bb827f96dF93936".parse().unwrap();
        let lock = B256::repeat_byte(0xab);
        let cd = mint_calldata(dest, 4_947_260_948, lock); // the real lock amount from the relayer's wire test
        assert_eq!(&cd[..4], &[0x15, 0x6e, 0x29, 0xf6]);
        assert_eq!(cd.len(), 4 + 3 * 32);
        assert_eq!(U256::from_be_slice(&cd[4 + 32..4 + 64]), U256::from(494_726_094_800_000_000u128));
        assert_eq!(&cd[4 + 64..], lock.as_slice());
    }

    #[test]
    fn burn_calldata_layout() {
        let cd = burn_calldata(U256::from(10u128.pow(18)), B256::repeat_byte(0x01));
        assert_eq!(&cd[..4], &[0xbc, 0xf6, 0x4e, 0x05]);
        assert_eq!(cd.len(), 4 + 2 * 32);
    }

    #[test]
    fn wei_to_glyphs_floors_and_reports_dust() {
        assert_eq!(wei_to_glyphs(U256::from(10u128.pow(18))), (10u128.pow(10), 0));
        assert_eq!(wei_to_glyphs(U256::from(100_000_001u128)), (1, 1));
    }

    #[test]
    fn usd_per_sigil_from_polygon_seed() {
        let px = usd_per_sigil_from_usdc(U256::from(10u128.pow(18)), U256::from(1000)).unwrap();
        assert!((px - 0.001).abs() < 1e-12);
    }

    #[test]
    fn descriptor_roundtrips_and_names_the_sibling() {
        let net = Network::mainnet();
        let d = Deployment { chain_id: 1868, token: Some(Address::ZERO), quote: Some("weth".into()), ..Default::default() };
        let desc = RelayDescriptor::from_deployment(&net, &d);
        let dir = std::env::temp_dir().join(format!("flux-soneium-relay-{}", std::process::id()));
        let p = desc.write(Some(&dir.join("relay.json"))).unwrap();
        let back = RelayDescriptor::read(Some(&p)).unwrap();
        assert_eq!(back.chain_id, 1868);
        assert_eq!(back.sibling.token, POLYGON_WSIGIL3);
        assert_eq!(back.relayer, "masked");
        assert_eq!(back.decimal_shift, 100_000_000);
        assert!(ensure_ready(&back).is_err(), "no pair yet → not ready");
        let _ = std::fs::remove_dir_all(dir);
    }
}
