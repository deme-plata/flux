//! quillon_roots.rs — flux-legacy: **SIGIL's state roots, computed OFF a Quillon node** (2026-11-02).
//!
//! SIGIL commits, in every block header, a root over ALL state: every `(wallet, token)` balance,
//! every DEX pool, every contract slot (`sigil-header` `wallet_state_root` / `dex_state_root` /
//! `contract_state_root`), and a follower refuses a block whose roots differ from its own. Two SIGIL
//! nodes with the same block hash at the same height therefore hold the same state.
//!
//! Quillon commits ONE root, over native QUG only (`balance_root_v1`), checked in shadow mode. Its
//! tokens, pools and contracts are committed nowhere, and its DEX is not on chain at all — so no
//! instrument could say whether two Quillon nodes agree on anything but plain QUG.
//!
//! This module is that instrument, read-only and outside consensus: it reads each node's
//! `GET /api/v1/sync/full-state` export, waits until both exports are at the SAME block height
//! (SIGIL rule: compare at equal height or not at all), builds SIGIL-shaped section roots and lists
//! exactly which entries differ. It is the measuring half of porting SIGIL's design; the consensus
//! half (one write chokepoint, DEX in blocks, header commitment, follower refusal) is separate work.
//!
//! Sections (leaf = `key \0 value`, sorted, BLAKE3 with a domain tag):
//! - `wallet`   — native QUG `wallet_balances` + every `token_balances` entry (key `wallet_token`);
//!                a zero balance is no leaf (absent ≡ 0, as in SIGIL).
//! - `dex`      — each pool's SIGIL fields: token0, token1, reserve0, reserve1, lp_token_supply,
//!                decimals. `created_at` / `provider` are bookkeeping, not state, and are left out.
//! - `contract` — each contract as canonical JSON (object keys sorted).
//! - `dex_counters` — Quillon's off-chain `dex_qug_credited/debited`. Reported, but node-local BY
//!                DESIGN today: a follower never sees a swap, so it never has these. Not a "root".
//! Not available: an event-log root (Quillon keeps no per-block event log).

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

pub type Entries = BTreeMap<String, String>;

