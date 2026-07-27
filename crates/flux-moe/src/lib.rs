//! # flux-moe — the Flux-native hybrid LLM (mixture-of-MODELS router + trainer).
//!
//! You can't fuse Qwen and Gemma into one set of weights — different
//! architectures + tokenizers. The Flux way to "hybridize" them is the **combo
//! pattern applied to models**: each model is an EXPERT, a router classifies the
//! prompt and sends it to the best expert (or ENSEMBLES two for high-stakes),
//! all over the distributed swarm (Ollama). One call: classify → route →
//! generate. The "hybrid" is the *composition*, not merged weights — and it runs
//! on the distributed AI you already have.
//!
//! Two halves:
//!   - **route** (this file) — pick/compose an expert at inference time.
//!   - [`trainer`] — the GPU+CPU-friendly fine-tune **orchestrator**: we don't
//!     reimplement training; the **HuggingFace MCP** (hf.co/mcp) supplies base
//!     models + datasets and HF peft/trl does the fine-tune, while flux-moe
//!     *dispatches* each job to GPU (QLoRA) or CPU (small/quantized) by model
//!     size + available compute. Each expert above is one trainable LoRA adapter.

pub mod builder;
pub mod autotune;
pub mod cloud;
pub mod hundred;
pub mod landed;
pub mod blast;
pub mod dataset;
pub mod distill;
pub mod eval;
pub mod mandate;
pub mod judge;
pub mod pipeline;
pub mod policy;
pub mod serve;
pub mod skillroute;
pub mod goalroute;
pub use goalroute::{route_from_consensus, route_from_goal_text, GoalRoutePlan};
pub mod toolcorpus;
pub mod trainer;

use serde::Deserialize;
use std::time::Duration;

/// What kind of work a prompt is — drives which expert handles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    Code,
    Sentiment,
    Trading,
    General,
}

/// One model in the pool.
#[derive(Debug, Clone)]
pub struct Expert {
    pub name: &'static str,
    /// Ollama model tag (e.g. "qwen3:4b").
    pub model: &'static str,
    pub handles: &'static [Task],
    // ── routing metrics (for the weighted scorer) ───────────────────────────
    /// Estimated time-to-first-token, ms (lower = snappier).
    pub ttft_ms: u32,
    /// Capability 0..1 — how strong this expert is on hard prompts.
    pub capability: f32,
    /// Cost per 1k tokens (gateway units) — lower = cheaper.
    pub cost_per_1k: f32,
    /// Is the model currently loadable/serving?
    pub available: bool,
}

impl Expert {
    /// Construct with `available: true` by default.
    pub const fn new(name: &'static str, model: &'static str, handles: &'static [Task], ttft_ms: u32, capability: f32, cost_per_1k: f32) -> Self {
        Expert { name, model, handles, ttft_ms, capability, cost_per_1k, available: true }
    }
}

/// The expert pool — Qwen family + Gemma, each owning the tasks it's best at.
/// Resolve the inference endpoint, safely.
///
/// SECURITY (2026-07-27 audit finding, HIGH). Three call sites used to default to
/// `http://202.122.49.242:22938` — a *specific rented Vast.ai A100*, in cleartext
/// HTTP, hardcoded as the fallback. That instance no longer exists (the Vast
/// account reports none), so the IP has been reassigned: every prompt sent by a
/// caller that had not set `FLUX_MOE_OLLAMA` would go, unencrypted, to a machine
/// belonging to a stranger. Prompts here routinely carry source code and
/// operational context.
///
/// Now: default is loopback, and cleartext egress to a *public* address is
/// refused unless explicitly acknowledged.
///
/// * default — `http://localhost:11434` (standard Ollama port, same default
///   `flux-context::dispatch` already used).
/// * `FLUX_MOE_OLLAMA` — override. Loopback and RFC1918/private hosts are fine in
///   cleartext (the WireGuard mesh is private); `https://` anywhere is fine.
/// * a **public** host over plain `http://` returns `Err` unless
///   `FLUX_ALLOW_CLEARTEXT_LLM=1` is set, so the unsafe case is opt-in and loud
///   rather than the silent default.
pub fn ollama_endpoint() -> Result<String, String> {
    let ep = std::env::var("FLUX_MOE_OLLAMA")
        .unwrap_or_else(|_| "http://localhost:11434".to_string());
    if !ep.starts_with("http://") {
        return Ok(ep); // https (or a non-http scheme) — not our problem to police
    }
    let host = ep
        .trim_start_matches("http://")
        .split(['/', ':'])
        .next()
        .unwrap_or("")
        .to_string();
    if is_private_host(&host) {
        return Ok(ep);
    }
    if std::env::var("FLUX_ALLOW_CLEARTEXT_LLM").as_deref() == Ok("1") {
        eprintln!(
            "⚠ flux-moe: sending prompts in CLEARTEXT to public host {host} \
             (FLUX_ALLOW_CLEARTEXT_LLM=1 acknowledged)"
        );
        return Ok(ep);
    }
    Err(format!(
        "refusing cleartext http:// to public host {host} — prompts would travel unencrypted \
         to a machine you may not control. Use https://, a private/loopback address, or set \
         FLUX_ALLOW_CLEARTEXT_LLM=1 to override."
    ))
}

/// Loopback or RFC1918/CGNAT/link-local — i.e. reachable only from a network the
/// operator controls, so cleartext is acceptable there.
fn is_private_host(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".local") || host.ends_with(".internal") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => {
            let o = ip.octets();
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || (o[0] == 100 && (64..128).contains(&o[1])) // 100.64/10 CGNAT
        }
        Ok(std::net::IpAddr::V6(ip)) => {
            ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7 ULA
        }
        Err(_) => false, // a public DNS name
    }
}

#[cfg(test)]
mod endpoint_guard_tests {
    use super::*;

    #[test]
    fn private_and_loopback_hosts_are_allowed_in_cleartext() {
        for h in ["localhost", "127.0.0.1", "10.77.0.3", "192.168.1.9", "172.16.4.2", "box.local"] {
            assert!(is_private_host(h), "{h} should count as private");
        }
    }

    #[test]
    fn public_hosts_are_not_private() {
        // the exact address this audit removed, plus a public DNS name
        for h in ["202.122.49.242", "8.8.8.8", "api.deepseek.com"] {
            assert!(!is_private_host(h), "{h} must NOT count as private");
        }
    }
}

pub fn registry() -> Vec<Expert> {
    vec![
        //              name           model               handles                            ttft  cap  cost
        Expert::new("qwen-coder",   "qwen2.5-coder:7b", &[Task::Code],                       300, 0.85, 0.20),
        Expert::new("qwen-trade",   "qwen3:8b",         &[Task::Trading, Task::Sentiment],   350, 0.80, 0.25),
        Expert::new("qwen-general", "qwen3:4b",         &[Task::General],                    200, 0.60, 0.10),
        Expert::new("gemma",        "gemma3:4b",        &[Task::General, Task::Sentiment],   180, 0.55, 0.08),
        // served by whatever `FLUX_MOE_OLLAMA` points at (see `ollama_endpoint`) — strong but
        // slower/pricier, so the scorer only picks them when the task needs the capability, not
        // for the cheap/simple 80%. NOTE: this used to name a specific public A100 by IP; that
        // rented box is gone (2026-07-27 audit: the Vast account reports no instances), so no
        // address is hardcoded here any more.
        Expert::new("qwen3.6",      "qwen3.6:latest",   &[Task::General, Task::Code, Task::Trading], 400, 0.92, 0.40),
        Expert::new("deepseek-r1",  "deepseek-r1:70b",  &[Task::General, Task::Trading],     1500, 0.95, 0.60),
    ]
}

/// Keyword signals per task, in priority order (Code > Sentiment > Trading).
/// Trimmed - boundary matching in `score_hits` handles spacing, so these also
/// catch word-start inflections ("buy" -> "buying", "price" -> "prices").
const CODE_KW: &[&str] = &["fn", "rust", "compile", "crate", "cargo", "fcx", "slint", "error[", "```", "function", "struct"];
const SENTIMENT_KW: &[&str] = &["sentiment", "news", "bullish", "bearish", "headline"];
const TRADING_KW: &[&str] = &["price", "btc", "arb", "dca", "swap", "buy", "sell", "market", "kaspa", "etc"];

