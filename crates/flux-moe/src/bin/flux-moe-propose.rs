//! flux-moe-propose — drive the next flux-moe improvement proposal THROUGH
//! flux-moe's OWN machinery: `unload_others` (resident-guard, so a 70B loads
//! SOLO and doesn't OOM) → `generate` (think:false, bounded KV, strips <think>).
//! No manual ollama/SSH babysitting — "kun brug flux moe".
//!
//!   FLUX_MOE_OLLAMA=http://<ip>:<port> flux-moe-propose [model] [prompt]
use std::env;

const DEFAULT_PROMPT: &str = "\
You are the judgment lane of flux-moe, authoring the NEXT additive feature for the \
flux-moe router crate. It already has: classify() keyword scorer; route()/route_weighted() O(n); \
score() weighted (40% latency/40% capability/20% cost); DispatchTable O(1) Task->best-expert; \
gate_low_confidence() drops sub-threshold ensemble members and renormalizes to 1.0; Modes Route+Ensemble. \
Constraints: pure-logic Rust, std-only, NO new deps, NO network, unit-testable, ADDITIVE (do not change existing fns). \
Reply with ONLY these three lines:\n\
FEATURE: <rust fn signature + one-line behavior>\n\
WHY: <one line>\n\
TEST: <one assertion that would pass>";

fn main() {
    let ep = env::var("FLUX_MOE_OLLAMA").unwrap_or_else(|_| "http://202.122.49.242:22938".into());
    let model = env::args().nth(1).unwrap_or_else(|| "deepseek-r1:70b".into());
    let prompt = env::args().nth(2).unwrap_or_else(|| DEFAULT_PROMPT.into());

    eprintln!("[propose] endpoint={ep} model={model} (via flux_moe::generate)");
    match flux_moe::unload_others(&ep, &model) {
        Ok(n) => eprintln!("[propose] resident-guard: unloaded {n} other model(s) — {model} gets the GPU solo"),
        Err(e) => eprintln!("[propose] unload_others warn (continuing): {e}"),
    }
    match flux_moe::generate(&ep, &model, &prompt) {
        Ok(ans) => println!("{}", ans.trim()),
        Err(e) => { eprintln!("[propose] generate error: {e}"); std::process::exit(1); }
    }
}
