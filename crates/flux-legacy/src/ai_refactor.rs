//! ai_refactor.rs (flux-legacy PROTOTYPE 3, P3-1) — AI-driven god-file decomposition.
//!
//! Prototype 1 measures, P2 plans + chunks a god-file by LOC. P3 reads the ACTUAL code and asks
//! deepseek-v4-flash for a *semantic* decomposition: which top-level items belong together, named,
//! with a one-line rationale each. The split stops being "~108 modules" and becomes a real map.
//!
//! Design: the crate stays transport-free (no reqwest). The two halves that carry the logic —
//! [`decompose_prompt`] (build the request) and [`parse_decomposition`] (parse the reply) — are PURE
//! and unit-tested with NO network. [`ai_decompose`] glues them via an injected `call` closure, so
//! the live DeepSeek HTTP lives in the bin (P3-2) and tests pass a canned closure. Propose-only.

use serde::{Deserialize, Serialize};

/// One module the AI proposes a god-file be split into.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposedModule {
    /// suggested module name (snake_case)
    pub name: String,
    /// the top-level item names (fn / struct / enum / impl target) that move here
    #[serde(default)]
    pub items: Vec<String>,
    /// one-line reason these items cohere
    #[serde(default)]
    pub rationale: String,
}

/// Build the decomposition prompt for a god-file. PURE. Truncates very large sources to keep the
/// request within budget (the item signatures, not bodies, are what the model needs to group).
pub fn decompose_prompt(file_name: &str, file_src: &str) -> String {
    const MAX_CHARS: usize = 48_000;
    let src = if file_src.len() > MAX_CHARS {
        &file_src[..MAX_CHARS]
    } else {
        file_src
    };
    format!(
        "You are a Rust refactoring assistant. The file `{file_name}` is a god-file (too large). \
         Decompose its TOP-LEVEL items (pub/private fn, struct, enum, trait, impl) into cohesive \
         modules. Return ONLY a JSON array, no prose, no markdown fences, of objects:\n\
         [{{\"name\":\"snake_case_module\",\"items\":[\"item_name\",...],\"rationale\":\"one short line\"}}]\n\
         Group by responsibility (e.g. all `*_api` handlers, all serialization, all types). 4–12 modules. \
         Use the EXACT item names as they appear. Here is the source:\n\n{src}"
    )
}

/// Parse the model's reply into proposed modules. PURE + tolerant: strips markdown fences and any
/// prose around the JSON, then parses the first top-level `[...]` array. Returns `[]` if no valid
/// array is found (never panics — a malformed AI reply just yields no plan).
pub fn parse_decomposition(reply: &str) -> Vec<ProposedModule> {
    let cleaned = reply.replace("```json", "").replace("```", "");
    // find the first balanced top-level [ ... ]
    let bytes = cleaned.as_bytes();
    let start = match cleaned.find('[') {
        Some(i) => i,
        None => return Vec::new(),
    };
    let mut depth = 0i32;
    let mut end = None;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        match b {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let json = match end {
        Some(e) => &cleaned[start..e],
        None => return Vec::new(),
    };
    serde_json::from_str::<Vec<ProposedModule>>(json).unwrap_or_default()
}

/// AI-decompose a god-file. `call` performs the actual model request (prompt -> reply); injected so
/// the crate needs no HTTP client and tests can mock it. Propose-only — returns the plan, writes nothing.
pub fn ai_decompose<F>(file_name: &str, file_src: &str, call: F) -> Result<Vec<ProposedModule>, String>
where
    F: Fn(&str) -> Result<String, String>,
{
    let prompt = decompose_prompt(file_name, file_src);
    let reply = call(&prompt)?;
    let mods = parse_decomposition(&reply);
    if mods.is_empty() {
        return Err(format!("no module decomposition parsed from reply ({} chars)", reply.len()));
    }
    Ok(mods)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_carries_name_source_and_json_shape() {
        let p = decompose_prompt("handlers.rs", "pub fn alpha() {}\npub struct Beta;");
        assert!(p.contains("handlers.rs"));
        assert!(p.contains("pub fn alpha"));
        assert!(p.contains("JSON array"));
        assert!(p.contains("rationale"));
    }

    #[test]
    fn prompt_truncates_huge_source() {
        let huge = "x".repeat(60_000);
        let p = decompose_prompt("big.rs", &huge);
        // bounded: header + at most 48k of source
        assert!(p.len() < 50_000, "prompt must cap source, got {}", p.len());
    }

    #[test]
    fn parses_clean_json_array() {
        let reply = r#"[{"name":"types","items":["Foo","Bar"],"rationale":"shared data types"},
                        {"name":"api","items":["handle_x"],"rationale":"http handlers"}]"#;
        let m = parse_decomposition(reply);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].name, "types");
        assert_eq!(m[0].items, vec!["Foo", "Bar"]);
        assert_eq!(m[1].name, "api");
    }

    #[test]
    fn parses_through_fences_and_prose() {
        let reply = "Here is the split:\n```json\n[{\"name\":\"io\",\"items\":[\"read_all\"]}]\n```\nDone.";
        let m = parse_decomposition(reply);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].name, "io");
        assert_eq!(m[0].rationale, ""); // optional field defaults
    }

    #[test]
    fn malformed_reply_yields_empty_not_panic() {
        assert!(parse_decomposition("sorry, I cannot do that").is_empty());
        assert!(parse_decomposition("[ not json ]").is_empty());
    }

    #[test]
    fn ai_decompose_uses_injected_transport() {
        // a mock "model" that returns a canned decomposition
        let mods = ai_decompose("f.rs", "pub fn a(){}", |prompt| {
            assert!(prompt.contains("f.rs"));
            Ok(r#"[{"name":"core","items":["a"],"rationale":"entry"}]"#.into())
        })
        .unwrap();
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].name, "core");
    }

    #[test]
    fn ai_decompose_errors_on_unparseable() {
        let e = ai_decompose("f.rs", "x", |_| Ok("no json here".into()));
        assert!(e.is_err());
    }
}