/// Count keyword hits that begin at a WORD BOUNDARY (prev char non-alphanumeric).
/// Kills substring false-positives ("arb" in "garbage", "etc" in "fetch") while
/// still matching word-start inflections. std-only, no regex dependency.
fn score_hits(p: &str, keywords: &[&str]) -> u32 {
    let bytes = p.as_bytes();
    let mut hits = 0u32;
    for &kw in keywords {
        if kw.is_empty() { continue; }
        let mut i = 0;
        while let Some(rel) = p[i..].find(kw) {
            let at = i + rel;
            let prev_ok = at == 0 || !bytes[at - 1].is_ascii_alphanumeric();
            if prev_ok { hits += 1; }
            i = at + kw.len();
        }
    }
    hits
}

/// Classify a prompt into a [`Task`] by SCORING keyword hits per task and taking
/// the strongest signal (priority tiebreak Code > Sentiment > Trading > General).
/// v0.2: replaces the old first-category-wins + bare-substring heuristic, which
/// misrouted "score the sentiment of this rust news" to Code (lone "rust" hit)
/// and "is this garbage?" to Trading (substring "arb").
pub fn classify(prompt: &str) -> Task {
    let p = prompt.to_lowercase();
    // Priority order; a later task must STRICTLY beat the running best to win,
    // so ties fall to the higher-priority (earlier) task.
    let ranked = [
        (Task::Code, score_hits(&p, CODE_KW)),
        (Task::Sentiment, score_hits(&p, SENTIMENT_KW)),
        (Task::Trading, score_hits(&p, TRADING_KW)),
    ];
    let mut best = (Task::General, 0u32);
    for (task, n) in ranked {
        if n > best.1 {
            best = (task, n);
        }
    }
    best.0
}

/// Pick the best expert for a task (first that handles it; falls back to general).
pub fn route(experts: &[Expert], task: Task) -> &Expert {
    experts.iter().find(|e| e.handles.contains(&task))
        .or_else(|| experts.iter().find(|e| e.handles.contains(&Task::General)))
        .unwrap_or(&experts[0])
}

/// Composite routing score (qwen3.6's weighted-multi-criteria design, hardened).
/// All three components are normalized to 0..1 so the weights mean what they say:
/// **40% latency** (snappy TTFT) · **40% capability** · **20% cost-efficiency**.
///
/// qwen3.6 proposed this but cast the composite `as i64` inside `max_by_key`, flooring every
/// sub-1.0 score to 0 → an arbitrary pick. We keep its weighting and compare the real `f64`.
pub fn score(e: &Expert) -> f64 {
    let latency = 1.0 / (1.0 + e.ttft_ms as f64 / 1000.0); // ttft 0→1.0, 1000ms→0.5
    let capability = e.capability as f64;                  // already 0..1
    let cost = 1.0 / (1.0 + e.cost_per_1k as f64);         // cheaper → higher
    0.4 * latency + 0.4 * capability + 0.2 * cost
}

/// Weighted expert selection: among **available** experts that handle `task` and fit `budget_per_1k`,
/// pick the highest [`score`]. Returns `None` if nothing qualifies (don't silently mis-route).
/// This is the production upgrade over [`route`]'s first-match — it won't overload a slow 70B on a
/// cheap simple task, and it respects the budget (the spend-discipline we learned the hard way).
pub fn route_weighted<'a>(experts: &'a [Expert], task: Task, budget_per_1k: f32) -> Option<&'a Expert> {
    experts.iter()
        .filter(|e| e.available && e.handles.contains(&task) && e.cost_per_1k <= budget_per_1k)
        .max_by(|a, b| score(a).partial_cmp(&score(b)).unwrap_or(std::cmp::Ordering::Equal))
}

/// A thinking model burns chain-of-thought *before* it answers; measured on r1 a
/// short routing answer is preceded by several times its length in reasoning. We
/// price that premium (reasoning bills as output, see [`crate::serve::cost_usd`]).
const REASONING_MULT: u64 = 4;

/// The USD price card for an expert. `deepseek-r1` is the reasoning model we price
/// precisely (it's the expensive veto); the local Qwen/Gemma experts run on our own
/// boxes, so they get a cheap nominal card — the $ in a mixed ensemble is dominated
/// by the DeepSeek call, which is exactly what budget-aware routing must see.
pub fn expert_price(e: &Expert) -> crate::serve::Price {
    if e.model.contains("deepseek-r1") {
        crate::serve::Price::DEEPSEEK_V4_FLASH
    } else {
        crate::serve::Price { input_per_1m: 0.05, cache_hit_per_1m: 0.01, output_per_1m: 0.10 }
    }
}

/// Pre-call USD estimate for one dispatch to `e`, via [`crate::serve::cost_usd`].
/// Thinking models add `REASONING_MULT × answer_tokens` of reasoning (billed as
/// output) — so r1 prices well above a same-length Qwen answer, the premium the
/// router needs to weigh. `prompt_tokens`/`answer_tokens` are the call's expected
/// shape (the caller knows roughly how big its prompt + wanted answer are).
pub fn estimate_cost_usd(e: &Expert, prompt_tokens: u64, answer_tokens: u64) -> f64 {
    let reasoning = if crate::serve::thinking_model(e.model) { answer_tokens * REASONING_MULT } else { 0 };
    let usage = crate::serve::Usage {
        prompt_tokens,
        completion_tokens: answer_tokens + reasoning,
        completion_tokens_details: crate::serve::CompletionTokensDetails { reasoning_tokens: reasoning },
        prompt_cache_hit_tokens: 0,
    };
    crate::serve::cost_usd(&usage, &expert_price(e))
}

/// [`route_weighted`] in **real dollars**: among available experts that handle `task`
/// AND whose estimated per-call cost ([`estimate_cost_usd`] for the given token shape)
/// fits `usd_budget`, pick the highest [`score`]. This is the cost_usd-wired router —
/// it knows what an r1 veto costs and drops it when the task isn't worth the price,
/// instead of using the abstract `cost_per_1k` proxy. `None` = nothing affordable
/// qualifies (never a silent over-budget mis-route).
pub fn route_weighted_usd<'a>(
    experts: &'a [Expert],
    task: Task,
    usd_budget: f64,
    prompt_tokens: u64,
    answer_tokens: u64,
) -> Option<&'a Expert> {
    experts.iter()
        .filter(|e| e.available
            && e.handles.contains(&task)
            && estimate_cost_usd(e, prompt_tokens, answer_tokens) <= usd_budget)
        .max_by(|a, b| score(a).partial_cmp(&score(b)).unwrap_or(std::cmp::Ordering::Equal))
}

#[inline]
fn task_slot(t: Task) -> usize {
    match t { Task::Code => 0, Task::Sentiment => 1, Task::Trading => 2, Task::General => 3 }
}

/// Precomputed **discriminant dispatch table** — qwen3.6's iterative improvement
/// (the flux-moe loop, #580). Instead of scanning the expert pool on every call
/// (`route`/`route_weighted` are O(n) per dispatch), precompute the best expert
/// per `Task` discriminant ONCE, then route in O(1). "Best" = highest [`score`]
/// among available experts that handle the task. Rebuild when availability changes.
#[derive(Debug, Clone)]
pub struct DispatchTable {
    by_task: [Option<usize>; 4],
}

impl DispatchTable {
    /// Build the table (O(n × tasks) once) — index into `experts` per discriminant.
    pub fn build(experts: &[Expert]) -> Self {
        let mut by_task = [None; 4];
        for t in [Task::Code, Task::Sentiment, Task::Trading, Task::General] {
            by_task[task_slot(t)] = experts.iter().enumerate()
                .filter(|(_, e)| e.available && e.handles.contains(&t))
                .max_by(|(_, a), (_, b)| score(a).partial_cmp(&score(b)).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i);
        }
        DispatchTable { by_task }
    }

    /// O(1) route: the precomputed best expert for `task`, else the General slot.
    pub fn route<'a>(&self, experts: &'a [Expert], task: Task) -> Option<&'a Expert> {
        self.by_task[task_slot(task)]
            .or(self.by_task[task_slot(Task::General)])
            .map(|i| &experts[i])
    }
}

