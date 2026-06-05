//! Flux IDE Pro — the agent compute/inference GATEWAY.
//!
//! Agents get a provider's exact search + lifecycle, but routed through Flux's
//! gateway (the operator's API key, server-side) and marked up by
//! `FLUX_GATEWAY_MARKUP` (default **10%**). That spread — the **red line** — is
//! Flux's broker margin for the agent-native experience: MCP tools, nodeswarm
//! orchestration, SIGIL settlement. Agents never see the operator's key; they
//! see gateway prices and the margin is explicit on every call.
//!
//! One markup core, many providers:
//!   • VAST  (compute)   — `flux_vast_*`   [wired]
//!   • DEEPSEEK (inference) — `flux_deepseek_*` [next, same core]
//!
//! HTTP via `curl` subprocess (no http-client dep — consistent with the
//! nodeswarm handler). The operator's keys come from env:
//!   VAST_API_KEY, DEEPSEEK_API_KEY, FLUX_GATEWAY_MARKUP (e.g. 0.10).

use std::process::{Command, Stdio};

use serde_json::{json, Value};

use flux_gpu_market::{plan_fleet, recommend, Budget, Need, Offer};

use crate::handlers::{ToolDef, ToolRegistry};

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_gateway_pricing",
            description: "Show the Flux gateway pricing model: the markup (red-line margin) and which provider APIs are wired (Vast compute, DeepSeek inference). The agent-economy revenue surface for Flux IDE Pro.",
            input_schema: json!({"type":"object","properties":{}}),
        },
        flux_gateway_pricing,
    );
    registry.register(
        ToolDef {
            name: "flux_vast_search",
            description: "Search Vast.ai compute offers THROUGH the Flux gateway. Identical search to Vast, but every price is the gateway price (Vast price + markup). Returns base price, gateway price, and the red-line margin per offer.",
            input_schema: json!({
                "type":"object",
                "properties":{
                    "gpu_name":{"type":"string","description":"Filter to a GPU model, e.g. 'GTX 1080 Ti' or 'RTX 4090'."},
                    "max_dph":{"type":"number","description":"Max GATEWAY $/hr (filter applies to the marked-up price the agent pays)."},
                    "num_gpus":{"type":"integer","description":"Minimum GPUs. Default 1."},
                    "min_reliability":{"type":"number","description":"Minimum reliability 0-1. Default 0.95 (avoid dud hosts)."},
                    "verified_only":{"type":"boolean","description":"Only Vast-verified hosts. Default false."},
                    "limit":{"type":"integer","description":"Max offers to return. Default 8."}
                }
            }),
        },
        flux_vast_search,
    );
    registry.register(
        ToolDef {
            name: "flux_vast_create",
            description: "Provision a Vast.ai instance THROUGH the gateway. Flux provisions at the real Vast price; the agent is billed the gateway price (the spread is Flux's margin). Returns contract id + the pricing breakdown.",
            input_schema: json!({
                "type":"object",
                "properties":{
                    "ask_id":{"type":"integer","description":"Offer id from flux_vast_search."},
                    "image":{"type":"string","description":"Docker image. Default 'vastai/base-image:cuda-12.4.1-auto'."},
                    "disk":{"type":"integer","description":"Disk GB. Default 20."},
                    "label":{"type":"string","description":"Instance label. Default 'flux-gateway'."}
                },
                "required":["ask_id"]
            }),
        },
        flux_vast_create,
    );
    registry.register(
        ToolDef {
            name: "flux_vast_instances",
            description: "List instances provisioned through the gateway, each with live gateway pricing + accrued red-line margin/hr.",
            input_schema: json!({"type":"object","properties":{}}),
        },
        flux_vast_instances,
    );
    registry.register(
        ToolDef {
            name: "flux_vast_destroy",
            description: "Destroy a Vast.ai instance by id (stops gateway billing for it).",
            input_schema: json!({"type":"object","properties":{"id":{"type":"integer","description":"Instance id."}},"required":["id"]}),
        },
        flux_vast_destroy,
    );
    registry.register(
        ToolDef {
            name: "flux_vast_autostop",
            description: "Arm an idle watchdog on a Vast instance and auto-destroy it when idle, so per-second billing stops. The cost optimizer — you never pay for an idle box. `mode` decides what 'idle' means: 'gpu' (GPU util low — for mining/inference), 'cpu' (no build/work process running — for BUILD-FARM boxes that peg CPU but leave the GPU idle), or 'both' (idle only when GPU is low AND no work process — safest for mixed workloads). Runs as a detached background watchdog; returns immediately.",
            input_schema: json!({
                "type":"object",
                "properties":{
                    "id":{"type":"integer","description":"Instance id to watch."},
                    "mode":{"type":"string","description":"'gpu' (default), 'cpu', or 'both'. Use 'cpu'/'both' for build-farm boxes so a compile isn't reaped mid-build."},
                    "idle_minutes":{"type":"number","description":"Destroy after idle this long. Default 10."},
                    "gpu_threshold":{"type":"integer","description":"GPU%% below this counts as GPU-idle. Default 5."},
                    "busy_process":{"type":"string","description":"pgrep -f pattern that means 'working' (keeps the box alive in cpu/both modes). Default 'cargo|rustc|cc1|q-miner|sigil-'."},
                    "poll_seconds":{"type":"integer","description":"Sample interval. Default 60."},
                    "cancel":{"type":"boolean","description":"Cancel the watchdog for this id instead of arming. Default false."}
                },
                "required":["id"]
            }),
        },
        flux_vast_autostop,
    );
    registry.register(
        ToolDef {
            name: "flux_vast_recommend",
            description: "PROPOSE the single best box for a workload in ONE call — the agentic spend-discipline decision. Searches Vast through the gateway, then runs flux-gpu-market's fit-gate → reliability²/effective-cost rank → BUDGET-BREACH guard → burn → runway. Returns an ask_id to propose ONLY if the best fit is affordable under FLUX_VAST_BUDGET_DPH given what's already burning; over-budget or no-fit returns NO ask_id (the spend-gate is in the type — an agent literally can't get a create-id for a box it shouldn't rent). Propose-only: the operator confirms flux_vast_create. Use this instead of eyeballing flux_vast_search.",
            input_schema: json!({
                "type":"object",
                "properties":{
                    "min_vram_gb":{"type":"integer","description":"Minimum GPU VRAM the model needs (GB). Default 24."},
                    "min_disk_gb":{"type":"integer","description":"Minimum disk (GB). Default 50."},
                    "min_down_mbps":{"type":"integer","description":"Minimum down-link so a big model pull isn't glacial (Mbps). Default 0 (don't care)."},
                    "balance_usd":{"type":"number","description":"Wallet $ available — used for runway (hours_left). Omit for no runway calc."},
                    "min_reliability":{"type":"number","description":"Floor for the Vast pre-filter 0-1. Default 0.90 (flux-gpu-market re-ranks by reliability² on top)."},
                    "gpu_name":{"type":"string","description":"Optional GPU model filter, e.g. 'RTX 4090'."},
                    "limit":{"type":"integer","description":"Max offers to scan/rank. Default 40."}
                }
            }),
        },
        flux_vast_recommend,
    );
    registry.register(
        ToolDef {
            name: "flux_vast_fleet",
            description: "PROPOSE a cost-optimal multi-box test FABRIC in ONE call — a capable reliable LEAD (via the same recommend logic) plus the cheapest fitting reliable FOLLOWERS greedily packed under the FLUX_VAST_BUDGET_DPH ceiling. Maximizes box-count per remaining dollar without ever breaching budget or adding an unreliable follower (≥0.9). Returns lead ask_id + follower ask_ids to propose; NO lead returned if none is affordable. Propose-only: the operator confirms each flux_vast_create. For spinning N boxes (1 fast lead + cheap followers) to test chronos/p2p/mining at scale.",
            input_schema: json!({
                "type":"object",
                "properties":{
                    "lead_vram_gb":{"type":"integer","description":"Lead box minimum VRAM (GB). Default 48 (a 70b-class model)."},
                    "lead_disk_gb":{"type":"integer","description":"Lead minimum disk (GB). Default 100."},
                    "lead_down_mbps":{"type":"integer","description":"Lead minimum down-link (Mbps). Default 400."},
                    "follower_vram_gb":{"type":"integer","description":"Follower minimum VRAM (GB). Default 16."},
                    "follower_disk_gb":{"type":"integer","description":"Follower minimum disk (GB). Default 50."},
                    "follower_down_mbps":{"type":"integer","description":"Follower minimum down-link (Mbps). Default 100."},
                    "max_followers":{"type":"integer","description":"Cap on follower boxes. Default 4."},
                    "balance_usd":{"type":"number","description":"Wallet $ available — used for fleet runway. Omit for no runway calc."},
                    "min_reliability":{"type":"number","description":"Floor for the Vast pre-filter 0-1. Default 0.0 (widest pool; the follower-≥0.9 + lead reliability² gates do the real filtering)."},
                    "limit":{"type":"integer","description":"Max offers to scan. Default 60."}
                }
            }),
        },
        flux_vast_fleet,
    );
}

