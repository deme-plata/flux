//! flux-moe-wrestle — run the 2-of-2 ensemble JUDGE (v0.2) on the live A100.
//!
//! qwen3.6 and deepseek-r1:70b both answer; a judge model picks the winner.
//!
//!   FLUX_MOE_OLLAMA=http://<ip>:<port> flux-moe-wrestle ["the prompt"]
//!   FLUX_MOE_A=qwen3.6 FLUX_MOE_B=deepseek-r1:70b FLUX_MOE_JUDGE=qwen3.6 flux-moe-wrestle
use flux_moe::judge::judge_pair;

fn env_or(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

fn main() {
    let endpoint = env_or("FLUX_MOE_OLLAMA", "http://202.122.49.242:22938");
    let a = env_or("FLUX_MOE_A", "qwen3.6");
    let b = env_or("FLUX_MOE_B", "deepseek-r1:70b");
    let judge = env_or("FLUX_MOE_JUDGE", "qwen3.6");
    let prompt = std::env::args().nth(1).unwrap_or_else(||
        "An agentic wallet sees a 4% edge but 2% slippage and thin liquidity (3 LPs). \
         TAKE or SKIP, and at what fraction of bankroll? Two sentences: decision + reason.".into());

    eprintln!("🤼 wrestle @ {endpoint}  A={a}  B={b}  judge={judge}");
    match judge_pair(&endpoint, &a, &b, &judge, &prompt) {
        Ok(v) => {
            println!("WINNER: {}", v.winner);
            println!("WHY: {}", v.rationale.replace('\n', " ").trim());
            println!("\n— A ({a}) —\n{}", v.answer_a.trim());
            println!("\n— B ({b}) —\n{}", v.answer_b.trim());
        }
        Err(e) => {
            eprintln!("wrestle failed: {e}");
            std::process::exit(1);
        }
    }
}
