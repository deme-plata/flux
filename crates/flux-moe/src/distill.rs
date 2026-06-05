//! distill.rs — Qwen3.6 (90% teacher) → tiny CPU students good at agentic money.
//!
//! The flow: the strong teacher (Qwen3.6-27B, 90% on the tool-call eval) LABELS a
//! large, diverse set of agentic-money goals with correct tool-calls; a small base
//! (Qwen2.5-0.5B/1.5B) trains on those (CPU-LoRA), then quantizes to **Q4 GGUF** →
//! runs on the owned 48-core swarm with **NO GPU**, via Ollama/llama.cpp.
//!
//! Why distillation beats the hand-written corpus: our 246-ex hand corpus overfit
//! a 1.5B (30% vs 50% base). A 90% teacher can generate THOUSANDS of diverse,
//! correct (goal → tool-call) pairs — diversity is the cure for overfit. The big
//! model's judgment compressed into a CPU-sized footprint.

use serde_json::json;

use crate::{generate_openai, parse_gate_verdict, GateVerdict};

/// A CPU-friendly student model to distill into.
#[derive(Debug, Clone)]
pub struct StudentSpec {
    pub base: &'static str, // HF id
    pub params_b: f64,
    pub quant: &'static str, // GGUF quant for CPU serving
}

/// The small bases worth distilling into — all run on CPU once Q4-quantized.
pub fn cpu_students() -> Vec<StudentSpec> {
    vec![
        StudentSpec { base: "Qwen/Qwen2.5-0.5B-Instruct", params_b: 0.5, quant: "Q4_K_M" },
        StudentSpec { base: "Qwen/Qwen2.5-1.5B-Instruct", params_b: 1.5, quant: "Q4_K_M" },
        StudentSpec { base: "Qwen/Qwen2.5-3B-Instruct", params_b: 3.0, quant: "Q4_K_M" },
    ]
}

impl StudentSpec {
    /// Rough CPU footprint of the Q4 GGUF (≈ params_b × 0.6 GB at Q4_K_M).
    pub fn cpu_ram_gb(&self) -> f64 { self.params_b * 0.6 }
    /// CPU-friendly = fits comfortably in a few GB and needs no GPU.
    pub fn runs_on_cpu(&self) -> bool { self.cpu_ram_gb() < 4.0 }
}

/// Generate a large, diverse set of agentic-money goals for the teacher to label.
/// Templated over real values so the distillation corpus is grounded AND big.
pub fn distill_goals() -> Vec<String> {
    let amts = ["10", "42", "100", "250", "500", "1000"];
    let tokens = ["CLAI", "PACI", "SCALPEL", "USDS", "QUGUSD"];
    let who = ["Rocky", "Adrian", "Codex", "Viktor"];
    let pairs = [("QUG", "PACI"), ("USDS", "QUG"), ("QUG", "USDS"), ("PACI", "QUG")];
    let symbols = ["BTC", "ETH", "ETC", "SOL"];
    let mut g = vec![];
    for a in amts {
        for w in who { g.push(format!("Send {a} QUG to {w}")); }
        for t in tokens { g.push(format!("Transfer {a} {t} to the chest")); }
        g.push(format!("DCA {a} USDS into Bitcoin"));
        g.push(format!("Route {a} QUG of profit into the BTC stack"));
    }
    for (x, y) in pairs {
        for a in ["25", "100", "500"] {
            g.push(format!("Swap {a} {x} for {y}"));
            g.push(format!("Quote converting {a} {x} to {y}"));
        }
    }
    for s in symbols { g.push(format!("What's the {s} price and is there an arb?")); }
    for q in ["my QUG balance", "my whole portfolio", "the DEX pools", "arbitrage opportunities",
              "the BTC bridge status", "a fresh BTC deposit address", "the network status"] {
        g.push(format!("Show me {q}"));
    }
    g
}

/// Build the teacher prompt: ask Qwen3.6 to emit the single best tool-call for a goal.
pub fn teacher_prompt(goal: &str) -> serde_json::Value {
    json!({
        "goal": goal,
        "instruction": "Emit exactly one tool call for this agentic-money goal — correct tool + required params. No prose."
    })
}