/// Drop ensemble members whose confidence is below `threshold`, then renormalize
/// the survivors' weights to sum to 1.0 (qwen3.6 v-next proposal, the #584 loop).
/// Keeps a low-confidence member from dragging the ensemble vote. If nothing
/// meets the threshold, returns empty so the caller can fall back to single Route.
pub fn gate_low_confidence(experts: &[(String, f32)], threshold: f32) -> Vec<(String, f32)> {
    let kept: Vec<(String, f32)> = experts.iter().filter(|(_, w)| *w >= threshold).cloned().collect();
    let sum: f32 = kept.iter().map(|(_, w)| *w).sum();
    if sum <= 0.0 { return Vec::new(); }
    kept.into_iter().map(|(n, w)| (n, w / sum)).collect()
}

/// Select the experts for an [`Mode::Ensemble`] call: the **top-2 distinct** available
/// experts that handle `task` within `budget_per_1k`, ranked by [`score`] (best first,
/// then the strongest *different* cross-check). High-stakes prompts run both and a judge
/// reconciles — and the pair feeds straight into [`gate_low_confidence`]. Returns 0, 1,
/// or 2 (fewer when the pool is thin), so the caller can degrade to single [`route_weighted`].
pub fn route_ensemble<'a>(experts: &'a [Expert], task: Task, budget_per_1k: f32) -> Vec<&'a Expert> {
    let mut cands: Vec<&Expert> = experts.iter()
        .filter(|e| e.available && e.handles.contains(&task) && e.cost_per_1k <= budget_per_1k)
        .collect();
    cands.sort_by(|a, b| score(b).partial_cmp(&score(a)).unwrap_or(std::cmp::Ordering::Equal));
    cands.into_iter().take(2).collect()
}

/// How to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// One best expert (fast, cheap).
    Route,
    /// Two experts (best + a Gemma cross-check) — for high-stakes prompts.
    Ensemble,
}

#[derive(Deserialize)]
struct OllamaResp {
    response: String,
}

/// Strip a reasoning model's `<think>…</think>` block from a response (in case `think:false` isn't
/// honored by an older ollama). Returns the trimmed answer.
pub fn strip_think(s: &str) -> String {
    if let (Some(a), Some(b)) = (s.find("<think>"), s.find("</think>")) {
        if b > a {
            let mut out = String::with_capacity(s.len());
            out.push_str(&s[..a]);
            out.push_str(&s[b + "</think>".len()..]);
            return out.trim().to_string();
        }
    }
    s.trim().to_string()
}

/// Generate from one model via Ollama (`POST /api/generate`). **Heavy/thinking-model aware** — this is
/// the fix that makes a big reasoning expert like `deepseek-r1:70b` (82 GB) actually usable on the A100:
///   - `think:false` for reasoning models (qwen3 / deepseek-r1) — the `<think>` phase otherwise eats the
///     token budget and the call times out (the 1/12 → 12/12 lesson, same as the tool-call path),
///   - `keep_alive: 30m` — the model stays RESIDENT, so an 82 GB model isn't reloaded on every call
///     (the per-case reload was the A100 "thrash" that errored every request),
///   - `num_gpu: 999` — force all layers onto the GPU,
///   - a longer timeout for thinking models (600 s vs 120 s; override with `FLUX_MOE_TIMEOUT_S`),
///   - strips any leaked `<think>` from the answer.
pub fn generate(endpoint: &str, model: &str, prompt: &str) -> Result<String, String> {
    // v0.3: a non-ollama endpoint (DeepSeek API, vLLM) speaks OpenAI /chat/completions.
    if !endpoint_is_ollama(endpoint) {
        return generate_openai(endpoint, model, prompt);
    }
    let thinking = crate::serve::thinking_model(model);
    let secs: u64 = std::env::var("FLUX_MOE_TIMEOUT_S").ok().and_then(|s| s.parse().ok())
        .unwrap_or(if thinking { 600 } else { 120 });
    let body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        "think": !thinking,            // disable the reasoning phase for thinking models
        "keep_alive": "30m",           // stay warm — no 82 GB reload per call (the A100 thrash fix)
        // num_ctx bounded: short router prompts don't need a huge KV cache, and a 70B's KV at 8k ctx
        // is what pushed it to 82 GB (couldn't co-reside with qwen). 4k keeps the footprint down.
        "options": { "temperature": 0, "num_gpu": 999, "num_ctx": 4096 }
    });
    let r: OllamaResp = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(secs))
        .build()
        .map_err(|e| e.to_string())?
        .post(format!("{endpoint}/api/generate"))
        .json(&body)
        .send()
        .map_err(|e| format!("connect {endpoint}: {e}"))?
        .error_for_status()
        .map_err(|e| format!("http: {e}"))?
        .json()
        .map_err(|e| format!("decode: {e}"))?;
    Ok(strip_think(&r.response))
}

/// Streaming sibling of [`generate`]: invoke `on_token` for each chunk as ollama emits it, so a
/// caller (e.g. an SSE endpoint) can forward tokens live instead of blocking for the whole answer.
/// Same think-aware discipline (`think:false` for thinking models). Ollama only. Returns the full text.
pub fn generate_stream<F: FnMut(&str)>(endpoint: &str, model: &str, prompt: &str, mut on_token: F) -> Result<String, String> {
    use std::io::BufRead;
    let thinking = crate::serve::thinking_model(model);
    let secs: u64 = std::env::var("FLUX_MOE_TIMEOUT_S").ok().and_then(|s| s.parse().ok())
        .unwrap_or(if thinking { 600 } else { 120 });
    let body = serde_json::json!({
        "model": model, "prompt": prompt, "stream": true,
        "think": !thinking, "keep_alive": "30m",
        "options": { "temperature": 0, "num_gpu": 999, "num_ctx": 4096 }
    });
    let resp = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(secs))
        .build().map_err(|e| e.to_string())?
        .post(format!("{endpoint}/api/generate"))
        .json(&body)
        .send().map_err(|e| format!("connect {endpoint}: {e}"))?
        .error_for_status().map_err(|e| format!("http: {e}"))?;
    let reader = std::io::BufReader::new(resp);
    let mut full = String::new();
    for line in reader.lines() {
        let line = match line { Ok(l) => l, Err(_) => break };
        if line.trim().is_empty() { continue; }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            if let Some(tok) = v.get("response").and_then(|x| x.as_str()) {
                if !tok.is_empty() { full.push_str(tok); on_token(tok); }
            }
            if v.get("done").and_then(|x| x.as_bool()) == Some(true) { break; }
        }
    }
    Ok(full)
}

#[derive(Deserialize)]
struct PsResp { #[serde(default)] models: Vec<PsModel> }
#[derive(Deserialize)]
struct PsModel { name: String }

/// Free the GPU for a heavy expert: unload every currently-resident model EXCEPT `keep_model`.
/// Ollama unloads a model when sent a request with `keep_alive:0`. Returns how many were unloaded.
///
/// Needed because a 70B (deepseek-r1) can't co-reside with qwen3.6 on a single 80GB A100 — the
/// co-load fills VRAM and the box drops `/api/chat` mid-load. Call this before driving a heavy model
/// so it loads SOLO (serve.rs's measured "give it the GPU alone").
pub fn unload_others(endpoint: &str, keep_model: &str) -> Result<usize, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30)).build().map_err(|e| e.to_string())?;
    let ps: PsResp = client.get(format!("{endpoint}/api/ps"))
        .send().map_err(|e| format!("ps: {e}"))?
        .json().map_err(|e| format!("ps decode: {e}"))?;
    let mut n = 0;
    for m in ps.models {
        if m.name == keep_model { continue; }
        // keep_alive:0 + empty prompt → ollama evicts the model from VRAM
        let body = serde_json::json!({ "model": m.name, "keep_alive": 0, "prompt": "" });
        let _ = client.post(format!("{endpoint}/api/generate")).json(&body).send();
        n += 1;
    }
    Ok(n)
}

/// Tool-call response shape from ollama `/api/chat`.
#[derive(Deserialize)]
struct ChatToolResp { message: ChatMsg }
#[derive(Deserialize)]
struct ChatMsg {
    #[serde(default)] tool_calls: Vec<ToolCall>,
    #[serde(default)] content: String,
}
#[derive(Deserialize)]
struct ToolCall { function: ToolFn }
#[derive(Deserialize)]
struct ToolFn { name: String, arguments: serde_json::Value }

