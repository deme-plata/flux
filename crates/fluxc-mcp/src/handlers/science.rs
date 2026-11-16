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

/// NIST abbreviates exactly three words in its names (63 of the 355 rows in 2022). Matching
/// on the expanded form lets "magnetic moment" and "mag. mom." find the same rows.
/// `gyromag.` goes first so it is not read as `gyro` + `mag.`.
const NIST_ABBREVIATIONS: [(&str, &str); 3] =
    [("gyromag.", "gyromagnetic"), ("mag.", "magnetic"), ("mom.", "moment")];

/// One-letter symbols, matched CASE-SENSITIVELY on the whole query: G is Newton's constant,
/// g is standard gravity. Checked before the case-insensitive aliases below.
const SYMBOLS: &[(&str, &str)] = &[
    ("c", "speed of light in vacuum"),
    ("G", "newtonian constant of gravitation"),
    ("g", "standard acceleration of gravity"),
    ("h", "planck constant"),
    ("k", "boltzmann constant"),
    ("e", "elementary charge"),
    ("u", "atomic mass constant"),
    ("R", "molar gas constant"),
    ("F", "faraday constant"),
];

/// Everyday names and symbols → NIST's own wording. An alias applies only when it is the
/// WHOLE query, so a short symbol like "e" never hijacks a longer search such as "electron".
/// Every target is checked against the 2022 table by a test.
const ALIASES: &[(&str, &str)] = &[
    ("alpha", "fine-structure constant"),
    ("α", "fine-structure constant"),
    ("fine structure constant", "fine-structure constant"),
    ("1/alpha", "inverse fine-structure constant"),
    ("alpha inverse", "inverse fine-structure constant"),
    ("inverse alpha", "inverse fine-structure constant"),
    ("inverse fine structure constant", "inverse fine-structure constant"),
    ("speed of light", "speed of light in vacuum"),
    ("big g", "newtonian constant of gravitation"),
    ("gravitational constant", "newtonian constant of gravitation"),
    ("newton's constant", "newtonian constant of gravitation"),
    ("newtonian constant", "newtonian constant of gravitation"),
    ("little g", "standard acceleration of gravity"),
    ("standard gravity", "standard acceleration of gravity"),
    ("g_n", "standard acceleration of gravity"),
    ("planck's constant", "planck constant"),
    ("hbar", "reduced planck constant"),
    ("h-bar", "reduced planck constant"),
    ("ħ", "reduced planck constant"),
    ("dirac constant", "reduced planck constant"),
    ("k_b", "boltzmann constant"),
    ("boltzmann's constant", "boltzmann constant"),
    ("electron charge", "elementary charge"),
    ("charge of the electron", "elementary charge"),
    ("mu0", "vacuum magnetic permeability"),
    ("μ0", "vacuum magnetic permeability"),
    ("magnetic constant", "vacuum magnetic permeability"),
    ("vacuum permeability", "vacuum magnetic permeability"),
    ("permeability of free space", "vacuum magnetic permeability"),
    ("epsilon0", "vacuum electric permittivity"),
    ("ε0", "vacuum electric permittivity"),
    ("electric constant", "vacuum electric permittivity"),
    ("vacuum permittivity", "vacuum electric permittivity"),
    ("permittivity of free space", "vacuum electric permittivity"),
    ("n_a", "avogadro constant"),
    ("avogadro's number", "avogadro constant"),
    ("avogadro number", "avogadro constant"),
    ("gas constant", "molar gas constant"),
    ("z0", "characteristic impedance of vacuum"),
    ("impedance of free space", "characteristic impedance of vacuum"),
    ("g-2", "muon magnetic moment anomaly"),
    ("muon g-2", "muon magnetic moment anomaly"),
    ("muon anomaly", "muon magnetic moment anomaly"),
    ("a_mu", "muon magnetic moment anomaly"),
    ("electron g-2", "electron magnetic moment anomaly"),
    ("electron anomaly", "electron magnetic moment anomaly"),
    ("a_e", "electron magnetic moment anomaly"),
    ("proton radius", "proton rms charge radius"),
    ("r_inf", "rydberg constant"),
    ("m_p", "proton mass"),
    ("m_e", "electron mass"),
    ("r_k", "von klitzing constant"),
    ("k_j", "josephson constant"),
    ("stefan boltzmann constant", "stefan-boltzmann constant"),
    ("amu", "atomic mass constant"),
    ("dalton", "atomic mass constant"),
    ("atomic mass unit", "atomic mass constant"),
    ("hartree", "hartree energy"),
    ("a0", "bohr radius"),
    ("mu_b", "bohr magneton"),
];

