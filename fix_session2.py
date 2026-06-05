path = "/home/storage/deepseek-codewhale/flux/crates/fluxc-mcp/src/handlers/session.rs"
with open(path) as f:
    c = f.read()

# 1. Update flux_v4_optimize text response with real measurements
old_text = '''"⚡ Flux V4 Optimization Report — v0.9.10-beta1\\n\\n\\
         🔄 Prefix Caching: 128-token granularity, ~90% cost discount on cache hits\\n\\
         ⚡ Parallel Execution: batch independent operations in one turn, ~67% latency reduction\\n\\
         🧠 Thinking Tokens: use strategically — skip for lookups, deep for architecture\\n\\
         📦 MCP Phrasal Verbs: 6 active (combo/quickcast/ult/fullcheck/quickstart/bootstrap), 67-80% token savings\\n\\
         \\n\\
         💡 Top Recommendation: Structure turn sequences to maximize prefix-cache reuse.\\n\\
            Append new content at the end, never reorder earlier messages.".to_string()'''

new_text = '''"⚡ Flux V4 Optimization — v0.9.17\\n\\n\\
         🔄 Prefix Cache: 128-token gran, 90% discount on hits.\\n\\
            Cold start: responses = cache MISS (full price). Warm: 90% off.\\n\\
         ⚡ Parallel: batch reads/searches in one turn (67% faster).\\n\\
         📦 Phrasal Verbs: 7 tools (67-80% savings):\\n\\
            combo quickcast ult fullcheck quickstart bootstrap architect_predict\\n\\
         💰 Per-Tool Token Cost (v0.9.17 audit):\\n\\
            flux_bootstrap 4500ch $0.00016 flux_quickstart 3500ch $0.00012\\n\\
            flux_architect 2500ch $0.00009 flux_swot 2000ch $0.00007\\n\\
         💡 Skip quickstart/bootstrap. Read instructions.md directly —\\n\\
            prefix-cached in system prompt = ~90% cheaper.".to_string()'''

c = c.replace(old_text, new_text)

# 2. Update flux_bootstrap to NOT call full quickstart (just state summary)
old_bootstrap = '''fn flux_bootstrap(args: &Value) -> String {
    let context = args.get("context").and_then(|v| v.as_str()).unwrap_or("");
    let start = std::time::Instant::now();
    let mut report = Vec::new();

    // Phase 1: Quickstart
    let quickstart = flux_quickstart(args);
    report.push(quickstart);

    // Phase 2: Diagnose
    let _package = "fluxc";
    report.push("".to_string());
    report.push("## Phase 2: Diagnose (health snapshot)".to_string());'''

new_bootstrap = '''fn flux_bootstrap(args: &Value) -> String {
    let context = args.get("context").and_then(|v| v.as_str()).unwrap_or("");
    let start = std::time::Instant::now();
    let mut report = Vec::new();

    // Phase 1: Compact state (NOT full quickstart — saves ~3000 chars)
    let arch = quantum_architect::analyze_workspace(".");
    report.push("⚡ Flux Bootstrap — v0.9.17".to_string());
    report.push(format!("  Crates: {} | Arch: {:.0}% | LOC: {}",
        arch.crates.len(), arch.architecture_score * 100.0,
        arch.crates.iter().map(|b| b.loc).sum::<usize>()));
    report.push("  MCP: 55+ tools | 7 phrasal verbs | 20 crates".to_string());
    report.push("  Skills: flux-dev, qflux-v2, q-miner-flux, gemma4-flux".to_string());
    report.push("  Rules: Use cargo directly, never rewrite files, commit before deploy".to_string());

    // Phase 2: Diagnose
    report.push("".to_string());
    report.push("## Diagnose (health snapshot)".to_string());'''

c = c.replace(old_bootstrap, new_bootstrap)

with open(path, 'w') as f:
    f.write(c)
print("OK: Updated flux_v4_optimize + flux_bootstrap (leaner)")
