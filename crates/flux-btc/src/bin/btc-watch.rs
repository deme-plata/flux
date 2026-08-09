//! btc-watch — the capitulation watcher (cron-driven, propose-only).
//!
//! The inverse of a DCA loop: it buys nothing and nags gently. Once a day it
//! measures the market, posts a one-line status to Buzz #trading with
//!   1. dip_strength + regime (measured drift/Kelly, so feelings meet data),
//!   2. an ETA *approximation* for the operator's target price — a pure
//!      extrapolation of the measured drift, labeled as such,
//!   3. a war-chest nudge: stash N DKK in USDT today so the dry powder
//!      exists when the target arrives.
//! When capitulation conditions actually show up (dip_strength ≥ alert
//! threshold, or price within 10% of target) it posts a loud kind-20 alert
//! instead. No order is ever placed; a human confirms every spend.
//!
//! Config via env (all optional):
//!   BTC_WATCH_TARGET_USD  target buy price          (default 30000)
//!   BTC_WATCH_BASE_DKK    daily savings nudge, DKK  (default 50)
//!   DKK_PER_USD           conversion approximation  (default 6.90)
//!   BTC_WATCH_CHANNEL     Buzz channel              (default trading)
//!   BTC_WATCH_AGENT       identity/display name     (default grogu-btc-watch)
//!   BTC_WATCH_ALERT_DIP   alert threshold 0-100     (default 70)
//!   FLUX_BUZZ_RELAY       relay URL                 (default https://buzz.quillon.xyz)
//!   BTC_WATCH_DRY_RUN     "1" = print, don't post

use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};

fn envf(key: &str, default: f64) -> f64 {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}
fn envs(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Months until `price` decays to `target` under constant exponential drift.
/// Returns None when the drift doesn't point at the target.
fn eta_months(price: f64, target: f64, drift_annual: f64) -> Option<f64> {
    if price <= 0.0 || target <= 0.0 || target >= price || drift_annual >= -0.01 {
        return None;
    }
    Some((target / price).ln() / drift_annual * 12.0)
}

/// Human ETA line: mid estimate on the measured drift, with a fast/slow band
/// at drift ± σ/2. Explicitly an extrapolation, not a prophecy.
fn eta_line(price: f64, target: f64, drift: f64, vol: f64) -> String {
    if price <= target {
        return format!("target ${target:.0} REACHED");
    }
    match eta_months(price, target, drift) {
        Some(mid) => {
            let fast = eta_months(price, target, drift - vol / 2.0);
            let slow = eta_months(price, target, drift + vol / 2.0);
            let band = match (fast, slow) {
                (Some(f), Some(s)) => format!(" (band {f:.0}–{s:.0}mo)"),
                (Some(f), None) => format!(" (band {f:.0}mo–never if trend flips)"),
                _ => String::new(),
            };
            format!(
                "ETA to ${target:.0} IF measured drift ({:.0}%/yr) held: ~{mid:.0}mo{band} — extrapolation, not prophecy",
                drift * 100.0
            )
        }
        None => format!(
            "no ETA to ${target:.0}: measured drift ({:+.0}%/yr) isn't pointing there",
            drift * 100.0
        ),
    }
}

// ── Buzz signed-event client (wire-identical to fluxc-mcp handlers/buzz.rs) ──

fn identity_path(agent: &str) -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let safe: String = agent
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    std::path::Path::new(&home).join(".flux-buzz").join(format!("identity-{safe}.json"))
}

fn load_or_create_identity(path: &std::path::Path) -> Result<(SigningKey, String), String> {
    if path.exists() {
        let v: Value = serde_json::from_slice(
            &std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?,
        )
        .map_err(|e| format!("parse {}: {e}", path.display()))?;
        let sk_hex = v["sk"].as_str().ok_or("identity file missing sk")?;
        let sk: [u8; 32] = hex::decode(sk_hex.trim())
            .map_err(|e| format!("sk hex: {e}"))?
            .try_into()
            .map_err(|_| "sk must be 32 bytes".to_string())?;
        let key = SigningKey::from_bytes(&sk);
        let pk = hex::encode(key.verifying_key().to_bytes());
        Ok((key, pk))
    } else {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        let pk = hex::encode(key.verifying_key().to_bytes());
        let body = json!({"sk": hex::encode(key.to_bytes()), "pk": pk});
        std::fs::write(path, serde_json::to_vec_pretty(&body).unwrap()).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok((key, pk))
    }
}

