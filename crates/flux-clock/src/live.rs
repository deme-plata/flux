//! Reading the live SIGIL node and the sigil-earth feed into [`crate::face::FaceInputs`].
//! Shared by the `flux-clock` CLI and the `flux_sigil_wallet_clock` MCP tool.

use crate::{earth, face};
use serde_json::Value;
use std::time::Duration;

pub fn get_json(url: &str) -> Result<Value, String> {
    let r = ureq::get(url).timeout(Duration::from_secs(10)).call().map_err(|e| format!("{url}: {e}"))?;
    let s = r.into_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&s).map_err(|e| format!("{url}: not JSON: {e}"))
}

/// The node's height. Producers answer `/v1/mining/challenge`; a FOLLOWER returns 503 there
/// (it cannot hand out work), so fall back to `/v1/network/topology → data.local_view.height`.
pub fn height(node: &str) -> Option<u64> {
    if let Some(h) = get_json(&format!("{node}/v1/mining/challenge")).ok().and_then(|c| c.get("height")?.as_u64()) { return Some(h); }
    get_json(&format!("{node}/v1/network/topology")).ok()?["data"]["local_view"]["height"].as_u64()
}

fn first_follower_height(topo: &Option<Value>) -> Option<u64> {
    topo.as_ref().and_then(|t| t["data"]["peer_views"].as_object()).and_then(|pv| pv.values().next()).and_then(|v| v["height"].as_u64())
}

/// Fit the Earth's slow hands from the feed's own series (annual LOD, annual + Chandler pole x).
pub fn earth_hands(series: &Value, mjd_now: f64) -> Vec<earth::EarthHand> {
    let rows = series["rows"].as_array().or(series.as_array()).cloned().unwrap_or_default();
    let lod: Vec<(f64, f64)> = rows.iter().filter_map(|r| Some((r["mjd"].as_f64()?, r["lod"].as_f64()?))).collect();
    let xp: Vec<(f64, f64)> = rows.iter().filter_map(|r| Some((r["mjd"].as_f64()?, r["xp"].as_f64()?))).collect();
    let mut hands = earth::fit_hands(&lod, mjd_now, &[("annual LOD", earth::DAYS_YEAR)]);
    hands.extend(earth::fit_hands(&xp, mjd_now, &[("annual pole x", earth::DAYS_YEAR), ("Chandler pole x", earth::CHANDLER_DAYS)]).into_iter().skip(1));
    hands
}

/// Build the face inputs from the live node + earth feed. `sample_secs` measures the block
/// rate from two height samples; the certificate, topology and earth feed are read once.
pub fn live_inputs(node: &str, earth_url: &str, energy_j: f64, sample_secs: f64) -> Result<face::FaceInputs, String> {
    let h0 = height(node).ok_or("node: no height")?;
    let t0 = std::time::Instant::now();
    let topo0 = get_json(&format!("{node}/v1/network/topology")).ok();
    let follower0 = first_follower_height(&topo0);
    std::thread::sleep(Duration::from_secs_f64(sample_secs.max(1.0)));
    let h1 = height(node).ok_or("node: no height (second sample)")?;
    let dt = t0.elapsed().as_secs_f64();
    let topo1 = get_json(&format!("{node}/v1/network/topology")).ok();
    let follower1 = first_follower_height(&topo1);
    let cert = get_json(&format!("{node}/v1/finality/certificate")).ok().and_then(|c| c.get("certificate").cloned());
    let earth_latest = get_json(&format!("{earth_url}/v1/earth/latest")).ok();
    let series = get_json(&format!("{earth_url}/v1/earth/series?days=2200")).ok();
    let mut i = face::FaceInputs { node: node.into(), height: h1, bps: (h1.saturating_sub(h0)) as f64 / dt, bps_basis: format!("Δheight/Δt over {dt:.1} s"), energy_j, ..Default::default() };
    if let Some(c) = &cert {
        i.cert_height = c["height"].as_u64();
        i.cert_age_s = c["certified_at_ms"].as_u64().map(|ms| (crate::now_ms().saturating_sub(ms)) as f64 / 1000.0);
        i.cert_bft = c["bft"].as_bool();
        i.cert_votes = c["votes"].as_array().map(|v| v.len() as u32);
    }
    if let (Some(f0), Some(f1)) = (follower0, follower1) { i.follower_dh = Some(f1.saturating_sub(f0) as f64); i.producer_dh = Some(h1.saturating_sub(h0) as f64); }
    if let Some(e) = &earth_latest {
        i.ut1_minus_utc_s = e["now"]["ut1utc_today"].as_f64().or(e["today"]["ut1utc"].as_f64());
        i.k_earth = e["today"]["k_resid"].as_f64();
        i.k_earth_regime = e["today"]["regime"].as_str().map(String::from);
        i.k_earth_p99 = e["ladder"]["k_resid"]["p99"].as_f64();
        i.lod_ms = e["today"]["lod"].as_f64();
    }
    if let Some(s) = &series {
        let now = earth_latest.as_ref().and_then(|e| e["now"]["pole_mjd"].as_f64()).unwrap_or(crate::now_unix() / 86400.0 + earth::MJD_UNIX);
        i.earth_hands = earth_hands(s, now);
    }
    Ok(i)
}