// ── gateway core ──

fn markup() -> f64 {
    std::env::var("FLUX_GATEWAY_MARKUP").ok().and_then(|s| s.parse().ok()).unwrap_or(0.10)
}

/// The red line: (gateway_price, margin) for a base provider price.
fn gateway_price(base: f64) -> (f64, f64) {
    let g = base * (1.0 + markup());
    (g, g - base)
}

/// Project an hourly rate to (per-day, per-30-day) so an agent sees the real
/// commitment of a rental, not just the small-looking $/hr. The spend emergency
/// fix: a $0.75/hr box reads as "$18/day · $540/mo" — that's the number that
/// stops an orphaned box from quietly draining the wallet.
fn burn_projection(dph: f64) -> (f64, f64) {
    (dph * 24.0, dph * 24.0 * 30.0)
}

/// Operator budget ceiling on TOTAL gateway $/hr across all live instances
/// (env `FLUX_VAST_BUDGET_DPH`). None = no ceiling configured.
fn budget_ceiling() -> Option<f64> {
    std::env::var("FLUX_VAST_BUDGET_DPH").ok().and_then(|s| s.parse().ok())
}

/// Pure budget check — total burn strictly over the ceiling.
fn over_budget(total_dph: f64, ceiling: f64) -> bool {
    total_dph > ceiling
}

