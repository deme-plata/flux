//! llm-trader — an open-model LLM proposes each swap; the Verified Execution
//! Gate decides whether it ever touches the chain.
//!
//! This is the full agentic-money loop the swarm rounds use:
//!   serve-check → LLM decide (free-form, lenient parse) → GATE → execute.
//! The model is the weak link; the gate is the seatbelt. A hallucinated
//! direction, a fat-finger size, a honeypot token, a thin-pool drain — all
//! stopped before any funds move.
//!
//! Run:  llm-trader [OLLAMA_URL] [MODEL] [N] [RPC_URL]
//!   e.g. llm-trader http://127.0.0.1:11434 qwen2.5:32b 6 http://127.0.0.1:8099
//!
//! Model notes (baked into agentic_money_kit::llm):
//!   • qwen2.5/qwen3 = tool-native, fast, reliable → prefer.
//!   • deepseek-r1   = no ollama tools + <think> starves json → the kit's
//!                     free-form + high num_predict + lenient parse handles it.

use agentic_money_kit::gate::{evaluate, Decision, GateConfig, Verdict};
use agentic_money_kit::llm::LlmConfig;
use agentic_money_kit::{llm, Rpc};

const TRADER: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const POOL: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const USDS: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const WQUG: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn main() {
    let mut args = std::env::args().skip(1);
    let ollama = args.next().unwrap_or_else(|| "http://127.0.0.1:11434".into());
    let model = args.next().unwrap_or_else(|| "qwen2.5:32b".into());
    let n: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(6);
    let rpc_url = args.next().unwrap_or_else(|| "http://127.0.0.1:8099".into());

    let model_cfg = LlmConfig::new(&ollama, &model);
    let rpc = Rpc::new(&rpc_url);
    let gate = GateConfig::default();

    // PRE-FLIGHT: never rely on a model that isn't serving.
    if !model_cfg.is_serving() {
        eprintln!("🚨 model not serving at {ollama} (check /api/tags). Aborting — no blind round.");
        std::process::exit(1);
    }
    println!("llm-trader → model {model} @ {ollama}, chain {rpc_url}, {n} rounds");

    let (mut executed, mut rejected, mut skipped, mut volume) = (0u32, 0u32, 0u32, 0u128);

    for i in 1..=n {
        let (ra, rb) = rpc.pool_reserves(0).unwrap_or((0, 0));
        let usds = rpc.balance(TRADER, USDS);
        let wqug = rpc.balance(TRADER, WQUG);

        let system = "You are a SIGIL DEX trading agent. Pick ONE small swap. \
            AtoB sells USDS for wQUG; BtoA sells wQUG for USDS. \
            End your reply with ONE json line: {\"dir\":\"AtoB\"|\"BtoA\",\"amount_in\":<int 100..2000>}.";
        let user = format!(
            "Pool reserves USDS={ra} wQUG={rb} (30bps fee). You hold USDS={usds} wQUG={wqug}. Decide."
        );

        let decision = match llm::decide(&model_cfg, system, &user) {
            Some(v) => v,
            None => {
                skipped += 1;
                println!("[{i}] — model gave no usable decision, skip");
                continue;
            }
        };
        let dir = decision.get("dir").and_then(|v| v.as_str()).unwrap_or("AtoB").to_string();
        let amount = decision.get("amount_in").and_then(|v| v.as_u64()).unwrap_or(500) as u128;
        let (token_in, token_out) = if dir == "AtoB" { ("USDS", "WQUG") } else { ("WQUG", "USDS") };
        let proposal = Decision { dir: dir.clone(), amount_in: amount, token_in: token_in.into(), token_out: token_out.into() };

        let (bal_in, res_in, res_out) = if dir == "AtoB" { (usds, ra, rb) } else { (wqug, rb, ra) };

        match evaluate(&gate, &proposal, bal_in, res_in, res_out) {
            Verdict::Reject(reason) => {
                rejected += 1;
                println!("[{i}] 🚫 gate rejected model proposal ({dir} {amount}): {reason}");
            }
            Verdict::Approve(d) => match rpc.swap(TRADER, POOL, &d.dir, d.amount_in, 1) {
                Ok(resp) if resp.contains("\"ok\":true") => {
                    executed += 1;
                    volume += d.amount_in;
                    println!("[{i}] ✅ model→gate→chain: {} {}", d.dir, d.amount_in);
                }
                Ok(resp) => println!("[{i}] ⚠️ swap: {}", resp.trim()),
                Err(e) => println!("[{i}] ⚠️ transport: {e}"),
            },
        }
    }

    println!(
        "\n── result ──\nexecuted={executed}  gate_rejected={rejected}  model_skipped={skipped}  volume_in={volume}"
    );
}
