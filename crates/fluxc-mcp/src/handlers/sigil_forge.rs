//! `flux_sigil_forge_*` — the second half of the Kristensen Realization
//! Functional: not "how good is this design" (that is
//! [`super::sigil_realization`]) but **"search the space, pick one, and keep a
//! receipt strong enough that anyone can rebuild it."**
//!
//! ## The pipeline these five tools implement
//!
//! ```text
//!   possibility  ->  verification  ->  consensus  ->  replication  ->  matter
//!   ----------      ------------      ---------      -----------      ------
//!   _search          _search           _anchor        _trace           the thing
//!                    (Theta_phys)      (digest)       (rebuild it)     you built
//!                                          |
//!                                      _reward  (who gets paid for the revision)
//!                                      _yield   (what the running thing earns)
//! ```
//!
//! The claim being made is deliberately deflationary. Nothing here fetches a
//! finished object from another branch of the wavefunction. What it does is
//! cheaper and actually works: **explore a combinatorial design space
//! arithmetically, discard everything the physics forbids, rank what survives,
//! then content-address the winner so the recipe — not the object — is the thing
//! that replicates.** A recipe copies for free; a building does not.
//!
//! ## Why `_anchor` exists at all
//!
//! Ten questions have to be answerable about any object that claims to be
//! verified: which design, which revision, who verified it, which supplier,
//! which robot, which material, which test, which state root, which payment,
//! and does the thing actually exist. A record that answers nine of them is not
//! 90% trustworthy — it is untrustworthy with a specific hole, so `_anchor`
//! names the hole rather than hiding it (`complete: false`, `unanswered: [..]`).
//!
//! Records chain: each one carries `prev`, the digest of the previous record for
//! the same design, so revisions form an append-only line you can walk with
//! `_trace`. That is the local half of provenance and it is real today. The
//! chain half — putting the digest where a stranger can check it — is a
//! PROPOSAL this tool prints and never executes; see `anchor_plan` in the
//! output and the money-confirmation rule.
//!
//! Ledger (append-only JSONL): `SIGIL_FORGE_LEDGER`.

use serde_json::{json, Value};
use std::time::Duration;

use crate::handlers::{ToolDef, ToolRegistry};
use flux_realization::{
    evaluate, velocity, Capability, Constraint, Design, Hardness, Peer, Provenance, Tracked,
};

const DEFAULT_LEDGER: &str = "/home/storage/claude-code/k-parameter-paper/gauge-series/sigil-forge-ledger.jsonl";

/// The ten questions, in the fixed order they are hashed in. Changing this
/// order changes every digest, so it is a wire format: append, never reorder.
const QUESTIONS: [&str; 10] = [
    "design",      // which design
    "revision",    // which revision
    "verified_by", // who verified it
    "supplier",    // which supplier
    "robot",       // which robot
    "material",    // which material
    "test",        // which test
    "state_root",  // which state root
    "payment",     // which payment
    "exists",      // which building/unit actually exists
];

/// A search that enumerates more than this is refusing, not thinking. The
/// Cartesian product of a modular design space grows fast enough that a typo in
/// one axis turns a 4,000-candidate sweep into a 4-million one.
const MAX_COMBOS: usize = 200_000;

fn ledger_path() -> String {
    std::env::var("SIGIL_FORGE_LEDGER").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| DEFAULT_LEDGER.to_string())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").trim().to_string()
}
fn fnum(v: &Value, k: &str, d: f64) -> f64 {
    v.get(k).and_then(|x| x.as_f64()).filter(|x| x.is_finite()).unwrap_or(d)
}

fn get_json(path: &str) -> Result<Value, String> {
    let url = format!("{}{}", crate::handlers::sigil_wallet::rpc_base(), path);
    match ureq::get(&url).timeout(Duration::from_secs(12)).call() {
        Ok(r) => {
            let body = r.into_string().map_err(|e| e.to_string())?;
            serde_json::from_str(&body).map_err(|e| format!("{url}: not JSON ({e})"))
        }
        Err(ureq::Error::Status(c, _)) => Err(format!("{url}: HTTP {c}")),
        Err(e) => Err(format!("{url}: {e}")),
    }
}

fn data(v: Value) -> Value {
    v.get("data").cloned().unwrap_or(v)
}

// ───────────────────────────── 1. SEARCH ─────────────────────────────

/// One selectable option on one axis of the design space.
struct Opt {
    label: String,
    /// Capability factors, `None` where this option says nothing about a term.
    caps: [Option<f64>; 5],
    constraints: Vec<Constraint>,
    days: f64,
    cost_usd: f64,
}

const CAP_NAMES: [&str; 5] = ["compute", "energy", "verifiable", "availability", "expandable"];

