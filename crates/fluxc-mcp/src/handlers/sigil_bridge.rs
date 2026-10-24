//! `flux_sigil_bridge` — the SIGIL ↔ wSIGIL3 (Polygon) bridge, both sides, in one call.
//!
//! Read-only. It never signs, never sends, never touches a key file. It answers the
//! questions that used to take a journalctl session, three curls and an `eth_call` script:
//!
//! * **Backing (the peg).** How much SIGIL the shielded vault holds on the SIGIL chain
//!   (every pool epoch — see `sigil-api::bridge_vault::PoolSnapshot`), against how much
//!   wSIGIL3 the bridge has issued on Polygon. The token was deployed with a 1.0 wSIGIL3
//!   pool seed that no lock backs; that seed is reported separately, never hidden inside
//!   "minted".
//! * **The route.** What `amount_sigil` SIGIL becomes on the other side: 1:1 into wSIGIL3
//!   through the bridge (no bridge fee), then into USDC through the Uniswap V2 pair, with
//!   the 0.3 % fee and the price impact of THIS size against THESE reserves.
//! * **Readiness.** Is the relayer running, does its key equal the contract's
//!   `operator()`, does the node report the vault honestly, is the pool deep enough to be
//!   a price at all, does USDS have a live oracle — ranked, with who has to act.
//!
//! Every call fires the `sigil_bridge_peg` webhook (exact-name subscription, the same
//! registry `flux_webhook_register` writes) so a dashboard or another agent can watch the
//! peg without polling two chains itself. The relayer fires the `sigil_bridge_*` lifecycle
//! events into the same registry.

use super::{ToolDef, ToolRegistry};
use fluxc_webhooks::webhook;
use serde_json::{json, Value};

pub const EV_PEG: &str = "sigil_bridge_peg";

const WSIGIL3: &str = "0x3fced760b0de6d57f96835c6110b1227941ec2e9";
const WSIGIL_LEGACY: &str = "0xc224602c32f5c7f68d3ef002ae4c99e4c7df25b7";
const PAIR_WSIGIL3: &str = "0x7e9c5d73104ed2fc2bc55139ac0999036eaf41d7";
const USDC: &str = "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359";
/// The 1.0 wSIGIL3 minted to the deployer to seed the Uniswap pair
/// (deploy tx 53473362…, block 93,331,958). No SIGIL lock stands behind it.
const SEED_WSIGIL3_WEI: u128 = 1_000_000_000_000_000_000;
const SIGIL_GLYPHS: f64 = 1e10;
const WEI: f64 = 1e18;
/// Below this much USDC in the pair, the pool is a number anyone can move for cents —
/// the same floor `sigil-oracle-feeder` refuses to price from.
const POOL_FLOOR_USD: f64 = 50.0;
const DEFAULT_RELAYER_STATUS: &str = "/home/orobit/sigil-bridge-relayer/status.json";

const SEL_TOTAL_SUPPLY: &str = "0x18160ddd";
const SEL_OPERATOR: &str = "0x570ca735";
const SEL_OWNER: &str = "0x8da5cb5b";
const SEL_RESERVES: &str = "0x0902f1ac";
const SEL_TOKEN0: &str = "0x0dfe1681";

fn polygon_rpcs() -> Vec<String> {
    let mut v: Vec<String> = std::env::var("FLUX_POLYGON_RPC").ok().into_iter().filter(|s| !s.is_empty()).collect();
    v.extend(flux_bitunix::pool::POLYGON_RPCS.iter().map(|s| s.to_string()));
    v
}

