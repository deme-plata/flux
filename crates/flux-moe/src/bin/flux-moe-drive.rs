//! flux-moe-drive — let a flux-moe expert DRIVE real Flux dev via a tool call.
//!
//! Dogfood: instead of curling ollama by hand, we ask the model (through
//! `flux_moe::tool_call`, which sets `think:false` for thinking models) to PROPOSE
//! the next flux-core action and emit the flux MCP tool call to do it. The caller
//! (Rocky/Claude) then executes the proposed `flux_combo{package}` for real.
//!
//!   FLUX_MOE_OLLAMA=http://<ip>:<port> flux-moe-drive [model]   (default qwen3.6)

fn main() {
    let endpoint = std::env::var("FLUX_MOE_OLLAMA")
        .unwrap_or_else(|_| "http://202.122.49.242:22938".into());
    // Model precedence: FLUX_MOE_MODEL env > argv[1] > default. route() always picks the
    // first expert per task, so the strong-but-slow experts (deepseek-r1:70b) never get
    // chosen naturally — this override forces ANY expert so we can drive + prove them.
    let model = std::env::var("FLUX_MOE_MODEL").ok()
        .or_else(|| std::env::args().nth(1))
        .unwrap_or_else(|| "qwen3.6".into());

    let system = "You are an agentic Flux developer working WITH Rocky (Claude) on the Flux toolchain — \
        an AI-native Rust build orchestrator, 95 crates at v0.22.0. The compiler core is the crate `fluxc-core` \
        (driver `fluxc`). Dogfood rule: NEVER raw cargo — you build/verify ONLY via the flux MCP combo tools. \
        To advance flux core you call flux_combo{package:\"fluxc-core\"} (compile+test+predict) or flux_iterate. \
        Propose the single best NEXT concrete action to build flux core WITH us and CALL the tool. One tool call.";
    let user = "We're building flux core together right now on a live A100. \
        Propose the next concrete step to advance fluxc-core and call the right flux tool.";

    // The flux MCP combo tools the model may call (we execute the chosen one for real).
    let tools = r#"[
      {"type":"function","function":{"name":"flux_combo","description":"Compile + test + predict a Flux crate (the core dev action).","parameters":{"type":"object","properties":{"package":{"type":"string"},"why":{"type":"string"}},"required":["package"]}}},
      {"type":"function","function":{"name":"flux_iterate","description":"AI iteration loop: compile+test+stats for fast coding loops.","parameters":{"type":"object","properties":{"package":{"type":"string"}},"required":["package"]}}},
      {"type":"function","function":{"name":"flux_health_report","description":"Architecture score + SWOT + prediction for a crate.","parameters":{"type":"object","properties":{"package":{"type":"string"}},"required":["package"]}}}
    ]"#;

    // Heavy reasoning experts (deepseek-r1:70b) can't co-reside with qwen on a single 80GB A100 →
    // free the GPU first so the 70B loads SOLO. Skipped for small/non-thinking models (no need).
    if flux_moe::serve::thinking_model(&model) {
        match flux_moe::unload_others(&endpoint, &model) {
            Ok(0) => {}
            Ok(n) => { eprintln!("🧹 freed GPU: unloaded {n} other model(s) so {model} loads solo"); std::thread::sleep(std::time::Duration::from_secs(4)); }
            Err(e) => eprintln!("⚠ unload_others: {e} (continuing)"),
        }
    }

    eprintln!("⚙️  flux-moe driving {model} @ {endpoint} …");
    match flux_moe::tool_call(&endpoint, &model, system, user, tools) {
        Ok((name, args)) => {
            println!("{model} PROPOSES → {name} {args}");
        }
        Err(e) => {
            eprintln!("no tool call: {e}");
            std::process::exit(1);
        }
    }
}