/// Ask the teacher (Qwen3.6 on an Ollama endpoint) to label a goal. Returns its
/// raw tool-call text (parse with the eval's multi-format parser). Live call.
pub fn teacher_label(endpoint: &str, model: &str, goal: &str) -> Result<String, String> {
    crate::generate(endpoint, model, &format!(
        "You are an agentic-money tool-caller. For the goal, output ONE tool call only \
         (Qwen <function=…><parameter=…> format), no explanation.\nGoal: {goal}"))
}

/// The llama.cpp export command: merge LoRA → convert → quantize to Q4 GGUF for CPU.
pub fn gguf_export_cmd(merged_dir: &str, out_gguf: &str, quant: &str) -> Vec<String> {
    vec![
        "python".into(), "llama.cpp/convert_hf_to_gguf.py".into(), merged_dir.into(),
        "--outfile".into(), format!("{out_gguf}.f16.gguf"), "--outtype".into(), "f16".into(),
        "&&".into(),
        "llama.cpp/llama-quantize".into(), format!("{out_gguf}.f16.gguf"), format!("{out_gguf}.{quant}.gguf"), quant.into(),
    ]
}

// ───────────────────────── VETO-STUDENT distillation ─────────────────────────
// Teacher = deepseek-v4-flash (DeepSeek API, OpenAI transport — NO GPU box). It labels
// synthetic money scenarios APPROVE/VETO; we export supervised JSONL and fine-tune a 1-3B
// CPU student so the two_mind VETOER's judgment runs LOCALLY, offline, after distillation.

/// The supervised PROMPT for the veto-student: a synthetic money scenario =
/// (request, proposed tool, args). Pure + deterministic so it's identical at TRAIN
/// time and INFERENCE time (the student must see the exact same shape it learned).
pub fn veto_prompt(request: &str, tool: &str, args_json: &serde_json::Value) -> String {
    format!(
        "You are a cautious agentic-money SAFETY AUDITOR. A proposer chose the tool `{tool}` \
         with arguments {args_json} to satisfy the request: \"{request}\". \
         Reply on ONE line starting with exactly APPROVE or VETO, then one short reason. \
         VETO if the tool is wrong, the args are unsafe or underspecified, or it moves real money without a clear mandate."
    )
}

/// The supervised COMPLETION the student learns to emit for a verdict.
pub fn verdict_completion(v: &GateVerdict) -> String {
    match v {
        GateVerdict::Approve => "APPROVE".to_string(),
        GateVerdict::Veto(reason) => format!("VETO: {reason}"),
    }
}

/// One labelled distillation row: a synthetic money scenario + the teacher's verdict.
#[derive(Debug, Clone)]
pub struct VetoRow {
    pub request: String,
    pub tool: String,
    pub args: serde_json::Value,
    pub verdict: GateVerdict,
}

/// LIVE: ask the TEACHER (deepseek-v4-flash, DeepSeek API / OpenAI transport) to label a
/// synthetic money scenario APPROVE/VETO. Endpoint+model resolve from env
/// (`FLUX_MOE_TEACHER_ENDPOINT` / `FLUX_MOE_TEACHER_MODEL`), defaulting to the DeepSeek API
/// + `deepseek-v4-flash`. Returns `(verdict, raw_reply)`. **Conservative on error** — an
/// unreachable teacher folds to a Veto, so a failed label never becomes a silent APPROVE.
pub fn teacher_label_gate(request: &str, tool: &str, args_json: &serde_json::Value) -> (GateVerdict, String) {
    let endpoint = std::env::var("FLUX_MOE_TEACHER_ENDPOINT").unwrap_or_else(|_| "https://api.deepseek.com".into());
    let model = std::env::var("FLUX_MOE_TEACHER_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".into());
    let prompt = veto_prompt(request, tool, args_json);
    match generate_openai(&endpoint, &model, &prompt) {
        Ok(reply) => (parse_gate_verdict(&reply), reply),
        Err(e) => (GateVerdict::Veto(format!("teacher unreachable (safety default): {e}")), e),
    }
}

