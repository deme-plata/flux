//! serve.rs — the user-facing "**Flux AI on Vast**" feature (propose-only).
//!
//! Goal: let a user spin up the Claude-Code-built flux-moe agentic model on a
//! rented Vast.ai GPU and use it for agentic-money Flux coding — competing with
//! DeepSeek / Claude Code. This module does NOT rent anything: like
//! `sigil-btc-miner::gpu::build_command` and `trainer::train_command`, it builds
//! the **provision spec + the remote install script + the endpoint URL** as
//! plain data, so the actual `vast create_instance` is a separate, explicit step
//! the user (or an MCP combo) triggers. No auto-spend.
//!
//! Composition: once the box serves Ollama, the existing [`crate::generate`] /
//! [`crate::flux_llm`] router targets it via `FLUX_MOE_OLLAMA=http://<ip>:11434`
//! — so serving is just "stand up Ollama on the box, point the router at it".

use serde::Deserialize;

/// What to serve and on what box.
#[derive(Debug, Clone)]
pub struct ServeSpec {
    /// Vast offer id (from search) — informational; the rental is triggered separately.
    pub offer_id: u64,
    /// Ollama model tag to serve, e.g. "qwen2.5-coder:7b".
    pub model: String,
    /// Optional HF LoRA adapter repo to overlay (the chronos / tool-call adapter).
    /// None = serve the stock base model.
    pub adapter_repo: Option<String>,
    /// Ollama serve port.
    pub port: u16,
}

impl ServeSpec {
    pub fn new(offer_id: u64, model: impl Into<String>) -> Self {
        Self { offer_id, model: model.into(), adapter_repo: None, port: 11434 }
    }
    pub fn with_adapter(mut self, repo: impl Into<String>) -> Self {
        self.adapter_repo = Some(repo.into());
        self
    }

    /// The remote bootstrap script: install Ollama, pull the model, serve on
    /// 0.0.0.0 so the flux-moe router can reach it. Run via SSH on the rented box.
    /// `hf_token` (read) is exported for adapter/model pulls from the Hub.
    pub fn install_script(&self, hf_token: &str) -> String {
        let mut s = String::new();
        s.push_str("set -e\n");
        s.push_str(&format!("export HF_TOKEN={hf_token}\n"));
        s.push_str("export OLLAMA_HOST=0.0.0.0:11434\n");
        s.push_str("curl -fsSL https://ollama.com/install.sh | sh\n");
        s.push_str("nohup ollama serve >/var/log/ollama.log 2>&1 &\n");
        s.push_str("sleep 5\n");
        s.push_str(&format!("ollama pull {}\n", self.model));
        if let Some(repo) = &self.adapter_repo {
            // Overlay a LoRA adapter via a Modelfile (Ollama ADAPTER directive).
            s.push_str("pip install -q huggingface_hub\n");
            s.push_str(&format!(
                "python -c \"from huggingface_hub import snapshot_download; snapshot_download('{repo}', local_dir='/workspace/adapter')\"\n"
            ));
            s.push_str(&format!(
                "printf 'FROM {}\\nADAPTER /workspace/adapter\\n' > /workspace/Modelfile\n",
                self.model
            ));
            s.push_str("ollama create flux-ai -f /workspace/Modelfile\n");
        }
        s.push_str("echo FLUX_AI_SERVE_READY\n");
        s
    }

    /// The model name the router should request (the adapter-overlaid model if any).
    pub fn served_model(&self) -> &str {
        if self.adapter_repo.is_some() { "flux-ai" } else { &self.model }
    }

    /// The endpoint URL to set as `FLUX_MOE_OLLAMA` once the box is up.
    pub fn endpoint(&self, host_ip: &str) -> String {
        format!("http://{host_ip}:{}", self.port)
    }

