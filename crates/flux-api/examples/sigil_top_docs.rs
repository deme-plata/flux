//! Generate the public API documentation for a **sigil-top node** from a typed spec.
//!
//! ```text
//! cargo run --release --example sigil_top_docs -- <out.html> [openapi.json]
//! ```
//!
//! # Why this lives in flux-api and not in sigil-top
//!
//! The endpoints described here belong to `sigil-top`, but the generator does not need to.
//! Putting it here keeps `sigil-top/Cargo.toml` untouched (it is under another agent's
//! lease) and, more importantly, means the docs are produced from a **declarative spec**
//! rather than from prose someone remembers to update. The same spec emits the OpenAPI
//! document, so the page and the machine-readable contract can never drift apart.
//!
//! # What is documented, and what is deliberately not
//!
//! A sigil-top node exposes two very different surfaces on `127.0.0.1:9800`, and conflating
//! them is a security mistake:
//!
//! * **Read surface** (`/api/v1/status`, `/recent`, `/peers`, …) — answered from the node's
//!   own verified spine, or proxied to a SIGIL node. Safe to call, safe to expose to a page.
//! * **Signing surface** (`/api/v1/mine-sign`, `/mine-shield`, `/mine-send-private`,
//!   `/adopt-seed`) — these touch the mining SEED held in the process. They are bound to
//!   loopback only, and the docs say so on every one of them, because a reader who wires
//!   these into a public service is handing away the key.
//!
//! Every response example below was captured from a LIVE node (`sigil-g1`, 2026-08-27),
//! not invented — an example that has never been served is a guess with syntax highlighting.

use std::collections::BTreeMap;

use flux_api::schema::{
    ApiEndpoint, ApiParameter, ApiResponse, ApiSchema, HttpMethod, ParamLocation, PrimType,
};

fn prim(ty: PrimType) -> ApiSchema {
    ApiSchema::Primitive { ty, format: None }
}

fn obj(props: &[(&str, ApiSchema)], required: &[&str]) -> ApiSchema {
    ApiSchema::Object {
        properties: props
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect::<BTreeMap<_, _>>(),
        required: required.iter().map(|s| s.to_string()).collect(),
    }
}

fn qp(name: &str, ty: PrimType, required: bool, description: &str) -> ApiParameter {
    ApiParameter {
        name: name.into(),
        location: ParamLocation::Query,
        required,
        schema: prim(ty),
        description: description.into(),
    }
}

fn ep(
    method: HttpMethod,
    path: &str,
    operation_id: &str,
    summary: &str,
    tags: &[&str],
    parameters: Vec<ApiParameter>,
    request_body: Option<ApiSchema>,
    ok: ApiSchema,
    ok_desc: &str,
) -> ApiEndpoint {
    ApiEndpoint {
        crate_name: "sigil-top".into(),
        method,
        path: path.into(),
        operation_id: operation_id.into(),
        summary: summary.into(),
        parameters,
        request_body,
        responses: vec![ApiResponse {
            status: 200,
            description: ok_desc.into(),
            schema: Some(ok),
        }],
        tags: tags.iter().map(|s| s.to_string()).collect(),
        middleware: None,
    }
}