fn parse_constraint(v: &Value) -> Result<Constraint, String> {
    let name = s(v, "name");
    if name.is_empty() {
        return Err("a constraint needs a name — 'power', 'wafer_supply', not 'constraint_3'".into());
    }
    let required = fnum(v, "required", f64::NAN);
    let available = fnum(v, "available", f64::NAN);
    if !required.is_finite() || !available.is_finite() {
        return Err(format!("constraint '{name}' needs finite `required` and `available`"));
    }
    let hardness = match s(v, "hardness").to_lowercase().as_str() {
        "hard" => Hardness::Hard,
        "" | "soft" => Hardness::Soft,
        other => return Err(format!("constraint '{name}': hardness must be 'hard' or 'soft', got '{other}'")),
    };
    let provenance = match s(v, "provenance").to_lowercase().as_str() {
        "measured" => Provenance::Measured,
        "derived" => Provenance::Derived,
        "protocol" => Provenance::Protocol,
        "unavailable" => Provenance::Unavailable,
        _ => Provenance::Placeholder,
    };
    Ok(Constraint {
        name,
        required,
        available,
        hardness,
        provenance,
        note: s(v, "note"),
    })
}

fn parse_option(v: &Value) -> Result<Opt, String> {
    let label = s(v, "label");
    if label.is_empty() {
        return Err("every option needs a `label`".into());
    }
    let mut caps = [None; 5];
    if let Some(c) = v.get("capability") {
        for (i, n) in CAP_NAMES.iter().enumerate() {
            if let Some(x) = c.get(*n).and_then(|x| x.as_f64()) {
                if !x.is_finite() || x < 0.0 {
                    return Err(format!("option '{label}': capability.{n} must be a finite factor >= 0"));
                }
                caps[i] = Some(x);
            }
        }
    }
    let mut constraints = Vec::new();
    if let Some(arr) = v.get("constraints").and_then(|x| x.as_array()) {
        for c in arr {
            constraints.push(parse_constraint(c).map_err(|e| format!("option '{label}': {e}"))?);
        }
    }
    Ok(Opt { label, caps, constraints, days: fnum(v, "days", 0.0).max(0.0), cost_usd: fnum(v, "cost_usd", 0.0).max(0.0) })
}

/// Merge same-named constraints across the chosen options: requirements ADD
/// (two subsystems both drawing power draw the sum) while availability takes the
/// MINIMUM (the tightest claim wins — if one module knows the feeder is 20 MW,
/// no other module gets to assume 40).
fn merge_constraints(parts: &[&Vec<Constraint>]) -> Vec<Constraint> {
    let mut out: Vec<Constraint> = Vec::new();
    for part in parts {
        for c in part.iter() {
            if let Some(existing) = out.iter_mut().find(|e| e.name == c.name) {
                existing.required += c.required;
                if c.available < existing.available {
                    existing.available = c.available;
                }
                // A merged constraint is only as trustworthy as its worst input,
                // and only as forgiving as its hardest.
                existing.provenance = Provenance::worst(existing.provenance, c.provenance);
                if c.hardness == Hardness::Hard {
                    existing.hardness = Hardness::Hard;
                }
            } else {
                out.push(c.clone());
            }
        }
    }
    out
}