    /// vLLM serve script — **the 2026-06-01 finding**: ollama's bundled
    /// llama.cpp build returns **HTTP 500** on Qwen3.6's new architecture
    /// (`hf.co/unsloth/Qwen3.6-27B-GGUF`) — the manifest pulls fine, generation
    /// fails — and co-loading it beside a 72B fills the A100. **vLLM supports the
    /// arch** and serves it OpenAI-compatibly. Use this (not `install_script`)
    /// for any new-arch model ollama can't load, and give it the GPU alone.
    /// SEAMLESS recipe (2026-06-01, every Vast ollama-image blocker pre-solved —
    /// see flux-moe SKILL.md): a **venv** dodges the `Cannot uninstall PyJWT`
    /// debian conflict that aborts a plain `pip install vllm`; the box has no pip
    /// and no systemd, so the venv brings pip and the caller launches with
    /// `setsid` (NOT a trailing-`sleep` ssh nohup — that gets SIGTERM'd). Reach
    /// port 8000 via an SSH tunnel (it isn't Vast-mapped). Hermes tool-parser so
    /// the model can drive a real tool loop.
    pub fn vllm_install_script(&self, hf_token: &str) -> String {
        let gguf = if self.model.to_lowercase().contains("gguf") {
            " --quantization gguf"
        } else {
            ""
        };
        format!(
            "#!/bin/bash\nexport HF_TOKEN={hf_token}\n\
             python3 -m venv /root/ve\n\
             /root/ve/bin/pip install -q -U pip\n\
             /root/ve/bin/pip install -q vllm\n\
             /root/ve/bin/python -m vllm.entrypoints.openai.api_server \
             --model {}{} --port 8000 --gpu-memory-utilization 0.85 \
             --max-model-len 16384 --enable-auto-tool-choice \
             --tool-call-parser hermes >> /root/vllm.log 2>&1\n",
            self.model, gguf
        )
    }

    /// vLLM's OpenAI-compatible base URL (router uses `/v1/chat/completions`).
    pub fn vllm_endpoint(&self, host_ip: &str) -> String {
        format!("http://{host_ip}:8000/v1")
    }

    /// llama.cpp serve script for **sharded** GGUF (e.g. DeepSeek-R1 671B 1.58-bit)
    /// — the 2026-06-01 fix: ollama refuses sharded GGUF (issue #5245), but
    /// llama.cpp loads shard-0 and auto-finds the rest + does MoE GPU/CPU offload.
    pub fn llamacpp_install_script(&self, hf_token: &str, hf_repo: &str, shard0: &str, gpu_layers: u32) -> String {
        format!(
            "#!/bin/bash\nset -e\nexport HF_TOKEN={hf_token}\n\
             pip install -q huggingface_hub\n\
             huggingface-cli download {hf_repo} --local-dir /models/r1 --include '*.gguf'\n\
             nohup llama-server -m /models/r1/{shard0} -ngl {gpu_layers} --host 0.0.0.0 --port 8000 \
             >/var/log/llama.log 2>&1 &\n"
        )
    }
}

/// The serving runtime that ACTUALLY loads a given model. The fix for ollama's
/// limits learned 2026-06-01: ollama **500s on new archs** (Qwen3.6) and **can't
/// load sharded GGUF** (R1 671B). flux-moe routes around it instead of being
/// blocked by it — *this is what the flux LLM is for*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime { Ollama, Vllm, LlamaCpp }

impl Runtime {
    pub fn why(self) -> &'static str {
        match self {
            Runtime::Ollama => "standard single-file GGUF, supported arch",
            Runtime::Vllm => "new arch ollama can't load (e.g. Qwen3.6) — vLLM supports it",
            Runtime::LlamaCpp => "sharded GGUF ollama refuses (e.g. R1 671B 1.58-bit) — llama.cpp loads shards",
        }
    }
}

/// Pick the runtime that will actually serve the model.
pub fn pick_runtime(sharded_gguf: bool, new_arch_ollama_cant_load: bool) -> Runtime {
    if sharded_gguf { Runtime::LlamaCpp }
    else if new_arch_ollama_cant_load { Runtime::Vllm }
    else { Runtime::Ollama }
}