// ── OpenAI /v1/chat/completions shape (vLLM) — note: `arguments` is a JSON *string*. ──
#[derive(Deserialize)]
struct OpenAiResp { #[serde(default)] choices: Vec<OpenAiChoice> }
#[derive(Deserialize)]
struct OpenAiChoice { message: OpenAiMsg }
#[derive(Deserialize)]
struct OpenAiMsg {
    #[serde(default)] tool_calls: Vec<OpenAiTC>,
    #[serde(default)] content: Option<String>,
}
#[derive(Deserialize)]
struct OpenAiTC { function: OpenAiFn }
#[derive(Deserialize)]
struct OpenAiFn { name: String, #[serde(default)] arguments: String }

/// Build the chat-completions URL for an OpenAI-style endpoint (DeepSeek API, vLLM).
/// `https://api.deepseek.com` -> `.../chat/completions`; a `.../v1` base keeps its /v1.
pub fn openai_chat_url(endpoint: &str) -> String {
    format!("{}/chat/completions", endpoint.trim_end_matches('/'))
}

/// Generate from an OpenAI-compatible endpoint (DeepSeek API, vLLM) via POST
/// /chat/completions. The Bearer key comes from `DEEPSEEK_API_KEY` (fallback
/// `FLUX_MOE_API_KEY`) - ollama needs no auth, this transport does. DeepSeek V4
/// returns chain-of-thought in `reasoning_content` and the answer in `content`;
/// we take `content` (then strip_think for any leaked markup). This is what lets
/// the two_mind VETOER run on the DeepSeek API with NO GPU box.
pub fn generate_openai(endpoint: &str, model: &str, prompt: &str) -> Result<String, String> {
    let key = std::env::var("DEEPSEEK_API_KEY")
        .or_else(|_| std::env::var("FLUX_MOE_API_KEY"))
        .map_err(|_| "OpenAI endpoint needs DEEPSEEK_API_KEY (or FLUX_MOE_API_KEY)".to_string())?;
    let secs: u64 = std::env::var("FLUX_MOE_TIMEOUT_S").ok().and_then(|s| s.parse().ok()).unwrap_or(180);
    let body = serde_json::json!({
        "model": model,
        "messages": [{ "role": "user", "content": prompt }],
        "stream": false,
        "max_tokens": 1024,
        "temperature": 0
    });
    let r: OpenAiResp = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(secs))
        .build().map_err(|e| e.to_string())?
        .post(openai_chat_url(endpoint))
        .bearer_auth(key)
        .json(&body)
        .send().map_err(|e| format!("connect {endpoint}: {e}"))?
        .error_for_status().map_err(|e| format!("http: {e}"))?
        .json().map_err(|e| format!("decode: {e}"))?;
    let content = r.choices.into_iter().next()
        .and_then(|c| c.message.content)
        .ok_or_else(|| "openai: no choices/content in response".to_string())?;
    Ok(strip_think(&content))
}

/// Detect the transport: an **ollama** server answers `GET /api/tags`; a **vLLM**/OpenAI
/// server does not (→ route to `/v1/chat/completions`). ~3 s probe, no caching.
pub fn endpoint_is_ollama(endpoint: &str) -> bool {
    // explicit override (set FLUX_MOE_VLLM=1 for a vLLM endpoint)
    if matches!(std::env::var("FLUX_MOE_VLLM").as_deref(), Ok("1") | Ok("true")) {
        return false;
    }
    // a `/v1` base URL is the vLLM / OpenAI shape; everything else is ollama-style
    if endpoint.trim_end_matches('/').ends_with("/v1") {
        return false;
    }
    // Probe /api/tags with a GENEROUS timeout — a box busy loading a 70B is slow but still ollama.
    // OLLAMA-FIRST: an inconclusive probe defaults to ollama (`/api/chat`), NOT /v1. The old 3s
    // timeout + `unwrap_or(false)` mis-sent a busy ollama box to /v1/chat/completions → the bug that
    // broke the live two-mind chain mid-load.
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .build().ok()
        .and_then(|c| c.get(format!("{endpoint}/api/tags")).send().ok())
        .map(|r| r.status().is_success())
        .unwrap_or(true)
}

/// Drive a single-turn **TOOL CALL** on an Ollama model — the agentic primitive
/// that lets a flux-moe expert actually *act* (call flux_combo, propose work, …).
/// Builds the request via [`crate::serve::ollama_toolcall_body`] (which sets
/// `think:false` for thinking models — the 2026-06-03 A100 fix that took tool
/// emission from 1/12 → 12/12) and POSTs to `/api/chat`. Returns
/// `(tool_name, arguments_json)` on a tool call, or `Err` carrying the plain
/// content when the model answered in prose instead of calling a tool.
pub fn tool_call(endpoint: &str, model: &str, system: &str, user: &str, tools_json: &str)
    -> Result<(String, serde_json::Value), String>
{
    // Transport split: ollama (`/api/chat` + think:false) vs vLLM/OpenAI (`/v1/chat/completions`
    // + tool_choice:auto). vLLM is the playbook §7 serving for the concurrent combo loop.
    let openai = !endpoint_is_ollama(endpoint);
    let (path, body) = if openai {
        (crate::serve::openai_toolcall_path(), crate::serve::openai_toolcall_body(model, system, user, tools_json))
    } else {
        (crate::serve::ollama_toolcall_path(), crate::serve::ollama_toolcall_body(model, system, user, tools_json))
    };
    let url = format!("{endpoint}{path}");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(240))
        .build().map_err(|e| e.to_string())?;
    // The live A100 endpoint is SHARED + memory-packed — it drops requests mid-flight under
    // load (2026-06-03 contention). Retry transient send/http errors before failing.
    let mut last = String::new();
    let mut raw: Option<String> = None;
    for attempt in 0..3 {
        match client.post(&url).header("content-type", "application/json").body(body.clone()).send() {
            Ok(r) => match r.error_for_status() {
                Ok(r) => match r.text() {
                    Ok(t) => { raw = Some(t); break; }
                    Err(e) => last = format!("read: {e}"),
                },
                Err(e) => last = format!("http: {e}"),
            },
            Err(e) => last = format!("connect {endpoint}: {e}"),
        }
        std::thread::sleep(Duration::from_millis(800 * (attempt + 1)));
    }
    let raw = raw.ok_or_else(|| format!("tool_call failed after 3 tries: {last}"))?;
    // Parse per-transport. ollama: arguments is a JSON object. OpenAI/vLLM: arguments is a
    // JSON *string* to re-parse. Content-recovery is the shared fallback (thinking models
    // often leave tool_calls empty and emit the call inside `content`).
    let (tc, content) = if openai {
        let r: OpenAiResp = serde_json::from_str(&raw).map_err(|e| format!("decode openai: {e}"))?;
        match r.choices.into_iter().next().map(|c| c.message) {
            Some(m) => {
                let tc = m.tool_calls.into_iter().next().and_then(|t|
                    serde_json::from_str::<serde_json::Value>(&t.function.arguments).ok()
                        .map(|args| (t.function.name, args)));
                (tc, m.content.unwrap_or_default())
            }
            None => (None, String::new()),
        }
    } else {
        let r: ChatToolResp = serde_json::from_str(&raw).map_err(|e| format!("decode ollama: {e}"))?;
        let tc = r.message.tool_calls.into_iter().next().map(|t| (t.function.name, t.function.arguments));
        (tc, r.message.content)
    };
    if let Some(t) = tc { return Ok(t); }
    parse_toolcall_from_content(&content)
        .ok_or_else(|| format!("no tool call; model said: {content}"))
}

/// Recover a tool call from free-text content (strips a leading `<think>…</think>`, then
/// scans for the first balanced `{…}` JSON object and interprets it as a call).
pub fn parse_toolcall_from_content(content: &str) -> Option<(String, serde_json::Value)> {
    let c = content.rsplit("</think>").next().unwrap_or(content);
    let bytes = c.as_bytes();
    let mut depth = 0i32;
    let mut start: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'{' {
            if depth == 0 { start = Some(i); }
            depth += 1;
        } else if b == b'}' {
            depth -= 1;
            if depth == 0 {
                if let Some(s) = start {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&c[s..=i]) {
                        if let Some(t) = interpret_call(&v) { return Some(t); }
                    }
                }
            }
        }
    }
    None
}