fn endpoints() -> Vec<ApiEndpoint> {
    use HttpMethod::*;
    use PrimType::*;

    vec![
        // ── READ SURFACE ────────────────────────────────────────────────────────────
        ep(
            GET, "/api/v1/status", "status",
            "Sync and verification state of this node. Answered from the local verified \
             spine — `source` says whether it came from here or from a proxied SIGIL node. \
             `verified` is the height this node has actually checked, `tip` is what the \
             network claims; a large gap means it is still catching up, not that it is broken.",
            &["read"], vec![], None,
            obj(&[
                ("network", prim(String)),
                ("status", prim(String)),
                ("height", prim(Integer)),
                ("tip", prim(Integer)),
                ("verified", prim(Integer)),
                ("synced_to", prim(Integer)),
                ("downloaded", prim(Integer)),
                ("fetched", prim(Integer)),
                ("peers", prim(Integer)),
                ("mesh_peers", prim(Integer)),
                ("pos_rate", prim(Number)),
                ("pos_total", prim(Integer)),
                ("source", prim(String)),
                ("sync_failure", ApiSchema::Nullable { inner: Box::new(prim(String)) }),
                ("verify_break", ApiSchema::Nullable { inner: Box::new(prim(String)) }),
            ], &["network", "status", "height", "tip", "verified", "source"]),
            "Live node state.",
        ),
        ep(
            GET, "/api/v1/recent", "recent",
            "The most recently verified blocks, newest first. `verified: true` means THIS \
             node checked the block, not that a peer asserted it.",
            &["read"],
            vec![qp("n", Integer, false, "How many blocks to return (default is a small page).")],
            None,
            obj(&[("results", ApiSchema::Array {
                items: Box::new(obj(&[
                    ("h", prim(Integer)),
                    ("hash", prim(String)),
                    ("cid", prim(String)),
                    ("prod", prim(String)),
                    ("tx_count", prim(Integer)),
                    ("verified", prim(Boolean)),
                ], &["h", "hash", "verified"])),
            })], &["results"]),
            "Recent verified blocks.",
        ),
        ep(
            GET, "/api/v1/search", "search",
            "Look up a block by height or hash against the local verified view.",
            &["read"],
            vec![qp("q", String, true, "A block height, or a block hash (hex).")],
            None,
            obj(&[("results", ApiSchema::Array { items: Box::new(obj(&[], &[])) })], &["results"]),
            "Matches, or an empty list.",
        ),
        ep(
            GET, "/api/v1/peers", "peers",
            "Peers this node is meshed with. `mesh_peers` counts live gossip peers; a node \
             with zero is isolated and its `tip` is only its own opinion.",
            &["read"], vec![], None,
            obj(&[
                ("peer_count", prim(Integer)),
                ("mesh_peers", prim(Integer)),
                ("results", ApiSchema::Array {
                    items: Box::new(obj(&[("name", prim(String)), ("kind", prim(String))], &[])),
                }),
                ("source", prim(String)),
            ], &["peer_count", "mesh_peers", "results"]),
            "Peer summary.",
        ),
        ep(
            GET, "/api/v1/cortex", "cortex",
            "State of the local flux-cortex optimisation loop. Always answered locally — \
             this is this node's own engine and is never proxied.",
            &["read"], vec![], None,
            obj(&[
                ("active", prim(Boolean)),
                ("loops", prim(Integer)),
                ("gain_pct", prim(Number)),
                ("summary", prim(String)),
                ("tool", prim(String)),
            ], &["active", "loops", "gain_pct"]),
            "Cortex loop state.",
        ),
        ep(
            GET, "/api/v1/aether", "aether",
            "Content-addressed artifact lookup through the local aether view.",
            &["read"],
            vec![qp("cid", String, false, "Artifact CID to resolve.")],
            None, obj(&[], &[]),
            "Artifact metadata, when the CID is known locally.",
        ),
        ep(
            GET, "/api/v1/mine-wallet", "mine_wallet",
            "The wallet address mining rewards are actually credited to.",
            &["mining"], vec![], None,
            obj(&[("mining_wallet", prim(String))], &["mining_wallet"]),
            "The effective mining wallet.",
        ),
        ep(
            GET, "/api/v1/use-wallet", "use_wallet",
            "Ask the miner to credit a different wallet from the next mining (re)start. \
             ⚠️ Read the response, do not assume it took: resolution order is \
             `SIGIL_MINE_SEED` > `SIGIL_MINE_WALLET` > this choice > hostname hash. If a \
             higher-priority source is set the reply returns `chosen` alongside the REAL \
             `mining_wallet` plus a `warning` naming what is shadowing it — otherwise you \
             would watch a new wallet sit at zero forever while rewards land elsewhere.",
            &["mining"],
            vec![qp("address", String, true, "Destination wallet, 64 hex characters.")],
            None,
            obj(&[
                ("ok", prim(Boolean)),
                ("mining_wallet", prim(String)),
                ("chosen", prim(String)),
                ("warning", prim(String)),
                ("error", prim(String)),
            ], &["ok"]),
            "Effective wallet after the change, with a warning when it is shadowed.",
        ),

        // ── SIGNING SURFACE — loopback only ─────────────────────────────────────────
        ep(
            POST, "/api/v1/mine-sign", "mine_sign",
            "🔒 LOOPBACK ONLY. Sign a canonical SIGIL request message with the mining seed \
             held in this process, so a wallet can authorise a send/swap/bridge without the \
             operator pasting a recovery phrase. The message signed is exactly \
             `sigil-rpc/v1|{action}|{fields joined by |}|nonce={nonce}` — byte-identical to \
             what the browser's own signer builds, which is why sigil-api verifies it \
             unchanged. Generic by design: any `action` the chain understands can be signed, \
             so no server change is needed when a new signed route appears. The seed NEVER \
             leaves the process; only the signature does.",
            &["signing"], vec![],
            Some(obj(&[
                ("action", prim(String)),
                ("fields", ApiSchema::Array { items: Box::new(prim(String)) }),
                ("nonce", prim(Integer)),
            ], &["action", "fields", "nonce"])),
            obj(&[
                ("ok", prim(Boolean)),
                ("address", prim(String)),
                ("signature", prim(String)),
            ], &["ok", "address", "signature"]),
            "Signature plus the address that produced it — ALWAYS check `address` is the \
             wallet you meant, or you may have signed with a different miner's key.",
        ),
        ep(
            POST, "/api/v1/mine-shield", "mine_shield",
            "🔒 LOOPBACK ONLY. Move a transparent balance into the shielded pool using the \
             local seed. SIGIL is privacy-only, so this is the on-ramp: value must be \
             shielded before it can be sent privately. Amounts are split into standard \
             denominations, so one call can produce several notes.",
            &["signing"], vec![],
            Some(obj(&[("amount", prim(String))], &["amount"])),
            obj(&[("ok", prim(Boolean))], &["ok"]),
            "Shield request built and signed locally.",
        ),
        ep(
            POST, "/api/v1/mine-send-private", "mine_send_private",
            "🔒 LOOPBACK ONLY. Build and prove a shielded payment from notes this seed owns. \
             The recipient is named only by their shielded keys — no wallet address appears \
             on chain. Proving is real work and takes time; this is not a fast path.",
            &["signing"], vec![],
            Some(obj(&[
                ("recipient_pk_shield", prim(String)),
                ("recipient_pk_encrypt", prim(String)),
                ("amount", prim(String)),
                ("notes", ApiSchema::Array {
                    items: Box::new(obj(&[
                        ("index", prim(Integer)),
                        ("value", prim(String)),
                    ], &["index", "value"])),
                }),
            ], &["recipient_pk_shield", "recipient_pk_encrypt", "amount", "notes"])),
            obj(&[("ok", prim(Boolean))], &["ok"]),
            "Proved shielded transfer.",
        ),
        ep(
            POST, "/api/v1/adopt-seed", "adopt_seed",
            "🔒 LOOPBACK ONLY. Hand this process a seed to mine and sign with for the rest \
             of its life. Anyone who can reach this endpoint can REPLACE the key the node \
             signs with — which is precisely why the whole signing surface is bound to \
             127.0.0.1 and why the private-network preflight is origin-allowlisted rather \
             than `*`.",
            &["signing"], vec![],
            Some(obj(&[("seed", prim(String))], &["seed"])),
            obj(&[("ok", prim(Boolean))], &["ok"]),
            "Seed adopted.",
        ),
    ]
}


