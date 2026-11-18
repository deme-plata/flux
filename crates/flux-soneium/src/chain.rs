//! Soneium network facts. Every address here comes from docs.soneium.org/docs/builders/contracts
//! (fetched 2026-09-08) and is re-verified live where it matters (`rpc::connect` checks the
//! chain id, `dex::verify_router` checks `WETH()` against the predeploy).

use alloy::primitives::Address;
use serde::{Deserialize, Serialize};

pub const MAINNET_CHAIN_ID: u64 = 1868;
pub const MINATO_CHAIN_ID: u64 = 1946;
/// Base (Coinbase's OP-Stack L2). Same predeploys as Soneium, so the whole crate applies.
/// Measured live from Epsilon 2026-11-09: `eth_chainId` 0x2105 / 0x14a34.
pub const BASE_CHAIN_ID: u64 = 8453;
pub const BASE_SEPOLIA_CHAIN_ID: u64 = 84532;

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

    /// Base mainnet (chain 8453). Added 2026-11-09 for the wSIGIL-on-Base plan.
    /// Every address verified on-chain from Epsilon that day: the router's `factory()` answers
    /// `0x8909dc15…` (3,076,298 pairs), `WETH()` answers the OP predeploy, the factory's
    /// `getPair(WETH, USDC)` is `0x88a43bbd…`, and `0x833589fc…` answers `symbol()` = "USDC".
    /// `usdc_e` here is Circle's NATIVE USDC on Base (6 dp), not a bridged twin — the field
    /// name is historical (Soneium only has USDC.e).
    pub fn base() -> Self {
        Network {
            name: "base",
            chain_id: BASE_CHAIN_ID,
            alchemy_slug: "base-mainnet",
            public_rpcs: vec!["https://mainnet.base.org", "https://base.drpc.org"],
            explorer: "https://basescan.org",
            weth: WETH_PREDEPLOY,
            usdc_e: Some("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"),
            usdt: None,
            astr: None,
            routers: vec![
                // Uniswap V2 Router02 on Base, as listed on docs.base.org/chain/contracts and
                // verified live 2026-11-09 (factory 0x8909dc15…, WETH = predeploy).
                RouterCandidate { name: "uniswap-v2", router: "0x4752ba5dbc23f44d87826276bf6fd6b1c372ad24" },
            ],
        }
    }

    /// Base Sepolia (chain 84532) — the rehearsal network for the Base leg. Verified live
    /// 2026-11-09: router `factory()` = `0x7ae58f10…` (11,078 pairs), `WETH()` = predeploy,
    /// Circle testnet USDC `0x036cbd53…` answers "USDC" / 6 dp. Gas 0.006 gwei.
    pub fn base_sepolia() -> Self {
        Network {
            name: "base-sepolia",
            chain_id: BASE_SEPOLIA_CHAIN_ID,
            alchemy_slug: "base-sepolia",
            public_rpcs: vec!["https://sepolia.base.org"],
            explorer: "https://sepolia.basescan.org",
            weth: WETH_PREDEPLOY,
            usdc_e: Some("0x036cbd53842c5426634e7929541ec2318f3dcf7e"),
            usdt: None,
            astr: None,
            routers: vec![
                RouterCandidate { name: "uniswap-v2", router: "0x1689e7b1f10000ae47ebfe339a4f69decd19f602" },
            ],
        }
    }

    /// Every network this crate knows, for tests and `--net` help text.
    pub fn all() -> Vec<Self> { vec![Self::mainnet(), Self::minato(), Self::base(), Self::base_sepolia()] }

    pub fn by_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "soneium" | "mainnet" | "1868" => Some(Self::mainnet()),
            "minato" | "testnet" | "1946" => Some(Self::minato()),
            "base" | "base-mainnet" | "8453" => Some(Self::base()),
            "base-sepolia" | "basesepolia" | "base-testnet" | "84532" => Some(Self::base_sepolia()),
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
    fn chain_ids_match_base_live_eth_chainid() {
        // eth_chainId measured live from Epsilon 2026-11-09: mainnet.base.org 0x2105,
        // sepolia.base.org 0x14a34.
        assert_eq!(Network::base().chain_id, 0x2105);
        assert_eq!(Network::base_sepolia().chain_id, 0x14a34);
        assert_eq!(Network::by_name("base").unwrap().chain_id, 8453);
        assert_eq!(Network::by_name("84532").unwrap().name, "base-sepolia");
        assert_eq!(Network::by_name("BASE-SEPOLIA").unwrap().alchemy_slug, "base-sepolia");
    }

    #[test]
    fn base_shares_the_op_stack_predeploys_with_soneium() {
        // The reason the whole crate applies to Base unchanged: same WETH predeploy, same
        // GasPriceOracle, same Uniswap-V2 router ABI.
        assert_eq!(Network::base().weth, Network::mainnet().weth);
        assert_eq!(Network::base_sepolia().weth, WETH_PREDEPLOY);
        assert!(Network::base().routers.iter().any(|r| r.name == "uniswap-v2"));
        assert!(Network::base_sepolia().routers.iter().any(|r| r.name == "uniswap-v2"));
    }

    #[test]
    fn network_names_are_unique() {
        let names: Vec<_> = Network::all().iter().map(|n| n.name).collect();
        let mut dedup = names.clone(); dedup.sort(); dedup.dedup();
        assert_eq!(names.len(), dedup.len());
        let ids: Vec<_> = Network::all().iter().map(|n| n.chain_id).collect();
        let mut d2 = ids.clone(); d2.sort(); d2.dedup();
        assert_eq!(ids.len(), d2.len());
    }

    #[test]
    fn all_constants_parse() {
        for n in Network::all() {
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