/// Lower case, NIST's three abbreviations expanded, whitespace collapsed.
fn normalize(s: &str) -> String {
    let mut t = s.trim().to_lowercase();
    for (short, full) in NIST_ABBREVIATIONS {
        t = t.replace(short, full);
    }
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Search on normalized names: exact first, then rows containing the query, then (if none)
/// rows containing every word of it in any order. Shortest names first, so a basic row comes
/// before its "in eV" / "energy equivalent" variants. Returns the hits and the alias applied.
fn find<'a>(t: &'a CodataTable, query: &str) -> (Vec<&'a Constant>, Option<&'static str>) {
    let mut q = normalize(query);
    let alias = SYMBOLS
        .iter()
        .find(|(sym, _)| *sym == query.trim())
        .or_else(|| ALIASES.iter().find(|(a, _)| *a == q))
        .map(|(_, target)| *target);
    if let Some(target) = alias {
        q = target.to_string();
    }
    let names: Vec<(String, &Constant)> = t.constants.iter().map(|c| (normalize(&c.name), c)).collect();
    if let Some((_, c)) = names.iter().find(|(n, _)| *n == q) {
        return (vec![*c], alias);
    }
    let mut hits: Vec<&(String, &Constant)> = names.iter().filter(|(n, _)| n.contains(&q)).collect();
    if hits.is_empty() {
        let words: Vec<&str> = q.split(' ').collect();
        hits = names.iter().filter(|(n, _)| words.iter().all(|w| n.contains(w))).collect();
    }
    hits.sort_by_key(|(n, _)| n.len());
    (hits.into_iter().map(|(_, c)| *c).collect(), alias)
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
    let (hits, alias) = find(t22, query);
    if hits.is_empty() {
        return json!({
            "ok": false,
            "error": format!("no CODATA 2022 row matches \"{query}\""),
            "hint": "words may come in any order and \"magnetic moment\" = NIST's \"mag. mom.\"; try fewer words. CODATA has no antiparticle rows.",
            "source": source,
        })
        .to_string();
    }
    let total = hits.len();
    let rows: Vec<Value> = hits.into_iter().take(limit).map(|c| constant_json(c, t18)).collect();
    let mut v = json!({ "ok": true, "query": query, "matches": total, "returned": rows.len(), "constants": rows, "source": source });
    if let Some(target) = alias {
        v["alias"] = json!(format!("\"{query}\" read as \"{target}\""));
    }
    v.to_string()
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
    fn every_alias_names_exactly_one_nist_row() {
        let t = t2022();
        for (alias, target) in SYMBOLS.iter().chain(ALIASES) {
            let n = t.constants.iter().filter(|c| normalize(&c.name) == *target).count();
            assert_eq!(n, 1, "alias {alias:?} → {target:?} names {n} rows of the 2022 table");
        }
    }

    #[test]
    fn spelled_out_and_abbreviated_find_the_same_row() {
        let a = call(flux_science_codata, json!({"name": "muon magnetic moment anomaly"}));
        let b = call(flux_science_codata, json!({"name": "muon mag. mom. anomaly"}));
        assert_eq!(a["matches"], 1);
        assert_eq!(a["constants"], b["constants"]);
        let g = call(flux_science_codata, json!({"name": "electron gyromagnetic ratio"}));
        assert_eq!(g["constants"][0]["name"], "electron gyromag. ratio");
    }

    #[test]
    fn aliases_and_any_word_order() {
        let alpha = call(flux_science_codata, json!({"name": "Alpha"}));
        assert_eq!(alpha["constants"][0]["codata_2022"]["value"].as_f64(), Some(7.297_352_564_3e-3));
        assert!(alpha["alias"].as_str().unwrap().contains("fine-structure constant"));
        let g = call(flux_science_codata, json!({"name": "big G"}));
        assert_eq!(g["constants"][0]["name"], "Newtonian constant of gravitation");
        // one-letter symbols are case-sensitive: G is Newton's constant, g is standard gravity
        assert_eq!(call(flux_science_codata, json!({"name": "G"}))["constants"][0]["name"], "Newtonian constant of gravitation");
        assert_eq!(call(flux_science_codata, json!({"name": "g"}))["constants"][0]["name"], "standard acceleration of gravity");
        let mu0 = call(flux_science_codata, json!({"name": "μ0"}));
        assert_eq!(mu0["constants"][0]["name"], "vacuum mag. permeability");
        let g2 = call(flux_science_codata, json!({"name": "g-2"}));
        assert_eq!(g2["constants"][0]["name"], "muon mag. mom. anomaly");
        // "e" is an alias only as the whole query; "electron" is still a plain search.
        assert_eq!(call(flux_science_codata, json!({"name": "e"}))["constants"][0]["name"], "elementary charge");
        assert!(call(flux_science_codata, json!({"name": "electron"}))["matches"].as_u64().unwrap() > 10);
        // words in any order, shortest name first
        let w = call(flux_science_codata, json!({"name": "magnetic moment proton"}));
        assert_eq!(w["constants"][0]["name"], "proton mag. mom.");
        let p = call(flux_science_codata, json!({"name": "planck"}));
        assert_eq!(p["constants"][0]["name"], "Planck mass", "shortest Planck row first");
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