fn flux_sigil_forge_search(args: &Value) -> String {
    let base_name = { let n = s(args, "name"); if n.is_empty() { "candidate".to_string() } else { n } };

    // ── parse the axes ──
    let modules = match args.get("modules").and_then(|x| x.as_array()) {
        Some(m) if !m.is_empty() => m,
        _ => return json!({"ok": false, "error": "modules is required: a non-empty array of {axis, options:[...]}",
                           "hint": "each option = {label, capability{compute,energy,verifiable,availability,expandable}, constraints:[{name,required,available,hardness,provenance,note}], days, cost_usd}"}).to_string(),
    };
    let mut axes: Vec<(String, Vec<Opt>)> = Vec::new();
    let mut total: usize = 1;
    for m in modules {
        let axis = { let a = s(m, "axis"); if a.is_empty() { format!("axis_{}", axes.len() + 1) } else { a } };
        let opts = match m.get("options").and_then(|x| x.as_array()) {
            Some(o) if !o.is_empty() => o,
            _ => return json!({"ok": false, "error": format!("axis '{axis}' has no options")}).to_string(),
        };
        let mut parsed = Vec::new();
        for o in opts {
            match parse_option(o) {
                Ok(p) => parsed.push(p),
                Err(e) => return json!({"ok": false, "error": format!("axis '{axis}': {e}")}).to_string(),
            }
        }
        total = total.saturating_mul(parsed.len());
        if total > MAX_COMBOS {
            return json!({"ok": false, "error": format!("design space is {total}+ combinations, above the {MAX_COMBOS} cap"),
                          "hint": "that is almost always a typo in one axis. Split the search, or prune the axis with the most options first"}).to_string();
        }
        axes.push((axis, parsed));
    }

    // ── the shared parts: peers, coordination ──
    let mut peers: Vec<Peer> = Vec::new();
    if let Some(arr) = args.get("peers").and_then(|x| x.as_array()) {
        for p in arr {
            peers.push(Peer::new(&s(p, "id"), fnum(p, "useful", 0.0), fnum(p, "verified", 0.0), fnum(p, "reliable", 0.0)));
        }
    }
    if peers.is_empty() {
        // A single peer of full weight: "one builder, working, verified". Named
        // as a placeholder so the reading never claims a measured fleet.
        peers.push(Peer::new("assumed_single_builder", 1.0, 1.0, 1.0));
    }
    let peer_prov = if args.get("peers").is_some() { Provenance::Derived } else { Provenance::Placeholder };
    let coordination = fnum(args, "coordination", 0.0).max(0.0);
    let coord_tracked = if args.get("coordination").is_some() {
        Tracked { value: coordination, provenance: Provenance::Derived, note: "K_coord supplied by the caller".into() }
    } else {
        Tracked { value: 0.0, provenance: Provenance::Placeholder, note: "K_coord defaulted to 0 — 'everything is already coordinated', which is almost never true".into() }
    };
    let top_n = args.get("top").and_then(|x| x.as_u64()).unwrap_or(8).clamp(1, 50) as usize;

    // ── enumerate ──
    let counts: Vec<usize> = axes.iter().map(|(_, o)| o.len()).collect();
    let mut idx = vec![0usize; axes.len()];
    let mut rows: Vec<Value> = Vec::new();
    let mut feasible = 0usize;
    let mut binder_tally: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();

    loop {
        // build this candidate
        let chosen: Vec<&Opt> = axes.iter().zip(idx.iter()).map(|((_, o), i)| &o[*i]).collect();
        let mut cap_vals: [Option<f64>; 5] = [None; 5];
        for o in &chosen {
            for i in 0..5 {
                if let Some(f) = o.caps[i] {
                    cap_vals[i] = Some(cap_vals[i].unwrap_or(1.0) * f);
                }
            }
        }
        let mk = |i: usize| -> Tracked<f64> {
            match cap_vals[i] {
                // Derived, not Measured: it is a product of declared factors.
                Some(v) => Tracked { value: v, provenance: Provenance::Derived, note: "product of the chosen options' declared factors".into() },
                None => Tracked { value: 0.0, provenance: Provenance::Unavailable, note: "no option on any axis declared this term — excluded from the mean, never substituted".into() },
            }
        };
        let capability = Capability {
            compute: mk(0), energy: mk(1), verifiable: mk(2), availability: mk(3), expandable: mk(4),
        };
        let parts: Vec<&Vec<Constraint>> = chosen.iter().map(|o| &o.constraints).collect();
        let constraints = merge_constraints(&parts);
        let days: f64 = chosen.iter().map(|o| o.days).sum();
        let cost: f64 = chosen.iter().map(|o| o.cost_usd).sum();
        let picks: Vec<String> = axes.iter().zip(chosen.iter()).map(|((a, _), o)| format!("{a}={}", o.label)).collect();

        let d = Design {
            name: format!("{base_name}[{}]", picks.join(" ")),
            capability,
            constraints,
            peers: peers.clone(),
            peer_provenance: peer_prov,
            coordination: coord_tracked.clone(),
        };
        let r = evaluate(&d);
        // Time is what Gamma_R divides by; a zero-day build is a data error, so
        // fall back to one day rather than manufacturing an infinite velocity.
        let t = if days > 0.0 { days } else { 1.0 };
        let v = velocity(&r, t, 0.0, 0.0);
        if r.feasibility.in_landscape {
            feasible += 1;
            if let Some(b) = &r.bottleneck.binding {
                *binder_tally.entry(b.clone()).or_insert(0) += 1;
            }
        }
        rows.push(json!({
            "name": r.name, "picks": picks, "k_r": r.k_r, "gamma_r": v.gamma_r,
            "in_landscape": r.feasibility.in_landscape, "violated_hard": r.feasibility.violated_hard,
            "limiting_factor": r.limiting_factor, "binding": r.bottleneck.binding,
            "capability_core": r.capability.core, "capability_excluded": r.capability.excluded,
            "days": days, "cost_usd": cost, "friction": r.friction, "provenance": r.provenance.code()
        }));

        // odometer increment
        let mut k = idx.len();
        loop {
            if k == 0 { break; }
            k -= 1;
            idx[k] += 1;
            if idx[k] < counts[k] { break; }
            idx[k] = 0;
            if k == 0 { k = usize::MAX; break; }
        }
        if k == usize::MAX { break; }
    }

    let cmp = |key: &'static str| move |a: &Value, b: &Value| {
        a.get(key).and_then(|x| x.as_f64()).unwrap_or(0.0)
            .partial_cmp(&b.get(key).and_then(|x| x.as_f64()).unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    };
    let mut by_kr = rows.clone();
    by_kr.sort_by(|a, b| cmp("k_r")(b, a));
    let mut by_gamma = rows.clone();
    by_gamma.sort_by(|a, b| cmp("gamma_r")(b, a));

    let mut binders: Vec<Value> = binder_tally.iter()
        .map(|(k, n)| json!({"constraint": k, "binding_in": n, "pct_of_feasible": if feasible > 0 { *n as f64 * 100.0 / feasible as f64 } else { 0.0 }}))
        .collect();
    binders.sort_by(|a, b| cmp("binding_in")(b, a));

    let headline = match (by_kr.first(), by_gamma.first()) {
        (Some(k), Some(g)) if k.get("name") == g.get("name") =>
            format!("one design wins both rankings: {}", k.get("name").and_then(|x| x.as_str()).unwrap_or("?")),
        (Some(k), Some(g)) => format!(
            "best single unit is {} (K_R {:.4}); best REALIZATION is {} (Gamma_R {:.4}) — they differ, which is the whole reason both are reported",
            k.get("name").and_then(|x| x.as_str()).unwrap_or("?"), k.get("k_r").and_then(|x| x.as_f64()).unwrap_or(0.0),
            g.get("name").and_then(|x| x.as_str()).unwrap_or("?"), g.get("gamma_r").and_then(|x| x.as_f64()).unwrap_or(0.0)),
        _ => "no candidates".to_string(),
    };

    json!({
        "ok": true,
        "searched": rows.len(),
        "in_landscape": feasible,
        "in_swampland": rows.len() - feasible,
        "headline": headline,
        "top_by_k_r": by_kr.into_iter().take(top_n).collect::<Vec<_>>(),
        "top_by_gamma_r": by_gamma.into_iter().take(top_n).collect::<Vec<_>>(),
        "binding_constraint_frequency": binders,
        "model": {
            "capability": "terms MULTIPLY across axes; a term no option declares is Unavailable and is EXCLUDED from the geometric mean, never substituted with a guess",
            "constraints": "same-named constraints merge: required ADDS, available takes the MINIMUM, hardness takes the HARDER, provenance takes the WORSE",
            "time": "days SUM across axes and feed Gamma_R; cost_usd sums and is reported but does NOT enter K_R"
        },
        "caveats": [
            "this searches a space you declared. It cannot find an option you did not list — the honest output of a sweep is a ranking of YOUR candidates, not of all possible designs",
            "binding_constraint_frequency is the highest-value line here: a constraint that binds in most of the feasible space is worth more engineering than any choice inside the space",
            "K_R is an engineering figure of merit, not a law — the 1/5 exponent, the log and the 1+ denominator are chosen and are falsifiable only as a ranking"
        ]
    }).to_string()
}

