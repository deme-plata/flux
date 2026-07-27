//! flux-twomind — the live two-mind agentic-money safety gate.
//!
//! qwen3.6 PROPOSES a tool call → (proposer unloaded so the 70B fits) → deepseek-r1:70b JUDGES it in
//! prose → the gate applies the money policy (real money ALWAYS needs a human). Run against the A100:
//!   FLUX_MOE_OLLAMA=http://<ip>:<port> flux-twomind "Send 9000 QUG to qnk-unknown"

fn main() {
    // Two-box shape: set FLUX_MOE_PROPOSER_EP (qwen box) + FLUX_MOE_VETOER_EP (deepseek box) to run
    // each model on its OWN GPU — no unload, no churn. If they're equal (or only FLUX_MOE_OLLAMA is
    // set), it falls back to the single-box flow (with the unload-between step).
    let base = match flux_moe::ollama_endpoint() {
        Ok(e) => e,
        Err(e) => { eprintln!("flux-twomind: {e}"); std::process::exit(2); }
    };
    let prop_ep = std::env::var("FLUX_MOE_PROPOSER_EP").unwrap_or_else(|_| base.clone());
    let veto_ep = std::env::var("FLUX_MOE_VETOER_EP").unwrap_or_else(|_| base.clone());
    let proposer = std::env::var("FLUX_TWOMIND_PROPOSER").unwrap_or_else(|_| "qwen3.6:latest".into());
    let vetoer = std::env::var("FLUX_TWOMIND_VETOER").unwrap_or_else(|_| "deepseek-r1:70b".into());
    let user = std::env::args().nth(1).unwrap_or_else(|| "Send 9000 QUG to qnk-new-unknown-address".into());

    let system = "You are an agentic-money assistant. Choose EXACTLY ONE tool to satisfy the user request, using the provided tools. Call the single best tool.";
    let tools = r#"[
      {"type":"function","function":{"name":"get_balance","description":"Get the user's QUG balance.","parameters":{"type":"object","properties":{}}}},
      {"type":"function","function":{"name":"dex_get_quote","description":"Quote a swap, no funds move.","parameters":{"type":"object","properties":{"from":{"type":"string"},"to":{"type":"string"},"amount":{"type":"number"}}}}},
      {"type":"function","function":{"name":"arb_scan","description":"Scan pools for arbitrage.","parameters":{"type":"object","properties":{}}}},
      {"type":"function","function":{"name":"send_qug","description":"Send QUG to an address. REAL MONEY, irreversible.","parameters":{"type":"object","properties":{"to":{"type":"string"},"amount":{"type":"number"}},"required":["to","amount"]}}},
      {"type":"function","function":{"name":"dex_swap","description":"Execute a token swap. REAL MONEY.","parameters":{"type":"object","properties":{"from":{"type":"string"},"to":{"type":"string"},"amount":{"type":"number"}}}}}
    ]"#;

    // CLOUD-VETOER mode (Viktor's shape): qwen3.6 on Epsilon (local, free) PROPOSES → DeepSeek's
    // hosted `deepseek-v4-flash` JUDGES via the API → the MONEY GATE decides. No GPU box at all.
    //   FLUX_TWOMIND_CLOUD=1 DEEPSEEK_API_KEY=sk-... flux-twomind "Send 9000 QUG to unknown"
    if matches!(std::env::var("FLUX_TWOMIND_CLOUD").as_deref(), Ok("1") | Ok("true")) {
        let prop_ep = std::env::var("FLUX_MOE_PROPOSER_EP").unwrap_or_else(|_| "http://localhost:11434".into());
        // qwen3.6 on Epsilon (Viktor's choice) — a THINKING model, so the toolcall body's think:false
        // is honored. Non-thinking small models (qwen2.5-coder:3b) 400 on /api/chat here (think:true
        // rejected) — the real speed fix is for serve.rs to OMIT `think` for non-thinking models.
        let proposer = std::env::var("FLUX_TWOMIND_PROPOSER").unwrap_or_else(|_| "qwen3.6:latest".into());
        let judge = std::env::var("FLUX_TWOMIND_VETOER").unwrap_or_else(|_| flux_moe::cloud::DEEPSEEK_FLASH.to_string());
        eprintln!("🧠 TWO-MIND (CLOUD vetoer)\n   proposer={proposer} @ {prop_ep} (Epsilon-local)\n   vetoer={judge} @ DeepSeek API\n   request: {user}\n");
        let (tool, args) = match flux_moe::tool_call(&prop_ep, &proposer, system, &user, tools) {
            Ok(x) => x,
            Err(e) => { eprintln!("proposer ({proposer}) failed: {e}"); std::process::exit(1); }
        };
        // FAST-TRACK read-only: a harmless read (get_balance, dex_get_quote, …) doesn't need — and a
        // cautious judge over-vetoes — a judgment call. Spend the judge ONLY on money/governance.
        if flux_moe::classify_tool(&tool) == flux_moe::MoneyClass::ReadOnly {
            println!("TOOL        {tool}");
            println!("CLASS       ReadOnly");
            println!("EXECUTE     true");
            println!("HUMAN REQ   false");
            println!("SIGNERS     [\"{proposer}\"]");
            println!("REASON      read-only · fast-tracked (no judge call needed)");
            println!("JUDGE COST  $0.000000  (skipped — read-only)");
            return;
        }
        // MANDATE — the deterministic envelope (amount cap + recipient/tool allowlists) runs
        // BEFORE we spend the judge: a call outside the envelope is denied for $0 and the vetoer
        // never runs. Env-tunable; defaults cap at 1000 over the money tools this bin offers.
        let mandate = flux_moe::mandate::Mandate {
            max_amount: std::env::var("FLUX_TWOMIND_MAX_AMOUNT").ok().and_then(|s| s.parse().ok()).unwrap_or(1000.0),
            allow_recipients: std::env::var("FLUX_TWOMIND_ALLOW_RECIPIENTS")
                .map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
                .unwrap_or_default(),
            allow_tools: std::env::var("FLUX_TWOMIND_ALLOW_TOOLS")
                .unwrap_or_else(|_| "send_qug,send_token,dex_swap,add_liquidity,ln_pay".into())
                .split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        };
        if let Err(why) = flux_moe::mandate::check_mandate(&tool, &args, &mandate) {
            println!("TOOL        {tool}");
            println!("CLASS       {:?}", flux_moe::classify_tool(&tool));
            println!("EXECUTE     false");
            println!("HUMAN REQ   false");
            println!("SIGNERS     [\"{proposer}\"]");
            println!("REASON      MANDATE DENIED: {why}");
            println!("JUDGE COST  $0.000000  (skipped — mandate denied before judge; cap ${:.2})", mandate.max_amount);
            return;
        }
        let jsys = "You are a cautious agentic-money SAFETY AUDITOR. Reply on one line STARTING WITH \
                    exactly APPROVE or VETO, then one short reason. VETO if the tool is wrong, the \
                    arguments are unsafe or underspecified, or it moves real money without a clear mandate.";
        let juser = format!("A proposer chose the tool `{tool}` with arguments {args} to satisfy: \"{user}\". Is this the correct, safe tool and arguments?");
        match flux_moe::cloud::deepseek_complete(&judge, jsys, &juser) {
            Ok(reply) => {
                let verdict = flux_moe::parse_gate_verdict(&reply.content);
                let d = flux_moe::mandate::gate_mandated(&proposer, &judge, &tool, &args, &verdict, &mandate);
                println!("TOOL        {}", d.tool);
                println!("CLASS       {:?}", d.class);
                println!("EXECUTE     {}", d.execute);
                println!("HUMAN REQ   {}", d.requires_human);
                println!("SIGNERS     {:?}", d.signers);
                println!("REASON      {}", d.reason);
                println!("JUDGE       {} → {}", judge, reply.content.lines().next().unwrap_or("").trim());
                println!("JUDGE COST  ${:.6}  ({}+{} tok)", reply.cost_usd, reply.usage.prompt_tokens, reply.usage.completion_tokens);
            }
            Err(e) => { eprintln!("cloud judge ({judge}) failed: {e}"); std::process::exit(1); }
        }
        return;
    }

    let two_box = prop_ep != veto_ep;
    eprintln!(
        "🧠 TWO-MIND ({} shape)\n   proposer={proposer} @ {prop_ep}\n   vetoer={vetoer} @ {veto_ep}\n   request: {user}\n",
        if two_box { "TWO-BOX · no churn" } else { "single-box · unload-between" }
    );
    let result = if two_box {
        flux_moe::two_mind_split(&prop_ep, &proposer, &veto_ep, &vetoer, system, &user, tools)
    } else {
        flux_moe::two_mind(&prop_ep, &proposer, &vetoer, system, &user, tools)
    };
    match result {
        Ok(d) => {
            println!("TOOL        {}", d.tool);
            println!("CLASS       {:?}", d.class);
            println!("EXECUTE     {}", d.execute);
            println!("HUMAN REQ   {}", d.requires_human);
            println!("SIGNERS     {:?}", d.signers);
            println!("REASON      {}", d.reason);
        }
        Err(e) => { eprintln!("two_mind failed: {e}"); std::process::exit(1); }
    }
}
