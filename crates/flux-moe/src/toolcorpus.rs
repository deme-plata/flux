//! toolcorpus.rs — the agentic-money / Flux **tool-call** fine-tune corpus.
//!
//! The differentiator behind "execution on par with Claude Code": a model that
//! emits the **correct MCP tool-call** for a goal, not chat. We encode the real
//! tool surfaces — `quillon-wallet` (agentic money) + `fluxc` (agentic code) —
//! as [`ToolSpec`]s, pair goals with the right call, and emit function-calling
//! JSONL (`messages` + `tools` per example, the format trl/peft SFT consumes).
//!
//! Scaled honestly: a hand-curated core PLUS templated generators that vary REAL
//! values — actual workspace crate names, real wallet addresses (from CLAUDE.md),
//! real tokens/pools/symbols — so every generated example is grounded AND
//! schema-valid ([`validate_all`] checks each). This is a representative
//! cross-section of the ~140 wallet + ~90 flux tools, template-expanded to a few
//! hundred goals; covering every last tool is mechanical follow-on. No tool is
//! CALLED here — these are training targets only.

use serde::Serialize;
use serde_json::{json, Value};

/// A tool the model can be trained to call (name + which params are required).
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// (param, required)
    pub params: &'static [(&'static str, bool)],
}

/// The real agentic-money (wallet) + agentic-code (flux) tools we train on.
pub fn tool_registry() -> Vec<ToolSpec> {
    vec![
        // ── agentic money: balances / transfers ──
        ToolSpec { name: "get_balance", description: "Get the wallet's QUG balance", params: &[] },
        ToolSpec { name: "get_token_balance", description: "Get balance of a custom token", params: &[("token", true)] },
        ToolSpec { name: "send_qug", description: "Send QUG to an address", params: &[("to", true), ("amount", true), ("memo", false)] },
        ToolSpec { name: "send_token", description: "Send a custom token", params: &[("to", true), ("amount", true), ("token", true)] },
        ToolSpec { name: "portfolio_overview", description: "Summarize all holdings", params: &[] },
        ToolSpec { name: "list_wallet_transactions", description: "List recent wallet transactions", params: &[] },
        ToolSpec { name: "tx_status", description: "Check a transaction's status", params: &[("tx_hash", true)] },
        ToolSpec { name: "wallet_identity", description: "Show the wallet's identity/address", params: &[] },
        // ── agentic money: DEX ──
        ToolSpec { name: "dex_get_quote", description: "Quote a DEX swap", params: &[("token_in", true), ("token_out", true), ("amount_in", true)] },
        ToolSpec { name: "dex_swap", description: "Execute a DEX swap", params: &[("token_in", true), ("token_out", true), ("amount_in", true), ("min_out", false)] },
        ToolSpec { name: "dex_list_pools", description: "List DEX liquidity pools", params: &[] },
        ToolSpec { name: "dex_list_tokens", description: "List tradeable tokens", params: &[] },
        ToolSpec { name: "add_liquidity", description: "Add liquidity to a pool", params: &[("token_a", true), ("token_b", true), ("amount_a", true), ("amount_b", true)] },
        ToolSpec { name: "lp_position_value", description: "Value an LP position in QUG", params: &[("pool_id", true)] },
        // ── agentic money: markets / arb / trading ──
        ToolSpec { name: "market_scan", description: "Live CEX price + arb signal for a symbol", params: &[("symbol", true)] },
        ToolSpec { name: "arb_scan", description: "Scan for arbitrage opportunities", params: &[] },
        ToolSpec { name: "strategy_dry_run", description: "Dry-run a trading strategy (propose-only)", params: &[("strategy", true)] },
        ToolSpec { name: "qwen_trade_prepare", description: "Prepare a trade proposal", params: &[("symbol", true)] },
        // ── agentic money: BTC / Lightning ──
        ToolSpec { name: "ln_pay", description: "Pay a Lightning invoice", params: &[("invoice", true)] },
        ToolSpec { name: "ln_invoice", description: "Create a Lightning invoice", params: &[("amount", true)] },
        ToolSpec { name: "ln_balance", description: "Check Lightning balance", params: &[] },
        ToolSpec { name: "btc_generate_deposit_address", description: "Get a BTC deposit address", params: &[] },
        ToolSpec { name: "btc_withdraw", description: "Withdraw BTC", params: &[("address", true), ("amount", true)] },
        ToolSpec { name: "btc_bridge_status", description: "Check the BTC bridge status", params: &[] },
        // ── Bitcoin economy combos (flux-market + sigil-bridge + Carl-Runefelt) ──
        ToolSpec { name: "btc_dca_buy", description: "Dollar-cost-average buy BTC (Carl-Runefelt: buy the dip, never sell the core)", params: &[("amount", true), ("interval", false)] },
        ToolSpec { name: "treasury_route_to_btc", description: "Route trading/mining profit into BTC accumulation", params: &[("amount", true)] },
        ToolSpec { name: "btc_arb_scan", description: "Scan Binance↔on-chain BTC arbitrage spread", params: &[] },
        ToolSpec { name: "polymarket_scan", description: "Scan Polymarket for buy-both arbitrage", params: &[] },
        ToolSpec { name: "nowpayments_exchange", description: "Exchange one asset for another via NOWPayments", params: &[("from", true), ("to", true), ("amount", true)] },
        ToolSpec { name: "bitrefill_order", description: "Spend BTC/LN on a gift card or food via Bitrefill", params: &[("merchant", true), ("amount", true)] },
        ToolSpec { name: "wolt_order", description: "Order food via Wolt, paid from the BTC stack", params: &[("restaurant", true), ("amount", true)] },
        ToolSpec { name: "gpu_mine_to_btc", description: "Mine a GPU coin (ETC) and auto-swap proceeds to BTC", params: &[("coin", false)] },
        // ── agentic money: tokens / deploy ──
        ToolSpec { name: "deploy_token", description: "Deploy a new token", params: &[("name", true), ("symbol", true), ("supply", true)] },
        ToolSpec { name: "mining_status", description: "Check mining status", params: &[] },
        ToolSpec { name: "start_mining", description: "Start mining", params: &[] },
        ToolSpec { name: "network_status", description: "Network status overview", params: &[] },
        // ── agentic code: build / test / predict ──
        ToolSpec { name: "flux_combo", description: "Compile + test + predict a Flux package", params: &[("package", true)] },
        ToolSpec { name: "flux_compile", description: "Compile a Flux package", params: &[("package", true)] },
        ToolSpec { name: "flux_test", description: "Run tests for a Flux package", params: &[("package", true)] },
        ToolSpec { name: "flux_predict", description: "Predict build time for a package", params: &[("package", true)] },
        ToolSpec { name: "flux_qspec", description: "Propose a fix for a compile error", params: &[("package", true)] },
        ToolSpec { name: "flux_batch_compile", description: "Compile several packages in parallel", params: &[("packages", true)] },
        ToolSpec { name: "flux_bench", description: "Benchmark a Flux package", params: &[("package", true)] },
        ToolSpec { name: "flux_format", description: "Format a Flux package", params: &[("package", true)] },
        ToolSpec { name: "flux_fix", description: "Auto-fix warnings in a package", params: &[("package", true)] },
        // ── agentic code: sim / zk / architect ──
        ToolSpec { name: "flux_chronos_run", description: "Run the deterministic gossip simulator", params: &[("nodes", true), ("latency_ms", false), ("drop", false)] },
        ToolSpec { name: "flux_zk_combo", description: "Verify STARK + lattice proofs with the 10ms gate", params: &[] },
        ToolSpec { name: "flux_architect_predict", description: "Architecture + build prediction for the workspace", params: &[] },
        ToolSpec { name: "flux_heatmap", description: "Show the workspace build heatmap", params: &[] },
        ToolSpec { name: "flux_ai_audit", description: "Audit a package for state-chokepoint violations", params: &[("package", true)] },
        // ── agentic code: swarm / version / ui ──
        ToolSpec { name: "flux_swarm_message", description: "Message another agent", params: &[("from", true), ("to", true), ("message", true)] },
        ToolSpec { name: "flux_swarm_claim", description: "Claim a task/lane", params: &[("task", true)] },
        ToolSpec { name: "flux_version_bump", description: "Bump the workspace version", params: &[] },
        ToolSpec { name: "flux_ui_deploy", description: "Deploy a static UI file with a cache-busted URL", params: &[("file", true), ("content", true)] },
        ToolSpec { name: "flux_ui_list", description: "List deployed static UI surfaces", params: &[] },
    ]
}

