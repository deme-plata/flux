//! The wrapped-SIGIL contract on Soneium: byte-identical artifact to Polygon's wSIGIL3
//! (`/root/.config/sigil/g3/SigilBridgeWrappedG3.json`, solc 0.8.26), so every selector,
//! event topic and the relayer's `mint(address,uint256,uint256)` call carry over unchanged.

use alloy::network::TransactionBuilder;
use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::providers::{DynProvider, Provider};
use alloy::rpc::types::TransactionRequest;
use alloy::sol;
use alloy::sol_types::SolValue;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_ARTIFACT_PATH: &str = "/root/.config/sigil/g3/SigilBridgeWrappedG3.json";
pub const DEFAULT_STATE_DIR: &str = "/root/.config/sigil/soneium";
/// Gas the identical bytecode took to deploy on Polygon (tx 53473362…, 2026-09-06).
/// Used only as a fallback when `eth_estimateGas` is unavailable.
pub const POLYGON_DEPLOY_GAS_MEASURED: u64 = 797_520;
/// One SIGIL = 10^10 glyphs on chain; the wrapped token is 18 dp.
pub const SIGIL_DECIMALS: u32 = 10;
pub const WRAPPED_DECIMALS: u32 = 18;
pub const DECIMAL_SHIFT: u128 = 10u128.pow(WRAPPED_DECIMALS - SIGIL_DECIMALS);

sol! {
    #[sol(rpc)]
    interface ISigilBridgeWrapped {
        function mint(address to, uint256 amount, uint256 lockId) external;
        function burn(uint256 amount, bytes32 destSigilAddress) external;
        function operator() external view returns (address);
        function owner() external view returns (address);
        function usedLock(uint256 lockId) external view returns (bool);
        function setOperator(address next) external;
        function totalSupply() external view returns (uint256);
        function balanceOf(address a) external view returns (uint256);
        function symbol() external view returns (string);
        function decimals() external view returns (uint8);
        event OperatorMinted(address indexed to, uint256 amount, uint256 indexed lockId);
        event BurnedTo(address indexed from, uint256 amount, bytes32 indexed destSigilAddress);
        event SeedMinted(address indexed to, uint256 amount);
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Artifact {
    pub abi: serde_json::Value,
    /// Creation bytecode, hex without 0x (solc `--bin` output).
    pub bin: String,
}

impl Artifact {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let p: PathBuf = path.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(DEFAULT_ARTIFACT_PATH));
        let raw = std::fs::read_to_string(&p).with_context(|| format!("reading artifact {}", p.display()))?;
        let a: Artifact = serde_json::from_str(&raw).context("artifact is not {abi,bin} JSON")?;
        if a.bin.is_empty() { bail!("artifact has empty bin"); }
        Ok(a)
    }

    /// The constructor is `(address _operator, uint256 seed)`; `seed` is minted to the
    /// deployer and flagged by the distinct `SeedMinted` event so an auditor can never
    /// mistake it for bridge-backed supply.
    pub fn deploy_code(&self, operator: Address, seed: U256) -> Result<Bytes> {
        deploy_code(&self.bin, operator, seed)
    }
}

pub fn deploy_code(bin_hex: &str, operator: Address, seed: U256) -> Result<Bytes> {
    let h = bin_hex.trim().strip_prefix("0x").unwrap_or(bin_hex.trim());
    let mut code = hex::decode(h).context("artifact bin is not hex")?;
    code.extend_from_slice(&(operator, seed).abi_encode_params());
    Ok(Bytes::from(code))
}

/// Persisted outcome, same shape as Polygon's `deployment.json` plus the chain id.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Deployment {
    pub chain_id: u64,
    pub token: Option<Address>,
    pub deploy_tx: Option<B256>,
    pub deployed_at: Option<u64>,
    pub operator: Option<Address>,
    pub seed_supply_wei: Option<U256>,
    pub router: Option<Address>,
    pub factory: Option<Address>,
    pub pair: Option<Address>,
    pub pool_tx: Option<B256>,
    pub quote: Option<String>,
    pub seed_wsigil_wei: Option<U256>,
    pub seed_quote_units: Option<U256>,
}