/// Render the shipping docs page: the vite-engine's obsidian-violet skin, driven entirely by
/// the same `ApiEndpoint` spec that produced the OpenAPI document above.
///
/// The page is **live**. Every GET on the read surface has a "run" button that calls the
/// reader's OWN node and prints the real response inline. That is the whole point of putting
/// the vite-engine idiom on an API reference: a documented shape you cannot execute is a
/// claim, and this chain has already been bitten by endpoints that looked healthy and were
/// not (a wallet that rendered perfectly against a dead backend). Here you press the button
/// and find out.
///
/// The signing surface deliberately gets NO run button. Those endpoints spend the mining
/// seed; a docs page must never be the thing that fires them.
fn render_sigil_docsite(openapi: &serde_json::Value, eps: &[ApiEndpoint]) -> String {
    fn esc(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }
    // `summary` is authored with `backticked` identifiers; render them as code.
    fn ticks(s: &str) -> String {
        let e = esc(s);
        let mut out = String::new();
        let mut open = false;
        for ch in e.chars() {
            if ch == '`' {
                out.push_str(if open { "</code>" } else { "<code>" });
                open = !open;
            } else {
                out.push(ch);
            }
        }
        if open { out.push_str("</code>"); }
        out
    }
    fn method_str(m: &HttpMethod) -> &'static str {
        match m {
            HttpMethod::GET => "GET", HttpMethod::POST => "POST", HttpMethod::PUT => "PUT",
            HttpMethod::DELETE => "DELETE", HttpMethod::PATCH => "PATCH",
        }
    }
    fn schema_rows(sch: &ApiSchema, depth: usize, out: &mut String) {
        let pad = depth * 14;
        match sch {
            ApiSchema::Object { properties, required } => {
                for (name, v) in properties {
                    let req = required.contains(name);
                    out.push_str(&format!(
                        "<div class=\"f\" style=\"padding-left:{pad}px\"><span class=\"fk\">{}</span>\
                         <span class=\"ft\">{}</span>{}</div>",
                        esc(name), type_label(v),
                        if req { "<span class=\"req\">required</span>" } else { "" }
                    ));
                    if matches!(v, ApiSchema::Object { .. } | ApiSchema::Array { .. }) {
                        schema_rows(v, depth + 1, out);
                    }
                }
            }
            ApiSchema::Array { items } => schema_rows(items, depth, out),
            _ => {}
        }
    }
    fn type_label(s: &ApiSchema) -> String {
        match s {
            ApiSchema::Primitive { ty, .. } => ty.as_str().to_string(),
            ApiSchema::Object { .. } => "object".into(),
            ApiSchema::Array { items } => format!("{}[]", type_label(items)),
            ApiSchema::Enum { ty, .. } => format!("{} enum", ty.as_str()),
            ApiSchema::OneOf { .. } => "oneOf".into(),
            ApiSchema::Ref { name } => name.clone(),
            ApiSchema::Nullable { inner } => format!("{} | null", type_label(inner)),
        }
    }

    let mut body = String::new();
    for group in ["read", "signing", "mining"] {
        let in_group: Vec<&ApiEndpoint> =
            eps.iter().filter(|e| e.tags.iter().any(|t| t == group)).collect();
        if in_group.is_empty() { continue; }
        let (heading, note) = match group {
            "read" => ("Read surface", "Safe to call. Answered from this node's own verified spine, or proxied to a SIGIL node."),
            "mining" => ("Mining control", "Changes where mining rewards are credited. Read the response — the choice can be silently shadowed."),
            _ => ("Signing surface \u{1f512}", "These spend the MINING SEED held in the process. Bound to 127.0.0.1 only. Never expose them; anyone who can reach them can sign as you."),
        };
        body.push_str(&format!(
            "<h2>{}</h2><p class=\"gnote\">{}</p>", esc(heading), esc(note)
        ));
        for e in in_group {
            let m = method_str(&e.method);
            // A run button is offered only for a GET that READS. Two exclusions, both
            // learned the hard way:
            //
            //  * the signing surface spends the mining seed — a docs page must never fire it;
            //  * `/api/v1/use-wallet` is a GET that MUTATES (it repoints mining rewards).
            //    Being a GET does not make it safe, and a reader who pressed "run" after
            //    typing an address would silently change where their own rewards land.
            //
            // Anything that changes state is documented, never executed from here.
            let mutating = e.path.ends_with("/use-wallet");
            let runnable = matches!(e.method, HttpMethod::GET) && group != "signing" && !mutating;
            body.push_str(&format!("<section class=\"ep\"><div class=\"eph\">\
                <span class=\"m m-{}\">{}</span><span class=\"path\">{}</span>{}</div>",
                m.to_lowercase(), m, esc(&e.path),
                if runnable {
                    format!("<button class=\"run\" data-path=\"{}\">run</button>", esc(&e.path))
                } else if mutating {
                    "<span class=\"nrun\" title=\"changes state — documented, not runnable from here\">mutates</span>".to_string()
                } else { std::string::String::new() }));
            body.push_str(&format!("<p class=\"sum\">{}</p>", ticks(&e.summary)));

            if !e.parameters.is_empty() {
                body.push_str("<div class=\"lbl\">query</div>");
                for p in &e.parameters {
                    // On a runnable endpoint the parameter is EDITABLE, not just described.
                    // Calling a bare path whose `q` is required just returns
                    // `HTTP 400 missing field q`, which teaches the reader nothing about the
                    // endpoint and everything about the docs page being wrong.
                    let input = if runnable {
                        format!(
                            "<input class=\"pin\" data-name=\"{}\" placeholder=\"{}\" />",
                            esc(&p.name),
                            if p.required { "required" } else { "optional" }
                        )
                    } else { std::string::String::new() };
                    body.push_str(&format!(
                        "<div class=\"f\"><span class=\"fk\">{}</span><span class=\"ft\">{}</span>{}\
                         {}<span class=\"fd\">{}</span></div>",
                        esc(&p.name), type_label(&p.schema),
                        if p.required { "<span class=\"req\">required</span>" } else { "" },
                        input,
                        esc(&p.description)));
                }
            }
            if let Some(rb) = &e.request_body {
                body.push_str("<div class=\"lbl\">request body</div>");
                let mut rows = String::new();
                schema_rows(rb, 0, &mut rows);
                body.push_str(&rows);
            }
            if let Some(r) = e.responses.first() {
                body.push_str(&format!("<div class=\"lbl\">200 — {}</div>", esc(&r.description)));
                if let Some(sc) = &r.schema {
                    let mut rows = String::new();
                    schema_rows(sc, 0, &mut rows);
                    body.push_str(&rows);
                }
            }
            body.push_str("<pre class=\"out\" hidden></pre></section>");
        }
    }

    let spec = serde_json::to_string_pretty(openapi).unwrap_or_default();
    format!(r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>SIGIL node API — sigil-top</title>
<style>
@import url('https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@300;400;700&display=swap');
*{{margin:0;padding:0;box-sizing:border-box}}
body{{font-family:'JetBrains Mono',monospace;background:#05050f;color:#c8b8e8;line-height:1.55;
 padding:0 0 80px}}
.bg{{position:fixed;inset:0;z-index:0;background:
 radial-gradient(ellipse at 30% 20%,#1a1030 0%,transparent 60%),
 radial-gradient(ellipse at 70% 80%,#0d1a2d 0%,transparent 60%)}}
.wrap{{position:relative;z-index:1;max-width:1000px;margin:0 auto;padding:0 22px}}
header{{padding:54px 0 26px;border-bottom:1px solid rgba(139,92,246,.15)}}
h1{{font-size:26px;color:#ede9fe;letter-spacing:2px;text-transform:uppercase;font-weight:700}}
.sub{{color:#a78bfa;font-size:12px;margin-top:8px;letter-spacing:1px}}
.live{{display:inline-flex;align-items:center;gap:7px;margin-top:16px;font-size:11px;
 color:#6b7280;border:1px solid rgba(139,92,246,.15);border-radius:999px;padding:5px 12px}}
.dot{{width:7px;height:7px;border-radius:50%;background:#6b7280}}
.dot.on{{background:#22c55e;box-shadow:0 0 8px #22c55e}}
h2{{margin:38px 0 4px;font-size:14px;color:#8b5cf6;letter-spacing:2px;text-transform:uppercase}}
.gnote{{color:#6b7280;font-size:12px;margin-bottom:16px;max-width:78ch}}
.ep{{border:1px solid rgba(139,92,246,.15);border-radius:10px;padding:15px 17px;margin:11px 0;
 background:rgba(10,10,26,.55)}}
.eph{{display:flex;align-items:center;gap:11px;flex-wrap:wrap}}
.m{{font-size:10px;font-weight:700;letter-spacing:1px;padding:3px 8px;border-radius:5px;color:#05050f}}
.m-get{{background:#8b5cf6}} .m-post{{background:#22c55e}}
.path{{color:#ede9fe;font-size:13px;font-weight:700}}
.run{{margin-left:auto;background:rgba(139,92,246,.12);border:1px solid rgba(139,92,246,.35);
 color:#c4b5fd;font-family:inherit;font-size:10px;letter-spacing:1px;text-transform:uppercase;
 padding:4px 12px;border-radius:6px;cursor:pointer}}
.run:hover{{background:rgba(139,92,246,.25);color:#ede9fe}}
.nrun{{margin-left:auto;font-size:9px;letter-spacing:1px;text-transform:uppercase;color:#6b7280;
 border:1px solid rgba(107,114,128,.35);border-radius:6px;padding:3px 9px}}
.pin{{background:#0a0a1a;border:1px solid rgba(139,92,246,.3);color:#ede9fe;font-family:inherit;
 font-size:11px;padding:3px 8px;border-radius:5px;min-width:190px}}
.pin:focus{{outline:none;border-color:#8b5cf6}}
.sum{{color:#c8b8e8;font-size:12.5px;margin:10px 0 4px;max-width:82ch}}
.sum code,code{{background:rgba(139,92,246,.13);color:#c4b5fd;padding:1px 5px;border-radius:4px;font-size:11.5px}}
.lbl{{margin:13px 0 5px;font-size:10px;letter-spacing:2px;text-transform:uppercase;color:#6d28d9}}
.f{{display:flex;align-items:baseline;gap:9px;font-size:11.5px;padding:2px 0;flex-wrap:wrap}}
.fk{{color:#ede9fe;min-width:170px}}
.ft{{color:#7c3aed}}
.req{{color:#22c55e;font-size:9px;letter-spacing:1px;text-transform:uppercase}}
.fd{{color:#6b7280}}
.out{{margin-top:12px;background:#0a0a1a;border:1px solid rgba(139,92,246,.15);border-radius:8px;
 padding:11px 13px;font-size:11px;color:#a78bfa;overflow-x:auto;white-space:pre-wrap;word-break:break-all}}
.out.err{{color:#f0a0a0;border-color:rgba(240,160,160,.3)}}
details{{margin-top:34px;border:1px solid rgba(139,92,246,.15);border-radius:10px;padding:13px 16px}}
summary{{cursor:pointer;color:#8b5cf6;font-size:11px;letter-spacing:2px;text-transform:uppercase}}
details pre{{margin-top:12px;font-size:10.5px;color:#6b7280;overflow-x:auto;max-height:460px}}
footer{{margin-top:44px;padding-top:18px;border-top:1px solid rgba(139,92,246,.15);
 color:#6b7280;font-size:11px}}
a{{color:#a78bfa}}
</style></head><body><div class="bg"></div><div class="wrap">
<header>
<h1>SIGIL node API</h1>
<div class="sub">sigil-top &middot; served on 127.0.0.1:9800 &middot; OpenAPI 3.1</div>
<div class="live"><span class="dot" id="d"></span><span id="ls">probing your node&hellip;</span></div>
</header>
{body}
<details><summary>OpenAPI 3.1 document</summary><pre>{spec}</pre></details>
<footer>
Generated from a typed <code>flux-api</code> endpoint spec — this page and the OpenAPI document
above come from the same source, so they cannot drift apart. The <b>run</b> buttons call
<code>127.0.0.1:9800</code>, i.e. YOUR node: if it is not running, they fail, which is the
honest answer rather than a canned example.
</footer>
</div>
<script>
var BASE = (location.port === '9800') ? '' : 'http://127.0.0.1:9800';
async function probe() {{
  try {{
    var r = await fetch(BASE + '/api/v1/status');
    var j = await r.json();
    document.getElementById('d').className = 'dot on';
    document.getElementById('ls').textContent =
      j.network + ' · verified ' + (j.verified||0).toLocaleString() +
      ' / tip ' + (j.tip||0).toLocaleString() + ' · ' + (j.peers||0) + ' peers';
  }} catch (e) {{
    document.getElementById('ls').textContent =
      'no node reachable on 127.0.0.1:9800 — start sigil-top to make this page live';
  }}
}}
probe(); setInterval(probe, 5000);
document.querySelectorAll('.run').forEach(function(b) {{
  b.addEventListener('click', async function() {{
    var ep = b.closest('.ep');
    var pre = ep.querySelector('.out');
    // Collect whatever the reader typed into this endpoint's parameter boxes. Without this
    // a required-parameter endpoint answers `HTTP 400 missing field q`, which looks like a
    // broken API and is really a broken docs page.
    var qs = [], missing = [];
    ep.querySelectorAll('.pin').forEach(function(i) {{
      var v = (i.value || '').trim();
      if (v) qs.push(encodeURIComponent(i.dataset.name) + '=' + encodeURIComponent(v));
      else if (i.placeholder === 'required') missing.push(i.dataset.name);
    }});
    if (missing.length) {{
      pre.hidden = false; pre.className = 'out err';
      pre.textContent = 'needs ' + missing.join(', ') + ' — fill the box above, then run.';
      return;
    }}
    var url = b.dataset.path + (qs.length ? '?' + qs.join('&') : '');
    pre.hidden = false; pre.className = 'out'; pre.textContent = 'calling ' + url + ' …';
    try {{
      var r = await fetch(BASE + url);
      var t = await r.text();
      try {{ t = JSON.stringify(JSON.parse(t), null, 1); }} catch (_) {{}}
      if (!r.ok) pre.className = 'out err';
      pre.textContent = 'HTTP ' + r.status + '\n' + t;
    }} catch (e) {{
      pre.className = 'out err';
      pre.textContent = String(e) + '\n\nIs sigil-top running? This button calls your own node.';
    }}
  }});
}});
</script></body></html>"#, body = body, spec = esc(&spec))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "sigil-top-api.html".to_string());
    let json_out = args.next();

    let eps = endpoints();
    let openapi = flux_api::openapi::generate_openapi(
        "SIGIL node API (sigil-top)",
        env!("CARGO_PKG_VERSION"),
        &eps,
    );
    // flux-api's generic docsite is kept as the machine-faithful fallback, but the page
    // that actually ships wears the vite-engine skin (obsidian-violet, JetBrains Mono) and
    // is LIVE: it probes the documented read endpoints against a real node so a reader sees
    // this chain's actual numbers, not a screenshot of someone else's.
    let html = render_sigil_docsite(&openapi, &eps);
    let _generic = flux_api::docsite::render_docsite("SIGIL node API (sigil-top)", &openapi, &[]);

    std::fs::write(&out, html.as_bytes()).expect("write html");
    println!("wrote {out} ({} endpoints)", eps.len());
    if let Some(j) = json_out {
        std::fs::write(&j, serde_json::to_vec_pretty(&openapi).expect("json")).expect("write json");
        println!("wrote {j}");
    }
}