/// `eth_call` with a User-Agent: publicnode answers 403 to a request without one, which
/// silently pushes every call onto the slower fallbacks.
fn eth_call(to: &str, data: &str) -> Result<String, String> {
    let body = json!({"jsonrpc":"2.0","id":1,"method":"eth_call","params":[{"to":to,"data":data},"latest"]});
    let mut last = String::from("no RPC tried");
    for rpc in polygon_rpcs() {
        let resp = ureq::post(&rpc)
            .timeout(std::time::Duration::from_secs(12))
            .set("User-Agent", "flux-sigil-bridge/1")
            .set("Content-Type", "application/json")
            .send_string(&body.to_string());
        let parsed = resp
            .map_err(|e| e.to_string())
            .and_then(|r| r.into_string().map_err(|e| e.to_string()))
            .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string()));
        match parsed {
            Ok(v) => match v.get("result").and_then(|x| x.as_str()) {
                Some(res) if res.len() > 2 => return Ok(res.to_string()),
                _ => last = format!("{rpc}: {}", v.get("error").map(|e| e.to_string()).unwrap_or_else(|| "empty result".into())),
            },
            Err(e) => last = format!("{rpc}: {e}"),
        }
    }
    Err(format!("no Polygon RPC answered: {last}"))
}

fn word(hex_blob: &str, i: usize) -> Option<u128> {
    let h = hex_blob.trim_start_matches("0x");
    let w = h.get(i * 64..(i + 1) * 64)?;
    // A V2 reserve is u112 and a supply fits u128 in practice; refuse a wider word rather
    // than truncating it into a believable wrong number.
    if !w[..32].chars().all(|c| c == '0') {
        return None;
    }
    u128::from_str_radix(&w[32..], 16).ok()
}

fn address_word(hex_blob: &str) -> Option<String> {
    let h = hex_blob.trim_start_matches("0x");
    (h.len() >= 64).then(|| format!("0x{}", &h[24..64].to_ascii_lowercase()))
}

fn sigil_get(path: &str) -> Result<Value, String> {
    let url = format!("{}{}", super::sigil_wallet::rpc_base(), path);
    let r = ureq::get(&url).timeout(std::time::Duration::from_secs(20)).call().map_err(|e| format!("{url}: {e}"))?;
    let text = r.into_string().map_err(|e| format!("{url}: {e}"))?;
    serde_json::from_str::<Value>(&text).map_err(|e| format!("{url}: {e}"))
}

fn glyphs(v: &Value) -> Option<u128> {
    match v {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_u64().map(u128::from),
        _ => None,
    }
}

/// Uniswap V2 `getAmountOut` with the 0.3 % fee.
pub fn v2_out(amount_in: f64, reserve_in: f64, reserve_out: f64) -> f64 {
    if amount_in <= 0.0 || reserve_in <= 0.0 || reserve_out <= 0.0 {
        return 0.0;
    }
    let a = amount_in * 997.0;
    a * reserve_out / (reserve_in * 1000.0 + a)
}

#[derive(Debug, Default, Clone)]
pub struct Observed {
    pub vault_backing_glyphs: Option<u128>,
    pub node_reports_epochs: bool,
    pub bridge_paused: Option<bool>,
    pub locks_unsettled: usize,
    pub wsigil3_supply_wei: Option<u128>,
    pub operator: Option<String>,
    pub relayer_polygon: Option<String>,
    pub relayer_active: Option<bool>,
    pub relayer_fatal: Vec<String>,
    pub pool_usdc: Option<f64>,
    pub usds_price_fresh: Option<bool>,
}