// ───────────────────────────── 2. ANCHOR ─────────────────────────────

/// Digest over the ten answers in `QUESTIONS` order plus the previous digest.
/// Field-separated so `{design:"ab", revision:"c"}` and `{design:"a",
/// revision:"bc"}` cannot collide.
fn digest(answers: &[(String, String)], prev: &str) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"sigil-forge-anchor-v1\x1e");
    for (k, v) in answers {
        h.update(k.as_bytes());
        h.update(b"\x1f");
        h.update(v.as_bytes());
        h.update(b"\x1e");
    }
    h.update(b"prev\x1f");
    h.update(prev.as_bytes());
    hex::encode(h.finalize().as_bytes())
}

fn read_ledger() -> Vec<Value> {
    std::fs::read_to_string(ledger_path())
        .map(|s| s.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).collect())
        .unwrap_or_default()
}

fn append_ledger(rec: &Value) -> Result<String, String> {
    use std::io::Write;
    let p = ledger_path();
    if let Some(dir) = std::path::Path::new(&p).parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut fh = std::fs::OpenOptions::new().create(true).append(true).open(&p).map_err(|e| format!("{p}: {e}"))?;
    writeln!(fh, "{}", rec).map_err(|e| e.to_string())?;
    Ok(p)
}

fn flux_sigil_forge_anchor(args: &Value) -> String {
    let design = s(args, "design");
    if design.is_empty() {
        return json!({"ok": false, "error": "design is required — it is the identity the revision chain hangs from",
                      "questions": QUESTIONS}).to_string();
    }

    let answers: Vec<(String, String)> = QUESTIONS.iter().map(|q| (q.to_string(), s(args, q))).collect();
    let unanswered: Vec<&str> = QUESTIONS.iter().zip(answers.iter()).filter(|(_, (_, v))| v.is_empty()).map(|(q, _)| *q).collect();

    // Walk the existing chain for this design.
    let ledger = read_ledger();
    let prior: Vec<&Value> = ledger.iter().filter(|r| r.get("design").and_then(|x| x.as_str()) == Some(design.as_str())).collect();
    let prev = prior.last().and_then(|r| r.get("digest")).and_then(|x| x.as_str()).unwrap_or("").to_string();

    let dg = digest(&answers, &prev);
    if prior.iter().any(|r| r.get("digest").and_then(|x| x.as_str()) == Some(dg.as_str())) {
        return json!({"ok": true, "duplicate": true, "digest": dg, "design": design,
                      "note": "byte-identical to the record already at this point in the chain — nothing appended. A re-anchor that changes nothing is a no-op, not a new revision"}).to_string();
    }

    let mut rec = json!({
        "v": 1, "ts": now_ms(), "design": design, "digest": dg, "prev": prev,
        "seq": prior.len(),
        "complete": unanswered.is_empty(),
        "unanswered": unanswered,
    });
    for (k, v) in &answers {
        rec[k] = json!(v);
    }
    if let Some(extra) = args.get("extra") {
        rec["extra"] = extra.clone();
    }

    let mut out = rec.clone();
    match append_ledger(&rec) {
        Ok(p) => out["ledger"] = json!(p),
        Err(e) => return json!({"ok": false, "error": format!("ledger write failed: {e}"), "digest": dg}).to_string(),
    }
    out["ok"] = json!(true);

    // The chain half — PROPOSED, never executed. A 32-byte digest fits inside
    // the 512-byte private memo a shielded send already carries, so the anchor
    // needs no consensus change; it needs an operator to authorize a payment.
    out["anchor_plan"] = json!({
        "executed": false,
        "why_not": "writing to SIGIL spends money and is outward-facing; this tool proposes and never sends",
        "how": "put the digest in the 512-byte private memo of a minimal shielded send to yourself — the memo field is live, so no consensus change is needed",
        "memo": format!("sigil-forge-anchor-v1:{dg}"),
        "tool": "flux_sigil_shielded_send",
        "verify_after": "re-read the note's memo and compare it to `digest` above; a receipt you have not read back is not a receipt"
    });
    out["note"] = if unanswered.is_empty() {
        json!("all ten questions answered — this record is self-sufficient provenance for one revision")
    } else {
        json!(format!("recorded with {} unanswered question(s): {}. A record with a hole is not 90% trustworthy, it is untrustworthy in a named place — fill them before treating this as verified", unanswered.len(), unanswered.join(", ")))
    };
    serde_json::to_string_pretty(&out).unwrap_or_else(|_| out.to_string())
}