/// A target tool-call: the name + the argument object the model should emit.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: &'static str,
    pub arguments: Value,
}
impl ToolCall {
    fn new(name: &'static str, arguments: Value) -> Self { Self { name, arguments } }
}

/// One function-calling training example (the format trl SFT consumes).
#[derive(Debug, Serialize)]
pub struct ToolExample {
    pub messages: Vec<Value>,
    pub tools: Vec<Value>,
}

fn schema_of(t: &ToolSpec) -> Value {
    let props: serde_json::Map<String, Value> = t.params.iter()
        .map(|(p, _)| ((*p).to_string(), json!({"type": "string"}))).collect();
    let required: Vec<&str> = t.params.iter().filter(|(_, req)| *req).map(|(p, _)| *p).collect();
    json!({
        "type": "function",
        "function": {
            "name": t.name, "description": t.description,
            "parameters": {"type": "object", "properties": props, "required": required}
        }
    })
}

/// Build a function-calling example from a goal + the intended call. Includes the
/// full tool registry as `tools` (the model must pick the right one).
pub fn to_example(goal: &str, call: &ToolCall) -> ToolExample {
    let tools: Vec<Value> = tool_registry().iter().map(schema_of).collect();
    ToolExample {
        messages: vec![
            json!({"role": "user", "content": goal}),
            json!({"role": "assistant", "tool_calls": [
                {"type": "function", "function": {"name": call.name, "arguments": call.arguments.to_string()}}
            ]}),
        ],
        tools,
    }
}

// ── real value pools (grounded, not synthetic) ──────────────────────────────

/// Real sibling-agent + operator addresses (from CLAUDE.md) + tokens/pools.
const ADDRS: &[(&str, &str)] = &[
    ("Rocky", "qnk7154929a6aa0c118791373ea21004aca6e494e6e031c36f780cd5acedf031ccb"),
    ("Adrian", "qnk1f97ff0b330c7790e8c82a57579052851d2c15239c78b6124fee6a74e4026d67"),
    ("Codex", "qnka3a92bba1f96"),
    ("Viktor", "qnkefca1e8c0723"),
];
const TOKENS: &[&str] = &["CLAI", "PACI", "SCALPEL", "QUGUSD", "USDS", "QSHARE"];
const PAIRS: &[(&str, &str)] = &[
    ("QUG", "PACI"), ("QUG", "SCALPEL"), ("QUG", "QUGUSD"), ("QUG", "USDS"),
    ("PACI", "QUG"), ("USDS", "QUG"), ("QUG", "CLAI"), ("SCALPEL", "QUG"),
];
const SYMBOLS: &[&str] = &["BTCUSDT", "ETHUSDT", "ETCUSDT", "SOLUSDT", "BNBUSDT", "KASUSDT"];
const QUG_AMOUNTS: &[u32] = &[1, 5, 10, 25, 50, 100, 250, 650, 1000];
/// Real workspace crate names (from flux_architect_predict, 86 crates).
const CRATES: &[&str] = &[
    "flux-moe", "flux-zk", "flux-api", "fluxc-core", "fluxc", "flux-p2p", "flux-db",
    "flux-market", "flux-chronos", "flux-cockpit", "flux-sigil", "flux-sqisign",
    "flux-recursive-proofs", "flux-zk-stark", "flux-lattice-guard", "flux-fleet",
    "flux-search", "flux-history", "flux-glossary", "flux-fcx", "flux-gpu",
    "flux-burst", "flux-quorum", "flux-aether", "flux-torrent", "flux-keel",
    "sigil-bridge", "sigil-rpc", "sigil-state", "sigil-emission", "sigil-oracle",
    "sigil-usds", "sigil-chronos", "flux-advisor", "flux-nations", "flux-oauth2",
    "flux-mempool", "flux-consensus", "flux-science", "flux-ai-bench",
];

// ── generators (each yields many grounded, schema-valid examples) ────────────

fn gen_sends() -> Vec<(String, ToolCall)> {
    let mut v = vec![];
    for (name, addr) in ADDRS {
        for &amt in &[10u32, 100, 650] {
            v.push((format!("Send {amt} QUG to {name}"),
                ToolCall::new("send_qug", json!({"to": addr, "amount": amt.to_string()}))));
        }
        for tok in &TOKENS[..3] {
            v.push((format!("Send 100 {tok} to {name} as a welcome drop"),
                ToolCall::new("send_token", json!({"to": addr, "amount": "100", "token": tok}))));
        }
    }
    v
}

fn gen_dex() -> Vec<(String, ToolCall)> {
    let mut v = vec![];
    for (a, b) in PAIRS {
        for &amt in &[25u32, 100] {
            v.push((format!("Quote swapping {amt} {a} into {b}"),
                ToolCall::new("dex_get_quote", json!({"token_in": a, "token_out": b, "amount_in": amt.to_string()}))));
            v.push((format!("Swap {amt} {a} for {b}"),
                ToolCall::new("dex_swap", json!({"token_in": a, "token_out": b, "amount_in": amt.to_string()}))));
        }
    }
    v
}

fn gen_markets() -> Vec<(String, ToolCall)> {
    let mut v = vec![];
    for s in SYMBOLS {
        v.push((format!("What's the {} price and is there an arb?", &s[..3]),
            ToolCall::new("market_scan", json!({"symbol": s}))));
        v.push((format!("Prepare a trade on {s}"),
            ToolCall::new("qwen_trade_prepare", json!({"symbol": s}))));
    }
    let _ = QUG_AMOUNTS; // amounts pool reserved for further send variants
    v
}

fn gen_code() -> Vec<(String, ToolCall)> {
    let mut v = vec![];
    for c in CRATES {
        v.push((format!("Compile and test {c}"), ToolCall::new("flux_combo", json!({"package": c}))));
        v.push((format!("How long will building {c} take?"), ToolCall::new("flux_predict", json!({"package": c}))));
    }
    // a spread of the other code verbs over a handful of crates
    for c in &CRATES[..12] {
        v.push((format!("{c} won't compile — propose a fix"), ToolCall::new("flux_qspec", json!({"package": c}))));
        v.push((format!("Run the tests for {c}"), ToolCall::new("flux_test", json!({"package": c}))));
        v.push((format!("Benchmark {c}"), ToolCall::new("flux_bench", json!({"package": c}))));
        v.push((format!("Audit {c} for state-chokepoint violations"), ToolCall::new("flux_ai_audit", json!({"package": c}))));
    }
    v
}

fn gen_chronos() -> Vec<(String, ToolCall)> {
    let mut v = vec![];
    for &n in &[4u32, 8, 12, 16, 24, 32] {
        for &lat in &[20u32, 80, 150] {
            v.push((format!("Run a chronos gossip sim with {n} nodes at {lat}ms latency"),
                ToolCall::new("flux_chronos_run", json!({"nodes": n, "latency_ms": lat}))));
        }
    }
    v
}

/// Hand-curated examples for the zero/odd-param tools (no good template).
fn curated() -> Vec<(String, ToolCall)> {
    let s = |g: &str, c: ToolCall| (g.to_string(), c);
    vec![
        s("How much QUG do I have?", ToolCall::new("get_balance", json!({}))),
        s("How many SCALPEL do I hold?", ToolCall::new("get_token_balance", json!({"token": "SCALPEL"}))),
        s("Show me my whole portfolio", ToolCall::new("portfolio_overview", json!({}))),
        s("List my recent transactions", ToolCall::new("list_wallet_transactions", json!({}))),
        s("What's my wallet address?", ToolCall::new("wallet_identity", json!({}))),
        s("Did tx 094561bf... confirm?", ToolCall::new("tx_status", json!({"tx_hash": "094561bf"}))),
        s("Find arbitrage opportunities", ToolCall::new("arb_scan", json!({}))),
        s("List the DEX pools", ToolCall::new("dex_list_pools", json!({}))),
        s("What tokens can I trade?", ToolCall::new("dex_list_tokens", json!({}))),
        s("Add liquidity: 100 QUG and 100 USDS", ToolCall::new("add_liquidity", json!({"token_a": "QUG", "token_b": "USDS", "amount_a": "100", "amount_b": "100"}))),
        s("Value my PACI/QUG LP position", ToolCall::new("lp_position_value", json!({"pool_id": "pool-955ce42686604519cb0a54cd5d186f82"}))),
        s("Pay this lightning invoice lnbc1...", ToolCall::new("ln_pay", json!({"invoice": "lnbc1..."}))),
        s("Create a lightning invoice for 5000 sats", ToolCall::new("ln_invoice", json!({"amount": "5000"}))),
        s("What's my lightning balance?", ToolCall::new("ln_balance", json!({}))),
        s("Give me a BTC deposit address", ToolCall::new("btc_generate_deposit_address", json!({}))),
        s("Withdraw 0.01 BTC to bc1qexample", ToolCall::new("btc_withdraw", json!({"address": "bc1qexample", "amount": "0.01"}))),
        s("Is the BTC bridge healthy?", ToolCall::new("btc_bridge_status", json!({}))),
        s("Deploy a token called Flux Liaison, symbol FLAI, supply 1000000", ToolCall::new("deploy_token", json!({"name": "Flux Liaison", "symbol": "FLAI", "supply": "1000000"}))),
        s("Am I mining? what's the status?", ToolCall::new("mining_status", json!({}))),
        s("Start mining", ToolCall::new("start_mining", json!({}))),
        s("How's the network doing?", ToolCall::new("network_status", json!({}))),
        s("Dry-run the MineThenDca strategy", ToolCall::new("strategy_dry_run", json!({"strategy": "MineThenDca"}))),
        s("Compile flux-zk and flux-recursive-proofs together", ToolCall::new("flux_batch_compile", json!({"packages": "flux-zk,flux-recursive-proofs"}))),
        s("Format the fluxc-core package", ToolCall::new("flux_format", json!({"package": "fluxc-core"}))),
        s("Auto-fix the warnings in flux-market", ToolCall::new("flux_fix", json!({"package": "flux-market"}))),
        s("Verify the ZK proofs under the 10ms gate", ToolCall::new("flux_zk_combo", json!({}))),
        s("Give me the workspace architecture + build prediction", ToolCall::new("flux_architect_predict", json!({}))),
        s("Show the build heatmap", ToolCall::new("flux_heatmap", json!({}))),
        s("Bump the workspace version", ToolCall::new("flux_version_bump", json!({}))),
        s("Tell rocky-sigil the bridge tests are green", ToolCall::new("flux_swarm_message", json!({"from": "rocky-moe", "to": "rocky-sigil", "message": "bridge tests green"}))),
        s("Claim the EMISSION lane", ToolCall::new("flux_swarm_claim", json!({"task": "EMISSION"}))),
        s("List the deployed UI surfaces", ToolCall::new("flux_ui_list", json!({}))),
    ]
}

/// Bitcoin-economy combos — the Carl-Runefelt / flux-market / sigil-bridge loop:
/// accumulate BTC via DCA + arb + mine→swap, route profit to BTC, spend from the
/// stack. Grounded in real amounts/merchants. (Carl-Runefelt: PROPOSE, never
/// auto-spend — these are training targets, not executions.)
fn gen_btc() -> Vec<(String, ToolCall)> {
    let mut v = vec![];
    for &amt in &[20u32, 50, 100, 250, 500] {
        v.push((format!("DCA {amt} USDS into Bitcoin"),
            ToolCall::new("btc_dca_buy", json!({"amount": amt.to_string()}))));
        v.push((format!("Route {amt} QUG of profit into the BTC stack"),
            ToolCall::new("treasury_route_to_btc", json!({"amount": amt.to_string()}))));
    }
    v.push(("Buy the dip — DCA 100 into BTC every day".into(),
        ToolCall::new("btc_dca_buy", json!({"amount": "100", "interval": "daily"}))));
    v.push(("Is there a Binance vs on-chain BTC arb right now?".into(), ToolCall::new("btc_arb_scan", json!({}))));
    v.push(("Scan Polymarket for a buy-both arbitrage".into(), ToolCall::new("polymarket_scan", json!({}))));
    v.push(("Exchange 0.01 BTC into USDS via NOWPayments".into(),
        ToolCall::new("nowpayments_exchange", json!({"from": "BTC", "to": "USDS", "amount": "0.01"}))));
    v.push(("Mine ETC on the GPU and swap it to Bitcoin".into(),
        ToolCall::new("gpu_mine_to_btc", json!({"coin": "ETC"}))));
    v.push(("Start GPU mining and auto-convert to BTC".into(), ToolCall::new("gpu_mine_to_btc", json!({}))));
    // spend from the stack (Bitrefill food menu + Wolt)
    for (m, amt) in [("ILD.PIZZA", "25"), ("Sunset Blvd", "18"), ("McDonald's", "12"), ("Flammen", "40"), ("Early Bird", "15")] {
        v.push((format!("Order food from {m} and pay from my BTC"),
            ToolCall::new("bitrefill_order", json!({"merchant": m, "amount": amt}))));
    }
    v.push(("Order a pizza on Wolt from the BTC stack".into(),
        ToolCall::new("wolt_order", json!({"restaurant": "ILD.PIZZA", "amount": "25"}))));
    v
}

/// The full corpus: curated + all generators.
pub fn seed_calls() -> Vec<(String, ToolCall)> {
    let mut v = curated();
    v.extend(gen_sends());
    v.extend(gen_dex());
    v.extend(gen_markets());
    v.extend(gen_code());
    v.extend(gen_chronos());
    v.extend(gen_btc());
    v
}

/// Emit the full corpus as function-calling JSONL.
pub fn to_jsonl() -> String {
    let mut out = String::new();
    for (goal, call) in seed_calls() {
        if let Ok(line) = serde_json::to_string(&to_example(&goal, &call)) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// Validate EVERY example: real tool + all required params present. A bad call
/// teaches the model wrong behavior, so this gates corpus emission.
pub fn validate_all() -> Result<usize, String> {
    let reg = tool_registry();
    let mut n = 0;
    for (goal, call) in seed_calls() {
        let spec = reg.iter().find(|t| t.name == call.name)
            .ok_or_else(|| format!("'{goal}' → unknown tool {}", call.name))?;
        for (p, req) in spec.params {
            if *req && call.arguments.get(p).is_none() {
                return Err(format!("'{goal}' → {} missing required param '{p}'", call.name));
            }
        }
        n += 1;
    }
    Ok(n)
}

/// Back-compat alias.
pub fn validate_seed() -> Result<usize, String> { validate_all() }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_is_a_few_hundred_valid_examples() {
        let n = validate_all().expect("all examples valid against their schema");
        assert_eq!(n, seed_calls().len());
        assert!(n >= 200, "want a few hundred grounded examples, got {n}");
    }

    #[test]
    fn examples_are_function_calling_jsonl() {
        let jsonl = to_jsonl();
        let lines: Vec<&str> = jsonl.lines().collect();
        assert_eq!(lines.len(), seed_calls().len());
        let v: Value = serde_json::from_str(lines[0]).unwrap();
        assert!(v.get("tools").unwrap().as_array().unwrap().len() >= 40, "full tool surface offered");
        let msgs = v.get("messages").unwrap().as_array().unwrap();
        assert_eq!(msgs[0]["role"], "user");
        assert!(msgs[1].get("tool_calls").is_some(), "assistant emits a tool_call");
    }

    #[test]
    fn covers_money_and_code_and_btc_surfaces() {
        let names: Vec<&str> = seed_calls().iter().map(|(_, c)| c.name).collect();
        for must in ["send_qug", "dex_swap", "flux_combo", "flux_chronos_run", "btc_withdraw", "ln_pay", "deploy_token"] {
            assert!(names.contains(&must), "missing tool coverage: {must}");
        }
    }

    #[test]
    fn covers_bitcoin_economy_combos() {
        let names: Vec<&str> = seed_calls().iter().map(|(_, c)| c.name).collect();
        for must in ["btc_dca_buy", "treasury_route_to_btc", "btc_arb_scan", "polymarket_scan", "nowpayments_exchange", "bitrefill_order", "wolt_order", "gpu_mine_to_btc"] {
            assert!(names.contains(&must), "missing BTC combo: {must}");
        }
    }

    #[test]
    fn distinct_tools_covered_is_broad() {
        let mut names: Vec<&str> = seed_calls().iter().map(|(_, c)| c.name).collect();
        names.sort_unstable(); names.dedup();
        assert!(names.len() >= 35, "want broad tool coverage, got {} distinct tools", names.len());
    }
}

// ───────────────────────── MONEY-CLASS CORPUS (the deny-list ground truth) ─────────────────────────
//
// `lib::classify_tool(tool) -> MoneyClass` is the safety gate behind TWO-MIND: RealMoney can never
// auto-execute (always needs a human), Governance needs 2-of-2, ReadOnly fast-tracks. A money-mover
// silently classed ReadOnly is the WORST failure — it would auto-execute a fund transfer. This corpus
// is the AUTHORITATIVE truth for the real quillon-wallet surface; the tests below verify the deny-list
// against it and TRACK (not hide) any tool it gets wrong, reported to the lib.rs owner.

use crate::{classify_tool, MoneyClass};

/// Authoritative money-class for the quillon-wallet tools that matter to the gate.
pub const MONEY_CLASS_CORPUS: &[(&str, MoneyClass)] = &[
    // ── RealMoney — moves/commits real funds, irreversible. NEVER auto-execute. ──
    ("send_qug", MoneyClass::RealMoney),
    ("send_token", MoneyClass::RealMoney),
    ("btc_withdraw", MoneyClass::RealMoney),
    ("dex_swap", MoneyClass::RealMoney),
    ("dex_quickstart_trade", MoneyClass::RealMoney),     // executes a swap
    ("execute_strategy", MoneyClass::RealMoney),         // runs trades
    ("add_liquidity", MoneyClass::RealMoney),
    ("ln_pay", MoneyClass::RealMoney),
    ("rwa_buy", MoneyClass::RealMoney),
    ("rwa_confirm", MoneyClass::RealMoney),
    ("rwa_offer", MoneyClass::RealMoney),                // lists/commits a real-world asset
    ("bank_apply_for_loan", MoneyClass::RealMoney),
    ("bank_payback_loan", MoneyClass::RealMoney),
    ("qshare_buyback", MoneyClass::RealMoney),
    ("qshare_mint", MoneyClass::RealMoney),              // mints shares against funds
    ("qshare_bootstrap_pool", MoneyClass::RealMoney),    // seeds a pool with funds
    ("deploy_token", MoneyClass::RealMoney),             // spends to deploy, irreversible
    ("deploy_smart_contract", MoneyClass::RealMoney),    // spends to deploy, irreversible
    ("broadcast_to_mainnet", MoneyClass::RealMoney),     // commits a (possibly fund-moving) tx to chain
    // ── Governance — governance / reputation money. 2-of-2. ──
    ("agent_submit", MoneyClass::Governance),
    ("agent_submit_batch", MoneyClass::Governance),      // batch of governance submits
    ("agent_create_mandate", MoneyClass::Governance),    // grants spend authority
    ("council_consensus", MoneyClass::Governance),
    // ── ReadOnly — reads / quotes / scans / dry-runs. MUST never be money. ──
    ("get_balance", MoneyClass::ReadOnly),
    ("get_token_balance", MoneyClass::ReadOnly),
    ("dex_get_quote", MoneyClass::ReadOnly),
    ("dex_list_pools", MoneyClass::ReadOnly),
    ("dex_list_tokens", MoneyClass::ReadOnly),
    ("arb_scan", MoneyClass::ReadOnly),
    ("market_scan", MoneyClass::ReadOnly),
    ("mining_status", MoneyClass::ReadOnly),
    ("mining_calculator", MoneyClass::ReadOnly),
    ("portfolio_overview", MoneyClass::ReadOnly),
    ("lp_position_value", MoneyClass::ReadOnly),
    ("earnings_breakdown", MoneyClass::ReadOnly),
    ("chain_overview", MoneyClass::ReadOnly),
    ("network_status", MoneyClass::ReadOnly),
    ("wallet_info", MoneyClass::ReadOnly),
    ("wallet_identity", MoneyClass::ReadOnly),
    ("tx_status", MoneyClass::ReadOnly),
    ("tx_status_signed", MoneyClass::ReadOnly),          // reads status — does NOT broadcast
    ("tx_summary", MoneyClass::ReadOnly),
    ("tx_history_filtered", MoneyClass::ReadOnly),
    ("list_wallet_transactions", MoneyClass::ReadOnly),
    ("bank_loan_status", MoneyClass::ReadOnly),
    ("bank_metrics", MoneyClass::ReadOnly),
    ("qshare_nav", MoneyClass::ReadOnly),
    ("qshare_premium_ratio", MoneyClass::ReadOnly),
    ("btc_bridge_status", MoneyClass::ReadOnly),
    ("btc_deposit_status", MoneyClass::ReadOnly),
    ("btc_generate_deposit_address", MoneyClass::ReadOnly), // receive-only, no fund move
    ("ln_balance", MoneyClass::ReadOnly),
    ("ln_invoice", MoneyClass::ReadOnly),                // creates an invoice to RECEIVE, not send
    ("strategy_dry_run", MoneyClass::ReadOnly),
    ("score_tx_dry", MoneyClass::ReadOnly),
    ("verify_on_chain", MoneyClass::ReadOnly),
    ("rwa_browse", MoneyClass::ReadOnly),
    ("agent_panel", MoneyClass::ReadOnly),
    ("agent_list_mandates", MoneyClass::ReadOnly),
    ("mcp_capabilities", MoneyClass::ReadOnly),
];

/// Tools `lib::classify_tool` currently MISCLASSIFIES as ReadOnly (per the corpus). These are
/// DANGEROUS gaps — money/governance movers the gate would let auto-execute. Reported to the lib.rs
/// owner via the swarm bus; NOT fixed here (this lane owns toolcorpus.rs only). When the deny-list is
/// hardened, `known_gaps_are_still_real` goes red to force removing the closed gap from this list.
pub const LIB_CLASSIFY_GAPS: &[&str] = &[
    // CLOSED 2026-06-03: lib::classify_tool was hardened — all former gaps (dex_quickstart_trade,
    // execute_strategy, broadcast_to_mainnet, qshare_mint, qshare_bootstrap_pool, deploy_token,
    // deploy_smart_contract, rwa_offer → RealMoney; agent_submit_batch, agent_create_mandate →
    // Governance) are now correctly classified. Empty = no known gaps. The corpus tests below now
    // require classify_tool to AGREE with the corpus on every tool.
];

#[cfg(test)]
mod money_class_tests {
    use super::*;

    fn corpus_class(tool: &str) -> MoneyClass {
        MONEY_CLASS_CORPUS.iter().find(|(t, _)| *t == tool).map(|(_, c)| *c)
            .unwrap_or_else(|| panic!("{tool} not in MONEY_CLASS_CORPUS"))
    }

    #[test]
    fn corpus_has_no_duplicate_tools() {
        let mut seen = std::collections::BTreeSet::new();
        for (t, _) in MONEY_CLASS_CORPUS {
            assert!(seen.insert(*t), "duplicate corpus entry: {t}");
        }
    }

    #[test]
    fn every_fund_mover_is_real_money_in_the_corpus() {
        // the canonical irreversible fund-movers MUST be RealMoney — the corpus's core promise
        for t in ["send_qug", "send_token", "btc_withdraw", "dex_swap", "dex_quickstart_trade",
                  "execute_strategy", "add_liquidity", "ln_pay", "rwa_buy", "rwa_confirm", "rwa_offer",
                  "bank_apply_for_loan", "bank_payback_loan", "qshare_buyback", "qshare_mint",
                  "qshare_bootstrap_pool", "deploy_token", "deploy_smart_contract", "broadcast_to_mainnet"] {
            assert_eq!(corpus_class(t), MoneyClass::RealMoney, "{t} must be RealMoney in the corpus");
        }
    }

    #[test]
    fn no_read_only_tool_is_tagged_as_money() {
        // reads/quotes/scans/dry-runs/receive-only must NEVER be money-classed
        for t in ["get_balance", "get_token_balance", "dex_get_quote", "arb_scan", "market_scan",
                  "mining_status", "portfolio_overview", "lp_position_value", "earnings_breakdown",
                  "tx_status", "tx_status_signed", "strategy_dry_run", "score_tx_dry",
                  "ln_invoice", "btc_generate_deposit_address", "rwa_browse"] {
            assert_eq!(corpus_class(t), MoneyClass::ReadOnly, "{t} must be ReadOnly in the corpus");
        }
    }

    #[test]
    fn classify_tool_matches_corpus_except_known_gaps() {
        // REGRESSION GUARD: every tool the deny-list isn't a known gap on MUST agree with the corpus.
        for (tool, want) in MONEY_CLASS_CORPUS {
            if LIB_CLASSIFY_GAPS.contains(tool) { continue; }
            assert_eq!(classify_tool(tool), *want,
                "classify_tool({tool}) disagrees with the corpus ({want:?}) — deny-list regressed");
        }
    }

    #[test]
    fn known_gaps_are_still_real() {
        // Each listed gap MUST (a) be a money/gov tool per the corpus, and (b) actually be
        // misclassified ReadOnly by classify_tool right now. If lib.rs gets hardened, this goes RED
        // → whoever fixed it removes the now-closed gap from LIB_CLASSIFY_GAPS. Keeps the list honest.
        for tool in LIB_CLASSIFY_GAPS {
            assert_ne!(corpus_class(tool), MoneyClass::ReadOnly, "{tool} in gaps but corpus says ReadOnly");
            assert_eq!(classify_tool(tool), MoneyClass::ReadOnly,
                "{tool} is NO LONGER a gap — lib.rs hardened it; remove it from LIB_CLASSIFY_GAPS");
        }
    }

    #[test]
    fn no_corpus_tool_is_an_untracked_gap() {
        // belt-and-suspenders: every corpus tool is EITHER correctly classified OR a tracked+reported
        // gap. Nothing slips through silently mis-gated.
        for (tool, want) in MONEY_CLASS_CORPUS {
            let got = classify_tool(tool);
            assert!(got == *want || LIB_CLASSIFY_GAPS.contains(tool),
                "{tool}: classify_tool={got:?} corpus={want:?} and NOT in LIB_CLASSIFY_GAPS — untracked!");
        }
    }
}
