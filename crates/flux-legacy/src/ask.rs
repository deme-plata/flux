//! ask.rs — feed the flux-legacy CORPUS to DeepSeek v4's long-context model.
//!
//! This is where **legacy** and **context-flux** exploit DeepSeek 4's 1M window together:
//! [`corpus`](crate::corpus) packs the highest-value Quillon code into ~1M tokens,
//! [`bundle_string`](crate::corpus::bundle_string) renders it, and `ask_deepseek` sends the whole
//! node to DeepSeek in ONE shot for a real architecture analysis.
//!
//! Sending the corpus PUBLISHES that source to an external API — fine when the operator asks, gate
//! otherwise. Key resolution mirrors the rest of the stack (env wins, else the config file).

use serde::Deserialize;

pub const DEEPSEEK_BASE: &str = "https://api.deepseek.com";
/// cheap long-context model
pub const MODEL_FLASH: &str = "deepseek-v4-flash";
/// deeper-reasoning long-context model
pub const MODEL_PRO: &str = "deepseek-v4-pro";

/// A whole-node analysis question worth asking the 1M window.
pub const DEFAULT_SYSTEM: &str = "You are a senior Rust architect auditing a large brownfield \
    blockchain node. You are given a token-budgeted bundle of its highest-value source (ranked by \
    crate fan-in; some files are signature-only outlines, marked as such). Be concrete and cite \
    crate/file names. Do not invent code that isn't shown.";

/// Resolve the DeepSeek key: `DEEPSEEK_API_KEY` env wins, else `DEEPSEEK_API_KEY_FILE`
/// (default `/root/.config/deepseek/api_key`). Fail loud.
pub fn api_key() -> Result<String, String> {
    if let Ok(k) = std::env::var("DEEPSEEK_API_KEY") {
        if !k.trim().is_empty() {
            return Ok(k.trim().to_string());
        }
    }
    let path = std::env::var("DEEPSEEK_API_KEY_FILE")
        .unwrap_or_else(|_| "/root/.config/deepseek/api_key".to_string());
    std::fs::read_to_string(&path)
        .map(|s| s.trim().to_string())
        .map_err(|e| format!("no DeepSeek API key (set DEEPSEEK_API_KEY or {path}): {e}"))
}

/// The model's answer + billed usage.
#[derive(Debug, Clone)]
pub struct AskResult {
    pub model: String,
    pub answer: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

/// POST `system` + `user` to DeepSeek `chat/completions` (deterministic, non-streaming) and return
/// the message content + usage. `timeout_s` should be generous for a 1M-token prompt (minutes).
pub fn ask_deepseek(model: &str, system: &str, user: &str, timeout_s: u64) -> Result<AskResult, String> {
    let key = api_key()?;
    let body = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user",   "content": user},
        ],
        "temperature": 0,
        "stream": false,
    });

    #[derive(Deserialize)]
    struct Resp { choices: Vec<Choice>, #[serde(default)] usage: Usage }
    #[derive(Deserialize)]
    struct Choice { message: Msg }
    #[derive(Deserialize)]
    struct Msg { content: String }
    #[derive(Deserialize, Default)]
    struct Usage {
        #[serde(default)] prompt_tokens: u64,
        #[serde(default)] completion_tokens: u64,
    }

    let resp: Resp = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_s))
        .build()
        .map_err(|e| e.to_string())?
        .post(format!("{DEEPSEEK_BASE}/chat/completions"))
        .bearer_auth(key)
        .json(&body)
        .send()
        .map_err(|e| format!("deepseek send: {e}"))?
        .error_for_status()
        .map_err(|e| format!("deepseek http: {e}"))?
        .json()
        .map_err(|e| format!("deepseek decode: {e}"))?;

    let answer = resp.choices.into_iter().next().map(|c| c.message.content).unwrap_or_default();
    Ok(AskResult {
        model: model.to_string(),
        answer,
        prompt_tokens: resp.usage.prompt_tokens,
        completion_tokens: resp.usage.completion_tokens,
    })
}

/// Convenience: build the analysis prompt (question + the corpus bundle) and ask DeepSeek.
/// Returns the answer; the caller already has the [`Corpus`](crate::corpus::Corpus) stats to print.
pub fn analyze_corpus(
    corpus: &crate::corpus::Corpus,
    question: &str,
    model: &str,
    timeout_s: u64,
) -> Result<AskResult, String> {
    let bundle = crate::corpus::bundle_string(corpus);
    let user = format!(
        "{question}\n\nHere is the codebase bundle ({} tokens packed of a {}-token budget, \
         {} files):\n\n{bundle}",
        corpus.tokens_packed, corpus.window_tokens, corpus.full_count + corpus.outline_count,
    );
    ask_deepseek(model, DEFAULT_SYSTEM, &user, timeout_s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_env_overrides_file() {
        std::env::set_var("DEEPSEEK_API_KEY", "sk-ask-test");
        assert_eq!(api_key().unwrap(), "sk-ask-test");
        std::env::remove_var("DEEPSEEK_API_KEY");
    }

    #[test]
    fn model_ids_are_the_v4_pair() {
        assert_eq!(MODEL_FLASH, "deepseek-v4-flash");
        assert_eq!(MODEL_PRO, "deepseek-v4-pro");
    }
}
