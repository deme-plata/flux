//! Flux Science tools: CODATA lookups and the antimatter-gravity recomputation, served from
//! the `flux-science` crate (Viktor 2026-11-16: "update fluxc compiler with the above").
//!
//! * `flux_science_codata` — any of NIST's CODATA rows (2022, with the 2018 value beside it),
//!   read from the BLAKE3-pinned listings embedded in `flux_science::electromagnetism`, so an
//!   agent quotes CODATA instead of a remembered number.
//! * `flux_science_antimatter_gravity` — the numbers of the paper
//!   `flux-science-antimatter-gravity-codata-2022.pdf`, from `antimatter_gravity::summary`,
//!   the same function the paper binary prints from.
//!
//! Both tools are pure: no network, no files, no state.

use std::sync::OnceLock;

use serde_json::{json, Value};

use crate::handlers::{ToolDef, ToolRegistry};
use flux_science::antimatter_gravity;
use flux_science::electromagnetism::{CodataTable, Constant, NIST_2018_BLAKE3, NIST_2022_BLAKE3};

const NIST_SOURCE: &str = "https://physics.nist.gov/cuu/Constants/Table/allascii.txt";

fn t2022() -> &'static CodataTable {
    static T: OnceLock<CodataTable> = OnceLock::new();
    T.get_or_init(CodataTable::nist_2022)
}

fn t2018() -> &'static CodataTable {
    static T: OnceLock<CodataTable> = OnceLock::new();
    T.get_or_init(CodataTable::nist_2018)
}

fn row_json(c: &Constant) -> Value {
    json!({
        "value": c.value,
        "uncertainty": c.sigma,
        "relative_uncertainty": c.rel_sigma(),
        "exact": c.exact,
        "unit": c.unit,
    })
}

/// One constant, the 2022 row first, with the 2018 row and the shift when 2018 lists it too.
fn constant_json(c22: &Constant, t18: &CodataTable) -> Value {
    let mut v = json!({ "name": c22.name, "codata_2022": row_json(c22) });
    if let Some(c18) = t18.get(&c22.name) {
        let shift = c22.value - c18.value;
        v["codata_2018"] = row_json(c18);
        v["shift_2018_to_2022"] = json!(shift);
        v["shift_in_2018_sigma"] = if c18.sigma > 0.0 { json!(shift / c18.sigma) } else { Value::Null };
    }
    v
}

/// Exact name first (case-insensitive), then every row whose name contains the query.
fn find<'a>(t: &'a CodataTable, query: &str) -> Vec<&'a Constant> {
    let q = query.trim().to_lowercase();
    if let Some(c) = t.constants.iter().find(|c| c.name.to_lowercase() == q) {
        return vec![c];
    }
    t.constants.iter().filter(|c| c.name.to_lowercase().contains(&q)).collect()
}

fn flux_science_codata(args: &Value) -> String {
    let (t22, t18) = (t2022(), t2018());
    let source = json!({
        "url": NIST_SOURCE,
        "codata_2022_blake3": NIST_2022_BLAKE3,
        "codata_2018_blake3": NIST_2018_BLAKE3,
        "rows_2022": t22.constants.len(),
        "rows_2018": t18.constants.len(),
    });
    let Some(query) = args.get("name").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()) else {
        return json!({
            "ok": true,
            "source": source,
            "note": "Pass name (NIST's own wording, or any part of it, case-insensitive), e.g. \"electron mass\", \"fine-structure\", \"Planck\". CODATA lists no antiparticle rows: antimatter values are matter rows carried over by CPT symmetry.",
            "examples": ["electron mass", "inverse fine-structure constant", "proton mass", "Newtonian constant of gravitation", "Rydberg constant", "vacuum mag. permeability"],
        })
        .to_string();
    };
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10).clamp(1, 50) as usize;
    let hits = find(t22, query);
    if hits.is_empty() {
        return json!({
            "ok": false,
            "error": format!("no CODATA 2022 row contains \"{query}\""),
            "hint": "NIST's names are long-form and abbreviated in places, e.g. \"mag. mom.\" for magnetic moment; try one word",
            "source": source,
        })
        .to_string();
    }
    let total = hits.len();
    let rows: Vec<Value> = hits.into_iter().take(limit).map(|c| constant_json(c, t18)).collect();
    json!({ "ok": true, "query": query, "matches": total, "returned": rows.len(), "constants": rows, "source": source })
        .to_string()
}