/// Rank what stands between the bridge and a working round trip. Pure, so the ranking
/// is pinned by tests rather than by whatever the chains say today.
pub fn blockers(o: &Observed) -> Vec<Value> {
    let mut out: Vec<(u8, Value)> = Vec::new();
    let mut add = |rank: u8, code: &str, who: &str, what: String, fix: String| {
        out.push((rank, json!({"code": code, "who": who, "what": what, "fix": fix})));
    };
    if let (Some(minted), Some(back)) = (o.wsigil3_supply_wei.map(|s| s.saturating_sub(SEED_WSIGIL3_WEI)), o.vault_backing_glyphs) {
        if minted > back.saturating_mul(100_000_000) {
            add(0, "peg_broken", "operator",
                format!("bridge-issued wSIGIL3 {:.10} exceeds the vault's backing {:.10} SIGIL", minted as f64 / WEI, back as f64 / SIGIL_GLYPHS),
                "pause the bridge (/v1/bridge/pause) and reconcile before anything else".into());
        }
    }
    match (&o.operator, &o.relayer_polygon) {
        (Some(op), Some(rel)) if !op.eq_ignore_ascii_case(rel) => add(1, "not_operator", "operator",
            format!("wSIGIL3 operator() is {op}, the relayer signs as {rel} — every mint would revert"),
            format!("the owner calls setOperator({rel}) on {WSIGIL3} (~35k gas)")),
        _ => {}
    }
    if !o.node_reports_epochs {
        add(2, "node_pre_fix", "deploy",
            "the SIGIL node runs bridge code from before 2026-10-15: vault backing reads 0 and unlocks cannot see notes in sealed pool epochs".into(),
            "deploy branch sigil-bridge-relay-1015 (sigil-api, no consensus change) to the SIGIL producer".into());
    }
    if o.relayer_active != Some(true) {
        add(3, "relayer_down", "operator",
            "the relayer is not running, so no lock is minted and no burn is paid back — the wallet's burn button is correctly disabled".into(),
            "after the items above: point relayer.env at wSIGIL3 (+ SIGIL_BRIDGE_DEPLOY_BLOCK=93331958), `sigil-relayer --preflight`, then lift the hold".into());
    }
    for f in &o.relayer_fatal {
        add(4, "relayer_preflight", "operator", format!("relayer preflight: {f}"), "see the relayer's status.json".into());
    }
    if o.bridge_paused == Some(true) {
        add(5, "bridge_paused", "operator", "the SIGIL bridge is paused by its admin".into(), "admin-signed /v1/bridge/pause {paused:false}".into());
    }
    if o.locks_unsettled > 0 {
        add(6, "locks_waiting", "chain", format!("{} lock(s) not settled yet — they mint only once every part is on the settled spine", o.locks_unsettled), "wait (~1–2 min at today's block rate)".into());
    }
    if o.pool_usdc.is_some_and(|u| u < POOL_FLOOR_USD) {
        add(7, "pool_dust", "operator",
            format!("the wSIGIL3/USDC pool holds ${:.4} of USDC — its price is not a market, and any real trade moves it by orders of magnitude", o.pool_usdc.unwrap_or(0.0)),
            "add liquidity on both sides once the bridge is live (bridge-backed wSIGIL3, not the unbacked seed)".into());
    }
    if o.usds_price_fresh == Some(false) {
        add(8, "usds_no_oracle", "operator", "USDS has no fresh price, so it cannot be minted or redeemed".into(),
            "master delegates the feeder wallet (OracleDelegate), then start sigil-oracle-feeder".into());
    }
    out.sort_by_key(|(r, _)| *r);
    out.into_iter().map(|(_, v)| v).collect()
}

fn systemctl_active(unit: &str) -> Option<bool> {
    let out = std::process::Command::new("systemctl").args(["is-active", unit]).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim() == "active")
}

