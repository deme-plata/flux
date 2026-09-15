//! `flux_sigil_wallet_clock` + `flux_sigil_clock_*` — the SIGIL datacenter Earth clock for the
//! wallet MCP, built on the `flux-clock` crate.
//!
//! * `flux_sigil_wallet_clock` — one reading of Kristensen Time (seconds / Margolus–Levitin
//!   ticks / agreed ticks), the temperatures `T_L`/`T_F`, the Earth Rotation Angle hand from
//!   the sigil-earth feed, the Earth's slow hands fitted from the feed's series with the
//!   honest CRT-calendar verdict, and the ledger as a Chinese-remainder clock. Pushed to every
//!   registered webhook as the `sigil_earth_clock` event.
//! * `flux_sigil_clock_crt` — run the arXiv:2608.07938 protocol (defaults = the paper's Fig. 2)
//!   and compare with the printed numbers.
//! * `flux_sigil_clock_hand` — this node's hand envelope for a period (what a peer publishes on
//!   `/sigil/g2/clock`); the decentralized composition itself runs as `flux-clock collect`.
//! * `flux_sigil_clock_collect` — run `flux-clock collect` for a few seconds and return the
//!   composed reading (needs the binary beside `fluxc` or `FLUX_CLOCK_BIN`).
//!
//! Endpoints: node `FLUX_SIGIL_RPC` (default `http://127.0.0.1:18181`, the same base every
//! wallet tool uses), earth `FLUX_SIGIL_EARTH` (default `http://127.0.0.1:8460`, the
//! sigil-earth service; `https://sigilgraph.org` works off-box).

use crate::handlers::{ToolDef, ToolRegistry};
use flux_clock::{crt, face, kristensen, live, p2p};
use fluxc_webhooks::webhook;
use serde_json::{json, Value};

pub const EVENT: &str = face::EVENT;

fn earth_base(a: &Value) -> String {
    a.get("earth_url").and_then(|v| v.as_str()).map(String::from)
        .unwrap_or_else(|| std::env::var("FLUX_SIGIL_EARTH").unwrap_or_else(|_| "http://127.0.0.1:8460".into()))
}

fn wallet_clock(a: &Value) -> String {
    let node = crate::handlers::sigil_wallet::rpc_base();
    let earth = earth_base(a);
    let energy = a.get("energy_j").and_then(|v| v.as_f64()).unwrap_or(kristensen::E_CORE_15W_1S_J);
    let sample = a.get("sample_secs").and_then(|v| v.as_f64()).unwrap_or(4.0).clamp(1.0, 30.0);
    let hook = a.get("webhook").and_then(|v| v.as_bool()).unwrap_or(true);
    match live::live_inputs(&node, &earth, energy, sample) {
        Ok(i) => {
            let mut v = face::read(&i);
            if hook { webhook::auto_dispatch(EVENT, v.clone()); v["webhook_event"] = json!(EVENT); v["webhook_listeners"] = json!(webhook::count_listeners(EVENT)); }
            v["reading"] = json!(format!("h={} · {:.2} blk/s · T_L={:.2e} K · ERA={:.3}° · K⊕={} {}", i.height, i.bps, kristensen::ledger_temperature_k(1.0 / i.bps.max(1e-9)), v["earth"]["rotation_hand"]["era_deg"].as_f64().unwrap_or(f64::NAN), i.k_earth.map(|k| format!("{k:.2}σ")).unwrap_or_else(|| "n/a".into()), i.k_earth_regime.clone().unwrap_or_default()));
            v.to_string()
        }
        Err(e) => json!({"ok": false, "error": e, "hint": "is sigil-api reachable (FLUX_SIGIL_RPC) and sigil-earth serve (FLUX_SIGIL_EARTH, :8460 on Epsilon)?"}).to_string(),
    }
}