// ───────────────────────────── 3. TRACE ─────────────────────────────

fn flux_sigil_forge_trace(args: &Value) -> String {
    let design = s(args, "design");
    let want_digest = s(args, "digest");
    let ledger = read_ledger();
    if ledger.is_empty() {
        return json!({"ok": true, "records": 0, "designs": [], "ledger": ledger_path(),
                      "note": "the ledger is empty — nothing has been anchored yet"}).to_string();
    }
    if design.is_empty() && want_digest.is_empty() {
        let mut names: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for r in &ledger {
            if let Some(d) = r.get("design").and_then(|x| x.as_str()) {
                *names.entry(d.to_string()).or_insert(0) += 1;
            }
        }
        return json!({"ok": true, "records": ledger.len(), "ledger": ledger_path(),
                      "designs": names.into_iter().map(|(d, n)| json!({"design": d, "revisions": n})).collect::<Vec<_>>(),
                      "hint": "pass design=<name> to walk one chain, or digest=<hex> for one record"}).to_string();
    }

    if !want_digest.is_empty() {
        return match ledger.iter().find(|r| r.get("digest").and_then(|x| x.as_str()) == Some(want_digest.as_str())) {
            Some(r) => {
                let mut o = r.clone();
                o["ok"] = json!(true);
                o["answers"] = json!(QUESTIONS.iter().map(|q| json!({"question": q, "answer": r.get(*q).and_then(|x| x.as_str()).unwrap_or("")})).collect::<Vec<_>>());
                serde_json::to_string_pretty(&o).unwrap_or_else(|_| o.to_string())
            }
            None => json!({"ok": false, "error": format!("no record with digest {want_digest}")}).to_string(),
        };
    }

    let chain: Vec<&Value> = ledger.iter().filter(|r| r.get("design").and_then(|x| x.as_str()) == Some(design.as_str())).collect();
    if chain.is_empty() {
        return json!({"ok": false, "error": format!("no records for design '{design}'")}).to_string();
    }
    // Verify the chain links AND re-derive every digest. A ledger nobody
    // re-hashes is a log file, not provenance.
    let mut broken: Vec<Value> = Vec::new();
    let mut expect_prev = String::new();
    for r in &chain {
        let prev = r.get("prev").and_then(|x| x.as_str()).unwrap_or("");
        if prev != expect_prev {
            broken.push(json!({"seq": r.get("seq"), "reason": "prev does not match the previous record's digest", "prev": prev, "expected": expect_prev}));
        }
        let answers: Vec<(String, String)> = QUESTIONS.iter().map(|q| (q.to_string(), r.get(*q).and_then(|x| x.as_str()).unwrap_or("").to_string())).collect();
        let recomputed = digest(&answers, prev);
        let stored = r.get("digest").and_then(|x| x.as_str()).unwrap_or("");
        if recomputed != stored {
            broken.push(json!({"seq": r.get("seq"), "reason": "digest does not match the record's own contents — the record was edited after it was written", "stored": stored, "recomputed": recomputed}));
        }
        expect_prev = stored.to_string();
    }
    let latest = chain.last().unwrap();
    json!({
        "ok": true, "design": design, "revisions": chain.len(),
        "chain_intact": broken.is_empty(),
        "broken_links": broken,
        "head_digest": latest.get("digest"),
        "head_complete": latest.get("complete"),
        "head_unanswered": latest.get("unanswered"),
        "answers": QUESTIONS.iter().map(|q| json!({"question": q, "answer": latest.get(*q).and_then(|x| x.as_str()).unwrap_or("")})).collect::<Vec<_>>(),
        "history": chain.iter().map(|r| json!({"seq": r.get("seq"), "ts": r.get("ts"), "revision": r.get("revision"), "digest": r.get("digest"), "complete": r.get("complete")})).collect::<Vec<_>>(),
        "note": "chain_intact means every digest was recomputed from the record's own ten answers and matched, and every prev pointed at its predecessor. It does NOT mean the answers are true — only that nobody edited them after the fact"
    }).to_string()
}

