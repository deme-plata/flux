//! flux-moe-bench - tokens/second of flux_moe::generate on a model (vs raw ollama).
use std::time::Instant;
fn main() {
    let endpoint = std::env::var("FLUX_MOE_OLLAMA").unwrap_or_else(|_| "http://localhost:11434".to_string());
    let model = std::env::args().nth(1).unwrap_or_else(|| "qwen3.6:latest".to_string());
    let prompt = "Write a detailed 150-word explanation of how a mixture-of-experts router dispatches a task to the best model. Be concrete and technical.";
    let t = Instant::now();
    match flux_moe::generate(&endpoint, &model, prompt) {
        Ok(out) => {
            let secs = t.elapsed().as_secs_f64();
            let chars = out.chars().count();
            let toks = (chars as f64 / 4.0) as u64;
            let words = out.split_whitespace().count();
            println!("FLUXMOE model={} chars={} approx_toks={} words={} wall={:.2}s tok_s={:.1}", model, chars, toks, words, secs, toks as f64 / secs);
        }
        Err(e) => { eprintln!("flux-moe generate err: {}", e); std::process::exit(1); }
    }
}