/// Interpret a JSON value as a tool call across the shapes ollama models emit.
fn interpret_call(v: &serde_json::Value) -> Option<(String, serde_json::Value)> {
    // {"function":{"name":..,"arguments":..}}
    if let Some(f) = v.get("function") {
        if let Some(n) = f.get("name").and_then(|x| x.as_str()) {
            return Some((n.to_string(), f.get("arguments").cloned().unwrap_or_else(|| serde_json::json!({}))));
        }
    }
    // {"name":..,"arguments"|"parameters":..}
    if let Some(n) = v.get("name").and_then(|x| x.as_str()) {
        let args = v.get("arguments").or_else(|| v.get("parameters")).cloned().unwrap_or_else(|| serde_json::json!({}));
        return Some((n.to_string(), args));
    }
    // {"tool":"flux_combo","package":".."} → name + the remaining fields as args
    if let Some(n) = v.get("tool").and_then(|x| x.as_str()) {
        let mut args = v.clone();
        if let Some(o) = args.as_object_mut() { o.remove("tool"); }
        return Some((n.to_string(), args));
    }
    None
}

/// The outcome of an Ensemble judge call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The primary (specialist) expert's answer wins.
    ExpertA,
    /// The cross-check (generalist) expert's answer wins.
    ExpertB,
    /// Indecision or unparseable - keep the specialist (safe default).
    Tie,
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Verdict::ExpertA => "A",
            Verdict::ExpertB => "B",
            Verdict::Tie => "Tie",
        })
    }
}

/// Parse a judge model's free-text reply into a deterministic [`Verdict`].
/// Mirrors [`parse_toolcall_from_content`]: pure, no I/O, fully unit-testable.
/// Only EXPLICIT signals decide (the rubric asks for "Winner: A/B"); anything
/// ambiguous or contradictory degrades to Tie, so we never discard the
/// specialist's answer on a guess - no fragile adjective-counting.
pub fn parse_judge_verdict(reply: &str) -> Verdict {
    let s = reply.to_lowercase();
    if s.contains("winner: a") || s.contains("winner is a") {
        return Verdict::ExpertA;
    }
    if s.contains("winner: b") || s.contains("winner is b") {
        return Verdict::ExpertB;
    }
    let a = s.contains("a is better") || s.contains("a wins") || s.contains("prefer a") || s.contains("choose a");
    let b = s.contains("b is better") || s.contains("b wins") || s.contains("prefer b") || s.contains("choose b");
    match (a, b) {
        (true, false) => Verdict::ExpertA,
        (false, true) => Verdict::ExpertB,
        _ => Verdict::Tie,
    }
}

/// Call a neutral judge model to decide between two answers (LLM-as-judge).
/// Gemma (a different family) judges for genuine independence, falling back to
/// the first registered expert. Stochastic reply -> deterministic [`Verdict`].
pub fn judge(endpoint: &str, prompt: &str, a: &str, b: &str) -> Result<Verdict, String> {
    let rubric = format!(
        "You are a strict technical judge. Compare two answers to the user's prompt.\n\
         Prompt: {prompt}\n\n\
         Answer A (specialist):\n{a}\n\n\
         Answer B (cross-check):\n{b}\n\n\
         Which is more accurate, helpful and safe? Reply exactly 'Winner: A' or \
         'Winner: B'. If truly equal, reply 'Tie'."
    );
    let experts = registry();
    let judge_expert = experts.iter().find(|e| e.name == "gemma")
        .or_else(|| experts.first())
        .ok_or("no judge expert in registry")?;
    let reply = generate(endpoint, judge_expert.model, &rubric)?;
    Ok(parse_judge_verdict(&reply))
}

/// THE COMBO: classify -> route -> generate. In Ensemble mode a neutral judge
/// VOTES between the specialist and a Gemma cross-check; the winner leads and the
/// loser stays visible for audit. v0.2: real voting, not just both labeled.
pub fn flux_llm(endpoint: &str, prompt: &str, mode: Mode) -> Result<String, String> {
    let experts = registry();
    let task = classify(prompt);
    let primary = route(&experts, task);
    let main = generate(endpoint, primary.model, prompt)?;
    if mode == Mode::Route {
        return Ok(format!("[{} - {:?}]\n{main}", primary.name, task));
    }
    let gemma = experts.iter().find(|e| e.name == "gemma")
        .ok_or("gemma expert not found in registry")?;
    let alt = generate(endpoint, gemma.model, prompt)
        .unwrap_or_else(|e| format!("(gemma unavailable: {e})"));
    // A judge failure must not sink the call - degrade to Tie (specialist leads).
    let verdict = judge(endpoint, prompt, &main, &alt).unwrap_or(Verdict::Tie);
    let (win_name, win, lose_name, lose) = match verdict {
        Verdict::ExpertB => (gemma.name, &alt, primary.name, &main),
        _ => (primary.name, &main, gemma.name, &alt),
    };
    let why = match verdict {
        Verdict::ExpertA => "specialist domain knowledge preferred",
        Verdict::ExpertB => "cross-check was more accurate/safe",
        Verdict::Tie => "comparable; defaulting to the specialist",
    };
    Ok(format!(
        "[ENSEMBLE - {task:?} - JUDGE: {verdict} - {why}]\n\
         --- winner ({win_name}) ---\n{win}\n\n\
         --- cross-check ({lose_name}) ---\n{lose}"
    ))
}

// ───────────────────────── TWO-MIND — live agentic-money safety gate ─────────────────────────
// The live-model realization of the `sigil-twomind` Gate (pure-logic sibling crate). Roles, proven on
// the A100 this session: qwen3.6 is the PROPOSER (emits native tool calls — flux_combo + 7/7 money
// eval), deepseek-r1:70b is the VETOER (reasons correctly but answers in PROSE, so it judges, it
// doesn't drive tools). The gate applies the money policy: real money ALWAYS needs a human, even on a
// clean 2-of-2; governance is OK on 2-of-2; a veto blocks everything.

/// The vetoer's verdict on a proposed tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateVerdict {
    Approve,
    Veto(String),
}

/// Parse a prose judge's reply (deepseek rambles) into APPROVE/VETO. VETO wins ambiguity — the
/// judgment lane is conservative by default (an unclear verdict is a veto, never a guessed approve).
pub fn parse_gate_verdict(reply: &str) -> GateVerdict {
    let u = reply.to_uppercase();
    let line = reply.lines().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("").to_string();
    if u.contains("APPROVE") && !u.contains("VETO") {
        GateVerdict::Approve
    } else if u.contains("VETO") {
        GateVerdict::Veto(if line.is_empty() { "vetoed".into() } else { line.chars().take(160).collect() })
    } else {
        GateVerdict::Veto(format!("ambiguous (safety default): {}", line.chars().take(120).collect::<String>()))
    }
}

/// Drive the PROSE vetoer (deepseek-r1:70b) to APPROVE or VETO a proposed tool call.
pub fn judge_proposal(endpoint: &str, model: &str, user_request: &str, tool: &str, args: &serde_json::Value) -> Result<GateVerdict, String> {
    // CORRECTNESS-ONLY vetoer. The money policy (deny-list + real-money-needs-a-human) is a SEPARATE
    // deterministic stage (`policy::gate_decision`), so the LLM must NOT also veto a well-formed money
    // action just because it moves funds — that double-count is what dropped measured precision to 0.78
    // (it false-vetoed a clean dex_swap + a read-only get_balance). Judging only correctness keeps recall
    // 1.0 (still catches wrong-tool / bad-args) while lifting precision toward 1.0.
    let prompt = format!(
        "You are a precise CORRECTNESS AUDITOR in a two-mind safety gate. A proposer chose the tool \
         `{tool}` with arguments {args} to satisfy the request: \"{user_request}\". Judge ONLY whether \
         this is the correct, well-formed tool call — do NOT judge money policy (a separate stage already \
         forces a human on every real-money move, so never veto a correct action merely because it \
         transfers funds). Reply on ONE line STARTING WITH exactly APPROVE or VETO, then one short reason. \
         VETO only if: the tool is WRONG for the request, a required argument is MISSING/empty, the \
         recipient or address is not a valid identifier, or the amount/target is underspecified or nonsensical. \
         APPROVE a correct, fully-specified call even when it moves money."
    );
    // Route by transport so the vetoer can be EITHER a local ollama model (qwen3.6 on
    // Epsilon) OR the DeepSeek cloud API (deepseek-4.2-flash, no GPU box) — the
    // production two-mind split: cheap local proposer + reliable API judge.
    let reply = if endpoint_is_ollama(endpoint) {
        generate(endpoint, model, &prompt)?
    } else {
        generate_openai(endpoint, model, &prompt)?
    };
    Ok(parse_gate_verdict(&reply))
}