// ───────────────────────────── 4. REWARD ─────────────────────────────

fn flux_sigil_forge_reward(args: &Value) -> String {
    let revision = s(args, "revision");
    if revision.is_empty() {
        return json!({"ok": false, "error": "revision is required — a reward with no revision to point at is a gift, not compensation"}).to_string();
    }
    let credits_min = fnum(args, "credits_min", 20_000_000.0).max(0.0);
    let credits_max = fnum(args, "credits_max", 50_000_000.0).max(credits_min);
    let contributors = match args.get("contributors").and_then(|x| x.as_array()) {
        Some(a) if !a.is_empty() => a.clone(),
        _ => return json!({"ok": false, "error": "contributors is required: [{id, role, wallet?, share?}]"}).to_string(),
    };

    // `impact` in [0,1] scales the grant between the floor and the ceiling. It is
    // the caller's judgement, and it is labelled as such — nothing here measures
    // the worth of a commit.
    let impact = fnum(args, "impact", 0.5).clamp(0.0, 1.0);
    let pool = credits_min + (credits_max - credits_min) * impact;

    let raw: Vec<f64> = contributors.iter().map(|c| fnum(c, "share", 1.0).max(0.0)).collect();
    let sum: f64 = raw.iter().sum();
    let splits: Vec<Value> = contributors.iter().zip(raw.iter()).map(|(c, w)| {
        let frac = if sum > 0.0 { w / sum } else { 0.0 };
        json!({
            "id": s(c, "id"), "role": s(c, "role"), "wallet": s(c, "wallet"),
            "share_pct": frac * 100.0,
            "credits": pool * frac,
        })
    }).collect();

    let missing_wallets: Vec<String> = contributors.iter().filter(|c| s(c, "wallet").is_empty()).map(|c| s(c, "id")).collect();

    json!({
        "ok": true,
        "executed": false,
        "kind": "IOU FÆLLED — a commons grant recorded as an obligation, NOT a settled payment",
        "revision": revision,
        "impact": impact,
        "pool_credits": pool,
        "range": {"min": credits_min, "max": credits_max},
        "split": splits,
        "missing_wallets": missing_wallets,
        "to_settle": {
            "sigil": "flux_sigil_shielded_send (privacy-only chain: transparent sends are retired)",
            "quillon": "mcp__quillon-wallet__send_qug",
            "gate": "every one of these is a real transfer and needs the operator to say go, per the money-confirmation rule. This tool has moved nothing"
        },
        "caveats": [
            "FÆLLED credits are an internal unit of account, not dollars. Do not print a USD figure beside them as if the two were exchangeable — there is no market that makes them so",
            "`impact` is a human judgement supplied in the call. Nothing in this tool measures the worth of a revision, and a number that came from an argument must never be reported as a measurement",
            "a split with a missing wallet cannot be settled for that contributor — fill it before proposing settlement"
        ]
    }).to_string()
}

// ───────────────────────────── 5. YIELD ─────────────────────────────