pub fn bridge_report(args: &Value) -> Value {
    let amount_sigil = args.get("amount_sigil").and_then(|v| v.as_f64()).filter(|a| *a > 0.0).unwrap_or(1.0);
    let status_path = args
        .get("relayer_status_file")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| std::env::var("FLUX_SIGIL_RELAYER_STATUS").ok())
        .unwrap_or_else(|| DEFAULT_RELAYER_STATUS.to_string());
    let mut errors: Vec<String> = Vec::new();
    let mut o = Observed::default();

    // ── SIGIL side ──
    let bridge = sigil_get("/v1/bridge/status").map_err(|e| errors.push(e)).ok();
    let bd = bridge.as_ref().map(|b| b["data"].clone()).unwrap_or(Value::Null);
    o.vault_backing_glyphs = glyphs(&bd["vault_note_balance"]);
    o.node_reports_epochs = bd.get("vault_notes").is_some();
    o.bridge_paused = bd.get("paused").and_then(|p| p.as_bool());
    let locks = sigil_get("/v1/bridge/locks?since=0").map_err(|e| errors.push(e)).ok();
    let lock_list = locks.as_ref().and_then(|l| l["data"].as_array().cloned()).unwrap_or_default();
    let settled = lock_list.iter().filter(|l| l["settled"].as_bool() == Some(true)).count();
    o.locks_unsettled = lock_list.len() - settled;
    let usds = sigil_get("/v1/usds/status").map_err(|e| errors.push(e)).ok();
    let ud = usds.as_ref().map(|u| u["data"].clone()).unwrap_or(Value::Null);
    o.usds_price_fresh = ud.get("price_fresh").and_then(|p| p.as_bool());
    let door = sigil_get("/v1/shielded/door").map_err(|e| errors.push(e)).ok();

    // ── Polygon side ──
    let mut call = |to: &str, sel: &str| eth_call(to, sel).map_err(|e| errors.push(e)).ok();
    let w3_supply = call(WSIGIL3, SEL_TOTAL_SUPPLY).and_then(|h| word(&h, 0));
    let legacy_supply = call(WSIGIL_LEGACY, SEL_TOTAL_SUPPLY).and_then(|h| word(&h, 0));
    o.operator = call(WSIGIL3, SEL_OPERATOR).and_then(|h| address_word(&h));
    let owner = call(WSIGIL3, SEL_OWNER).and_then(|h| address_word(&h));
    let token0 = call(PAIR_WSIGIL3, SEL_TOKEN0).and_then(|h| address_word(&h));
    let reserves = call(PAIR_WSIGIL3, SEL_RESERVES);
    o.wsigil3_supply_wei = w3_supply;
    let (pool_usdc, pool_w3) = match (&token0, reserves.as_deref().and_then(|r| Some((word(r, 0)?, word(r, 1)?)))) {
        (Some(t0), Some((r0, r1))) => {
            let usdc_first = t0.eq_ignore_ascii_case(USDC);
            let (u, w) = if usdc_first { (r0, r1) } else { (r1, r0) };
            (Some(u as f64 / 1e6), Some(w as f64 / WEI))
        }
        _ => (None, None),
    };
    o.pool_usdc = pool_usdc;

    // ── relayer ──
    o.relayer_active = systemctl_active("sigil-bridge-relayer");
    let relayer_status: Option<Value> = std::fs::read_to_string(&status_path).ok().and_then(|t| serde_json::from_str(&t).ok());
    o.relayer_polygon = args
        .get("relayer_address")
        .and_then(|v| v.as_str())
        .map(|s| s.to_ascii_lowercase())
        .or_else(|| relayer_status.as_ref().and_then(|s| s["polygon_relayer"].as_str()).map(|s| s.to_ascii_lowercase()));
    o.relayer_fatal = relayer_status
        .as_ref()
        .and_then(|s| s["preflight"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter(|p| p["fatal"].as_bool() == Some(true) && p["code"].as_str() != Some("not_operator"))
        .filter_map(|p| p["detail"].as_str().map(str::to_string))
        .collect();

    // ── peg + route ──
    let minted_wei = w3_supply.map(|s| s.saturating_sub(SEED_WSIGIL3_WEI));
    let backing_sigil = o.vault_backing_glyphs.map(|g| g as f64 / SIGIL_GLYPHS);
    let minted_sigil = minted_wei.map(|m| m as f64 / WEI);
    let usdc_out = match (pool_w3, pool_usdc) {
        (Some(w), Some(u)) => Some(v2_out(amount_sigil, w, u)),
        _ => None,
    };
    let spot = match (pool_w3, pool_usdc) {
        (Some(w), Some(u)) if w > 0.0 => Some(u / w),
        _ => None,
    };
    let impact = match (usdc_out, spot) {
        (Some(out), Some(p)) if p > 0.0 => Some((1.0 - out / (amount_sigil * p)) * 100.0),
        _ => None,
    };

    let bl = blockers(&o);
    let verdict = if bl.iter().any(|b| b["code"] == "peg_broken") {
        "peg_broken"
    } else if bl.iter().any(|b| matches!(b["code"].as_str(), Some("not_operator" | "node_pre_fix" | "relayer_down" | "relayer_preflight" | "bridge_paused"))) {
        "blocked"
    } else {
        "ready"
    };
    let headline = match verdict {
        "ready" => format!("Bridge ready: {amount_sigil} SIGIL → {amount_sigil} wSIGIL3 → {:.6} USDC.", usdc_out.unwrap_or(0.0)),
        "peg_broken" => "ALARM: more wSIGIL3 issued by the bridge than the vault holds.".to_string(),
        _ => format!(
            "Bridge not ready — {} blocker(s); first: {}",
            bl.len(),
            bl.first().and_then(|b| b["what"].as_str()).unwrap_or("?")
        ),
    };

    let report = json!({
        "ok": errors.is_empty(),
        "verdict": verdict,
        "headline": headline,
        "sigil": {
            "api": super::sigil_wallet::rpc_base(),
            "vault_backing_sigil": backing_sigil,
            "vault_notes": bd.get("vault_notes"),
            "vault_notes_spent": bd.get("vault_notes_spent"),
            "vault_by_epoch": bd.get("vault_by_epoch"),
            "node_reports_every_epoch": o.node_reports_epochs,
            "bridge_paused": o.bridge_paused,
            "relayer_wallet": bd.get("relayer"),
            "locks": {"total": lock_list.len(), "settled": settled, "unsettled": o.locks_unsettled},
            "door": door.as_ref().map(|d| d["door"].clone()),
        },
        "polygon": {
            "wsigil3": {"address": WSIGIL3, "total_supply": w3_supply.map(|s| s as f64 / WEI),
                        "unbacked_pool_seed": SEED_WSIGIL3_WEI as f64 / WEI, "bridge_issued": minted_sigil,
                        "operator": o.operator, "owner": owner},
            "legacy_wsigil": {"address": WSIGIL_LEGACY, "total_supply": legacy_supply.map(|s| s as f64 / WEI),
                              "note": "superseded token; minted against a g1 lock, backed by nothing since the g2 reset"},
            "pool": {"pair": PAIR_WSIGIL3, "usdc": pool_usdc, "wsigil3": pool_w3, "spot_usdc_per_wsigil3": spot,
                     "is_a_market": pool_usdc.map(|u| u >= POOL_FLOOR_USD)},
        },
        "peg": {
            "bridge_issued_wsigil3": minted_sigil,
            "vault_backing_sigil": backing_sigil,
            "covered": match (minted_sigil, backing_sigil) { (Some(m), Some(b)) => Some(m <= b + 1e-10), _ => None },
            "spare_backing_sigil": match (minted_sigil, backing_sigil) { (Some(m), Some(b)) => Some(b - m), _ => None },
        },
        "route": {
            "sigil_in": amount_sigil,
            "wsigil3_out": amount_sigil,
            "bridge_fee": 0,
            "usdc_out": usdc_out,
            "price_impact_pct": impact,
            "note": "bridge leg 1:1 (glyphs × 10^8 = wei); Uniswap V2 leg with the 0.3% fee against live reserves",
        },
        "usds": {
            "live": ud.get("live"), "price": ud.get("price"), "price_fresh": o.usds_price_fresh,
            "feeder": ud.get("feeder"), "supply": ud.get("usds_supply"),
        },
        "relayer": {
            "service_active": o.relayer_active,
            "status_file": status_path,
            "status": relayer_status,
            "polygon_address": o.relayer_polygon,
            "is_operator": match (&o.operator, &o.relayer_polygon) { (Some(a), Some(b)) => Some(a.eq_ignore_ascii_case(b)), _ => None },
        },
        "blockers": bl,
        "errors": errors,
        "webhook_event": EV_PEG,
    });
    webhook::auto_dispatch(EV_PEG, report.clone());
    report
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_sigil_bridge",
            description: "SIGIL ↔ wSIGIL3 (Polygon) bridge, both chains in one READ-ONLY call: vault backing on SIGIL (every pool epoch) vs bridge-issued wSIGIL3 (the 1.0 pool seed reported separately), the SIGIL→wSIGIL3→USDC route quote for `amount_sigil` (V2 0.3% fee + price impact), USDS oracle state, relayer service/status/operator check, and a ranked blocker list saying who must act. Fires the `sigil_bridge_peg` webhook. Never signs or sends.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "amount_sigil": {"type": "number", "description": "Size to quote through the route (default 1 SIGIL)"},
                    "relayer_address": {"type": "string", "description": "Relayer Polygon address to check against operator() (default: from the relayer's status.json)"},
                    "relayer_status_file": {"type": "string", "description": "Path of the relayer's status.json (default /home/orobit/sigil-bridge-relayer/status.json, env FLUX_SIGIL_RELAYER_STATUS)"}
                }
            }),
        },
        |a| serde_json::to_string_pretty(&bridge_report(a)).unwrap_or_else(|e| format!("✗ {e}")),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_out_matches_the_uniswap_formula() {
        // 1 token into 10/10 reserves: 997*10 / (10*1000 + 997) = 0.906610...
        let out = v2_out(1.0, 10.0, 10.0);
        assert!((out - 9970.0 / 10997.0).abs() < 1e-12, "{out}");
        assert_eq!(v2_out(0.0, 10.0, 10.0), 0.0);
        assert_eq!(v2_out(1.0, 0.0, 10.0), 0.0);
    }

    #[test]
    fn words_refuse_values_wider_than_u128() {
        let ok = format!("0x{:064x}", 12345u128);
        assert_eq!(word(&ok, 0), Some(12345));
        let wide = format!("0x1{}", "0".repeat(63));
        assert_eq!(word(&wide, 0), None);
        assert_eq!(address_word(&format!("0x{}{}", "0".repeat(24), "39d1d26d59eebcf1b4b7b4863ad38a6d226f6840")).as_deref(),
                   Some("0x39d1d26d59eebcf1b4b7b4863ad38a6d226f6840"));
    }

    fn live_1015() -> Observed {
        Observed {
            vault_backing_glyphs: Some(0),
            node_reports_epochs: false,
            bridge_paused: Some(false),
            locks_unsettled: 0,
            wsigil3_supply_wei: Some(SEED_WSIGIL3_WEI),
            operator: Some("0x39d1d26d59eebcf1b4b7b4863ad38a6d226f6840".into()),
            relayer_polygon: Some("0x9f574f1bb153c18f0d93c0fdd73cc4d93252bc6e".into()),
            relayer_active: Some(false),
            relayer_fatal: vec![],
            pool_usdc: Some(0.098),
            usds_price_fresh: Some(false),
        }
    }

    /// What the chains said on 2026-10-15, ranked: operator first, then the node, then
    /// the held relayer; the dust pool and the dead oracle are reported, last.
    #[test]
    fn the_measured_state_ranks_the_operator_first() {
        let b = blockers(&live_1015());
        let codes: Vec<&str> = b.iter().map(|x| x["code"].as_str().unwrap()).collect();
        assert_eq!(codes, vec!["not_operator", "node_pre_fix", "relayer_down", "pool_dust", "usds_no_oracle"]);
        assert!(b[0]["fix"].as_str().unwrap().contains("setOperator(0x9f574f1bb153c18f0d93c0fdd73cc4d93252bc6e)"));
    }

    /// The seed is not a peg failure: 1.0 wSIGIL3 with an empty vault is "nothing bridged".
    #[test]
    fn the_unbacked_seed_is_not_counted_as_bridge_issued() {
        let o = live_1015();
        assert!(!blockers(&o).iter().any(|x| x["code"] == "peg_broken"));
        let mut over = o.clone();
        over.wsigil3_supply_wei = Some(SEED_WSIGIL3_WEI + 2 * 1_000_000_000_000_000_000);
        over.vault_backing_glyphs = Some(10_000_000_000); // 1 SIGIL backing 2 issued
        assert_eq!(blockers(&over)[0]["code"], "peg_broken");
        over.vault_backing_glyphs = Some(20_000_000_000);
        assert!(!blockers(&over).iter().any(|x| x["code"] == "peg_broken"), "exactly covered is covered");
    }

    #[test]
    fn a_fully_fixed_bridge_has_only_market_items_left() {
        let mut o = live_1015();
        o.operator = o.relayer_polygon.clone();
        o.node_reports_epochs = true;
        o.relayer_active = Some(true);
        o.pool_usdc = Some(500.0);
        o.usds_price_fresh = Some(true);
        assert!(blockers(&o).is_empty());
    }
}
