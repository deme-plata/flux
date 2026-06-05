// fluxc-gemma — Gemma4 as primary coder, DeepSeek as reviewer
//
// Strategy: Gemma4 generates ALL code (free). DeepSeek reviews ONLY
// when Gemma4 confidence < threshold. Result: 80-90% token savings.
//
// Flow:
//   1. Gemma4 generates code → confidence score (0-1)
//   2. If confidence > 0.7: apply immediately (free)
//   3. If confidence < 0.7: DeepSeek reviews (minimal tokens)
//   4. Gemma4 fixes DeepSeek's feedback (free)
//   5. Build + test (fluxc, free)

use serde::{Serialize, Deserialize};
use std::time::Duration;

const OLLAMA_URL: &str = "http://localhost:11434/api/generate";
const MODEL: &str = "gemma4:latest";

#[derive(Serialize)] struct OllamaRequest { model: String, prompt: String, stream: bool }
#[derive(Deserialize)] struct OllamaResponse { response: String, eval_count: Option<u64>, total_duration: Option<u64>, done: bool }

fn ollama(prompt: &str) -> Result<OllamaResponse, String> {
    let client = reqwest::blocking::Client::new();
    client.post(OLLAMA_URL).json(&OllamaRequest{model:MODEL.into(),prompt:prompt.to_string(),stream:false})
        .timeout(Duration::from_secs(30))
        .send().map_err(|e| format!("Ollama: {}", e))?
        .json().map_err(|e| format!("Parse: {}", e))
}

#[derive(Serialize, Clone)]
pub struct GemmaResult { pub success: bool, pub model: String, pub tokens: u64, pub time_ms: u64, pub output: String, pub confidence: f64 }

// ═══════════════════════════════════════════════════════════════
// Primary: Gemma4 code generation (FREE)
// ═══════════════════════════════════════════════════════════════

/// Generate Rust code. Gemma4 does the work. Returns confidence score.
pub fn gemma_generate(prompt: &str) -> GemmaResult {
    let full = format!("You are a Rust expert. Write ONLY the code, no explanation. Use `eprintln!` not `tracing`. Task: {}", prompt);
    match ollama(&full) {
        Ok(r) => {
            let resp = r.response.clone(); let output = extract_code(&resp);
            let conf = confidence(&output);
            GemmaResult { success: true, model: MODEL.into(), tokens: r.eval_count.unwrap_or(0), time_ms: r.total_duration.unwrap_or(0)/1_000_000, output, confidence: conf }
        },
        Err(e) => GemmaResult { success: false, model: MODEL.into(), tokens: 0, time_ms: 0, output: e, confidence: 0.0 },
    }
}

/// Gemma4 reviews its own code — finds bugs, suggests fixes. FREE.
pub fn gemma_self_review(code: &str) -> GemmaResult {
    let prompt = format!("Review this Rust code. List issues (numbered, one per line). If none, say 'OK'.\n```rust\n{}\n```", code);
    match ollama(&prompt) {
        Ok(r) => GemmaResult { success: true, model: MODEL.into(), tokens: r.eval_count.unwrap_or(0), time_ms: r.total_duration.unwrap_or(0)/1_000_000, output: r.response.clone(), confidence: if r.response.contains("OK") { 0.95 } else { 0.3 } },
        Err(e) => GemmaResult { success: false, model: MODEL.into(), tokens: 0, time_ms: 0, output: e, confidence: 0.0 },
    }
}

/// Gemma4 fixes a compiler error. FREE.
pub fn gemma_fix(error: &str, code: &str) -> GemmaResult {
    let prompt = format!("Fix this Rust compiler error. Return ONLY corrected code.\nError: {}\n```rust\n{}\n```", error, code);
    match ollama(&prompt) {
        Ok(r) => {
            let resp = r.response.clone(); let fixed = extract_code(&resp);
            GemmaResult { success: true, model: MODEL.into(), tokens: r.eval_count.unwrap_or(0), time_ms: r.total_duration.unwrap_or(0)/1_000_000, output: fixed, confidence: 0.8 }
        },
        Err(e) => GemmaResult { success: false, model: MODEL.into(), tokens: 0, time_ms: 0, output: e, confidence: 0.0 },
    }
}

// ═══════════════════════════════════════════════════════════════
// Scoring: determine if DeepSeek review is needed (PAID)
// ═══════════════════════════════════════════════════════════════

/// Returns true if DeepSeek review is worth the tokens.
/// Only call DeepSeek when Gemma4 confidence < 0.7.
pub fn needs_deepseek_review(result: &GemmaResult) -> bool {
    result.confidence < 0.7 && result.success
}

/// DeepSeek review prompt — minimal tokens, just validation.
/// Call ONLY when needs_deepseek_review() returns true.
pub fn deepseek_review_prompt(code: &str, gemma_confidence: f64) -> String {
    format!("Quick review (1-2 lines): is this Rust code correct? Confidence was {:.0}%. Code:\n```rust\n{}\n```", gemma_confidence*100.0, code)
}