fn flux_sigil_forge_yield(args: &Value) -> String {
    let window = fnum(args, "window_secs", 45.0).clamp(15.0, 300.0);
    let wallet = s(args, "wallet");

    let supply_of = |v: &Value| -> f64 {
        data(v.clone()).get("native_supply").and_then(|x| x.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or(f64::NAN)
    };

    let s0 = match get_json("/v1/supply") { Ok(v) => v, Err(e) => return json!({"ok": false, "error": e}).to_string() };
    let m0 = match get_json("/v1/mining/miners") { Ok(v) => v, Err(e) => return json!({"ok": false, "error": e}).to_string() };
    let h0 = data(m0.clone()).get("height").and_then(|x| x.as_f64()).unwrap_or(f64::NAN);
    std::thread::sleep(Duration::from_secs_f64(window));
    let s1 = match get_json("/v1/supply") { Ok(v) => v, Err(e) => return json!({"ok": false, "error": e}).to_string() };
    let m1 = match get_json("/v1/mining/miners") { Ok(v) => v, Err(e) => return json!({"ok": false, "error": e}).to_string() };
    let d1 = data(m1.clone());
    let h1 = d1.get("height").and_then(|x| x.as_f64()).unwrap_or(f64::NAN);

    let minted_glyphs = supply_of(&s1) - supply_of(&s0);
    let blocks = h1 - h0;
    let net_hps = d1.get("net_hps").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let live_miners = d1.get("live_miners").and_then(|x| x.as_f64()).unwrap_or(0.0);

    // Your share of the network, if you named a wallet.
    let my_hps = if wallet.is_empty() {
        d1.get("my_hps").and_then(|x| x.as_f64()).unwrap_or(0.0)
    } else {
        d1.get("miners").and_then(|x| x.as_array()).map(|a| a.iter()
            .filter(|m| m.get("wallet").and_then(|x| x.as_str()).map(|w| w.eq_ignore_ascii_case(&wallet)).unwrap_or(false))
            .map(|m| m.get("hps").and_then(|x| x.as_f64()).unwrap_or(0.0)).sum::<f64>()).unwrap_or(0.0)
    };
    let share = if net_hps > 0.0 { my_hps / net_hps } else { 0.0 };

    // 10 decimals on g2: 1 SIGIL = 1e10 glyphs.
    const GLYPHS_PER_SIGIL: f64 = 1e10;
    let sigil_per_hour_network = if window > 0.0 { minted_glyphs / GLYPHS_PER_SIGIL * 3600.0 / window } else { 0.0 };
    let sigil_per_hour_you = sigil_per_hour_network * share;

    // The price. Measured, and measured to be absent.
    let nation = get_json("/v1/nation/status").ok().map(data);
    let oracle_e8 = nation.as_ref().and_then(|n| n.get("oracle_price_usd_e8")).and_then(|x| x.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    let have_price = oracle_e8 > 0.0;
    let usd_per_hour = if have_price { Some(sigil_per_hour_you * oracle_e8 / 1e8) } else { None };

    // The pool. Also measured, also measured to be absent.
    let pools = get_json("/v1/pools").ok();
    let pool_count = pools.as_ref().and_then(|p| p.get("pools")).and_then(|x| x.as_array()).map(|a| a.len()).unwrap_or(0);

    let mut blockers: Vec<Value> = Vec::new();
    if !have_price {
        blockers.push(json!({"blocker": "no oracle price", "evidence": "/v1/nation/status -> oracle_price_usd_e8 = 0",
                             "consequence": "mining yield in USD is UNCOMPUTABLE, not merely unknown — every fiat figure would be invented",
                             "relieved_by": "an OraclePush from the master wallet"}));
    }
    if pool_count == 0 {
        blockers.push(json!({"blocker": "no DEX pool exists on sigil-g2", "evidence": "/v1/pools -> []",
                             "consequence": "the auto-LP button has nothing to deposit into; SIGIL/USDS must be CREATED before liquidity can be added",
                             "relieved_by": "seeding a SIGIL/USDS pool (LpDeposit against a new pool), which is a real capital commitment and an operator decision"}));
    }

    json!({
        "ok": true,
        "window_secs": window,
        "measured": {
            "blocks_in_window": blocks,
            "block_rate_per_s": if window > 0.0 { blocks / window } else { 0.0 },
            "minted_sigil_in_window": minted_glyphs / GLYPHS_PER_SIGIL,
            "net_hps": net_hps, "live_miners": live_miners,
            "your_hps": my_hps, "your_share_of_network": share
        },
        "yield": {
            "network_sigil_per_hour": sigil_per_hour_network,
            "your_sigil_per_hour": sigil_per_hour_you,
            "your_usd_per_hour": usd_per_hour,
            "basis": "minted supply DELTA over the window times your hash share. This is measured emission, not a reward-curve model — it needs no assumption about the halving schedule"
        },
        "auto_lp": {
            "executed": false,
            "possible": pool_count > 0,
            "pools_on_chain": pool_count,
            "proposal": if pool_count > 0 {
                json!({"action": "deposit a fraction of mined SIGIL into the SIGIL/USDS pool", "tool": "flux_sigil_wallet_pools then LpDeposit",
                       "gate": "a real capital move — operator confirms, this tool never sends"})
            } else {
                json!({"action": "none available", "why": "the pool does not exist yet"})
            }
        },
        "blockers": blockers,
        "headline": if blockers.is_empty() {
            format!("{sigil_per_hour_you:.4} SIGIL/hour at {:.1}% of network hash", share * 100.0)
        } else {
            format!("{sigil_per_hour_you:.4} SIGIL/hour at {:.1}% of network hash — but {} thing(s) block turning that into value: {}",
                    share * 100.0, blockers.len(),
                    blockers.iter().filter_map(|b| b.get("blocker").and_then(|x| x.as_str())).collect::<Vec<_>>().join("; "))
        },
        "caveats": [
            "a short window on a variable-rate chain is noisy: the block count in 45 s has a wide spread, so treat one reading as an estimate and take several before acting on it",
            "your_sigil_per_hour assumes reward accrues in proportion to hash share. Dev-fee carves (750 bps, of which 200 bps goes to the welfare treasury) come off the top, so this is an UPPER bound on what lands in your wallet"
        ]
    }).to_string()
}

// ───────────────────────────── registry ─────────────────────────────

pub fn register(registry: &mut ToolRegistry) {
    registry.register(ToolDef {
        name: "flux_sigil_forge_search",
        description: "SEARCH THE POSSIBILITY SPACE WITHOUT BUILDING ANYTHING. Give it a modular design space — axes (soc, cooling, power, enclosure, ...) each with options that declare capability factors, resource constraints, build days and cost — and it enumerates the Cartesian product, applies the Theta_phys feasibility gate (hard-constraint violations are Swampland and score exactly 0), and ranks every survivor by K_R (best single unit) AND Gamma_R (best REALIZATION, i.e. capability per unit build time). The most valuable line in the output is `binding_constraint_frequency`: a constraint that binds across most of the feasible space is worth more engineering than any choice inside the space. Capability terms multiply across axes; a term no option declares is Unavailable and is EXCLUDED from the geometric mean rather than guessed. Args: name, modules (required), peers, coordination, top.",
        input_schema: json!({"type":"object","properties":{
            "name":{"type":"string"},
            "modules":{"type":"array","items":{"type":"object","properties":{
                "axis":{"type":"string"},
                "options":{"type":"array","items":{"type":"object","properties":{
                    "label":{"type":"string"},
                    "capability":{"type":"object","properties":{"compute":{"type":"number"},"energy":{"type":"number"},"verifiable":{"type":"number"},"availability":{"type":"number"},"expandable":{"type":"number"}}},
                    "constraints":{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"},"required":{"type":"number"},"available":{"type":"number"},"hardness":{"type":"string","enum":["hard","soft"]},"provenance":{"type":"string"},"note":{"type":"string"}}}},
                    "days":{"type":"number"},"cost_usd":{"type":"number"}}}}}}},
            "peers":{"type":"array","items":{"type":"object"}},
            "coordination":{"type":"number"},
            "top":{"type":"integer"}},"required":["modules"]}),
    }, flux_sigil_forge_search);

    registry.register(ToolDef {
        name: "flux_sigil_forge_anchor",
        description: "KEEP THE RECEIPT. Records one revision of one physical design against the ten questions that have to be answerable about anything claiming to be verified: which design, which revision, who verified it, which supplier, which robot, which material, which test, which state root, which payment, and whether the thing actually exists. BLAKE3-digests the answers together with the previous record's digest, so revisions form an append-only chain per design that flux_sigil_forge_trace can re-derive and check. Unanswered questions are NAMED (`unanswered`, `complete:false`) rather than hidden — a record with a hole is not 90% trustworthy. Writing the digest to SIGIL is PROPOSED and never executed: the output carries a ready memo string for a shielded send (the 512-byte memo field is live, so no consensus change is needed), gated on operator confirmation. Args: design (required) plus any of the ten question fields, and `extra`.",
        input_schema: json!({"type":"object","properties":{
            "design":{"type":"string"},"revision":{"type":"string"},"verified_by":{"type":"string"},
            "supplier":{"type":"string"},"robot":{"type":"string"},"material":{"type":"string"},
            "test":{"type":"string"},"state_root":{"type":"string"},"payment":{"type":"string"},
            "exists":{"type":"string"},"extra":{"type":"object"}},"required":["design"]}),
    }, flux_sigil_forge_anchor);

    registry.register(ToolDef {
        name: "flux_sigil_forge_trace",
        description: "REBUILD FROM THE RECEIPT. Walks the anchor ledger. With no arguments it lists every design and how many revisions each has; with design=<name> it walks that chain, RE-DERIVES every digest from the record's own ten answers and checks every prev link, and reports `chain_intact` plus any record that was edited after it was written; with digest=<hex> it returns one record with its ten answers spelled out. `chain_intact` proves nobody edited the ledger — it does NOT prove the answers are true, and the output says so. Args: design, digest.",
        input_schema: json!({"type":"object","properties":{"design":{"type":"string"},"digest":{"type":"string"}}}),
    }, flux_sigil_forge_trace);

    registry.register(ToolDef {
        name: "flux_sigil_forge_reward",
        description: "PROPOSE (never send) an IOU FÆLLED commons grant for one revision, split across contributors. Scales a credit pool between `credits_min` and `credits_max` by a caller-supplied `impact` in [0,1], then divides it by each contributor's `share` weight. Returns `executed:false` always — settling it means a real transfer through flux_sigil_shielded_send (SIGIL is privacy-only; transparent sends are retired) or mcp__quillon-wallet__send_qug, and that needs the operator to say go. FÆLLED credits are an internal unit of account and are NOT dollars; the tool refuses to print an exchange rate because no market makes one. `impact` is a judgement passed in, not a measurement, and is labelled as such in the output. Args: revision (required), contributors (required, [{id, role, wallet, share}]), impact, credits_min, credits_max.",
        input_schema: json!({"type":"object","properties":{
            "revision":{"type":"string"},
            "contributors":{"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"role":{"type":"string"},"wallet":{"type":"string"},"share":{"type":"number"}}}},
            "impact":{"type":"number"},"credits_min":{"type":"number"},"credits_max":{"type":"number"}},"required":["revision","contributors"]}),
    }, flux_sigil_forge_reward);

    registry.register(ToolDef {
        name: "flux_sigil_forge_yield",
        description: "WHAT IS THIS HASH ACTUALLY WORTH? Samples sigil-g2 twice over a window and derives your mining yield from the MEASURED minted-supply delta times your share of network hash — no reward-curve or halving assumption anywhere. Then it tries to convert that to value and REPORTS THE BLOCKERS IT FINDS instead of inventing numbers: if /v1/nation/status shows oracle_price_usd_e8 = 0 there is no price and USD/hour is returned as null, not estimated; if /v1/pools is empty there is no SIGIL/USDS pool, so the automatic-liquidity action is reported as impossible with the reason. Any LP deposit is proposed with executed:false and left to the operator. Args: window_secs (15..300, default 45), wallet (defaults to this node's own hash).",
        input_schema: json!({"type":"object","properties":{"window_secs":{"type":"number"},"wallet":{"type":"string"}}}),
    }, flux_sigil_forge_yield);
}