// ── 2026-06-03 finding — measured on a verified A100-SXM4-80GB ──────────────
// Two updates to the 2026-06-01 runtime notes above:
//   1. `qwen3.6` DOES load on **ollama** directly via the distributed tag
//      `qwen3.6:latest` (arch `qwen35moe`, 36B MoE Q4). The earlier "ollama 500s
//      on Qwen3.6 → use vLLM" applies to the raw GGUF repo
//      (`hf.co/...Qwen3.6-27B-GGUF`), NOT the ollama-packaged tag. Prefer the tag.
//   2. THE tool-call fix: qwen3.6 / deepseek-r1 are *thinking* models. Driven
//      through OpenAI `/v1/chat/completions` with a small token budget, the
//      `<think>` phase eats the budget and emits NO tool_calls — a 12-case
//      flux-crate routing eval scored **1/12**. The same model over ollama's
//      native `/api/chat` with **`think:false`** scored **12/12**.

/// True for models whose reasoning phase must be disabled (`think:false`) to get
/// reliable native tool-calls. Measured 2026-06-03 on an A100: ON → 1/12, OFF → 12/12.
pub fn thinking_model(model: &str) -> bool {
    let m = model.to_lowercase();
    m.starts_with("qwen3") || m.contains("qwen3.") || m.contains("deepseek-r1")
}

/// The ollama endpoint to drive a tool loop on: native `/api/chat` (which honors
/// `think`), NOT the OpenAI `/v1/chat/completions` path that hit the thinking trap.
pub fn ollama_toolcall_path() -> &'static str { "/api/chat" }

/// Build the ollama `/api/chat` request body for a single-turn **tool call** on a
/// (possibly thinking) model. Encodes the 2026-06-03 A100 fixes:
///   - `think:false` for thinking models (1/12 → 12/12 tool emission),
///   - **`num_ctx:8192` + `num_gpu:999`** — the GPU-FIT fix: ollama's default ctx
///     (131072) balloons a 70B's KV-cache past 80 GB → it spills to CPU and the
///     box reads VRAM-full / GPU-0% / CPU-pegged and *hangs*. Small ctx + full
///     offload keeps it ~45 GB, 100% GPU. (Forgetting this is exactly why a first
///     flux-moe drive looked like "flux-moe doesn't work".)
///   - `stream:false`, `temperature:0` for deterministic routing.
/// `tools_json` is passed verbatim. Data-only — no serde_json dependency here.
pub fn ollama_toolcall_body(model: &str, system: &str, user: &str, tools_json: &str) -> String {
    // Only thinking models (qwen3*/deepseek-r1) take `think`. Sending `think:true` to a NON-thinking
    // model (qwen2.5-coder:3b, glm-4.7-flash, gemma) makes ollama 400 the /api/chat request — so OMIT
    // the field entirely for them. (This blocked fast small local proposers for two-mind.)
    let think_field = if thinking_model(model) { "\"think\":false," } else { "" };
    format!(
        // keep_alive:30m → a big reasoning model (deepseek-r1:70b) stays resident instead of reloading
        // every call (the A100 thrash). num_ctx 4096 (down from 8192) → the 70B's KV cache was what
        // bloated it to 82GB and blocked co-residence with qwen; short router prompts don't need 8k.
        "{{\"model\":{m},\"messages\":[{{\"role\":\"system\",\"content\":{s}}},\
         {{\"role\":\"user\",\"content\":{u}}}],\"tools\":{tools},{think_field}\"keep_alive\":\"30m\",\
         \"stream\":false,\"options\":{{\"temperature\":0,\"num_predict\":600,\"num_ctx\":4096,\"num_gpu\":999}}}}",
        m = json_str(model), s = json_str(system), u = json_str(user), tools = tools_json,
    )
}

/// The OpenAI `/v1/chat/completions` path — for **vLLM** (the playbook §7 serving for the
/// concurrent combo loop) and any OpenAI-compatible server. The flux-api surface targets
/// exactly this shape, so flux-moe drives vLLM the same way an SDK would.
pub fn openai_toolcall_path() -> &'static str { "/v1/chat/completions" }

