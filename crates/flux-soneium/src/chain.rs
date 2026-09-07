//! Soneium network facts. Every address here comes from docs.soneium.org/docs/builders/contracts
//! (fetched 2026-09-08) and is re-verified live where it matters (`rpc::connect` checks the
//! chain id, `dex::verify_router` checks `WETH()` against the predeploy).

use alloy::primitives::Address;
use serde::{Deserialize, Serialize};

pub const MAINNET_CHAIN_ID: u64 = 1868;
pub const MINATO_CHAIN_ID: u64 = 1946;

/// OP-Stack predeploys — identical on every OP chain, Soneium included.
pub const WETH_PREDEPLOY: &str = "0x4200000000000000000000000000000000000006";
pub const GAS_PRICE_ORACLE: &str = "0x420000000000000000000000000000000000000F";
pub const MULTICALL3: &str = "0xcA11bde05977b3631167028862bE2a173976CA11";
pub const PERMIT2: &str = "0x000000000022D473030F116dDEE9F6B43aC78BA3";

/// A candidate Uniswap-V2-compatible router. Candidates are VERIFIED, never trusted:
/// see [`crate::dex::verify_router`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouterCandidate {
    pub name: &'static str,
    pub router: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Network {
    pub name: &'static str,
    pub chain_id: u64,
    /// Alchemy network slug: `https://{slug}.g.alchemy.com/v2/{key}`.
    pub alchemy_slug: &'static str,
    pub public_rpcs: Vec<&'static str>,
    pub explorer: &'static str,
    pub weth: &'static str,
    /// Bridged USDC (6 dp). Mainnet: USDC.e via the OP USDC bridge adapter.
    pub usdc_e: Option<&'static str>,
    pub usdt: Option<&'static str>,
    pub astr: Option<&'static str>,
    pub routers: Vec<RouterCandidate>,
}

impl Network {
    pub fn mainnet() -> Self {
        Network {
            name: "soneium",
            chain_id: MAINNET_CHAIN_ID,
            alchemy_slug: "soneium-mainnet",
            public_rpcs: vec!["https://rpc.soneium.org", "https://soneium.drpc.org"],
            explorer: "https://soneium.blockscout.com",
            weth: WETH_PREDEPLOY,
            usdc_e: Some("0xbA9986D2381edf1DA03B0B9c1f8b00dc4AacC369"),
            usdt: Some("0x3A337a6adA9d885b6Ad95ec48F9b75f197b5AE35"),
            astr: Some("0x2CAE934a1e84F693fbb78CA5ED3B0A6893259441"),
            routers: vec![
                // Sonus (sonus.exchange). Measured 2026-09-08: factory 0xdb5d9562…, 49 pairs,
                // WETH/USDC.e pair 0x6de2b8f2… with 0.93 ETH / 2,317 USDC.e → ETH ≈ $2,488.
                RouterCandidate { name: "sonus", router: "0xA0133D304c54AB0ba9fBe4468018a5717f460D3a" },
                // Verified `UniswapV2Router02` on Blockscout; factory 0x97febbc2…, 17 pairs,
                // WETH/USDC.e 0.42 ETH / 1,056 USDC.e. Real but shallower than Sonus.
                RouterCandidate { name: "univ2-273f", router: "0x273F68c234fA55b550b40E563c4a488e0D334320" },
            ],
        }
    }

    pub fn minato() -> Self {
        Network {
            name: "minato",
            chain_id: MINATO_CHAIN_ID,
            alchemy_slug: "soneium-minato",
            public_rpcs: vec!["https://rpc.minato.soneium.org"],
            explorer: "https://soneium-minato.blockscout.com",
            weth: WETH_PREDEPLOY,
            usdc_e: Some("0xE9A198d38483aD727ABC8b0B1e16B2d338CF0391"),
            usdt: None,
            astr: None,
            // No verified V2 router pinned on Minato yet — pass `--router 0x…` and let
            // `verify_router` prove it before anything is sent.
            routers: vec![],
        }
    }

    pub fn by_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "soneium" | "mainnet" | "1868" => Some(Self::mainnet()),
            "minato" | "testnet" | "1946" => Some(Self::minato()),
            _ => None,
        }
    }

    pub fn weth_addr(&self) -> Address { parse(self.weth) }
    pub fn usdc_e_addr(&self) -> Option<Address> { self.usdc_e.map(parse) }
    pub fn tx_url(&self, hash: &str) -> String { format!("{}/tx/{}", self.explorer, hash) }
    pub fn addr_url(&self, addr: &str) -> String { format!("{}/address/{}", self.explorer, addr) }
}

/// Parse a hex address that is known-good at compile time. Panics only on a typo in this
/// file, which the `all_constants_parse` test catches.
pub fn parse(s: &str) -> Address {
    s.parse::<Address>().unwrap_or_else(|e| panic!("bad address constant {s}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_ids_match_soneium_docs() {
        // eth_chainId measured live 2026-09-08: mainnet 0x74c, minato 0x79a.
        assert_eq!(Network::mainnet().chain_id, 0x74c);
        assert_eq!(Network::minato().chain_id, 0x79a);
    }

    #[test]
    fn all_constants_parse() {
        for n in [Network::mainnet(), Network::minato()] {
            let _ = n.weth_addr();
            let _ = n.usdc_e_addr();
            for a in [n.usdt, n.astr].into_iter().flatten() { let _ = parse(a); }
            for r in &n.routers { let _ = parse(r.router); }
        }
        for a in [GAS_PRICE_ORACLE, MULTICALL3, PERMIT2] { let _ = parse(a); }
    }

    #[test]
    fn weth_is_the_op_stack_predeploy() {
        assert_eq!(Network::mainnet().weth, "0x4200000000000000000000000000000000000006");
        assert_eq!(Network::minato().weth, Network::mainnet().weth);
    }

    #[test]
    fn by_name_aliases() {
        assert_eq!(Network::by_name("MAINNET").unwrap().chain_id, 1868);
        assert_eq!(Network::by_name("1946").unwrap().name, "minato");
        assert!(Network::by_name("polygon").is_none());
    }

    #[test]
    fn alchemy_slugs() {
        assert_eq!(Network::mainnet().alchemy_slug, "soneium-mainnet");
        assert_eq!(Network::minato().alchemy_slug, "soneium-minato");
    }
}