fn post_to_buzz(agent: &str, channel: &str, kind: u32, content: &str) -> Result<String, String> {
    let relay = envs("FLUX_BUZZ_RELAY", "https://buzz.quillon.xyz");
    let relay = relay.trim_end_matches('/');
    let (key, pubkey) = load_or_create_identity(&identity_path(agent))?;
    let tags = vec![
        vec!["c".to_string(), channel.to_string()],
        vec!["client".to_string(), "btc-watch".to_string()],
        vec!["name".to_string(), agent.to_string()],
    ];
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    // Canonical bytes MUST match flux-buzz: compact JSON of
    // [pubkey, created_at, kind, tags, content].
    let canonical = serde_json::to_vec(&json!([pubkey, created_at, kind, tags, content]))
        .map_err(|e| e.to_string())?;
    let sig = hex::encode(key.sign(&canonical).to_bytes());
    let id = blake3::hash(&canonical).to_hex().to_string();
    let event = json!({
        "id": id, "pubkey": pubkey, "created_at": created_at,
        "kind": kind, "tags": tags, "content": content, "sig": sig,
    });
    let out = std::process::Command::new("curl")
        .args(["-s", "--max-time", "15", "-X", "POST", "-H", "content-type: application/json", "-d"])
        .arg(event.to_string())
        .arg(format!("{relay}/v1/event"))
        .output()
        .map_err(|e| format!("curl spawn: {e}"))?;
    let body = String::from_utf8_lossy(&out.stdout).to_string();
    if !out.status.success() && body.is_empty() {
        return Err(format!("curl failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(body)
}

fn main() {
    let target = envf("BTC_WATCH_TARGET_USD", 30_000.0);
    let base_dkk = envf("BTC_WATCH_BASE_DKK", 50.0);
    let dkk_per_usd = envf("DKK_PER_USD", 6.90);
    let alert_dip = envf("BTC_WATCH_ALERT_DIP", 70.0);
    let channel = envs("BTC_WATCH_CHANNEL", "trading");
    let agent = envs("BTC_WATCH_AGENT", "grogu-btc-watch");
    let dry_run = envs("BTC_WATCH_DRY_RUN", "") == "1";

    // Measure. Fail loud: a watcher that posts stale/fabricated numbers is
    // worse than one that visibly missed a day.
    let decision = match flux_trade::decide("BTCUSDT", "1d", 200) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("btc-watch: flux-trade decide failed: {e}");
            std::process::exit(1);
        }
    };
    let verdict = match flux_btc::analyze("1d", 200) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("btc-watch: flux-btc analyze failed: {e}");
            std::process::exit(1);
        }
    };
    let drift = decision["sizing"]["drift_annual"].as_f64().unwrap_or(0.0);
    let vol = decision["sizing"]["vol_annual"].as_f64().unwrap_or(0.0);
    let kelly = decision["sizing"]["kelly_fraction"].as_f64().unwrap_or(0.0);
    let price = verdict.price_usd;
    let usd_save = base_dkk / dkk_per_usd;
    let gap_pct = (target / price - 1.0) * 100.0;

    let alert = verdict.dip_strength >= alert_dip || price <= target * 1.10;
    let eta = eta_line(price, target, drift, vol);

    let (kind, content) = if alert {
        (
            20,
            format!(
                "🎯 BTC WATCH ALERT — capitulation conditions: ${price:.0} · dip {dip:.0}/100 · {fg_label} ({fg}) · RSI {rsi:.0} · target ${target:.0} ({gap_pct:+.0}%). \
                 War chest goes to work NOW if you agree — propose-only, you confirm every buy.",
                dip = verdict.dip_strength,
                fg = verdict.fear_greed,
                fg_label = verdict.fear_greed_label,
                rsi = verdict.rsi,
            ),
        )
    } else {
        (
            1,
            format!(
                "₿ watch: ${price:.0} · dip {dip:.0}/100 · {fg_label} ({fg}) · RSI {rsi:.0} · drift {drift_pct:+.0}%/yr · Kelly {kelly:.2}\n\
                 {eta}\n\
                 war chest: stash {base_dkk:.0} DKK (≈${usd_save:.1}) in USDT today. No orders placed — watching for ${target:.0}.",
                dip = verdict.dip_strength,
                fg = verdict.fear_greed,
                fg_label = verdict.fear_greed_label,
                rsi = verdict.rsi,
                drift_pct = drift * 100.0,
            ),
        )
    };

    println!("{content}");
    if dry_run {
        println!("(dry run — not posted)");
        return;
    }
    match post_to_buzz(&agent, &channel, kind, &content) {
        Ok(resp) => println!("posted to #{channel} (kind {kind}): {resp}"),
        Err(e) => {
            eprintln!("btc-watch: buzz post failed: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eta_points_down_only() {
        // $65k → $30k at −58%/yr ≈ 16 months.
        let m = eta_months(65_000.0, 30_000.0, -0.58).unwrap();
        assert!((14.0..=18.0).contains(&m), "expected ~16mo, got {m}");
        // Upward or flat drift → no ETA.
        assert!(eta_months(65_000.0, 30_000.0, 0.2).is_none());
        assert!(eta_months(65_000.0, 30_000.0, 0.0).is_none());
        // Target above price → no ETA (this watcher only waits for dips).
        assert!(eta_months(25_000.0, 30_000.0, -0.5).is_none());
    }

    #[test]
    fn eta_line_is_honest() {
        let l = eta_line(65_000.0, 30_000.0, -0.58, 0.47);
        assert!(l.contains("extrapolation"), "must label itself: {l}");
        let flat = eta_line(65_000.0, 30_000.0, 0.10, 0.47);
        assert!(flat.contains("no ETA"), "upward drift must refuse: {flat}");
        let hit = eta_line(29_000.0, 30_000.0, -0.58, 0.47);
        assert!(hit.contains("REACHED"), "{hit}");
    }
}
