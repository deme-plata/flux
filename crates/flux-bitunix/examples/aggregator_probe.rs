//! Read-only end-to-end probe; never submits a trade or prints API keys/account details.
use flux_bitunix::{aggregator, Bitunix};
use serde_json::json;
fn main() {
    let capabilities = aggregator::capabilities();
    let params = [("symbols","BTCUSDT,ETHUSDT"),("chainId","8453"),
        ("sellToken","0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE"),
        ("buyToken","0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
        ("sellAmount","1000000000000000")].into_iter()
        .map(|(k,v)|(k.to_string(),v.to_string())).collect::<Vec<_>>();
    let prices = aggregator::prices(&params);
    let auth = Bitunix::from_env().and_then(|b| b.account("USDT")).is_ok();
    let cross_params = [("originChain","base"),("destinationChain","arbitrum"),
        ("sellToken","0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
        ("buyToken","0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
        ("sellAmount","10000000"),("originAddress","0x56eb0ad2dc746540fab5c02478b31e2aa9ddc38c"),
        ("destinationAddress","0x56eb0ad2dc746540fab5c02478b31e2aa9ddc38c")]
        .into_iter().map(|(k,v)|(k.to_string(),v.to_string())).collect::<Vec<_>>();
    let cross = aggregator::zerox("/cross-chain/quotes", &cross_params)
        .unwrap_or_else(|e|json!({"ok":false,"error":e}));
    let solana = aggregator::solana_instructions(&json!({"amount_in":"10000000",
        "taker":"11111111111111111111111111111111",
        "token_in":"So11111111111111111111111111111111111111112",
        "token_out":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"}))
        .unwrap_or_else(|e|json!({"ok":false,"error":e}));
    let summary = json!({"bitunix_signed_read_ok":auth,"capabilities":capabilities,"prices":prices,
        "cross_chain_sample":cross,"solana_sample":solana,"sample_wallets":"non-user addresses; no signing or submission"});
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
}