fn clock_crt(a: &Value) -> String {
    let periods: Vec<u64> = a.get("periods").and_then(|v| v.as_array()).map(|arr| arr.iter().filter_map(|x| x.as_u64()).collect()).filter(|p: &Vec<u64>| !p.is_empty()).unwrap_or_else(|| crt::FIGURE2_PERIODS.to_vec());
    if !crt::pairwise_coprime(&periods) { return json!({"ok": false, "error": "periods must be pairwise coprime", "periods": periods}).to_string(); }
    let trials = a.get("trials").and_then(|v| v.as_u64()).unwrap_or(1000).clamp(10, 200_000) as u32;
    let seed = a.get("seed").and_then(|v| v.as_u64()).unwrap_or(20_260_811);
    let reports: Vec<crt::SimulationReport> = match a.get("z").and_then(|v| v.as_u64()) {
        Some(z) => vec![crt::simulate(&periods, z as u32, trials, seed)],
        None => crt::FIGURE2_Z.iter().map(|&z| crt::simulate(&periods, z, trials, seed ^ z as u64)).collect(),
    };
    let is_fig2 = periods == crt::FIGURE2_PERIODS;
    json!({
        "ok": true, "paper": "arXiv:2608.07938 — Quantum Chinese Remainder Clock (Nagar, Hamma, Palmero, Radzihovsky, Yang, Lloyd)",
        "periods": periods, "range": crt::range(&periods).to_string(), "trials": trials,
        "z_min_for_99pct": crt::z_min(periods.len(), 0.99), "z_min_shorthand": crt::z_min_approx(periods.len(), 0.01),
        "paper_fig2_p_err_lt_1": if is_fig2 { json!(crt::FIGURE2_P) } else { Value::Null },
        "reports": reports,
        "note": "P(|error|<1) per Z; catastrophic = wrong integer part (errors of order the range); sigma_predicted = 1/(2Z√m)",
    }).to_string()
}

fn clock_hand(a: &Value) -> String {
    let period = match a.get("period").and_then(|v| v.as_u64()) { Some(p) if p >= 2 => p, _ => return json!({"ok": false, "error": "period (integer ≥ 2) required", "recommended": p2p::RECOMMENDED_PERIODS}).to_string() };
    let unit = a.get("unit").and_then(|v| v.as_str()).unwrap_or("block").to_string();
    let node = crate::handlers::sigil_wallet::rpc_base();
    let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("fluxc-mcp").to_string();
    let t = if unit == "block" { live::height(&node).map(|h| h as f64) } else {
        let ut1 = live::get_json(&format!("{}/v1/earth/latest", earth_base(a))).ok().and_then(|e| e["now"]["ut1utc_today"].as_f64()).unwrap_or(0.0);
        Some(flux_clock::now_unix() + ut1)
    };
    match t {
        Some(t) => {
            let e = p2p::HandEnvelope::new(&name, &unit, period, t.rem_euclid(period as f64), 1);
            json!({"ok": true, "envelope": e, "topic": p2p::CLOCK_TOPIC, "publish_with": format!("flux-clock hand --period {period} --unit {unit} --peer <collector multiaddr>"), "note": "the hand carries only t mod period; the time is recoverable only by composing enough independent hands"}).to_string()
        }
        None => json!({"ok": false, "error": "could not read the time source (node height)"}).to_string(),
    }
}