fn flux_science_antimatter_gravity(args: &Value) -> String {
    let s = antimatter_gravity::summary(t2022(), t2018());
    let all = s.to_json();
    match args.get("section").and_then(|v| v.as_str()) {
        None | Some("") | Some("all") => json!({ "ok": true, "summary": all }).to_string(),
        Some(sec) => match all.get(sec) {
            Some(block) => json!({ "ok": true, "section": sec, "summary": block, "paper": all["paper"] }).to_string(),
            None => {
                let names: Vec<&str> = all.as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
                json!({ "ok": false, "error": format!("no section \"{sec}\""), "sections": names }).to_string()
            }
        },
    }
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(ToolDef {
        name: "flux_science_codata",
        description: "Look up a CODATA physical constant (NIST's 2022 adjustment, the latest; the 2018 value and the 2018→2022 shift in 2018 standard deviations beside it). Read from the BLAKE3-pinned NIST listings embedded in flux-science, so the answer is NIST's printed row, not a remembered number; the hashes are in every reply. Args: name (NIST wording or any part of it, case-insensitive; omit for an overview), limit (default 10, max 50). Pure: no network.",
        input_schema: json!({"type":"object","properties":{"name":{"type":"string"},"limit":{"type":"integer"}}}),
    }, flux_science_codata);
    registry.register(ToolDef {
        name: "flux_science_antimatter_gravity",
        description: "Does antimatter fall? The numbers of the Flux Science paper flux-science-antimatter-gravity-codata-2022.pdf, recomputed from CODATA 2022 on each call by the same function the paper prints from: ALPHA-g's magnetic scale, a refit of its 2023 counts (balance −0.752 g, counting-only no-gravity 11.9σ vs the paper's ~3.4σ with systematics), the trapped-state moments, BASE's proton/H⁻ ratio and gravity bounds, GBAR's fall times and quantum states. Each block carries the experiment's printed value beside the recomputed one. Args: section (codata, hydrogen, alpha_g_scale, trapped_moments_at_1_t_mu_b, alpha_g_refit, base, gbar; omit for all). Pure: no network.",
        input_schema: json!({"type":"object","properties":{"section":{"type":"string"}}}),
    }, flux_science_antimatter_gravity);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(f: fn(&Value) -> String, args: Value) -> Value {
        serde_json::from_str(&f(&args)).expect("tool returns JSON")
    }

    #[test]
    fn codata_exact_name_returns_nists_row_and_the_2018_shift() {
        let v = call(flux_science_codata, json!({"name": "electron mass"}));
        assert_eq!(v["ok"], true);
        assert_eq!(v["matches"], 1, "an exact name returns that row only");
        let c = &v["constants"][0];
        assert_eq!(c["codata_2022"]["value"].as_f64(), Some(9.109_383_713_9e-31));
        assert_eq!(c["codata_2018"]["value"].as_f64(), Some(9.109_383_701_5e-31));
        assert!(c["shift_in_2018_sigma"].as_f64().unwrap() > 0.0);
        assert_eq!(v["source"]["codata_2022_blake3"].as_str(), Some(NIST_2022_BLAKE3));
    }

    #[test]
    fn codata_substring_search_and_misses() {
        let v = call(flux_science_codata, json!({"name": "Planck", "limit": 3}));
        assert!(v["matches"].as_u64().unwrap() > 3 && v["returned"] == 3);
        let exact = call(flux_science_codata, json!({"name": "speed of light in vacuum"}));
        assert_eq!(exact["constants"][0]["codata_2022"]["exact"], true);
        assert!(exact["constants"][0]["shift_in_2018_sigma"].is_null(), "exact constants have no sigma");
        let miss = call(flux_science_codata, json!({"name": "antiproton mass"}));
        assert_eq!(miss["ok"], false, "CODATA has no antiparticle rows");
        let overview = call(flux_science_codata, json!({}));
        assert_eq!(overview["source"]["rows_2022"], 355);
    }

    #[test]
    fn antimatter_tool_serves_the_papers_numbers() {
        let v = call(flux_science_antimatter_gravity, json!({}));
        let b = v["summary"]["alpha_g_refit"]["logistic"]["balance_g"].as_f64().unwrap();
        assert!((b + 0.752).abs() < 5e-4, "balance {b}");
        let base = call(flux_science_antimatter_gravity, json!({"section": "base"}));
        assert!((base["summary"]["r_difference_ppt"].as_f64().unwrap() + 0.043).abs() < 5e-4);
        let bad = call(flux_science_antimatter_gravity, json!({"section": "nope"}));
        assert_eq!(bad["ok"], false);
        assert!(bad["sections"].as_array().unwrap().iter().any(|s| s == "gbar"));
    }

    #[test]
    fn both_tools_are_registered() {
        let mut r = ToolRegistry::new();
        register(&mut r);
        let names: Vec<String> = r.tools_schema().iter().filter_map(|t| t["name"].as_str().map(String::from)).collect();
        assert!(names.contains(&"flux_science_codata".to_string()));
        assert!(names.contains(&"flux_science_antimatter_gravity".to_string()));
    }
}
