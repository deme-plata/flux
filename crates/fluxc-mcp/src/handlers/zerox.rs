//! flux_0x_* — Flux-native 0x Protocol MCP combos. Agents get the full 0x surface (Swap API v2 +
//! Cross-Chain API) as one-call tools, executed by the `flux-0x` engine (key server-side). The
//! "flux way": the spec is the flux-api `ApiEndpoint` set; these tools drive it.

use serde_json::{json, Value};
use crate::handlers::{ToolDef, ToolRegistry};
use flux_0x::{zerox_spec, Zerox};

fn arg<'a>(a: &'a Value, k: &str) -> &'a str { a.get(k).and_then(|v| v.as_str()).unwrap_or("") }
fn chain(a: &Value, k: &str, d: u64) -> u64 { a.get(k).and_then(|v| v.as_u64()).unwrap_or(d) }
fn out(r: Result<Value, String>) -> String {
    match r { Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_default(), Err(e) => format!("✗ {e}") }
}
fn zx() -> Result<Zerox, String> { Zerox::from_env() }

pub fn register(registry: &mut ToolRegistry) {
    registry.register(ToolDef {
        name: "flux_0x_price",
        description: "0x Swap API v2 — INDICATIVE price for a same-chain token swap (no taker needed). The fast 'how much would I get' check before a firm quote. Params: chainId (1=Ethereum, 137=Polygon, 8453=Base, 42161=Arbitrum, …), sellToken + buyToken (addresses), sellAmount (base units).",
        input_schema: json!({"type":"object","properties":{
            "chainId":{"type":"integer","description":"EVM chain id (default 1)"},
            "sellToken":{"type":"string"},"buyToken":{"type":"string"},"sellAmount":{"type":"string","description":"base units"}},
            "required":["sellToken","buyToken","sellAmount"]}),
    }, |a| out(zx().and_then(|z| z.swap_price(chain(a,"chainId",1), arg(a,"sellToken"), arg(a,"buyToken"), arg(a,"sellAmount")))));

    registry.register(ToolDef {
        name: "flux_0x_quote",
        description: "0x Swap API v2 — FIRM quote with executable calldata (Permit2) for a same-chain swap. Requires taker. Returns transaction.to/data/value, allowanceTarget, and the Permit2 EIP-712 message to sign. Params: chainId, sellToken, buyToken, sellAmount, taker.",
        input_schema: json!({"type":"object","properties":{
            "chainId":{"type":"integer"},"sellToken":{"type":"string"},"buyToken":{"type":"string"},
            "sellAmount":{"type":"string"},"taker":{"type":"string"}},
            "required":["sellToken","buyToken","sellAmount","taker"]}),
    }, |a| out(zx().and_then(|z| z.swap_quote(chain(a,"chainId",1), arg(a,"sellToken"), arg(a,"buyToken"), arg(a,"sellAmount"), arg(a,"taker")))));

    registry.register(ToolDef {
        name: "flux_0x_crosschain",
        description: "0x Cross-Chain API — bridge-and-swap quotes across 25+ chains incl. Solana (chain id 999999999991) in one call (aggregates 12+ bridges). Params: originChain + destinationChain (chain ids), sellToken (on origin) + buyToken (on destination), sellAmount (base units), taker (originAddress), destAddress (recipient on destination — REQUIRED for non-EVM like Solana), sort (price|speed). Returns ranked quotes + allowanceTarget + issues.",
        input_schema: json!({"type":"object","properties":{
            "originChain":{"type":"integer"},"destinationChain":{"type":"integer"},
            "sellToken":{"type":"string"},"buyToken":{"type":"string"},"sellAmount":{"type":"string"},
            "taker":{"type":"string","description":"originAddress"},
            "destAddress":{"type":"string","description":"recipient on destination chain; required for Solana/non-EVM"},
            "sort":{"type":"string","description":"price|speed"}},
            "required":["originChain","destinationChain","sellToken","buyToken","sellAmount","taker"]}),
    }, |a| out(zx().and_then(|z| z.crosschain_quotes(chain(a,"originChain",1), chain(a,"destinationChain",137), arg(a,"sellToken"), arg(a,"buyToken"), arg(a,"sellAmount"), arg(a,"taker"), arg(a,"destAddress"), arg(a,"sort")))));

    registry.register(ToolDef {
        name: "flux_0x_crosschain_status",
        description: "0x Cross-Chain API — track a submitted cross-chain trade by its origin tx hash. Params: originChain, originTxHash, quoteId (from flux_0x_crosschain).",
        input_schema: json!({"type":"object","properties":{
            "originChain":{"type":"integer"},"originTxHash":{"type":"string"},"quoteId":{"type":"string"}},
            "required":["originChain","originTxHash"]}),
    }, |a| out(zx().and_then(|z| z.crosschain_status(chain(a,"originChain",1), arg(a,"originTxHash"), arg(a,"quoteId")))));

    registry.register(ToolDef {
        name: "flux_0x_sources",
        description: "0x Swap API — list the liquidity sources (DEXes/AMMs) aggregated on a chain. Param: chainId.",
        input_schema: json!({"type":"object","properties":{"chainId":{"type":"integer"}}}),
    }, |a| out(zx().and_then(|z| z.sources(chain(a,"chainId",1)))));

    registry.register(ToolDef {
        name: "flux_0x_quickswap",
        description: "COMBO: the full same-chain swap flow in one call — indicative price → firm quote with calldata. Returns both, so an agent sees the rate then gets the ready-to-sign transaction. Params: chainId, sellToken, buyToken, sellAmount, taker.",
        input_schema: json!({"type":"object","properties":{
            "chainId":{"type":"integer"},"sellToken":{"type":"string"},"buyToken":{"type":"string"},
            "sellAmount":{"type":"string"},"taker":{"type":"string"}},
            "required":["sellToken","buyToken","sellAmount","taker"]}),
    }, flux_0x_quickswap);

    registry.register(ToolDef {
        name: "flux_0x_openapi",
        description: "Emit the OpenAPI 3.1 spec of the entire 0x engine — generated the flux way from the flux-api ApiEndpoint definitions (Swap + Cross-Chain). Proof the surface is fully modeled; feed to any SDK generator.",
        input_schema: json!({"type":"object","properties":{}}),
    }, |_a| flux_api::generate_openapi("0x Flux Engine", "2.0.0", &zerox_spec()).to_string());
}

fn flux_0x_quickswap(a: &Value) -> String {
    let z = match zx() { Ok(z) => z, Err(e) => return format!("✗ {e}") };
    let (chain_id, sell, buy, amt, taker) = (chain(a,"chainId",1), arg(a,"sellToken"), arg(a,"buyToken"), arg(a,"sellAmount"), arg(a,"taker"));
    let price = z.swap_price(chain_id, sell, buy, amt);
    let quote = z.swap_quote(chain_id, sell, buy, amt, taker);
    serde_json::to_string_pretty(&json!({
        "step1_price": price.unwrap_or_else(|e| json!({"error": e})),
        "step2_quote": quote.unwrap_or_else(|e| json!({"error": e})),
        "note": "step1 = indicative rate · step2 = firm quote with executable calldata + Permit2 message to sign",
    })).unwrap_or_default()
}
