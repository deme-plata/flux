//! RPC selection: Alchemy first, public fallback, chain id verified on every connection.

use crate::chain::Network;
use alloy::network::EthereumWallet;
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const ALCHEMY_KEY_PATH: &str = "/root/.config/alchemy/api_key";
pub const DEFAULT_SIGNER_KEY_PATH: &str = "/root/.config/sigil/uniswap-pool-operational.key";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RpcSource {
    Alchemy,
    Public,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Endpoint {
    pub source: RpcSource,
    /// Never contains the key — the key is appended only at connect time.
    pub display: String,
    #[serde(skip)]
    pub url: String,
}

/// Alchemy API key: env `ALCHEMY_API_KEY`, else the key file. Whitespace-trimmed.
pub fn alchemy_key() -> Option<String> {
    if let Ok(k) = std::env::var("ALCHEMY_API_KEY") {
        let k = k.trim().to_string();
        if !k.is_empty() { return Some(k); }
    }
    std::fs::read_to_string(ALCHEMY_KEY_PATH).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Ordered endpoint list for a network. Alchemy first when a key is present.
pub fn endpoints(net: &Network, key: Option<&str>) -> Vec<Endpoint> {
    let mut out = Vec::new();
    if let Some(k) = key {
        out.push(Endpoint {
            source: RpcSource::Alchemy,
            display: format!("https://{}.g.alchemy.com/v2/<key>", net.alchemy_slug),
            url: format!("https://{}.g.alchemy.com/v2/{}", net.alchemy_slug, k),
        });
    }
    for u in &net.public_rpcs {
        out.push(Endpoint { source: RpcSource::Public, display: u.to_string(), url: u.to_string() });
    }
    out
}

/// Alchemy answers `-32600 "<NETWORK> is not enabled for this app. Visit this page to
/// enable the network: https://dashboard.alchemy.com/apps/<id>/networks"` when the app
/// has not switched Soneium on. That is an operator click, not a code bug — surface it.
pub fn alchemy_disabled_hint(err: &str) -> Option<String> {
    if !err.contains("not enabled for this app") { return None; }
    let url = err.split_whitespace().find(|w| w.starts_with("https://dashboard.alchemy.com/")).unwrap_or("https://dashboard.alchemy.com/");
    // The provider wraps the message in its own JSON, so the URL word can carry `"}}` etc.
    let url = url.trim_end_matches(|c: char| !(c.is_ascii_alphanumeric() || "/-_.".contains(c)));
    Some(format!("Alchemy app has this network disabled — enable it here: {url}"))
}

#[derive(Clone)]
pub struct Connected {
    pub net: Network,
    pub provider: DynProvider,
    pub endpoint: Endpoint,
    /// Endpoints that were tried and skipped, with the reason (Alchemy-disabled hint etc).
    pub skipped: Vec<String>,
}

impl std::fmt::Debug for Connected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connected").field("net", &self.net.name).field("endpoint", &self.endpoint.display).field("skipped", &self.skipped).finish()
    }
}

/// Read-only provider. Tries endpoints in order; each must answer `eth_chainId` with the
/// network's id or it is skipped (a wrong-chain RPC is worse than none).
pub async fn connect(net: &Network) -> Result<Connected> {
    connect_with(net, None).await
}

/// Same as [`connect`] but with a signing wallet attached (for deploy / pool / swaps).
pub async fn connect_with(net: &Network, wallet: Option<EthereumWallet>) -> Result<Connected> {
    let key = alchemy_key();
    let mut skipped = Vec::new();
    for ep in endpoints(net, key.as_deref()) {
        let url = match ep.url.parse() { Ok(u) => u, Err(e) => { skipped.push(format!("{}: bad url ({e})", ep.display)); continue; } };
        let provider: DynProvider = match &wallet {
            Some(w) => ProviderBuilder::new().wallet(w.clone()).connect_http(url).erased(),
            None => ProviderBuilder::new().connect_http(url).erased(),
        };
        match provider.get_chain_id().await {
            Ok(id) if id == net.chain_id => {
                return Ok(Connected { net: net.clone(), provider, endpoint: ep, skipped });
            }
            Ok(id) => skipped.push(format!("{}: answered chain id {id}, expected {}", ep.display, net.chain_id)),
            Err(e) => {
                let s = e.to_string();
                skipped.push(match alchemy_disabled_hint(&s) { Some(h) => format!("{}: {h}", ep.display), None => format!("{}: {s}", ep.display) });
            }
        }
    }
    bail!("no working RPC for {} (chain {}): {}", net.name, net.chain_id, skipped.join(" | "))
}

/// Load the operational signer. Same file the Polygon deploy used, so the SAME address
/// (`0x39D1D26d…6840`) is the funding target on Soneium — one address to top up.
pub fn load_signer(path: Option<&Path>) -> Result<PrivateKeySigner> {
    let p: PathBuf = match path {
        Some(p) => p.to_path_buf(),
        None => std::env::var("SONEIUM_KEY_PATH").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(DEFAULT_SIGNER_KEY_PATH)),
    };
    let raw = std::fs::read_to_string(&p).with_context(|| format!("reading signer key {}", p.display()))?;
    let h = raw.trim().strip_prefix("0x").unwrap_or(raw.trim());
    h.parse::<PrivateKeySigner>().context("signer key is not a valid secp256k1 private key")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alchemy_first_when_key_present() {
        let eps = endpoints(&Network::mainnet(), Some("abc"));
        assert_eq!(eps[0].source, RpcSource::Alchemy);
        assert_eq!(eps[0].url, "https://soneium-mainnet.g.alchemy.com/v2/abc");
        assert!(!eps[0].display.contains("abc"), "display must never leak the key");
        assert_eq!(eps[1].source, RpcSource::Public);
        assert_eq!(eps[1].url, "https://rpc.soneium.org");
    }

    #[test]
    fn public_only_without_key() {
        let eps = endpoints(&Network::minato(), None);
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].url, "https://rpc.minato.soneium.org");
    }

    #[test]
    fn detects_alchemy_disabled_error_verbatim() {
        // Exact text measured 2026-09-08 against app 1gruvm76shefbkkp.
        let e = r#"server returned an error response: error code -32600: SONEIUM_MAINNET is not enabled for this app. Visit this page to enable the network: https://dashboard.alchemy.com/apps/1gruvm76shefbkkp/networks"#;
        let h = alchemy_disabled_hint(e).unwrap();
        assert!(h.ends_with("https://dashboard.alchemy.com/apps/1gruvm76shefbkkp/networks"), "{h}");
        // The live provider error arrives JSON-wrapped; the trailing junk must not stick to the URL.
        let wrapped = format!("{e}\"}}}}");
        assert!(alchemy_disabled_hint(&wrapped).unwrap().ends_with("/networks"));
        assert!(alchemy_disabled_hint("connection refused").is_none());
    }

    #[test]
    fn endpoint_serialisation_skips_url() {
        let ep = &endpoints(&Network::mainnet(), Some("secret"))[0];
        let js = serde_json::to_string(ep).unwrap();
        assert!(!js.contains("secret"));
        assert!(js.contains("<key>"));
    }
}