/// Build an OpenAI `/v1/chat/completions` tool-call body. vLLM started with
/// `--enable-auto-tool-choice --tool-call-parser hermes` honors `tool_choice:"auto"` —
/// the very option ollama 400s on, which is why ollama and vLLM need separate transports.
pub fn openai_toolcall_body(model: &str, system: &str, user: &str, tools_json: &str) -> String {
    format!(
        "{{\"model\":{m},\"messages\":[{{\"role\":\"system\",\"content\":{s}}},\
         {{\"role\":\"user\",\"content\":{u}}}],\"tools\":{tools},\"tool_choice\":\"auto\",\
         \"temperature\":0,\"max_tokens\":600,\"stream\":false}}",
        m = json_str(model), s = json_str(system), u = json_str(user), tools = tools_json,
    )
}

/// Minimal JSON string encoder (quote + escape) so this data-only module can
/// build request bodies without pulling serde_json in.
fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

// ── DeepSeek call accounting ────────────────────────────────────────────────
// Budget-aware routing needs to know what a call (especially a reasoning-model
// VETO) actually costs, so route_weighted can weigh "is r1's judgment worth its
// price". The OpenAI-compatible `usage` block carries the token counts; this maps
// them to USD with tunable per-model rates.

/// The chain-of-thought slice of `completion_tokens` reported by a reasoning model.
/// It is a SUBSET of `completion_tokens` and is billed at the output rate.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct CompletionTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: u64,
}

/// Token usage from a DeepSeek (OpenAI-compatible) chat completion `usage` block.
/// `prompt_cache_hit_tokens` is the slice of `prompt_tokens` served from the context
/// cache (billed at the cheap cache rate); `reasoning_tokens` lives under
/// `completion_tokens_details` and bills as output.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    #[serde(default)]
    pub completion_tokens_details: CompletionTokensDetails,
    #[serde(default)]
    pub prompt_cache_hit_tokens: u64,
}

/// Per-1M-token USD rates for a model. Tunable consts below (deepseek-v4-flash).
#[derive(Debug, Clone, Copy)]
pub struct Price {
    /// Fresh (cache-miss) prompt tokens, USD per 1M.
    pub input_per_1m: f64,
    /// Cached prompt tokens (context-cache hit), USD per 1M — cheaper than input.
    pub cache_hit_per_1m: f64,
    /// Output tokens (completion, reasoning included), USD per 1M.
    pub output_per_1m: f64,
}

impl Price {
    /// deepseek-v4-flash published rates (USD / 1M tokens). Tunable — adjust here
    /// when DeepSeek reprices; reasoning is charged at `output_per_1m`.
    pub const DEEPSEEK_V4_FLASH: Price = Price {
        input_per_1m: 0.27,
        cache_hit_per_1m: 0.07,
        output_per_1m: 1.10,
    };
}