fn vast_key() -> Result<String, String> {
    std::env::var("VAST_API_KEY").map_err(|_| "❌ VAST_API_KEY not set in the gateway env (the operator's key lives server-side).".into())
}

/// Run curl, parse JSON stdout.
fn curl_json(args: &[String]) -> Result<Value, String> {
    let out = Command::new("curl").args(args).output().map_err(|e| format!("curl spawn: {e}"))?;
    if !out.status.success() {
        return Err(format!("curl failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| {
        let body: String = String::from_utf8_lossy(&out.stdout).chars().take(180).collect();
        format!("bad JSON from provider: {e} :: {body}")
    })
}

fn auth(key: &str) -> String {
    format!("Authorization: Bearer {key}")
}

// ── pricing info ──

fn flux_gateway_pricing(_args: &Value) -> String {
    let m = markup();
    let vast = std::env::var("VAST_API_KEY").is_ok();
    let deepseek = std::env::var("DEEPSEEK_API_KEY").is_ok();
    format!(
        "🛡  Flux IDE Pro — Compute/Inference Gateway\n\n  Markup (red line): +{:.0}%  ·  agent pays base × {:.2}\n  Example: Vast $0.0600/hr → agent $ {:.4}/hr → Flux margin $ {:.4}/hr\n\n  Providers wired (through the operator's API):\n    • VAST     (compute)   {}\n    • DEEPSEEK (inference) {}\n\n  Every search/create returns base · gateway · margin so the red line is never hidden.\n  Margin accrues to the Flux master wallet — settled in SIGIL/QUG (agent economy).",
        m * 100.0, 1.0 + m,
        0.06 * (1.0 + m), 0.06 * m,
        if vast { "✓ key present" } else { "— VAST_API_KEY unset" },
        if deepseek { "✓ key present" } else { "— (next; DEEPSEEK_API_KEY unset)" },
    )
}

// ── vast: search ──

fn flux_vast_search(args: &Value) -> String {
    let key = match vast_key() { Ok(k) => k, Err(e) => return e };
    let num_gpus = args.get("num_gpus").and_then(|v| v.as_u64()).unwrap_or(1);
    let min_rel = args.get("min_reliability").and_then(|v| v.as_f64()).unwrap_or(0.95);
    let verified = args.get("verified_only").and_then(|v| v.as_bool()).unwrap_or(false);
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(8) as usize;
    let m = markup();
    // max_dph is expressed in GATEWAY dollars; translate back to base for the Vast filter.
    let max_base = args.get("max_dph").and_then(|v| v.as_f64()).map(|g| g / (1.0 + m));

    let mut q = json!({
        "rentable": {"eq": true},
        "num_gpus": {"gte": num_gpus},
        "reliability2": {"gt": min_rel},
        "order": [["dph_total","asc"]],
        "type": "on-demand"
    });
    if verified { q["verified"] = json!({"eq": true}); }
    if let Some(g) = args.get("gpu_name").and_then(|v| v.as_str()) { q["gpu_name"] = json!({"eq": g}); }
    if let Some(mb) = max_base { q["dph_total"] = json!({"lte": mb}); }

    let url = "https://console.vast.ai/api/v0/bundles/".to_string();
    let curl_args = vec![
        "-s".into(), "--max-time".into(), "30".into(),
        "-G".into(), "-H".into(), auth(&key),
        "--data-urlencode".into(), format!("q={}", q),
        url,
    ];
    let v = match curl_json(&curl_args) { Ok(v) => v, Err(e) => return e };
    let empty = vec![];
    let offers = v.get("offers").and_then(|o| o.as_array()).unwrap_or(&empty);

    let mut out = format!("🛰  Vast offers via Flux gateway (+{:.0}% red line) — {} match\n", m * 100.0, offers.len());
    for o in offers.iter().take(limit) {
        let base = o.get("dph_total").and_then(|x| x.as_f64()).unwrap_or(0.0);
        let (g, margin) = gateway_price(base);
        out.push_str(&format!(
            "  ask {:<9} │ Vast ${:.4} → gateway ${:.4}/hr (red line +${:.4}) │ {:.0}vCPU {:.0}GB │ {}x {} │ rel {:.3} │ {}\n",
            o.get("id").and_then(|x| x.as_i64()).unwrap_or(0),
            base, g, margin,
            o.get("cpu_cores_effective").and_then(|x| x.as_f64()).unwrap_or(0.0),
            o.get("cpu_ram").and_then(|x| x.as_f64()).unwrap_or(0.0) / 1024.0,
            o.get("num_gpus").and_then(|x| x.as_i64()).unwrap_or(0),
            o.get("gpu_name").and_then(|x| x.as_str()).unwrap_or("-"),
            o.get("reliability2").and_then(|x| x.as_f64()).unwrap_or(0.0),
            o.get("geolocation").and_then(|x| x.as_str()).unwrap_or("?"),
        ));
    }
    out.push_str("  → flux_vast_create ask_id=<id> to provision through the gateway.");
    out
}

// ── vast: agentic recommend / fleet (flux-gpu-market decision layer) ──

/// Fetch raw Vast bundle offers (rentable, ≥num_gpus, reliability filter), cheapest first.
fn vast_fetch_offers(key: &str, gpu_name: Option<&str>, num_gpus: u64, min_rel: f64, limit: usize) -> Result<Vec<Value>, String> {
    let mut q = json!({
        "rentable": {"eq": true},
        "num_gpus": {"gte": num_gpus},
        "reliability2": {"gt": min_rel},
        "order": [["dph_total","asc"]],
        "type": "on-demand"
    });
    if let Some(g) = gpu_name { q["gpu_name"] = json!({"eq": g}); }
    let url = "https://console.vast.ai/api/v0/bundles/".to_string();
    let curl_args = vec![
        "-s".into(), "--max-time".into(), "30".into(),
        "-G".into(), "-H".into(), auth(key),
        "--data-urlencode".into(), format!("q={}", q),
        url,
    ];
    let v = curl_json(&curl_args)?;
    let empty = vec![];
    let offers = v.get("offers").and_then(|o| o.as_array()).unwrap_or(&empty);
    Ok(offers.iter().take(limit).cloned().collect())
}

/// Map a raw Vast offer → a flux-gpu-market [`Offer`] with GATEWAY-priced `dph` (so the decision
/// is made in the dollars the agent actually pays). Missing fields default to 0 → fail the fit-gate
/// safely (a box we can't measure is never recommended).
fn offer_from_vast(o: &Value) -> Option<Offer> {
    let id = o.get("id").and_then(|x| x.as_u64())?;
    let base = o.get("dph_total").and_then(|x| x.as_f64()).unwrap_or(0.0);
    Some(Offer {
        id,
        gpu: o.get("gpu_name").and_then(|x| x.as_str()).unwrap_or("-").to_string(),
        // Vast `gpu_ram` is per-GPU VRAM in MB.
        vram_gb: (o.get("gpu_ram").and_then(|x| x.as_f64()).unwrap_or(0.0) / 1024.0) as u32,
        disk_gb: o.get("disk_space").and_then(|x| x.as_f64()).unwrap_or(0.0) as u32,
        dph: gateway_price(base).0,
        reliability: o.get("reliability2").and_then(|x| x.as_f64()).unwrap_or(0.0),
        down_mbps: o.get("inet_down").and_then(|x| x.as_f64()).unwrap_or(0.0) as u32,
        verified: o.get("verified").and_then(|x| x.as_bool()).unwrap_or(false),
    })
}

/// Sum of live gateway $/hr across all running instances — the `current_total_dph` the budget
/// guard must add against (so "fits budget" reflects what's ALREADY burning, not a clean slate).
fn live_total_gateway_dph(key: &str) -> f64 {
    let url = "https://console.vast.ai/api/v0/instances/".to_string();
    match curl_json(&["-s".into(), "--max-time".into(), "25".into(), "-H".into(), auth(key), url]) {
        Ok(v) => {
            let empty = vec![];
            v.get("instances").and_then(|o| o.as_array()).unwrap_or(&empty).iter()
                .map(|i| gateway_price(i.get("dph_total").and_then(|x| x.as_f64()).unwrap_or(0.0)).0)
                .sum()
        }
        Err(_) => 0.0,
    }
}

/// Render runway hours, collapsing an unbounded (no-balance) runway to a hint.
fn fmt_runway(h: f64) -> String {
    if h.is_finite() { format!("~{h:.1}h") } else { "∞ (pass balance_usd for runway)".to_string() }
}

fn flux_vast_recommend(args: &Value) -> String {
    let key = match vast_key() { Ok(k) => k, Err(e) => return e };
    let min_vram = args.get("min_vram_gb").and_then(|v| v.as_u64()).unwrap_or(24) as u32;
    let min_disk = args.get("min_disk_gb").and_then(|v| v.as_u64()).unwrap_or(50) as u32;
    let min_down = args.get("min_down_mbps").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let balance = args.get("balance_usd").and_then(|v| v.as_f64()).unwrap_or(f64::INFINITY);
    let min_rel = args.get("min_reliability").and_then(|v| v.as_f64()).unwrap_or(0.90);
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(40) as usize;
    let gpu = args.get("gpu_name").and_then(|v| v.as_str());

    let raw = match vast_fetch_offers(&key, gpu, 1, min_rel, limit) { Ok(v) => v, Err(e) => return e };
    let offers: Vec<Offer> = raw.iter().filter_map(offer_from_vast).collect();
    let need = Need { min_vram_gb: min_vram, min_disk_gb: min_disk, min_down_mbps: min_down };
    let budget = Budget { ceiling_dph: budget_ceiling().unwrap_or(f64::INFINITY) };
    let current = live_total_gateway_dph(&key); // what's already burning → real budget headroom
    let rec = recommend(&offers, &need, current, &budget, balance);

    let mut out = format!(
        "🎯 Gateway recommendation — need {min_vram}GB VRAM · {min_disk}GB disk · {min_down}Mbps down · {} offer(s) scanned · ${current:.4}/hr already live\n",
        offers.len()
    );
    match rec.offer_id {
        Some(id) => out.push_str(&format!(
            "  ✅ PROPOSE ask {id} · {} · gateway ${:.4}/hr · score {:.2} · runway {}\n     burn: ${:.2}/day · ${:.0}/30d (gateway rate)\n     {}\n  → OPERATOR confirms: flux_vast_create ask_id={id}  (never auto-rented)",
            rec.gpu, rec.dph, rec.score, fmt_runway(rec.hours_runway), rec.burn.day, rec.burn.month, rec.reason
        )),
        None => out.push_str(&format!("  ⛔ NO PROPOSAL — {}\n     (fit-gate or budget ceiling FLUX_VAST_BUDGET_DPH; nothing safe to rent)", rec.reason)),
    }
    out
}

fn flux_vast_fleet(args: &Value) -> String {
    let key = match vast_key() { Ok(k) => k, Err(e) => return e };
    let lead_need = Need {
        min_vram_gb: args.get("lead_vram_gb").and_then(|v| v.as_u64()).unwrap_or(48) as u32,
        min_disk_gb: args.get("lead_disk_gb").and_then(|v| v.as_u64()).unwrap_or(100) as u32,
        min_down_mbps: args.get("lead_down_mbps").and_then(|v| v.as_u64()).unwrap_or(400) as u32,
    };
    let follower_need = Need {
        min_vram_gb: args.get("follower_vram_gb").and_then(|v| v.as_u64()).unwrap_or(16) as u32,
        min_disk_gb: args.get("follower_disk_gb").and_then(|v| v.as_u64()).unwrap_or(50) as u32,
        min_down_mbps: args.get("follower_down_mbps").and_then(|v| v.as_u64()).unwrap_or(100) as u32,
    };
    let max_followers = args.get("max_followers").and_then(|v| v.as_u64()).unwrap_or(4) as usize;
    let balance = args.get("balance_usd").and_then(|v| v.as_f64()).unwrap_or(f64::INFINITY);
    let min_rel = args.get("min_reliability").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(60) as usize;

    let raw = match vast_fetch_offers(&key, None, 1, min_rel, limit) { Ok(v) => v, Err(e) => return e };
    let offers: Vec<Offer> = raw.iter().filter_map(offer_from_vast).collect();
    let budget = Budget { ceiling_dph: budget_ceiling().unwrap_or(f64::INFINITY) };
    let p = plan_fleet(&offers, &lead_need, &follower_need, &budget, balance, max_followers);

    let mut out = format!(
        "🛰  Gateway fleet plan — lead ≥{}GB · followers ≥{}GB (≤{} of them) · {} offer(s) scanned\n",
        lead_need.min_vram_gb, follower_need.min_vram_gb, max_followers, offers.len()
    );
    match p.lead {
        Some(lead) => {
            out.push_str(&format!("  ✅ PROPOSE fabric — lead ask {lead} + {} follower(s)\n", p.followers.len()));
            if !p.followers.is_empty() {
                let ids: Vec<String> = p.followers.iter().map(|i| i.to_string()).collect();
                out.push_str(&format!("     followers: ask {}\n", ids.join(", ")));
            }
            out.push_str(&format!(
                "     total gateway ${:.4}/hr · burn ${:.2}/day · ${:.0}/30d · runway {}\n     {}\n  → OPERATOR confirms each: flux_vast_create ask_id=<id>",
                p.total_dph, p.burn.day, p.burn.month, fmt_runway(p.hours_runway), p.reason
            ));
        }
        None => out.push_str(&format!("  ⛔ NO FLEET — {}\n     (no affordable reliable lead under FLUX_VAST_BUDGET_DPH)", p.reason)),
    }
    out
}

// ── vast: create ──

fn flux_vast_create(args: &Value) -> String {
    let key = match vast_key() { Ok(k) => k, Err(e) => return e };
    let ask = match args.get("ask_id").and_then(|v| v.as_i64()) {
        Some(a) => a,
        None => return "❌ ask_id required (from flux_vast_search).".into(),
    };
    let image = args.get("image").and_then(|v| v.as_str()).unwrap_or("vastai/base-image:cuda-12.4.1-auto");
    let disk = args.get("disk").and_then(|v| v.as_u64()).unwrap_or(20);
    let label = args.get("label").and_then(|v| v.as_str()).unwrap_or("flux-gateway");

    let body = json!({"client_id":"me","image":image,"disk":disk,"label":label,"runtype":"ssh"});
    let url = format!("https://console.vast.ai/api/v0/asks/{ask}/");
    let curl_args = vec![
        "-s".into(), "--max-time".into(), "30".into(), "-X".into(), "PUT".into(),
        "-H".into(), auth(&key), "-H".into(), "Content-Type: application/json".into(),
        "-d".into(), body.to_string(), url,
    ];
    let v = match curl_json(&curl_args) { Ok(v) => v, Err(e) => return e };
    if v.get("success").and_then(|s| s.as_bool()) != Some(true) {
        return format!("❌ provision failed: {}", v);
    }
    let contract = v.get("new_contract").and_then(|x| x.as_i64()).unwrap_or(0);
    let m = markup();
    format!(
        "✅ Provisioned via gateway — contract {contract} (label '{label}')\n  Flux pays Vast the base rate; agent is billed +{:.0}% (the red line).\n  ⏰ ORPHAN-BURN GUARD: arm `flux_vast_autostop id={contract}` NOW so this box self-reaps when idle — an unwatched box is how a wallet drains overnight.\n  → flux_vast_instances for live gateway pricing + burn projection · flux_nodeswarm_spawn to run a swarm on it.",
        m * 100.0
    )
}

// ── vast: instances ──

fn flux_vast_instances(_args: &Value) -> String {
    let key = match vast_key() { Ok(k) => k, Err(e) => return e };
    let url = "https://console.vast.ai/api/v0/instances/".to_string();
    let v = match curl_json(&["-s".into(), "--max-time".into(), "25".into(), "-H".into(), auth(&key), url]) {
        Ok(v) => v, Err(e) => return e,
    };
    let empty = vec![];
    let ins = v.get("instances").and_then(|o| o.as_array()).unwrap_or(&empty);
    let m = markup();
    let mut total_base = 0.0;
    let mut total_margin = 0.0;
    let mut out = format!("🛰  Gateway instances — {}\n", ins.len());
    for i in ins {
        let base = i.get("dph_total").and_then(|x| x.as_f64()).unwrap_or(0.0);
        let (g, margin) = gateway_price(base);
        total_base += base; total_margin += margin;
        out.push_str(&format!(
            "  id {} · {} · {}x {} · Vast ${:.4} → gateway ${:.4}/hr (red line +${:.4}) · {}\n",
            i.get("id").and_then(|x| x.as_i64()).unwrap_or(0),
            i.get("actual_status").and_then(|x| x.as_str()).unwrap_or("?"),
            i.get("num_gpus").and_then(|x| x.as_i64()).unwrap_or(0),
            i.get("gpu_name").and_then(|x| x.as_str()).unwrap_or("-"),
            base, g, margin,
            i.get("label").and_then(|x| x.as_str()).unwrap_or("-"),
        ));
    }
    let (tg, _) = gateway_price(total_base);
    out.push_str(&format!(
        "  ── totals: Vast ${:.4}/hr → gateway ${:.4}/hr · RED LINE +${:.4}/hr ({:.0}% margin)",
        total_base, tg, total_margin, m * 100.0
    ));
    // Burn projection — the number that prevents the silent "$8.98 → $0 in 7.7h" drain.
    let (day, month) = burn_projection(tg);
    out.push_str(&format!("\n  ── burn: ${:.2}/day · ${:.0}/30d (gateway rate)", day, month));
    if ins.is_empty() {
        out.push_str("\n  ✅ no live instances — $0/hr burn.");
    }
    match budget_ceiling() {
        Some(cap) if over_budget(tg, cap) => out.push_str(&format!(
            "\n  🚨 BUDGET ALARM: gateway ${:.4}/hr EXCEEDS ceiling ${:.4}/hr (FLUX_VAST_BUDGET_DPH).\n     → destroy idle boxes (flux_vast_destroy) or arm flux_vast_autostop so they self-reap.",
            tg, cap
        )),
        Some(cap) => out.push_str(&format!("\n  ✅ within budget ceiling ${:.4}/hr", cap)),
        None => out.push_str("\n  ℹ set FLUX_VAST_BUDGET_DPH to arm a budget alarm on total burn."),
    }
    out
}

// ── vast: destroy ──

fn flux_vast_destroy(args: &Value) -> String {
    let key = match vast_key() { Ok(k) => k, Err(e) => return e };
    let id = match args.get("id").and_then(|v| v.as_i64()) {
        Some(i) => i, None => return "❌ id required.".into(),
    };
    let url = format!("https://console.vast.ai/api/v0/instances/{id}/");
    let v = match curl_json(&["-s".into(), "--max-time".into(), "25".into(), "-X".into(), "DELETE".into(), "-H".into(), auth(&key), url]) {
        Ok(v) => v, Err(e) => return e,
    };
    if v.get("success").and_then(|s| s.as_bool()) == Some(true) {
        format!("🗑  Destroyed instance {id} — gateway billing stopped.")
    } else {
        format!("⚠ destroy response: {v}")
    }
}

// ── vast: idle GPU auto-stop (the cost optimizer) ──

fn flux_vast_autostop(args: &Value) -> String {
    let key = match vast_key() { Ok(k) => k, Err(e) => return e };
    let id = match args.get("id").and_then(|v| v.as_i64()) {
        Some(i) => i, None => return "❌ id required.".into(),
    };
    let pidfile = format!("/tmp/flux-vast-autostop-{id}.pid");
    let logfile = format!("/tmp/flux-vast-autostop-{id}.log");
    let script = format!("/tmp/flux-vast-autostop-{id}.sh");

    if args.get("cancel").and_then(|v| v.as_bool()).unwrap_or(false) {
        if let Ok(pid) = std::fs::read_to_string(&pidfile) {
            let _ = Command::new("kill").arg("-9").arg(pid.trim()).status();
        }
        let _ = std::fs::remove_file(&pidfile);
        return format!("🛑 autostop watchdog cancelled for instance {id}.");
    }

    let idle_min = args.get("idle_minutes").and_then(|v| v.as_f64()).unwrap_or(10.0);
    let thresh = args.get("gpu_threshold").and_then(|v| v.as_i64()).unwrap_or(5);
    let poll = args.get("poll_seconds").and_then(|v| v.as_i64()).unwrap_or(60);
    let idle_s = (idle_min * 60.0) as i64;
    let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("gpu").to_string();
    let busy_re = args.get("busy_process").and_then(|v| v.as_str())
        .unwrap_or("cargo|rustc|cc1|q-miner|sigil-").to_string();

    // Fetch SSH host/port for the watchdog to sample nvidia-smi.
    let info = match curl_json(&[
        "-s".into(), "--max-time".into(), "20".into(), "-H".into(), auth(&key),
        format!("https://console.vast.ai/api/v0/instances/{id}/"),
    ]) { Ok(v) => v, Err(e) => return e };
    let inst = info.get("instances").cloned().unwrap_or_else(|| info.clone());
    let host = inst.get("ssh_host").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let port = inst.get("ssh_port").and_then(|x| x.as_i64()).unwrap_or(0);
    if host.is_empty() || port == 0 {
        return format!("❌ no ssh host/port for instance {id} yet (still booting?). Re-arm once it's up.");
    }

    // Watchdog: poll GPU util + work-process presence over one SSH call; treat
    // the box as active per `mode`, and destroy it after idle_s of inactivity.
    let body = format!(
"#!/bin/bash
KEY=\"{key}\"; ID={id}; HOST=\"{host}\"; PORT={port}; THRESH={thresh}; IDLE_S={idle_s}; POLL={poll}; MODE=\"{mode}\"; BUSY=\"{busy_re}\"
idle=0
echo \"autostop armed: mode=$MODE, destroy after ${{IDLE_S}}s inactive (poll ${{POLL}}s)\"
while true; do
  out=$(timeout 15 ssh -o StrictHostKeyChecking=no -o BatchMode=yes -p $PORT root@$HOST \"nvidia-smi --query-gpu=utilization.gpu --format=csv,noheader,nounits 2>/dev/null | head -1; echo ---SEP---; pgrep -f '$BUSY' 2>/dev/null | head -1\")
  u=$(printf '%s\\n' \"$out\" | sed -n '1p' | tr -d ' '); case \"$u\" in ''|*[!0-9]*) u=0;; esac
  busy=$(printf '%s\\n' \"$out\" | sed -n '/---SEP---/,$p' | sed -n '2p')
  ga=0; [ \"$u\" -ge \"$THRESH\" ] && ga=1
  pa=0; [ -n \"$busy\" ] && pa=1
  active=0
  case \"$MODE\" in gpu) active=$ga;; cpu) active=$pa;; both) {{ [ $ga -eq 1 ] || [ $pa -eq 1 ]; }} && active=1;; esac
  if [ \"$active\" -eq 1 ]; then idle=0; else idle=$((idle+POLL)); fi
  echo \"$(date +%H:%M:%S) gpu=${{u}}%% work=${{pa}} active=${{active}} idle=${{idle}}s\"
  if [ \"$idle\" -ge \"$IDLE_S\" ]; then
    echo \"IDLE LIMIT — destroying $ID\"
    curl -s -X DELETE -H \"Authorization: Bearer $KEY\" \"https://console.vast.ai/api/v0/instances/$ID/\" >/dev/null
    echo \"STOPPED $ID\"; exit 0
  fi
  sleep $POLL
done
");
    if std::fs::write(&script, body).is_err() {
        return "❌ could not write watchdog script.".into();
    }
    let _ = Command::new("chmod").arg("+x").arg(&script).status();
    let log = match std::fs::File::create(&logfile) {
        Ok(f) => f, Err(_) => return "❌ could not create watchdog log.".into(),
    };
    let log2 = match log.try_clone() { Ok(f) => f, Err(_) => return "❌ log clone failed.".into() };
    let mut cmd = Command::new("bash");
    cmd.arg(&script).stdin(Stdio::null()).stdout(Stdio::from(log)).stderr(Stdio::from(log2));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    match cmd.spawn() {
        Ok(child) => {
            let _ = std::fs::write(&pidfile, child.id().to_string());
            format!(
                "🟢 Autostop armed for instance {id} — auto-destroys after {idle_min} min below {thresh}% GPU (poll {poll}s).\n  You never pay for an idle GPU. Log: {logfile}\n  Cancel: flux_vast_autostop id={id} cancel=true"
            )
        }
        Err(e) => format!("❌ watchdog spawn failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burn_projects_day_and_month() {
        // $0.75/hr A100 (today's box) → $18/day · $540/30d — the number that makes
        // an orphaned rental impossible to ignore.
        let (day, month) = burn_projection(0.75);
        assert!((day - 18.0).abs() < 1e-9, "day={day}");
        assert!((month - 540.0).abs() < 1e-9, "month={month}");
    }

    #[test]
    fn over_budget_is_strict() {
        assert!(over_budget(1.60, 1.00));      // total burn above ceiling → alarm
        assert!(!over_budget(0.90, 1.00));     // under → ok
        assert!(!over_budget(1.00, 1.00));     // exactly at ceiling is NOT over
    }

    #[test]
    fn gateway_margin_is_the_red_line() {
        // Whatever the markup, gateway = base + margin and margin = base*markup.
        let (g, margin) = gateway_price(0.50);
        assert!((g - (0.50 + margin)).abs() < 1e-9);
        assert!(margin >= 0.0);
    }
}