/// Cost estimate for the collaborative approach.
pub fn estimate_cost(gemma_generations: u32, deepseek_reviews: u32) -> serde_json::Value {
    let gemma_cost = 0.0; // FREE
    let ds_input_tokens = deepseek_reviews as f64 * 200.0; // ~200 tokens per review
    let ds_cost = ds_input_tokens / 1_000_000.0 * 0.14; // $0.14/M
    let all_ds_cost = (gemma_generations + deepseek_reviews) as f64 * 500.0 / 1_000_000.0 * 0.14;

    serde_json::json!({
        "approach": "Gemma4 primary + DeepSeek reviewer",
        "gemma4_generations": gemma_generations,
        "deepseek_reviews": deepseek_reviews,
        "gemma_cost": "$0.00",
        "deepseek_cost": format!("${:.4}", ds_cost),
        "total_cost": format!("${:.4}", ds_cost),
        "all_deepseek_would_cost": format!("${:.4}", all_ds_cost),
        "savings_pct": format!("{:.0}%", (1.0 - ds_cost/all_ds_cost.max(0.001)) * 100.0),
        "recommendation": "Use Gemma4 for ALL generation. DeepSeek only for review when confidence < 70%.",
    })
}

/// Check Gemma4 status.

/// Confidence Cascade: Gemma4 generates + self-reviews. Two scores combined. Escalate to DeepSeek only when cascade < 0.5.
pub fn confidence_cascade(prompt: &str) -> serde_json::Value {
    let gen = gemma_generate(prompt);
    if !gen.success { return serde_json::json!({"error":"Gemma4 failed","deepseek_needed":true}); }
    let output = gen.output.clone(); let review = gemma_self_review(&output);
    let cascade = gen.confidence * review.confidence;
    let ds = cascade < 0.5 || (gen.confidence - review.confidence).abs() > 0.4;
    serde_json::json!({"code":output,"gen_conf":gen.confidence,"review_conf":review.confidence,"cascade":cascade,"deepseek_needed":ds,"cost":if ds{"~$0.0003"}else{"$0.00"}})
}

/// Automated Dev Interview — Gemma4 reflects post-commit
pub fn gemma_dev_interview(commit_msg: &str, pkg: &str, build_ms: u64, passed: u32, failed: u32) -> serde_json::Value {
    let prompt = format!("You built {} after {}. {}ms, {} pass/{} fail. Key insight? Rate 1-10. Under 100 words.", pkg, commit_msg, build_ms, passed, failed);
    let insight = ollama(&prompt).map(|r| r.response).unwrap_or_default();
    let iv = serde_json::json!({"type":"dev_interview","agent":"gemma4","wallet":"pending","commit":commit_msg,"pkg":pkg,"build_ms":build_ms,"tests_passed":passed,"insight":insight});
    if let Ok(c) = reqwest::blocking::Client::new().post("http://127.0.0.1:9099/dev_interview").json(&iv).send() { let _ = c; }
    iv
}
pub fn gemma_status() -> serde_json::Value {
    let tags = reqwest::blocking::Client::new().get("http://localhost:11434/api/tags")
        .send().ok().and_then(|r| r.json::<serde_json::Value>().ok());
    let models: Vec<String> = tags.as_ref().and_then(|t| t["models"].as_array())
        .map(|a| a.iter().filter_map(|m| m["name"].as_str().map(String::from)).collect()).unwrap_or_default();
    serde_json::json!({"gemma4_available":models.contains(&MODEL.to_string()),"ollama_models":models,"cost":"FREE","speed":"~3 tok/s","context":"8K"})
}

// ═══════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════

fn extract_code(response: &str) -> String {
    if let Some(start) = response.find("```rust") {
        if let Some(end) = response[start+7..].find("```") {
            return response[start+7..start+7+end].trim().to_string();
        }
    }
    if let Some(start) = response.find("```") {
        if let Some(end) = response[start+3..].find("```") {
            return response[start+3..start+3+end].trim().to_string();
        }
    }
    response.trim().to_string()
}

fn confidence(code: &str) -> f64 {
    if code.is_empty() { return 0.0; }
    let mut score = 0.5;
    if code.contains("fn ") { score += 0.1; }
    if code.contains("use ") { score += 0.05; }
    if code.contains("pub ") { score += 0.05; }
    if code.contains("impl ") { score += 0.1; }
    if code.contains("let ") { score += 0.05; }
    if code.contains("->") { score += 0.05; }
    if code.contains("#[") { score += 0.05; }
    if code.contains("//") { score += 0.05; }
    if score > 1.0 { 1.0 } else { score }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_status() { assert!(gemma_status().is_object()); }
    #[test] fn test_confidence_empty() { assert_eq!(confidence(""), 0.0); }
    #[test] fn test_confidence_good() { let c = confidence("fn main() { let x = 1; }"); assert!(c > 0.5); }
    #[test] fn test_needs_review_high_conf() { let r = GemmaResult{success:true,model:"g".into(),tokens:0,time_ms:0,output:"fn x(){}".into(),confidence:0.9}; assert!(!needs_deepseek_review(&r)); }
    #[test] fn test_needs_review_low_conf() { let r = GemmaResult{success:true,model:"g".into(),tokens:0,time_ms:0,output:"???".into(),confidence:0.3}; assert!(needs_deepseek_review(&r)); }
    #[test] fn test_extract_code() { assert!(extract_code("```rust\nfn x(){}\n```").contains("fn x()")); }
    #[test] fn test_estimate_cost() { let e = estimate_cost(10, 2); assert!(e["savings_pct"].as_str().unwrap().contains("%")); }
}