fn clock_collect(a: &Value) -> String {
    let bin = std::env::var("FLUX_CLOCK_BIN").ok().map(std::path::PathBuf::from)
        .or_else(|| std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("flux-clock"))))
        .filter(|p| p.exists());
    let Some(bin) = bin else { return json!({"ok": false, "error": "flux-clock binary not found (build with `fluxc build -p flux-clock` or set FLUX_CLOCK_BIN)"}).to_string() };
    let secs = a.get("secs").and_then(|v| v.as_f64()).unwrap_or(20.0).clamp(2.0, 120.0);
    let bound = a.get("bound").and_then(|v| v.as_u64()).unwrap_or(p2p::DEFAULT_BOUND as u64);
    let unit = a.get("unit").and_then(|v| v.as_str()).unwrap_or("block");
    let listen = a.get("listen").and_then(|v| v.as_str()).unwrap_or("/ip4/0.0.0.0/tcp/0");
    let mut cmd = std::process::Command::new(&bin);
    cmd.arg("collect").arg("--secs").arg(format!("{secs}")).arg("--bound").arg(bound.to_string()).arg("--unit").arg(unit).arg("--listen").arg(listen);
    if let Some(peers) = a.get("peers").and_then(|v| v.as_array()) { for p in peers.iter().filter_map(|x| x.as_str()) { cmd.arg("--peer").arg(p); } }
    if a.get("webhook").and_then(|v| v.as_bool()).unwrap_or(true) { cmd.arg("--webhook"); }
    match cmd.output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            match serde_json::from_str::<Value>(&stdout) {
                Ok(v) => json!({"ok": out.status.success(), "collect": v, "stderr": String::from_utf8_lossy(&out.stderr).chars().take(400).collect::<String>()}).to_string(),
                Err(_) => json!({"ok": false, "error": "flux-clock collect produced no JSON", "stdout": stdout.chars().take(400).collect::<String>(), "stderr": String::from_utf8_lossy(&out.stderr).chars().take(400).collect::<String>()}).to_string(),
            }
        }
        Err(e) => json!({"ok": false, "error": format!("spawn {}: {e}", bin.display())}).to_string(),
    }
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(ToolDef {
        name: "flux_sigil_wallet_clock",
        description: "The SIGIL datacenter Earth clock: Kristensen Time (seconds / Margolus–Levitin ticks / agreed ticks = height) with T_L, T_F, γ_C, the Earth Rotation Angle hand (UT1 from the sigil-earth feed), the Earth's slow hands + CRT-calendar verdict, and the ledger as a Chinese-remainder clock (arXiv:2608.07938). Every number labelled MEASURED/DERIVED/ANALOGY/MODEL_CHOICE. Fires the `sigil_earth_clock` webhook. Args: sample_secs (block-rate window, default 4), energy_j (default 15), earth_url, webhook (default true).",
        input_schema: json!({"type":"object","properties":{"sample_secs":{"type":"number"},"energy_j":{"type":"number"},"earth_url":{"type":"string"},"webhook":{"type":"boolean"}}}),
    }, wallet_clock);
    registry.register(ToolDef {
        name: "flux_sigil_clock_crt",
        description: "Run the quantum Chinese-remainder clock protocol of arXiv:2608.07938 (Nagar, Hamma, Palmero, Radzihovsky, Yang, Lloyd): m hands with pairwise-coprime periods, Heisenberg-limited phase measurement (their Eq. 6), the fault-tolerant reconstruction, Z_min (Eq. 8). Defaults reproduce their Fig. 2 (periods 2,3,5,7,11; Z 1,3,5,7; 1000 trials). Args: periods[], z, trials, seed.",
        input_schema: json!({"type":"object","properties":{"periods":{"type":"array","items":{"type":"integer"}},"z":{"type":"integer"},"trials":{"type":"integer"},"seed":{"type":"integer"}}}),
    }, clock_crt);
    registry.register(ToolDef {
        name: "flux_sigil_clock_hand",
        description: "This node's hand of the decentralized CRT clock: the envelope {period, t mod period} a peer publishes on /sigil/g2/clock (unit block = height from sigil-api, or second = UT1). Args: period (coprime with the other peers'; recommended 10007/10009/10037), unit, name.",
        input_schema: json!({"type":"object","properties":{"period":{"type":"integer"},"unit":{"type":"string","enum":["block","second"]},"name":{"type":"string"}},"required":["period"]}),
    }, clock_hand);
    registry.register(ToolDef {
        name: "flux_sigil_clock_collect",
        description: "Listen on flux-p2p for hands of the decentralized CRT clock for `secs` seconds and compose the time (the paper's protocol + redundancy: names a peer whose hand disagrees). Runs the flux-clock binary. Args: secs (default 20), bound (default 1e8), unit, listen, peers[] (multiaddrs), webhook.",
        input_schema: json!({"type":"object","properties":{"secs":{"type":"number"},"bound":{"type":"integer"},"unit":{"type":"string"},"listen":{"type":"string"},"peers":{"type":"array","items":{"type":"string"}},"webhook":{"type":"boolean"}}}),
    }, clock_collect);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crt_tool_reproduces_the_paper_by_default() {
        let out: Value = serde_json::from_str(&clock_crt(&json!({"trials": 400}))).unwrap();
        assert_eq!(out["ok"], true);
        assert_eq!(out["range"], "2310");
        let reps = out["reports"].as_array().unwrap();
        assert_eq!(reps.len(), 4);
        assert!(reps[3]["p_err_lt_1"].as_f64().unwrap() > 0.97);
        assert!(reps[0]["p_err_lt_1"].as_f64().unwrap() < 0.25);
        let bad: Value = serde_json::from_str(&clock_crt(&json!({"periods": [4, 6]}))).unwrap();
        assert_eq!(bad["ok"], false);
    }

    #[test]
    fn hand_tool_requires_a_period() {
        let out: Value = serde_json::from_str(&clock_hand(&json!({}))).unwrap();
        assert_eq!(out["ok"], false);
        assert_eq!(out["recommended"][0], 10_007);
    }
}