/// USD cost of one DeepSeek call. Cache-hit prompt tokens bill at the cheap cache
/// rate, the remaining prompt at input rate, and ALL completion tokens — reasoning
/// included (it's a subset of `completion_tokens`) — at the output rate. The `.max`
/// guards the case where an API reports completion_tokens without folding reasoning
/// in, so reasoning is never undercounted. This is the number budget-aware routing
/// consults to price a veto.
pub fn cost_usd(u: &Usage, p: &Price) -> f64 {
    let cached = u.prompt_cache_hit_tokens.min(u.prompt_tokens);
    let fresh_prompt = u.prompt_tokens - cached;
    let output = u.completion_tokens.max(u.completion_tokens_details.reasoning_tokens);
    (fresh_prompt as f64) / 1e6 * p.input_per_1m
        + (cached as f64) / 1e6 * p.cache_hit_per_1m
        + (output as f64) / 1e6 * p.output_per_1m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_script_serves_and_pulls() {
        let spec = ServeSpec::new(38116562, "qwen2.5-coder:7b");
        let sh = spec.install_script("hf_TESTTOKEN");
        assert!(sh.contains("ollama serve"));
        assert!(sh.contains("ollama pull qwen2.5-coder:7b"));
        assert!(sh.contains("OLLAMA_HOST=0.0.0.0:11434"));
        assert!(sh.trim_end().ends_with("FLUX_AI_SERVE_READY"));
        assert_eq!(spec.served_model(), "qwen2.5-coder:7b");
    }

    #[test]
    fn adapter_overlay_builds_modelfile() {
        let spec = ServeSpec::new(1, "qwen3:1.5b").with_adapter("rocky/flux-moe-chronos-lora");
        let sh = spec.install_script("hf_X");
        assert!(sh.contains("snapshot_download('rocky/flux-moe-chronos-lora'"));
        assert!(sh.contains("ollama create flux-ai"));
        assert_eq!(spec.served_model(), "flux-ai"); // router asks for the adapter model
    }

    #[test]
    fn endpoint_url_for_router() {
        let spec = ServeSpec::new(1, "qwen3:4b");
        assert_eq!(spec.endpoint("203.0.113.7"), "http://203.0.113.7:11434");
    }

    #[test]
    fn runtime_selector_routes_around_ollama_limits() {
        // Qwen3.6 = new arch ollama 500s on → vLLM
        assert_eq!(pick_runtime(false, true), Runtime::Vllm);
        // DeepSeek-R1 671B = sharded GGUF ollama refuses → llama.cpp
        assert_eq!(pick_runtime(true, false), Runtime::LlamaCpp);
        // qwen2.5:72b = standard single-file, supported → ollama
        assert_eq!(pick_runtime(false, false), Runtime::Ollama);
    }

    #[test]
    fn llamacpp_script_downloads_shards_and_serves() {
        let spec = ServeSpec::new(1, "deepseek-r1");
        let sh = spec.llamacpp_install_script("hf_X", "unsloth/DeepSeek-R1-GGUF", "R1-IQ1_S-00001-of-00003.gguf", 30);
        assert!(sh.contains("huggingface-cli download unsloth/DeepSeek-R1-GGUF"));
        assert!(sh.contains("llama-server -m /models/r1/R1-IQ1_S-00001-of-00003.gguf"));
        assert!(sh.contains("-ngl 30"));
    }

    #[test]
    fn thinking_models_need_think_disabled() {
        // the 2026-06-03 A100 finding: these reason by default → must disable for tool-calls
        assert!(thinking_model("qwen3.6"));
        assert!(thinking_model("qwen3:32b"));
        assert!(thinking_model("deepseek-r1:70b"));
        // non-thinking models keep reasoning off-by-default; don't force it
        assert!(!thinking_model("qwen2.5-coder:7b"));
        assert!(!thinking_model("gemma2:9b"));
    }

    #[test]
    fn toolcall_body_disables_thinking_for_qwen36() {
        let tools = "[{\"type\":\"function\",\"function\":{\"name\":\"flux_combo\"}}]";
        let body = ollama_toolcall_body("qwen3.6", "route it", "build a form builder", tools);
        // the fix that took the wrestle from 1/12 → 12/12
        assert!(body.contains("\"think\":false"), "thinking model MUST disable think");
        assert!(body.contains("\"num_ctx\":4096"), "MUST cap ctx (4096) or the 70B's KV spills + the box hangs");
        assert!(body.contains("\"num_gpu\":999"), "MUST force full GPU offload");
        assert!(body.contains("\"stream\":false"));
        assert!(body.contains("\"temperature\":0"));
        assert!(body.contains("flux_combo"), "tools passed through verbatim");
        assert_eq!(ollama_toolcall_path(), "/api/chat");
    }

    #[test]
    fn toolcall_body_omits_think_for_nonthinking() {
        // non-thinking models 400 on a `think` field → it MUST be omitted entirely (the qwen2.5-coder
        // /api/chat 400 fix). The rest of the body stays intact + valid.
        let body = ollama_toolcall_body("qwen2.5-coder:7b", "s", "u", "[]");
        assert!(!body.contains("\"think\""), "non-thinking models must NOT include a think field (ollama 400s)");
        assert!(body.contains("\"keep_alive\":\"30m\""), "rest of the body intact");
        assert!(body.contains("\"num_gpu\":999"));
        // still parses as valid JSON (no dangling comma from the omitted field)
        assert!(serde_json::from_str::<serde_json::Value>(&body).is_ok(), "body must be valid JSON: {body}");
    }

    #[test]
    fn toolcall_body_escapes_json_in_prompt() {
        // a prompt with quotes/newlines must not break the request body
        let body = ollama_toolcall_body("qwen3.6", "say \"hi\"\nthen route", "u", "[]");
        assert!(body.contains("\\\"hi\\\""), "quotes escaped");
        assert!(body.contains("\\n"), "newline escaped");
    }

    #[test]
    fn cost_usd_matches_real_deepseek_block() {
        // the real usage block from a deepseek-v4-flash veto: prompt 15, completion 31
        // (of which 27 reasoning), no cache hit.
        let u = Usage {
            prompt_tokens: 15,
            completion_tokens: 31,
            completion_tokens_details: CompletionTokensDetails { reasoning_tokens: 27 },
            prompt_cache_hit_tokens: 0,
        };
        let c = cost_usd(&u, &Price::DEEPSEEK_V4_FLASH);
        // 15 in @ 0.27/1M  +  31 out @ 1.10/1M   (reasoning ⊂ completion, billed as output)
        let expected = 15.0 / 1e6 * 0.27 + 31.0 / 1e6 * 1.10;
        assert!((c - expected).abs() < 1e-15, "got {c}, want {expected}");
        assert!(c > 0.0);
    }

    #[test]
    fn usage_deserializes_deepseek_json() {
        let j = r#"{"prompt_tokens":15,"completion_tokens":31,
                    "completion_tokens_details":{"reasoning_tokens":27},
                    "prompt_cache_hit_tokens":0}"#;
        let u: Usage = serde_json::from_str(j).unwrap();
        assert_eq!(u.prompt_tokens, 15);
        assert_eq!(u.completion_tokens, 31);
        assert_eq!(u.completion_tokens_details.reasoning_tokens, 27);
        // round-trips into the same cost as the hand-built struct
        let expected = 15.0 / 1e6 * 0.27 + 31.0 / 1e6 * 1.10;
        assert!((cost_usd(&u, &Price::DEEPSEEK_V4_FLASH) - expected).abs() < 1e-15);
    }

    #[test]
    fn cost_usd_discounts_cache_hits() {
        let p = Price::DEEPSEEK_V4_FLASH;
        let miss = Usage { prompt_tokens: 1000, ..Default::default() };
        let hit = Usage { prompt_tokens: 1000, prompt_cache_hit_tokens: 1000, ..Default::default() };
        assert!(cost_usd(&hit, &p) < cost_usd(&miss, &p), "cache hits must be cheaper");
        // a fully-cached 1000-token prompt costs exactly the cache rate
        assert!((cost_usd(&hit, &p) - 1000.0 / 1e6 * p.cache_hit_per_1m).abs() < 1e-15);
    }

    #[test]
    fn vllm_serves_new_arch_alone() {
        // the Qwen3.6 finding: ollama 500s on the arch; vLLM serves it OpenAI-compat
        let spec = ServeSpec::new(1, "hf.co/unsloth/Qwen3.6-27B-GGUF:Q4_K_M");
        let sh = spec.vllm_install_script("hf_X");
        assert!(sh.contains("python3 -m venv"), "must venv (PyJWT/debian fix)");
        assert!(sh.contains("hf.co/unsloth/Qwen3.6-27B-GGUF:Q4_K_M"));
        assert!(sh.contains("--quantization gguf"), "GGUF model → gguf quant");
        assert!(sh.contains("--tool-call-parser hermes"), "tool-calling enabled");
        assert_eq!(spec.vllm_endpoint("1.2.3.4"), "http://1.2.3.4:8000/v1");
    }
}