/// Best-effort numeric amount from a proposed call's args, for the policy auto-cap.
/// Scans the common money fields; absent/unparseable → 0.0 (read-only / local write). Pure.
pub fn extract_amount(args: &serde_json::Value) -> f64 {
    for k in ["amount", "amount_in", "value", "qty", "size"] {
        if let Some(v) = args.get(k) {
            if let Some(n) = v.as_f64() { return n; }
            if let Some(s) = v.as_str() { if let Ok(n) = s.parse::<f64>() { return n; } }
        }
    }
    0.0
}

/// The RAW (unparsed) amount field the proposer emitted, if present — string form so
/// [`policy::check_amount`] can reject unbounded/junk values (`"all"`/`"max"`/`""`/…). `None`
/// means the tool carries no scalar amount field (e.g. `ln_pay`'s invoice), so the amount-sanity
/// check is skipped (no spurious empty-amount veto). Distinct from [`extract_amount`], which parses
/// to `f64` for the auto-cap and so silently turns `"all"` into `0.0`.
pub fn raw_amount_field(args: &serde_json::Value) -> Option<String> {
    for k in ["amount", "amount_in", "value", "qty", "size"] {
        if let Some(v) = args.get(k) {
            return Some(match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            });
        }
    }
    None
}

/// PURE composition: bridge the LLM vetoer's [`GateVerdict`] into the money policy and return the
/// final [`policy::Decision`]. Separated from the network so it's unit-testable: correctness-veto +
/// money-class human-gate, no double-counting. (Vetoer judges correctness; policy owns money.)
///
/// When the proposer emitted an amount field, routes through the STRING-AWARE
/// [`policy::gate_decision_checked`] so an unbounded/junk amount (the "send everything", `amount="all"`
/// blind spot the 24-case eval caught the LLM vetoer APPROVING) is hard-VETOED before signing —
/// `extract_amount` would have parsed `"all"` to `0.0` and merely escalated to NeedHuman. When no
/// amount field is present (e.g. `ln_pay`/invoice), falls back to the numeric gate.
pub fn compose_decision(policy: &crate::policy::Policy, tool: &str, args: &serde_json::Value, verdict: &GateVerdict) -> crate::policy::Decision {
    let vv = match verdict {
        GateVerdict::Approve => crate::policy::VetoerVerdict::Approve,
        GateVerdict::Veto(r) => crate::policy::VetoerVerdict::Veto(r.clone()),
    };
    match raw_amount_field(args) {
        Some(raw) => crate::policy::gate_decision_checked(policy, tool, &raw, &vv),
        None => crate::policy::gate_decision(policy, tool, extract_amount(args), &vv),
    }
}

/// THE TWO-MIND MONEY KEYSTONE — one auditable call: PROPOSE → VETO(correctness) → POLICY(money).
/// The proposer (qwen3.6 on Epsilon ollama) emits the native tool call; the vetoer (deepseek-v4-flash
/// on the DeepSeek API) judges CORRECTNESS only; [`compose_decision`] applies the deterministic
/// money policy (deny-list + real-money-needs-a-human). Endpoints route by transport automatically,
/// so the cheap local proposer and the reliable cloud judge are just two URLs. Returns the proposed
/// `(tool, args)` and the final [`policy::Decision`].
pub fn decide(
    proposer_endpoint: &str, proposer_model: &str,
    vetoer_endpoint: &str, vetoer_model: &str,
    system: &str, user: &str, tools_json: &str,
    policy: &crate::policy::Policy,
) -> Result<(String, serde_json::Value, crate::policy::Decision), String> {
    let (tool, args) = tool_call(proposer_endpoint, proposer_model, system, user, tools_json)?;
    let verdict = judge_proposal(vetoer_endpoint, vetoer_model, user, &tool, &args)?;
    let decision = compose_decision(policy, &tool, &args, &verdict);
    Ok((tool, args, decision))
}

/// Money risk of a tool (the deny-list discipline). Mirrors `sigil_twomind::classify`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoneyClass { ReadOnly, Governance, RealMoney }

pub fn classify_tool(tool: &str) -> MoneyClass {
    const REAL: &[&str] = &["send_qug", "send_token", "btc_withdraw", "dex_swap", "add_liquidity",
        "bank_apply_for_loan", "bank_payback_loan", "rwa_buy", "rwa_confirm", "ln_pay", "qshare_buyback",
        // hardened (closing toolcorpus::LIB_CLASSIFY_GAPS — money movers that auto-executed without a human):
        "dex_quickstart_trade", "execute_strategy", "broadcast_to_mainnet", "qshare_mint",
        "qshare_bootstrap_pool", "deploy_token", "deploy_smart_contract", "rwa_offer"];
    const GOV: &[&str] = &["agent_submit", "council_consensus", "collect_dev_fee", "gov_payout",
        "agent_submit_batch", "agent_create_mandate"];
    if REAL.contains(&tool) { MoneyClass::RealMoney }
    else if GOV.contains(&tool) { MoneyClass::Governance }
    else { MoneyClass::ReadOnly }
}

/// The auditable two-mind decision.
#[derive(Debug, Clone)]
pub struct GateDecision {
    pub execute: bool,
    pub requires_human: bool,
    pub class: MoneyClass,
    pub tool: String,
    pub signers: Vec<String>,
    pub reason: String,
}

/// Apply the two-mind gate: 2-of-2 proposer+vetoer, money-classed. Real money ALWAYS needs a human.
pub fn gate(proposer: &str, vetoer: &str, tool: &str, verdict: &GateVerdict) -> GateDecision {
    let class = classify_tool(tool);
    match verdict {
        GateVerdict::Veto(why) => GateDecision {
            execute: false, requires_human: false, class, tool: tool.into(),
            signers: vec![proposer.into()], reason: format!("VETOED by {vetoer}: {why}"),
        },
        GateVerdict::Approve => {
            let two = vec![proposer.to_string(), vetoer.to_string()];
            match class {
                MoneyClass::ReadOnly => GateDecision { execute: true, requires_human: false, class, tool: tool.into(), signers: two, reason: "read-only · fast-track (2-of-2)".into() },
                MoneyClass::Governance => GateDecision { execute: true, requires_human: false, class, tool: tool.into(), signers: two, reason: "governance money · 2-of-2 PASS".into() },
                MoneyClass::RealMoney => GateDecision { execute: false, requires_human: true, class, tool: tool.into(), signers: two, reason: "real money · 2-of-2 ok but HUMAN required".into() },
            }
        }
    }
}

/// The full TWO-MIND loop on one endpoint: PROPOSE (qwen tool_call) → unload the proposer (the heavy
/// vetoer can't co-reside on one A100) → JUDGE (deepseek prose verdict) → GATE (money policy).
pub fn two_mind(endpoint: &str, proposer_model: &str, vetoer_model: &str, system: &str, user: &str, tools_json: &str) -> Result<GateDecision, String> {
    let (tool, args) = tool_call(endpoint, proposer_model, system, user, tools_json)?;
    let _ = unload_others(endpoint, vetoer_model);            // free the GPU for the 70B
    std::thread::sleep(Duration::from_secs(3));
    let verdict = judge_proposal(endpoint, vetoer_model, user, &tool, &args)?;
    Ok(gate(proposer_model, vetoer_model, &tool, &verdict))
}

