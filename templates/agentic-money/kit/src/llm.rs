//! Open-model tool-call decide — turn an LLM into a money-decision source,
//! safely and reliably, with the hard-won serving lessons baked in.
//!
//! Talks to an OpenAI-ish `/api/chat` (ollama) endpoint over the std-only HTTP
//! client. Returns a parsed JSON decision (or `None` — a `None` is a SKIP, the
//! gate never sees a malformed decision).
//!
//! LESSONS BAKED IN (from live A100/4090 swarm rounds):
//!   • **deepseek-r1 has NO ollama tool-calling** — sending `tools:[…]` returns
//!     `{"error":"…does not support tools"}` instantly. And `format:"json"`
//!     STARVES it: R1's `<think>` block eats the whole `num_predict` budget →
//!     empty output. So for reasoning models: free-form + HIGH `num_predict`
//!     + lenient extraction (strip `<think>`, take the LAST `{…}`, else regex).
//!   • **qwen2.5 / qwen3 ARE tool-native** (via ollama, or vLLM with
//!     `--tool-call-parser hermes --enable-auto-tool-choice`) — prefer them.
//!   • **VERIFY the model is serving first** — a vLLM box showing `VRAM 0.9/80`
//!     was OOM'd / not serving. Hit `/api/tags` before relying on it.
//!
//! This module uses the robust path that works for BOTH families: free-form
//! generation + lenient parse. It's slower than native tool-calls but never
//! returns garbage to the gate.

use crate::rpc::Rpc;

/// Where + how to call the model.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// e.g. `http://127.0.0.1:11434` (ollama) or a Vast box `http://IP:PORT`.
    pub base_url: String,
    /// e.g. `qwen2.5:32b`, `deepseek-r1:70b`.
    pub model: String,
    /// 0.0 = deterministic. Keep low for money decisions.
    pub temperature: f64,
    /// HIGH for reasoning models so `<think>` doesn't starve the answer.
    pub num_predict: u32,
}

impl LlmConfig {
    pub fn new(base_url: &str, model: &str) -> Self {
        Self { base_url: base_url.to_string(), model: model.to_string(), temperature: 0.2, num_predict: 4000 }
    }

    /// Is the endpoint up and serving at least one model? Call before a round.
    pub fn is_serving(&self) -> bool {
        Rpc::new(&self.base_url)
            .get("/api/tags")
            .ok()
            .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok())
            .and_then(|v| v.get("models").map(|m| m.as_array().map(|a| !a.is_empty()).unwrap_or(false)))
            .unwrap_or(false)
    }
}

/// Ask the model for ONE decision. `system` frames the role + the required
/// output shape; `user` is the live situation. Returns the parsed JSON object,
/// or `None` (= SKIP this turn) if the model didn't emit a usable decision.
///
/// The `system` prompt MUST instruct the model to end with a single JSON line.
/// Convention used by the templates: `{"dir":"AtoB"|"BtoA","amount_in":<int>}`.
pub fn decide(cfg: &LlmConfig, system: &str, user: &str) -> Option<serde_json::Value> {
    let body = serde_json::json!({
        "model": cfg.model,
        "stream": false,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
        "options": {"temperature": cfg.temperature, "num_predict": cfg.num_predict},
    });
    let resp = post_chat(cfg, &body)?;
    let content = serde_json::from_str::<serde_json::Value>(&resp)
        .ok()?
        .get("message")?
        .get("content")?
        .as_str()?
        .to_string();
    extract_decision(&content)
}

fn post_chat(cfg: &LlmConfig, body: &serde_json::Value) -> Option<String> {
    Rpc::new(&cfg.base_url)
        .with_timeout(std::time::Duration::from_secs(400))
        .post("/api/chat", &body.to_string())
        .ok()
}

/// Lenient extraction: strip `<think>…</think>`, take the LAST balanced
/// `{…"dir"…}` object; failing that, regex a `dir` token + the first 3–5 digit
/// integer out of the prose. Mirrors the shell harness in
/// `sigil/scripts/swarm-money-round.sh`.
pub fn extract_decision(raw: &str) -> Option<serde_json::Value> {
    let cleaned = strip_think(raw);

    // 1. last {...} that mentions "dir", parsed as JSON
    if let Some(obj) = last_dir_object(&cleaned) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&obj) {
            return Some(v);
        }
    }

    // 2. lenient fallback: pull a direction + the first int from the prose
    let dir = ["AtoB", "BtoA"].iter().find(|d| cleaned.contains(**d))?;
    let amount = first_int(&cleaned)?;
    Some(serde_json::json!({"dir": dir, "amount_in": amount}))
}

fn strip_think(s: &str) -> String {
    // remove every <think>…</think> span (greedy-safe, non-nested)
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        if let Some(end) = rest[start..].find("</think>") {
            rest = &rest[start + end + "</think>".len()..];
        } else {
            rest = ""; // unterminated think → drop the tail
            break;
        }
    }
    out.push_str(rest);
    out
}

/// Find the last `{…}` substring containing `"dir"`. Tracks brace depth so a
/// nested object is captured whole.
fn last_dir_object(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut best: Option<String> = None;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let mut depth = 0i32;
            let mut j = i;
            while j < bytes.len() {
                match bytes[j] {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            let cand = &s[i..=j];
                            if cand.contains("\"dir\"") {
                                best = Some(cand.to_string());
                            }
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    best
}

/// First run of 3–5 digits in the text (the proposed amount).
fn first_int(s: &str) -> Option<u128> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let len = i - start;
            if (3..=12).contains(&len) {
                return s[start..i].parse::<u128>().ok();
            }
        } else {
            i += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_clean_json_after_think() {
        let raw = "<think>let me reason about the pool a lot…</think>\nFinal: {\"dir\":\"AtoB\",\"amount_in\":500}";
        let d = extract_decision(raw).expect("should parse");
        assert_eq!(d["dir"], "AtoB");
        assert_eq!(d["amount_in"], 500);
    }

    #[test]
    fn takes_last_dir_object() {
        let raw = "first thought {\"dir\":\"BtoA\",\"amount_in\":100} then revised {\"dir\":\"AtoB\",\"amount_in\":750}";
        let d = extract_decision(raw).unwrap();
        assert_eq!(d["dir"], "AtoB");
        assert_eq!(d["amount_in"], 750);
    }

    #[test]
    fn lenient_fallback_from_prose() {
        let raw = "I'll go AtoB with about 1200 units given the depth.";
        let d = extract_decision(raw).unwrap();
        assert_eq!(d["dir"], "AtoB");
        assert_eq!(d["amount_in"], 1200);
    }

    #[test]
    fn unterminated_think_is_dropped() {
        let raw = "<think>endless reasoning with no close and a stray AtoB 999";
        // no closing tag → whole tail dropped → no decision
        assert!(extract_decision(raw).is_none());
    }
}
