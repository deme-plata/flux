//! skillroute.rs — teach Qwen to START the right flux-dev skill from a command.
//!
//! Like Claude Code matches a request to a skill, the flux-moe agent must map a
//! user command → the correct flux skill + its start action. This is the
//! registry + a keyword router + training-example emitter (the "flux-dev skills
//! for Qwen" curriculum). Feeds the distillation corpus so a CPU student learns
//! to invoke skills properly, not just answer.

/// A flux skill the agent can start.
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: &'static str,
    pub triggers: &'static [&'static str],
    pub start: &'static str, // the action to take
}

/// The real flux skill surface (matches ~/.claude/skills + the MCP combos).
pub fn skills() -> Vec<Skill> {
    vec![
        Skill { name: "flux-dev", triggers: &["compile", "build", "test", "fix the build", "cargo", "fluxc"],
                start: "flux_combo / flux_qspec (git-first dev loop)" },
        Skill { name: "flux-moe", triggers: &["train", "llm", "agentic model", "qwen", "tool-call", "distill", "fine-tune"],
                start: "flux-moe: corpus → QLoRA/distill → serve" },
        Skill { name: "flux-fabric", triggers: &["spin a fabric", "rent", "vast", "n nodes", "test at scale", "fleet"],
                start: "flux-fabric: search→create→flux-ssh install→matrix→teardown" },
        Skill { name: "sigil", triggers: &["sigil block", "sigil-node", "state root", "verify-before-sync", "sigil chain", "produce-block"],
                start: "sigil: block production / sigil-node work" },
        Skill { name: "flux-zk", triggers: &["stark", "lattice", "zk proof", "zk gate", "10ms", "recursive proof", "tip-proof"],
                start: "flux-zk: flux_zk_combo (--features pq)" },
        Skill { name: "sigil-book", triggers: &["chapter", "shadows in the chain", "novel", "write the book", "rebuild the book"],
                start: "sigil-book: write/build/publish chapters" },
        Skill { name: "flux-strategist", triggers: &["3-year plan", "remember this", "recall", "capital strategy", "saylor", "qshare", "qcredit"],
                start: "flux-strategist: remember/recall/plan over the record cache" },
        Skill { name: "carl-runefelt-btc", triggers: &["dca the dip", "stack sats", "accumulate btc", "never sell", "runefelt"],
                start: "carl-runefelt-btc: propose BTC accumulation (never auto-spend)" },
        Skill { name: "quillonos", triggers: &["os.html", "browser miner", "wasi", "quillonos", "in-tab"],
                start: "quillonos: browser-first WASI userspace" },
    ]
}

/// Route a natural-language command to the best skill (first trigger hit).
pub fn route(cmd: &str) -> Option<Skill> {
    let c = cmd.to_lowercase();
    skills().into_iter().find(|s| s.triggers.iter().any(|t| c.contains(t)))
}

/// Emit training examples (command → "start <skill>") for the Qwen curriculum.
/// Each skill's triggers become user-command exemplars mapping to its start.
pub fn to_training_jsonl() -> String {
    use serde_json::json;
    let mut out = String::new();
    for s in skills() {
        for t in s.triggers {
            let goal = format!("User says: \"{t}\" — which flux skill do I start and how?");
            let answer = format!("Start the **{}** skill → {}", s.name, s.start);
            let ex = json!({"messages":[{"role":"user","content":goal},{"role":"assistant","content":answer}]});
            out.push_str(&ex.to_string());
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_commands_to_the_right_skill() {
        assert_eq!(route("compile flux-moe and run the tests").unwrap().name, "flux-dev");
        assert_eq!(route("spin a fabric of 10 vast nodes").unwrap().name, "flux-fabric");
        assert_eq!(route("write the next chapter of shadows in the chain").unwrap().name, "sigil-book");
        assert_eq!(route("train a qwen tool-call model").unwrap().name, "flux-moe");
        assert_eq!(route("verify the 10ms zk gate").unwrap().name, "flux-zk");
    }

    #[test]
    fn unknown_command_routes_to_nothing() {
        assert!(route("what's the weather in copenhagen").is_none());
    }

    #[test]
    fn training_corpus_covers_every_skill() {
        let jsonl = to_training_jsonl();
        for s in skills() {
            assert!(jsonl.contains(s.name), "training data must cover skill {}", s.name);
        }
        assert!(jsonl.lines().count() >= 30, "want a real skill-routing curriculum");
    }
}