/// TWO-MIND, **two-box shape** — the robust production wiring. Proposer and vetoer live on SEPARATE
/// endpoints, each model resident on its own GPU: NO unload, NO churn, so the single-box crash (one
/// A100 dying under propose→unload→load-70B) can't happen. PROPOSE on box A → JUDGE on box B → GATE.
/// (Matches the measured "the 70B needs the GPU alone" finding — give it its own box.)
pub fn two_mind_split(
    proposer_endpoint: &str, proposer_model: &str,
    vetoer_endpoint: &str, vetoer_model: &str,
    system: &str, user: &str, tools_json: &str,
) -> Result<GateDecision, String> {
    let (tool, args) = tool_call(proposer_endpoint, proposer_model, system, user, tools_json)?;
    let verdict = judge_proposal(vetoer_endpoint, vetoer_model, user, &tool, &args)?;
    Ok(gate(proposer_model, vetoer_model, &tool, &verdict))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_amount_reads_money_fields() {
        assert_eq!(extract_amount(&json!({"to":"x","amount":"50"})), 50.0);
        assert_eq!(extract_amount(&json!({"amount_in":120})), 120.0);
        assert_eq!(extract_amount(&json!({})), 0.0);                 // read-only → 0
        assert_eq!(extract_amount(&json!({"package":"fluxc-core"})), 0.0);
    }

    #[test]
    fn compose_real_money_approve_needs_human() {
        // correctness-APPROVE on a money tool → policy still forces a human (no double-veto)
        let p = crate::policy::Policy::default();
        let d = compose_decision(&p, "send_qug", &json!({"to":"qnk7","amount":"50"}), &GateVerdict::Approve);
        assert!(d.needs_human(), "real money + approve → NEED_HUMAN, got {d:?}");
    }

    #[test]
    fn compose_readonly_approve_allows() {
        let p = crate::policy::Policy::default();
        let d = compose_decision(&p, "get_balance", &json!({}), &GateVerdict::Approve);
        assert!(d.is_allow(), "read-only + approve → ALLOW, got {d:?}");
    }

    #[test]
    fn compose_veto_blocks_even_readonly() {
        // a correctness veto blocks regardless of money class
        let p = crate::policy::Policy::default();
        let d = compose_decision(&p, "get_balance", &json!({}), &GateVerdict::Veto("wrong tool".into()));
        assert!(d.is_veto(), "vetoer veto → VETO, got {d:?}");
    }

    #[test]
    fn compose_hard_vetoes_unbounded_amount_through_the_live_seam() {
        // The exact case deepseek-v4-flash APPROVED on the 24-case eval: "send everything", amount="all".
        // Now the live seam (compose_decision) routes amount-bearing money tools through the checked
        // gate, so it HARD-VETOES — not merely NeedHuman — even on a correctness-APPROVE.
        let p = crate::policy::Policy::default();
        let d = compose_decision(&p, "send_qug", &json!({"to":"qnkdeadbeef","amount":"all"}), &GateVerdict::Approve);
        assert!(d.is_veto(), "send_qug amount=all must be hard-vetoed via the live seam, got {d:?}");
        // a concrete amount still resolves to NeedHuman (unchanged behavior)
        let ok = compose_decision(&p, "send_qug", &json!({"to":"qnk7","amount":"50"}), &GateVerdict::Approve);
        assert!(ok.needs_human());
        // a money tool with NO amount field (ln_pay/invoice) is NOT spuriously empty-amount-vetoed
        let ln = compose_decision(&p, "ln_pay", &json!({"invoice":"lnbc2500n1pjabc..."}), &GateVerdict::Approve);
        assert!(ln.needs_human(), "ln_pay has no amount field → normal money policy (NeedHuman), got {ln:?}");
    }

    #[test]
    fn tool_call_parses_ollama_chat_response() {
        // the shape ollama /api/chat returns for a tool-calling model
        let sample = r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"flux_combo","arguments":{"package":"fluxc-core"}}}]}}"#;
        let r: ChatToolResp = serde_json::from_str(sample).unwrap();
        let tc = r.message.tool_calls.into_iter().next().unwrap();
        assert_eq!(tc.function.name, "flux_combo");
        assert_eq!(tc.function.arguments["package"], "fluxc-core");
    }

    #[test]
    fn recovers_tool_call_from_content_all_shapes() {
        // ollama left tool_calls empty and put the call in content — 3 shapes models emit.
        let a = parse_toolcall_from_content("<think>let me build core</think> {\"function\":{\"name\":\"flux_combo\",\"arguments\":{\"package\":\"fluxc-core\"}}}").unwrap();
        assert_eq!(a.0, "flux_combo");
        assert_eq!(a.1["package"], "fluxc-core");
        let b = parse_toolcall_from_content("sure: {\"name\":\"flux_iterate\",\"arguments\":{\"package\":\"flux-moe\"}}").unwrap();
        assert_eq!(b.0, "flux_iterate");
        assert_eq!(b.1["package"], "flux-moe");
        let c = parse_toolcall_from_content("call {\"tool\":\"flux_health_report\",\"package\":\"flux-graph\"} now").unwrap();
        assert_eq!(c.0, "flux_health_report");
        assert_eq!(c.1["package"], "flux-graph");
        assert!(parse_toolcall_from_content("no json here at all").is_none());
    }

    #[test]
    fn tool_call_response_without_tool_is_handled() {
        let prose = r#"{"message":{"role":"assistant","content":"I would build fluxc-core."}}"#;
        let r: ChatToolResp = serde_json::from_str(prose).unwrap();
        assert!(r.message.tool_calls.is_empty());
        assert!(r.message.content.contains("fluxc-core"));
    }

    #[test]
    fn classifies_tasks() {
        assert_eq!(classify("write a rust fn to fold the chain"), Task::Code);
        assert_eq!(classify("what's the BTC price and is there an arb?"), Task::Trading);
        assert_eq!(classify("score the sentiment of these bitcoin headlines"), Task::Sentiment);
        assert_eq!(classify("explain the SIGIL economy"), Task::General);
    }

    #[test]
    fn routes_to_the_right_expert() {
        let e = registry();
        assert_eq!(route(&e, Task::Code).name, "qwen-coder");
        assert_eq!(route(&e, Task::Trading).name, "qwen-trade");
        assert_eq!(route(&e, Task::General).name, "qwen-general");
    }

    #[test]
    fn unknown_task_falls_back_to_general() {
        // a pool with no Code expert still routes Code → a General expert.
        let pool = vec![Expert::new("g", "gemma3:4b", &[Task::General], 180, 0.55, 0.08)];
        assert_eq!(route(&pool, Task::Code).name, "g");
    }

    #[test]
    fn dispatch_table_is_o1_and_matches_weighted_pick() {
        // qwen3.6's improvement: the precomputed table must pick the SAME expert the
        // O(n) weighted scorer would — just in O(1) — and fall back to General.
        let e = registry();
        let t = DispatchTable::build(&e);
        for task in [Task::Code, Task::Sentiment, Task::Trading, Task::General] {
            let want = e.iter()
                .filter(|x| x.available && x.handles.contains(&task))
                .max_by(|a, b| score(a).partial_cmp(&score(b)).unwrap());
            assert_eq!(t.route(&e, task).map(|x| x.name), want.map(|x| x.name), "task {task:?}");
        }
        // Pool with only a General expert → any task routes to it via the fallback slot.
        let pool = vec![Expert::new("g", "gemma3:4b", &[Task::General], 180, 0.55, 0.08)];
        let tp = DispatchTable::build(&pool);
        assert_eq!(tp.route(&pool, Task::Code).unwrap().name, "g");
    }

    #[test]
    fn gate_low_confidence_drops_and_renormalizes() {
        // qwen3.6 v-next: drop sub-threshold members, renormalize survivors to 1.0.
        // (qwen's own test indexed [1] after dropping "b" — off-by-one; corrected here.)
        let g = gate_low_confidence(&[("a".into(), 0.9), ("b".into(), 0.3)], 0.5);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].0, "a");
        assert!((g[0].1 - 1.0).abs() < 1e-9);          // lone survivor → weight 1.0
        // two survivors renormalize to sum 1.0
        let g2 = gate_low_confidence(&[("a".into(), 0.6), ("b".into(), 0.6)], 0.5);
        let sum: f32 = g2.iter().map(|(_, w)| w).sum();
        assert!((sum - 1.0).abs() < 1e-6);
        // nothing qualifies → empty (caller falls back to single Route)
        assert!(gate_low_confidence(&[("a".into(), 0.1)], 0.5).is_empty());
    }

    #[test]
    fn route_ensemble_picks_top2_distinct_by_score() {
        let e = registry();
        let pair = route_ensemble(&e, Task::Trading, 1.0);
        assert_eq!(pair.len(), 2, "two experts for an ensemble");
        assert_ne!(pair[0].name, pair[1].name, "distinct experts");
        assert!(score(pair[0]) >= score(pair[1]), "ranked best-first");
        // the ensemble's lead expert is the same one route_weighted would pick solo.
        assert_eq!(pair[0].name, route_weighted(&e, Task::Trading, 1.0).unwrap().name);
        // thin pool → at most what's available (degrade, don't panic).
        let solo = vec![Expert::new("only", "m", &[Task::Code], 100, 0.5, 0.1)];
        assert_eq!(route_ensemble(&solo, Task::Code, 1.0).len(), 1);
        // budget below every cost → empty (caller falls back to single Route).
        assert!(route_ensemble(&e, Task::Trading, 0.0).is_empty());
    }

    #[test]
    fn weighted_avoids_the_slow_70b_for_general() {
        // the whole point of qwen3.6's scorer: the slow/pricey 70B never wins a normal task —
        // its 1500ms TTFT sinks the latency term even though its capability is highest.
        let e = registry();
        let pick = route_weighted(&e, Task::General, 1.0).unwrap();
        assert_ne!(pick.name, "deepseek-r1", "must not route a normal General task to the slow 70B");
        // and it must out-score the 70B explicitly
        let ds = e.iter().find(|x| x.name == "deepseek-r1").unwrap();
        assert!(score(pick) > score(ds));
    }

    #[test]
    fn weighted_respects_budget() {
        let e = registry();
        // a tight budget excludes the pricey experts (qwen3.6 0.40, deepseek 0.60)
        let pick = route_weighted(&e, Task::General, 0.15).unwrap();
        assert!(pick.cost_per_1k <= 0.15);
    }

    #[test]
    fn estimate_prices_reasoning_premium() {
        let e = registry();
        let r1 = e.iter().find(|x| x.model.contains("deepseek-r1")).unwrap();
        let q = e.iter().find(|x| x.name == "qwen-general").unwrap();
        // same token shape: r1 costs strictly more — its reasoning bills as output,
        // and it's on the DeepSeek price card, not the cheap local one.
        assert!(estimate_cost_usd(r1, 1000, 200) > estimate_cost_usd(q, 1000, 200));
    }

    #[test]
    fn route_weighted_usd_drops_r1_on_tight_budget() {
        let e = registry();
        // a 1000-tok prompt + 200-tok answer. r1's reasoning-inflated cost blows a
        // sub-cent budget; a cheap local expert still fits → it's the pick, not r1.
        let tight = route_weighted_usd(&e, Task::General, 0.0005, 1000, 200).unwrap();
        assert!(!tight.model.contains("deepseek-r1"), "r1 too pricey for the budget");
        // a generous budget lets the affordable set through; whatever wins must be
        // within the estimate (the USD gate actually bound the choice).
        let rich = route_weighted_usd(&e, Task::General, 1.0, 1000, 200).unwrap();
        assert!(estimate_cost_usd(rich, 1000, 200) <= 1.0);
    }

    #[test]
    fn route_weighted_usd_none_when_unaffordable() {
        let e = registry();
        // zero budget → nothing affordable, never a silent over-budget mis-route.
        assert!(route_weighted_usd(&e, Task::General, 0.0, 1000, 200).is_none());
    }

    #[test]
    fn weighted_skips_unavailable() {
        let mut e = registry();
        for x in e.iter_mut() { if x.name != "gemma" { x.available = false; } }
        // only gemma is up → it's the pick despite others scoring higher when available
        assert_eq!(route_weighted(&e, Task::General, 1.0).unwrap().name, "gemma");
    }

    #[test]
    fn weighted_returns_none_when_nothing_qualifies() {
        let e = registry();
        // no expert handles a budget of 0 → None, never a silent mis-route
        assert!(route_weighted(&e, Task::General, 0.0).is_none());
    }

    #[test]
    fn score_does_not_floor_sub_one_values() {
        // the bug we fixed: composites are all < 1.0; they must still rank distinctly, not tie at 0.
        let fast = Expert::new("f", "m", &[Task::General], 100, 0.6, 0.1);
        let slow = Expert::new("s", "m", &[Task::General], 2000, 0.6, 0.1);
        assert!(score(&fast) > score(&slow));
        assert!(score(&fast) < 1.0 && score(&slow) > 0.0); // both in (0,1), not floored
    }

    #[test]
    fn strip_think_removes_reasoning_block() {
        let raw = "<think>let me reason about which tool…</think>{\"tool\":\"get_balance\"}";
        assert_eq!(strip_think(raw), "{\"tool\":\"get_balance\"}");
    }

    #[test]
    fn strip_think_is_noop_without_a_block() {
        assert_eq!(strip_think("  {\"tool\":\"arb_scan\"}  "), "{\"tool\":\"arb_scan\"}");
    }

    #[test]
    fn deepseek_and_qwen3_are_thinking_models() {
        // the heavy reasoning experts that generate() must drive with think:false + the long timeout
        assert!(crate::serve::thinking_model("deepseek-r1:70b"));
        assert!(crate::serve::thinking_model("qwen3.6:latest"));
        assert!(!crate::serve::thinking_model("gemma3:4b"));
    }

    #[test]
    fn sentiment_beats_code_when_signal_is_stronger() {
        // flaw #1 (first-category-wins): a lone Code keyword must not dominate
        // when the Sentiment signal is numerically stronger.
        assert_eq!(classify("score the sentiment of this rust news"), Task::Sentiment);
    }

    #[test]
    fn substring_false_positive_is_rejected() {
        // flaw #2 (bare substring): "arb" inside "garbage" must NOT be Trading.
        assert_eq!(classify("is this garbage?"), Task::General);
    }

    #[test]
    fn judge_verdict_picks_a_on_explicit_winner() {
        assert_eq!(parse_judge_verdict("After review, Winner: A is the call."), Verdict::ExpertA);
    }

    #[test]
    fn judge_verdict_picks_b_on_explicit_winner() {
        assert_eq!(parse_judge_verdict("Answer B is better; Winner: B."), Verdict::ExpertB);
    }

    #[test]
    fn judge_verdict_tie_on_equal() {
        assert_eq!(parse_judge_verdict("Both are comparable. Tie."), Verdict::Tie);
    }

    #[test]
    fn judge_verdict_garble_defaults_to_tie() {
        // unparseable judge output must NOT silently pick a side
        assert_eq!(parse_judge_verdict("zxcv qwerty blorp"), Verdict::Tie);
    }

    // ── TWO-MIND gate (pure logic) ──
    #[test]
    fn gate_verdict_parsing_is_veto_biased() {
        assert_eq!(parse_gate_verdict("APPROVE — correct read-only tool"), GateVerdict::Approve);
        assert!(matches!(parse_gate_verdict("VETO: amount unspecified"), GateVerdict::Veto(_)));
        // a reply that mentions both → veto wins (safety)
        assert!(matches!(parse_gate_verdict("I would APPROVE but actually VETO this"), GateVerdict::Veto(_)));
        // ambiguous → veto by default, never a guessed approve
        assert!(matches!(parse_gate_verdict("hmm not sure really"), GateVerdict::Veto(_)));
    }

    #[test]
    fn tool_money_classes() {
        assert_eq!(classify_tool("get_balance"), MoneyClass::ReadOnly);
        assert_eq!(classify_tool("agent_submit"), MoneyClass::Governance);
        assert_eq!(classify_tool("send_qug"), MoneyClass::RealMoney);
        assert_eq!(classify_tool("dex_swap"), MoneyClass::RealMoney);
    }

    #[test]
    fn gate_real_money_needs_human_even_on_approve() {
        let d = gate("qwen3.6", "deepseek-r1", "send_qug", &GateVerdict::Approve);
        assert!(!d.execute && d.requires_human);
        assert_eq!(d.signers, vec!["qwen3.6".to_string(), "deepseek-r1".to_string()]);
    }

    #[test]
    fn gate_readonly_fasttracks_and_veto_blocks() {
        let ok = gate("qwen3.6", "deepseek-r1", "get_balance", &GateVerdict::Approve);
        assert!(ok.execute && !ok.requires_human);
        let no = gate("qwen3.6", "deepseek-r1", "get_balance", &GateVerdict::Veto("looks like a probe".into()));
        assert!(!no.execute);
        assert!(no.reason.contains("VETOED by deepseek-r1"));
        assert_eq!(no.signers, vec!["qwen3.6".to_string()]); // vetoer did NOT co-sign
    }

    #[test]
    fn openai_url_bare_host() {
        assert_eq!(openai_chat_url("https://api.deepseek.com"), "https://api.deepseek.com/chat/completions");
    }

    #[test]
    fn openai_url_v1_base_and_trailing_slash() {
        assert_eq!(openai_chat_url("https://api.deepseek.com/v1"), "https://api.deepseek.com/v1/chat/completions");
        assert_eq!(openai_chat_url("http://box:8000/"), "http://box:8000/chat/completions");
    }
}