/// The comparable sections of one node's state, keyed by section name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub height: u64,
    pub version: String,
    pub sections: BTreeMap<&'static str, Entries>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diff {
    pub key: String,
    pub a: Option<String>,
    pub b: Option<String>,
    /// b − a as a decimal number of whole units (24 decimals), when both are integers
    pub delta_units: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SectionReport {
    pub section: String,
    pub entries_a: usize,
    pub entries_b: usize,
    pub root_a: String,
    pub root_b: String,
    pub agree: bool,
    pub differing: usize,
    /// sum over differing entries of (b − a), whole units, when integer
    pub net_delta_units: f64,
    pub top: Vec<Diff>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShadowReport {
    pub node_a: String,
    pub node_b: String,
    pub height: u64,
    pub version_a: String,
    pub version_b: String,
    pub sections: Vec<SectionReport>,
    pub tries: usize,
}

const UNIT: f64 = 1e24;

/// Canonical JSON: object keys sorted at every depth (independent of serde_json's map feature).
pub fn canonical(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .iter()
                .map(|k| format!("{}:{}", serde_json::to_string(k).unwrap_or_default(), canonical(&m[*k])))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(a) => format!("[{}]", a.iter().map(canonical).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

fn amount(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn is_zero(s: &str) -> bool {
    s.chars().all(|c| c == '0')
}

/// Build the sections from a full-state export (`{"data": {...}}` or the bare object).
pub fn sections(export: &Value) -> Result<State, String> {
    let d = export.get("data").unwrap_or(export);
    let height = d.get("block_height").and_then(Value::as_u64).ok_or("full-state: no block_height")?;
    let version = d.get("version").and_then(Value::as_str).unwrap_or("?").to_string();
    let obj = |name: &str| d.get(name).and_then(Value::as_object);

    let mut wallet = Entries::new();
    for (k, v) in obj("wallet_balances").into_iter().flatten() {
        let a = amount(v).ok_or_else(|| format!("wallet {k}: amount is not a number"))?;
        if !is_zero(&a) {
            wallet.insert(format!("{k}_QUG"), a);
        }
    }
    for (k, v) in obj("token_balances").into_iter().flatten() {
        let a = amount(v).ok_or_else(|| format!("token {k}: amount is not a number"))?;
        if !is_zero(&a) {
            wallet.insert(k.clone(), a);
        }
    }

    let mut dex = Entries::new();
    for (id, p) in obj("liquidity_pools").into_iter().flatten() {
        let f = |n: &str| p.get(n).map(canonical).unwrap_or_else(|| "null".into());
        dex.insert(
            id.clone(),
            format!("t0={} t1={} r0={} r1={} lp={} d0={} d1={}",
                    f("token0"), f("token1"), f("reserve0"), f("reserve1"), f("lp_token_supply"),
                    f("token0_decimals"), f("token1_decimals")),
        );
    }

    let mut contract = Entries::new();
    for (id, c) in obj("contracts").into_iter().flatten() {
        contract.insert(id.clone(), canonical(c));
    }

    let mut counters = Entries::new();
    for (side, name) in [("credited", "dex_qug_credited"), ("debited", "dex_qug_debited")] {
        for (k, v) in obj(name).into_iter().flatten() {
            if let Some(a) = amount(v) {
                if !is_zero(&a) {
                    counters.insert(format!("{k}_{side}"), a);
                }
            }
        }
    }

    let mut sections = BTreeMap::new();
    sections.insert("wallet", wallet);
    sections.insert("dex", dex);
    sections.insert("contract", contract);
    sections.insert("dex_counters", counters);
    Ok(State { height, version, sections })
}

/// Section root: BLAKE3 over a domain tag, the section name and the sorted `key \0 value \n` leaves.
pub fn root(section: &str, e: &Entries) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"flux-legacy/quillon-roots/v1\0");
    h.update(section.as_bytes());
    h.update(b"\0");
    for (k, v) in e {
        h.update(k.as_bytes());
        h.update(b"\0");
        h.update(v.as_bytes());
        h.update(b"\n");
    }
    h.finalize().to_hex().to_string()
}

fn int(s: &str) -> Option<i128> {
    s.parse::<i128>().ok()
}

/// Every key whose value differs (or exists on one side only), biggest |delta| first.
pub fn diff(a: &Entries, b: &Entries) -> Vec<Diff> {
    let mut out = Vec::new();
    for k in a.keys().chain(b.keys().filter(|k| !a.contains_key(*k))) {
        let (va, vb) = (a.get(k), b.get(k));
        if va == vb {
            continue;
        }
        let delta = match (va.map_or(Some(0), |s| int(s)), vb.map_or(Some(0), |s| int(s))) {
            (Some(x), Some(y)) => Some((y - x) as f64 / UNIT),
            _ => None,
        };
        out.push(Diff { key: k.clone(), a: va.cloned(), b: vb.cloned(), delta_units: delta });
    }
    out.sort_by(|x, y| {
        let m = |d: &Diff| d.delta_units.map_or(f64::INFINITY, f64::abs);
        m(y).partial_cmp(&m(x)).unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

/// Compare two equal-height states section by section.
pub fn compare(node_a: &str, a: &State, node_b: &str, b: &State, top: usize, tries: usize) -> ShadowReport {
    let mut sections = Vec::new();
    for (name, ea) in &a.sections {
        let empty = Entries::new();
        let eb = b.sections.get(name).unwrap_or(&empty);
        let d = diff(ea, eb);
        let (ra, rb) = (root(name, ea), root(name, eb));
        sections.push(SectionReport {
            section: name.to_string(),
            entries_a: ea.len(),
            entries_b: eb.len(),
            agree: ra == rb,
            root_a: ra,
            root_b: rb,
            differing: d.len(),
            net_delta_units: d.iter().filter_map(|x| x.delta_units).sum(),
            top: d.into_iter().take(top).collect(),
        });
    }
    ShadowReport { node_a: node_a.into(), node_b: node_b.into(), height: a.height,
                   version_a: a.version.clone(), version_b: b.version.clone(), sections, tries }
}

fn fetch(client: &reqwest::blocking::Client, node: &str) -> Result<State, String> {
    let url = format!("{}/api/v1/sync/full-state", node.trim_end_matches('/'));
    let v: Value = client.get(&url).send().and_then(|r| r.json()).map_err(|e| format!("{url}: {e}"))?;
    sections(&v)
}

/// Which side to re-read next: the one BEHIND, so the snapshot of the one ahead stays fixed and the
/// laggard catches up to it. Measured 2026-11-02: re-reading both together missed equal height 20/20
/// times (the follower was always the one block a fresh producer block puts it behind).
pub fn reread_side(ha: u64, hb: u64) -> Option<char> {
    match ha.cmp(&hb) {
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Less => Some('a'),
        std::cmp::Ordering::Greater => Some('b'),
    }
}

/// Read both exports at the same moment (two threads), then re-read only the side that is behind
/// until both are at the same height. Never compares at unequal height.
pub fn shadow(node_a: &str, node_b: &str, tries: usize, top: usize) -> Result<ShadowReport, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let (ra, rb) = std::thread::scope(|s| {
        let ha = s.spawn(|| fetch(&client, node_a));
        let hb = s.spawn(|| fetch(&client, node_b));
        (ha.join().unwrap_or_else(|_| Err("thread".into())), hb.join().unwrap_or_else(|_| Err("thread".into())))
    });
    let (mut a, mut b) = (ra?, rb?);
    for t in 1..=tries.max(1) {
        match reread_side(a.height, b.height) {
            None => return Ok(compare(node_a, &a, node_b, &b, top, t)),
            Some('a') => { std::thread::sleep(std::time::Duration::from_millis(700)); a = fetch(&client, node_a)?; }
            Some(_) => { std::thread::sleep(std::time::Duration::from_millis(700)); b = fetch(&client, node_b)?; }
        }
    }
    if a.height == b.height {
        return Ok(compare(node_a, &a, node_b, &b, top, tries));
    }
    Err(format!("never at equal height in {tries} tries (last {} vs {}) — refusing to compare", a.height, b.height))
}

pub fn render(r: &ShadowReport) -> String {
    let mut s = format!(
        "⚖ SHADOW — SIGIL-style roots at equal height {}\n   A {} ({})\n   B {} ({})   · equal height after {} tr{}\n\n",
        r.height, r.node_a, r.version_a, r.node_b, r.version_b, r.tries, if r.tries == 1 { "y" } else { "ies" }
    );
    for x in &r.sections {
        let note = if x.section == "dex_counters" { "  (node-local by design: the DEX is off chain)" } else { "" };
        s.push_str(&format!(
            "{} {:<13} A {:>6} · B {:>6} entries · {} differ · net B−A {:+.6}{}\n      root A {}…  B {}…\n",
            if x.agree { "✅" } else { "🔴" }, x.section, x.entries_a, x.entries_b, x.differing,
            x.net_delta_units, note, &x.root_a[..16], &x.root_b[..16]
        ));
        for d in &x.top {
            let delta = d.delta_units.map_or("(not a number)".to_string(), |v| format!("{v:+.6}"));
            let key = if d.key.len() > 72 { format!("{}…", &d.key[..72]) } else { d.key.clone() };
            s.push_str(&format!("      {key}  {delta}{}\n",
                match (&d.a, &d.b) { (None, _) => "  (only on B)", (_, None) => "  (only on A)", _ => "" }));
        }
    }
    s.push_str("\n   events: no root — Quillon keeps no per-block event log.\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn export(h: u64, w: &[(&str, &str)], pools: Value) -> Value {
        let wb: serde_json::Map<String, Value> = w.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
        json!({"data": {"block_height": h, "version": "t", "wallet_balances": wb,
                        "token_balances": {"aa_51": "5", "bb_51": "0"}, "liquidity_pools": pools,
                        "contracts": {"c1": {"z": 1, "a": [2, {"y": 0, "b": 1}]}},
                        "dex_qug_credited": {"aa": "3"}, "dex_qug_debited": {}}})
    }

    #[test]
    fn roots_ignore_order_and_zeroes_but_catch_any_value() {
        let p = json!({});
        let a = sections(&export(7, &[("aa", "10"), ("bb", "20"), ("cc", "0")], p.clone())).unwrap();
        let b = sections(&export(7, &[("bb", "20"), ("aa", "10")], p.clone())).unwrap();
        assert_eq!(root("wallet", &a.sections["wallet"]), root("wallet", &b.sections["wallet"]),
                   "same balances in another order, plus a zero balance, are the same state");
        assert_eq!(a.sections["wallet"].len(), 3, "aa, bb native + aa_51 token; zeroes are no leaf");
        let c = sections(&export(7, &[("aa", "10"), ("bb", "21")], p)).unwrap();
        assert_ne!(root("wallet", &a.sections["wallet"]), root("wallet", &c.sections["wallet"]));
    }

    #[test]
    fn pool_root_is_reserves_not_bookkeeping() {
        let pool = |r0: &str, when: &str| json!({"pool-1": {"token0": "QUG", "token1": "X", "reserve0": r0,
            "reserve1": "9", "lp_token_supply": "3", "token0_decimals": 24, "token1_decimals": 24,
            "created_at": when, "provider": [1, 2]}});
        let a = sections(&export(7, &[], pool("100", "2026-04-03T11:09:28.536929522Z"))).unwrap();
        let b = sections(&export(7, &[], pool("100", "2026-04-03T11:09:28Z"))).unwrap();
        let c = sections(&export(7, &[], pool("101", "2026-04-03T11:09:28Z"))).unwrap();
        assert_eq!(root("dex", &a.sections["dex"]), root("dex", &b.sections["dex"]),
                   "created_at precision is an import artefact, not state");
        assert_ne!(root("dex", &a.sections["dex"]), root("dex", &c.sections["dex"]), "a reserve moved");
    }

    #[test]
    fn diff_names_each_account_and_its_delta_biggest_first() {
        let q = |x: i64| (x as i128 * 1_000_000_000_000_000_000_000_000).to_string();
        let a: Entries = [("w1", q(100)), ("w2", q(50)), ("w3", q(1))].iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        let b: Entries = [("w1", q(100)), ("w2", q(8)), ("w4", q(2))].iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        let d = diff(&a, &b);
        let got: Vec<(&str, f64)> = d.iter().map(|x| (x.key.as_str(), x.delta_units.unwrap())).collect();
        assert_eq!(got, vec![("w2", -42.0), ("w4", 2.0), ("w3", -1.0)]);
    }

    #[test]
    fn the_node_behind_is_the_one_read_again() {
        assert_eq!(reread_side(24_435_536, 24_435_535), Some('b'), "B (follower) behind → re-read B, keep A");
        assert_eq!(reread_side(10, 11), Some('a'));
        assert_eq!(reread_side(7, 7), None, "equal height → compare now");
    }

    #[test]
    fn canonical_json_sorts_keys_at_every_depth() {
        assert_eq!(canonical(&json!({"z": 1, "a": [2, {"y": 0, "b": 1}]})), r#"{"a":[2,{"b":1,"y":0}],"z":1}"#);
    }

    #[test]
    fn unequal_height_exports_are_never_compared_by_the_reader() {
        // compare() itself is pure; the guard lives in shadow(): heights must match or it refuses.
        let a = sections(&export(7, &[("aa", "1")], json!({}))).unwrap();
        assert_eq!(a.height, 7);
        assert!(sections(&json!({"data": {}})).is_err(), "an export without a height is refused");
    }
}
