//! cloud.rs — the **DeepSeek cloud API** as a flux-moe expert (the off-box judge).
//!
//! Local ollama experts (qwen3.6 on Epsilon) answer via [`crate::generate`]; the
//! authoritative JUDGE is `deepseek-v4-flash` over DeepSeek's OpenAI-compatible
//! cloud API — a different transport (HTTPS + Bearer auth) that returns a real
//! `usage` block, so [`crate::serve::cost_usd`] can price exactly what each veto
//! cost. This is the cross-transport 2-of-2 gate: cheap-local proposes, cloud
//! judges, and the router knows the dollar price of the judgment ([[route_weighted_usd]]).

use crate::serve::{cost_usd, Price, Usage};

/// DeepSeek OpenAI-compatible base. `/chat/completions` + `/models`.
pub const DEEPSEEK_BASE: &str = "https://api.deepseek.com";
/// The flash model id (Viktor's "4.2 flash"). Priced by [`Price::DEEPSEEK_V4_FLASH`].
pub const DEEPSEEK_FLASH: &str = "deepseek-v4-flash";

/// Resolve the API key: `DEEPSEEK_API_KEY` env wins, else the file at
/// `DEEPSEEK_API_KEY_FILE` (default `/root/.config/deepseek/api_key`). Fail loud — a
/// missing key must not silently degrade the gate to a single-model rubber-stamp.
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

/// A cloud completion: the text + its billed usage + the USD cost (via `cost_usd`).
#[derive(Debug, Clone)]
pub struct CloudReply {
    pub content: String,
    pub usage: Usage,
    pub cost_usd: f64,
}

/// Call DeepSeek `chat/completions` (model e.g. [`DEEPSEEK_FLASH`]). Deterministic
/// (`temperature:0`). Returns the message content + the priced [`Usage`].
pub fn deepseek_complete(model: &str, system: &str, user: &str) -> Result<CloudReply, String> {
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

    #[derive(serde::Deserialize)]
    struct Resp { choices: Vec<Choice>, usage: Usage }
    #[derive(serde::Deserialize)]
    struct Choice { message: Msg }
    #[derive(serde::Deserialize)]
    struct Msg { content: String }

    let resp: Resp = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
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

    let content = resp.choices.into_iter().next().map(|c| c.message.content).unwrap_or_default();
    let cost = cost_usd(&resp.usage, &Price::DEEPSEEK_V4_FLASH);
    Ok(CloudReply { content, usage: resp.usage, cost_usd: cost })
}

/// First non-empty line starts with APPROVE → approved. Anything else (REJECT,
/// prose, empty) → not approved: the gate fails CLOSED, never rubber-stamps.
pub fn parse_approval(judge_out: &str) -> bool {
    judge_out
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .map(|l| l.to_uppercase().starts_with("APPROVE"))
        .unwrap_or(false)
}

/// The outcome of a cross-transport gate.
#[derive(Debug, Clone)]
pub struct CloudVerdict {
    /// What the local proposer (qwen3.6) produced.
    pub proposal: String,
    /// The cloud judge's full verdict text.
    pub verdict: String,
    /// True iff the judge APPROVED (fails closed otherwise).
    pub approved: bool,
    /// USD the judge call cost (the priced veto).
    pub judge_cost_usd: f64,
}

/// The 2-of-2 gate Viktor wants: a LOCAL ollama expert (`proposer_model`, e.g.
/// `qwen3.6:latest` on Epsilon, free) drafts via [`crate::generate`]; the DeepSeek
/// CLOUD judge (`judge_model`, e.g. [`DEEPSEEK_FLASH`]) reviews and verdicts, with
/// the dollar cost attached. Cheap local proposes, authoritative cloud judges.
pub fn propose_then_cloud_judge(
    local_endpoint: &str,
    proposer_model: &str,
    judge_model: &str,
    task: &str,
) -> Result<CloudVerdict, String> {
    let proposal = crate::generate(local_endpoint, proposer_model, task)
        .map_err(|e| format!("proposer {proposer_model}: {e}"))?;
    let system = "You are a strict, impartial reviewer. Reply with EXACTLY 'APPROVE' or \
                  'REJECT' on the first line, then ONE sentence why.";
    let user = format!(
        "Task:\n{task}\n\nProposed answer:\n{proposal}\n\nIs the proposed answer correct, \
         complete, and a good response to the task?"
    );
    let jr = deepseek_complete(judge_model, system, &user)?;
    let approved = parse_approval(&jr.content);
    Ok(CloudVerdict { proposal, verdict: jr.content, approved, judge_cost_usd: jr.cost_usd })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_parsing_fails_closed() {
        assert!(parse_approval("APPROVE\nlooks correct"));
        assert!(parse_approval("  approve - fine"));
        assert!(!parse_approval("REJECT\nmissing edge case"));
        assert!(!parse_approval("hmm, not sure")); // prose → not approved
        assert!(!parse_approval(""));              // empty → not approved
        assert!(!parse_approval("The answer APPROVE")); // approve not at line start → closed
    }

    #[test]
    fn api_key_env_overrides_file() {
        std::env::set_var("DEEPSEEK_API_KEY", "sk-test-from-env");
        assert_eq!(api_key().unwrap(), "sk-test-from-env");
        std::env::remove_var("DEEPSEEK_API_KEY");
    }

    #[test]
    fn flash_model_id_is_priced() {
        // the cloud judge id matches the price card we bill it with
        assert_eq!(DEEPSEEK_FLASH, "deepseek-v4-flash");
        let u = Usage { prompt_tokens: 100, completion_tokens: 50, ..Default::default() };
        assert!(cost_usd(&u, &Price::DEEPSEEK_V4_FLASH) > 0.0);
    }
}