impl Deployment {
    pub fn path(dir: Option<&Path>) -> PathBuf {
        dir.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_DIR)).join("deployment.json")
    }
    pub fn load(dir: Option<&Path>, chain_id: u64) -> Self {
        let p = Self::path(dir);
        std::fs::read_to_string(&p).ok().and_then(|s| serde_json::from_str::<Deployment>(&s).ok()).filter(|d| d.chain_id == chain_id).unwrap_or(Deployment { chain_id, ..Default::default() })
    }
    pub fn save(&self, dir: Option<&Path>) -> Result<()> {
        let p = Self::path(dir);
        if let Some(parent) = p.parent() { std::fs::create_dir_all(parent)?; }
        std::fs::write(&p, serde_json::to_string_pretty(self)?).with_context(|| format!("writing {}", p.display()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployReceipt {
    pub token: Address,
    pub tx: B256,
    pub block: Option<u64>,
    pub gas_used: u64,
    pub gas_limit: u64,
}

/// Deploy with `estimate_gas × 1.12`. The provider must carry the signing wallet.
pub async fn deploy(p: &DynProvider, from: Address, code: Bytes) -> Result<DeployReceipt> {
    let tx = TransactionRequest::default().with_from(from).with_deploy_code(code);
    let est = p.estimate_gas(tx.clone()).await.context("estimate_gas(deploy)")?;
    let limit = est * 112 / 100;
    let pending = p.send_transaction(tx.with_gas_limit(limit)).await.context("send deploy")?;
    let rcpt = pending.get_receipt().await.context("deploy receipt")?;
    if !rcpt.status() { bail!("deploy tx {} reverted", rcpt.transaction_hash); }
    let token = rcpt.contract_address.context("receipt has no contract_address")?;
    Ok(DeployReceipt { token, tx: rcpt.transaction_hash, block: rcpt.block_number, gas_used: rcpt.gas_used, gas_limit: limit })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenStatus {
    pub token: Address,
    pub symbol: String,
    pub decimals: u8,
    pub total_supply: U256,
    pub holder: Address,
    pub holder_balance: U256,
    pub operator: Address,
    pub owner: Address,
}

pub async fn token_status(p: &DynProvider, token: Address, holder: Address) -> Result<TokenStatus> {
    let c = ISigilBridgeWrapped::new(token, p.clone());
    Ok(TokenStatus {
        token,
        symbol: c.symbol().call().await.context("symbol()")?,
        decimals: c.decimals().call().await?,
        total_supply: c.totalSupply().call().await?,
        holder,
        holder_balance: c.balanceOf(holder).call().await?,
        operator: c.operator().call().await?,
        owner: c.owner().call().await?,
    })
}

/// Whole SIGIL (f64, for CLI flags) → 18-dp wei. Rejects negatives/NaN; rounds to 1e-9 SIGIL.
pub fn sigil_to_wei(sigil: f64) -> Result<U256> {
    if !(sigil.is_finite() && sigil >= 0.0) { bail!("amount must be a finite non-negative number"); }
    let nano = (sigil * 1e9).round() as u128; // 9 dp of SIGIL is plenty for a seed amount
    Ok(U256::from(nano) * U256::from(10u128.pow(WRAPPED_DECIMALS - 9)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::{SolCall, SolEvent};

    #[test]
    fn selectors_are_byte_identical_to_polygon_and_the_js() {
        // sigil-relayer main.rs:67 / sigil-metamask.js SEL_BURN / wsigil-market.js.
        assert_eq!(hex::encode(ISigilBridgeWrapped::mintCall::SELECTOR), "156e29f6");
        assert_eq!(hex::encode(ISigilBridgeWrapped::burnCall::SELECTOR), "bcf64e05");
        assert_eq!(hex::encode(ISigilBridgeWrapped::balanceOfCall::SELECTOR), "70a08231");
        assert_eq!(hex::encode(ISigilBridgeWrapped::totalSupplyCall::SELECTOR), "18160ddd");
        let topic = hex::encode(ISigilBridgeWrapped::BurnedTo::SIGNATURE_HASH);
        assert!(topic.starts_with("95d8284568") && topic.ends_with("c9b79785"), "BurnedTo topic drifted: {topic}");
    }

    #[test]
    fn deploy_code_is_bin_then_two_static_words() {
        let op: Address = "0x39D1D26d59eEbcf1b4b7b4863aD38a6D226F6840".parse().unwrap();
        let seed = U256::from(10u128.pow(18));
        let code = deploy_code("0x6080", op, seed).unwrap();
        assert_eq!(code.len(), 2 + 64);
        assert_eq!(&code[..2], &[0x60, 0x80]);
        assert_eq!(&code[2 + 12..2 + 32], op.as_slice());
        assert_eq!(U256::from_be_slice(&code[2 + 32..]), seed);
        assert!(deploy_code("zz", op, seed).is_err());
    }

    #[test]
    fn decimal_shift_matches_the_relayer() {
        assert_eq!(DECIMAL_SHIFT, 100_000_000);
        assert_eq!(sigil_to_wei(1.0).unwrap(), U256::from(10u128.pow(18)));
        assert_eq!(sigil_to_wei(100.0).unwrap(), U256::from(100u128 * 10u128.pow(18)));
        assert_eq!(sigil_to_wei(0.5).unwrap(), U256::from(5 * 10u128.pow(17)));
        assert!(sigil_to_wei(-1.0).is_err());
        assert!(sigil_to_wei(f64::NAN).is_err());
    }

    #[test]
    fn deployment_roundtrip_is_chain_scoped() {
        let dir = std::env::temp_dir().join(format!("flux-soneium-test-{}", std::process::id()));
        let d = Deployment { chain_id: 1868, token: Some(Address::ZERO), ..Default::default() };
        d.save(Some(&dir)).unwrap();
        assert_eq!(Deployment::load(Some(&dir), 1868).token, Some(Address::ZERO));
        // A Minato reader must not pick up mainnet state.
        assert_eq!(Deployment::load(Some(&dir), 1946).token, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn real_artifact_loads_when_present() {
        if let Ok(a) = Artifact::load(None) {
            assert!(a.bin.len() > 1000);
            assert!(a.abi.as_array().map(|v| v.len()).unwrap_or(0) >= 20);
            let ctor = a.abi.as_array().unwrap().iter().find(|e| e["type"] == "constructor").unwrap();
            let ins: Vec<&str> = ctor["inputs"].as_array().unwrap().iter().map(|i| i["type"].as_str().unwrap()).collect();
            assert_eq!(ins, ["address", "uint256"]);
        }
    }
}