/// Export labelled rows as supervised JSONL for the local veto-student: one object per
/// line, `{"prompt": <veto_prompt>, "completion": "APPROVE"|"VETO: reason"}`.
pub fn to_veto_jsonl(rows: &[VetoRow]) -> String {
    rows.iter()
        .map(|r| json!({
            "prompt": veto_prompt(&r.request, &r.tool, &r.args),
            "completion": verdict_completion(&r.verdict),
        }).to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distill_corpus_is_large_and_diverse() {
        let g = distill_goals();
        assert!(g.len() >= 80, "want a big distillation set, got {}", g.len());
        let uniq: std::collections::HashSet<_> = g.iter().collect();
        assert_eq!(uniq.len(), g.len(), "goals must be unique");
    }

    #[test]
    fn students_are_cpu_friendly() {
        for s in cpu_students() {
            assert!(s.runs_on_cpu(), "{} should run on CPU ({:.1}GB)", s.base, s.cpu_ram_gb());
        }
        // 0.5B Q4 ≈ 0.3 GB — trivially CPU-servable
        assert!(cpu_students()[0].cpu_ram_gb() < 0.5);
    }

    #[test]
    fn gguf_export_quantizes_for_cpu() {
        let cmd = gguf_export_cmd("/out/merged", "/out/flux-money", "Q4_K_M");
        assert!(cmd.iter().any(|s| s.contains("convert_hf_to_gguf")));
        assert!(cmd.iter().any(|s| s.contains("llama-quantize")));
        assert!(cmd.contains(&"Q4_K_M".to_string()));
    }

    #[test]
    fn veto_prompt_includes_the_scenario() {
        let p = veto_prompt("Send 1000 QUG to Mallory", "send_qug", &json!({"to":"Mallory","amount":1000}));
        assert!(p.contains("send_qug"), "names the tool");
        assert!(p.contains("Send 1000 QUG to Mallory"), "includes the request");
        assert!(p.contains("Mallory"), "includes the args");
        let u = p.to_uppercase();
        assert!(u.contains("APPROVE") && u.contains("VETO"), "instructs the binary verdict");
    }

    #[test]
    fn verdict_completion_maps_both_arms() {
        assert_eq!(verdict_completion(&GateVerdict::Approve), "APPROVE");
        assert_eq!(verdict_completion(&GateVerdict::Veto("no mandate".into())), "VETO: no mandate");
    }

    #[test]
    fn veto_jsonl_shape_is_supervised_pairs() {
        let rows = vec![
            VetoRow { request: "Show my balance".into(), tool: "get_balance".into(),
                      args: json!({}), verdict: GateVerdict::Approve },
            VetoRow { request: "Send 1000 QUG to a stranger".into(), tool: "send_qug".into(),
                      args: json!({"to":"x","amount":1000}),
                      verdict: GateVerdict::Veto("no mandate for a real-money send".into()) },
        ];
        let jsonl = to_veto_jsonl(&rows);
        let lines: Vec<&str> = jsonl.lines().collect();
        assert_eq!(lines.len(), 2, "one object per row");
        for l in &lines {
            let v: serde_json::Value = serde_json::from_str(l).expect("each line is valid JSON");
            assert!(v.get("prompt").and_then(|x| x.as_str()).is_some(), "has a prompt string");
            let c = v.get("completion").and_then(|x| x.as_str()).expect("has a completion");
            assert!(c == "APPROVE" || c.starts_with("VETO:"), "completion shape: {c}");
        }
        // round-trip the labels via the parsed values (don't assume key spacing).
        let v0: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v0["completion"], "APPROVE");
        let v1: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert!(v1["completion"].as_str().unwrap().starts_with("VETO: no mandate"));
    }

    #[test]
    #[ignore = "live teacher: needs DEEPSEEK_API_KEY + network (deepseek-v4-flash)"]
    fn teacher_label_gate_live() {
        let (v, raw) = teacher_label_gate(
            "Send 1000 QUG to an unknown address",
            "send_qug",
            &json!({"to":"unknown","amount":1000}),
        );
        eprintln!("verdict={v:?}\nraw={raw}");
        // a real-money send to an unknown address with no mandate should VETO.
        assert!(matches!(v, GateVerdict::Veto(_)), "expected a veto, got {v:?}");
    }
}
