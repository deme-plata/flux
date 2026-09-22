//! slagteren-mcp — secure MCP server for Slagteren på Suensonsvej (Rust).
//!
//! Reads the shop's LIVE catalog via the public WooCommerce Store API and
//! (with a shop-issued key) creates DRAFT orders via the authenticated REST API.
//! Transport: newline-delimited JSON-RPC 2.0 over stdio (MCP stdio transport).
//!
//! SECURITY (utmost-secure posture):
//!   * The WooCommerce key lives SERVER-SIDE only (env vars). It is never logged,
//!     never placed in a tool result, never shown to the assistant or customer.
//!   * DRY-RUN by default. Unpriced/ambiguous items become non-authorizing quote
//!     requests. create_order only contacts the shop with explicit WooCommerce
//!     write access and a persistent provenance key matching the shop's key pin.
//!   * WOO_URL for writes MUST be https:// — plaintext is refused.
//!   * Orders are created status="pending", set_paid=false — a draft the shop
//!     and customer confirm. Nothing is auto-paid or auto-fulfilled.
//!   * Minimal dependency surface: rustls TLS (no OpenSSL), no async runtime.

use std::io::{self, BufRead, Write};
use std::time::Duration;
use serde_json::{json, Value};
mod order_flow;
mod prov;


const STORE_BASE: &str = "https://slagterensuensonsvej.dk/wp-json/wc/store/v1";
const PROTOCOL_VERSION: &str = "2024-11-05";
const HTTP_TIMEOUT: Duration = Duration::from_secs(45);

// ---------- tiny base64 (avoids a dependency) ----------
fn b64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

fn hex_encode(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b { s.push_str(&format!("{:02x}", x)); }
    s
}
fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    // Operate on raw bytes so a multibyte UTF-8 char can never make us slice at a
    // non-char-boundary (which would panic; with panic=abort that kills the server).
    let b = s.as_bytes();
    if b.len() % 2 != 0 { return Err("odd-length hex".into()); }
    let digit = |c: u8| (c as char).to_digit(16).ok_or_else(|| "invalid hex digit".to_string());
    (0..b.len()).step_by(2)
        .map(|i| Ok(((digit(b[i])? << 4) | digit(b[i + 1])?) as u8))
        .collect()
}

/// HTTP client with a small, rustls-only dependency surface.
/// Provenance is handled separately by the crypto-agile prov module.
fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(HTTP_TIMEOUT)
        .user_agent("slagteren-mcp/0.2 (+quillon.xyz)")
        .build()
}

// ---------- WooCommerce Store API (public, read-only) ----------
fn store_get(path: &str) -> Result<Value, String> {
    let url = format!("{STORE_BASE}/{path}");
    agent().get(&url).call().map_err(|e| e.to_string())?
        .into_json::<Value>().map_err(|e| e.to_string())
}

/// Exact price in minor units (øre). None if the product is variable/weight-priced
/// (WooCommerce reports type="variable" with price "0" — that is UNKNOWN, not free).
fn price_ore(p: &Value) -> Option<i64> {
    if p.get("type").and_then(|t| t.as_str()) == Some("variable") { return None; }
    let pr = p.get("prices")?;
    let raw = pr.get("price")?.as_str()?;
    if raw.is_empty() { return None; }
    let ore: i64 = raw.parse().ok()?;
    // A fixed product priced 0 is implausible for a butcher; treat 0 as unknown.
    if ore == 0 { return None; }
    Some(ore)
}

fn ore_to_dkk(ore: i64) -> f64 { (ore as f64) / 100.0 }

/// Exact Danish money string from øre — never rounds øre away (123.45 → "123,45 kr").
fn kr(ore: i64) -> String {
    if ore % 100 == 0 { format!("{} kr", ore / 100) }
    else { format!("{},{:02} kr", ore / 100, (ore % 100).abs()) }
}

/// Heuristic lead time: catering/menu items state "N dages varsel" in the text.
/// Returns Some(days) if found. in_stock=true does NOT mean available today.
fn lead_time_days(text: &str) -> Option<i64> {
    let t = text.to_lowercase();
    let words: Vec<&str> = t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    for (i, w) in words.iter().enumerate() {
        if let Ok(n) = w.parse::<i64>() {
            if (1..=30).contains(&n) {
                for nxt in words.iter().skip(i + 1).take(2) {
                    if nxt.starts_with("dag") { return Some(n); }
                }
            }
        }
    }
    None
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">")
     .replace("&quot;", "\"").replace("&#8217;", "\u{2019}").replace("&#039;", "'")
     .replace("&#8211;", "\u{2013}").replace("&nbsp;", " ")
}

fn strip_html(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c { '<' => in_tag = true, '>' => in_tag = false, _ if !in_tag => out.push(c), _ => {} }
    }
    html_unescape(out.trim())
}

fn normalise(p: &Value) -> Value {
    let cats: Vec<String> = p.get("categories").and_then(|c| c.as_array())
        .map(|a| a.iter().filter_map(|c| c.get("name").and_then(|n| n.as_str()).map(String::from)).collect())
        .unwrap_or_default();
    let full_desc = p.get("description").and_then(|s| s.as_str()).map(strip_html).unwrap_or_default();
    let summary = p.get("short_description").and_then(|s| s.as_str()).map(strip_html)
        .filter(|s| !s.is_empty()).unwrap_or_else(|| full_desc.clone());
    let ore = price_ore(p);
    let variable = p.get("type").and_then(|t| t.as_str()) == Some("variable") || ore.is_none();
    let lead = lead_time_days(&format!("{summary} {full_desc}"));
    json!({
        "id": p.get("id").and_then(|i| i.as_u64()),
        "name": html_unescape(p.get("name").and_then(|n| n.as_str()).unwrap_or("")),
        "price_dkk": ore.map(ore_to_dkk),      // display; null when variable/unknown
        "price_ore": ore,                       // exact money; null when variable/unknown
        "variable_price": variable,             // true = efter vægt / variabel — NOT free
        "lead_time_days": lead,                 // Some(n) => catering, ikke i aften
        "in_stock": p.get("is_in_stock").and_then(|b| b.as_bool()).unwrap_or(true),
        "categories": cats,
        "url": p.get("permalink").and_then(|u| u.as_str()),
        "summary": summary.chars().take(200).collect::<String>(),
    })
}

fn list_products() -> Result<Vec<Value>, String> {
    // Paginate so a catalog >100 is never silently truncated (bug: per_page=100 cap).
    let mut all = Vec::new();
    for page in 1..=20 {
        let v = store_get(&format!("products?per_page=100&page={page}"))?;
        let arr = v.as_array().cloned().unwrap_or_default();
        let n = arr.len();
        all.extend(arr.iter().map(normalise));
        if n < 100 { break; }
    }
    Ok(all)
}

fn search_products(query: &str) -> Result<Vec<Value>, String> {
    let q = urlencode(query);
    let v = store_get(&format!("products?search={q}&per_page=20"))?;
    Ok(v.as_array().map(|a| a.iter().take(8).map(normalise).collect()).unwrap_or_default())
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b { b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
                  _ => out.push_str(&format!("%{:02X}", b)) }
    }
    out
}

fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(String::from).collect()
}

/// Character count (NOT byte length) — Danish æ/ø/å are 2 bytes each, so byte
/// length would let short words clear the "substantial word" thresholds.
fn clen(s: &str) -> usize { s.chars().count() }

/// Two tokens match if equal, or one is a prefix of the other and ≥5 chars
/// (so "oksesteg" matches "oksestegs" — Danish inflection/compounding).
fn tok_match(a: &str, b: &str) -> bool {
    a == b || (clen(a) >= 5 && b.starts_with(a)) || (clen(b) >= 5 && a.starts_with(b))
}

/// Local best match (fallback when the Store API search returns nothing).
fn best_match<'a>(want: &str, catalog: &'a [Value]) -> Option<&'a Value> {
    let w: Vec<String> = tokens(want);
    let want_l = want.to_lowercase();
    let mut best: Option<&Value> = None;
    let mut best_score = 0i32;
    for p in catalog {
        let name = p.get("name").and_then(|n| n.as_str()).unwrap_or("").to_lowercase();
        let cats = p.get("categories").and_then(|c| c.as_array())
            .map(|a| a.iter().filter_map(|c| c.as_str()).collect::<Vec<_>>().join(" ")).unwrap_or_default().to_lowercase();
        let hay: Vec<String> = tokens(&format!("{name} {cats}"));
        // prefix-aware token overlap
        let mut score = w.iter().filter(|t| hay.iter().any(|h| tok_match(t, h))).count() as i32;
        // strong bonus: a content word (≥4 chars) appearing as a substring of the
        // product NAME — "flæskesteg" ⊂ "flæskestegs menu" but not "…kalv og flæsk".
        score += 2 * w.iter().filter(|t| clen(t) >= 4 && name.contains(t.as_str())).count() as i32;
        if name.contains(&want_l) || want_l.contains(&name) { score += 3; }
        if score > best_score { best = Some(p); best_score = score; }
    }
    if best_score > 0 { best } else { None }
}

/// Strip a leading quantity: "3 frikadeller" / "2 stk grillmenu" → (3, "frikadeller").
fn parse_qty(want: &str) -> (i64, String) {
    let t = want.trim();
    let mut it = t.split_whitespace();
    if let Some(first) = it.next() {
        if let Ok(n) = first.parse::<i64>() {
            if (1..=99).contains(&n) {
                let rest: Vec<&str> = it.filter(|w| *w != "stk" && *w != "stk." && *w != "styk").collect();
                if !rest.is_empty() { return (n, rest.join(" ")); }
            }
        }
    }
    (1, t.to_string())
}

/// Resolve a natural-language request to a real product + quantity.
/// Store-API `?search=` FIRST (handles Danish inflection), local match as fallback.
fn resolve_item(want: &str, catalog: &[Value]) -> Option<(Value, i64)> {
    let (qty, term) = parse_qty(want);
    // Candidate pool = Store-API search hits (fuzzy/inflection) UNION full catalog,
    // ranked by our prefix-aware matcher so the true best wins regardless of source.
    // (e.g. "flæskesteg" → "Flæskestegs menu" even when the API ranks another
    // "…flæsk…" product first or omits the menu from its search results.)
    let mut pool: Vec<Value> = search_products(&term).unwrap_or_default();
    pool.extend(catalog.iter().cloned());
    best_match(&term, &pool).cloned().map(|p| (p, qty))
}

/// Shared resolution: returns (lines, unmatched, fixed_total_ore, variable_names, lead_notes).
fn resolve_all(items: &[String], catalog: &[Value])
    -> (Vec<Value>, Vec<String>, i64, Vec<String>, Vec<String>) {
    let mut lines = Vec::new();
    let mut unmatched = Vec::new();
    let mut fixed_total_ore: i64 = 0;
    let mut variable = Vec::new();
    let mut lead_notes = Vec::new();
    for want in items {
        match resolve_item(want, catalog) {
            Some((p, qty)) => {
                let name = p.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                let ore = p.get("price_ore").and_then(|v| v.as_i64());
                let is_var = p.get("variable_price").and_then(|b| b.as_bool()).unwrap_or(false);
                let lead = p.get("lead_time_days").and_then(|v| v.as_i64());
                let line_ore = ore.map(|o| o * qty);
                if let Some(o) = line_ore { fixed_total_ore += o; }
                if is_var || ore.is_none() { variable.push(name.clone()); }
                if let Some(d) = lead { lead_notes.push(format!("{name}: {d} dages varsel (catering)")); }
                lines.push(json!({
                    "requested": want, "product": name, "id": p.get("id"), "quantity": qty,
                    "price_dkk": ore.map(ore_to_dkk),
                    "line_total_dkk": line_ore.map(ore_to_dkk),
                    "variable_price": is_var || ore.is_none(),
                    "lead_time_days": lead,
                    "in_stock": p.get("in_stock"), "url": p.get("url"),
                }));
            }
            None => unmatched.push(want.clone()),
        }
    }
    (lines, unmatched, fixed_total_ore, variable, lead_notes)
}

fn total_note(fixed_ore: i64, variable: &[String]) -> String {
    if variable.is_empty() {
        kr(fixed_ore)
    } else if fixed_ore == 0 {
        format!("pris og mængde skal bekræftes for {}", variable.join(", "))
    } else {
        format!("fra {} + {} vare(r) efter vægt/variabel pris ({})",
                kr(fixed_ore), variable.len(), variable.join(", "))
    }
}

fn draft_order(items: &[String]) -> Value {
    let catalog = match list_products() { Ok(c) => c, Err(e) => return json!({"error": e}) };
    let (lines, unmatched, fixed_ore, variable, lead_notes) = resolve_all(items, &catalog);
    json!({
        "status": "DRAFT — kunden bekræfter selv den endelige bestilling",
        "lines": lines, "unmatched": unmatched,
        "fixed_total_dkk": ore_to_dkk(fixed_ore),
        "has_variable_items": !variable.is_empty(),
        "total_note": total_note(fixed_ore, &variable),
        "lead_time_warning": if lead_notes.is_empty() { Value::Null }
            else { json!(format!("Bemærk leveringstid — kan ikke leveres i aften: {}", lead_notes.join("; "))) },
        "shop": "Slagteren på Suensonsvej, Suensonsvej 71C, 9900 Frederikshavn"
    })
}

/// The one writing tool. Server-side key; dry-run unless explicitly enabled.
/// Redact known secret substrings (and their base64) from any string that might be
/// surfaced to the assistant/customer. Replaces the actual credential, so it cannot
/// leak even if an underlying error echoes the Authorization header value.
fn scrub(s: &str, secrets: &[&str]) -> String {
    let mut out = s.to_string();
    for sec in secrets {
        if sec.len() >= 4 { out = out.replace(sec, "***"); }
    }
    out
}

// ---------- MCP tool registry ----------
fn tools() -> Value {
    json!([
        {"name":"list_products","description":"Hent hele Slagteren på Suensonsvejs LIVE varekatalog (navn, pris i DKK, lagerstatus, kategori, link).","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
        {"name":"search_products","description":"Søg i det live varekatalog på fritekst (fx 'kødpakke','grillmenu','frikadeller').","inputSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}},
        {"name":"get_product","description":"Hent én vare ud fra dens id.","inputSchema":{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"],"additionalProperties":false}},
        {"name":"draft_order","description":"Saml et UDKAST til en bestilling ud fra ønsker i naturligt sprog. Intet indsendes — kunden bekræfter selv.","inputSchema":{"type":"object","properties":{"items":{"type":"array","items":{"type":"string"}}},"required":["items"],"additionalProperties":false}},
        {"name":"create_order","description":"Lav et sikkert ordreflow. Uprisede vægtvarer bliver en signeret prisforespørgsel; faste varer bliver et uforpligtende udkast. Kun eksplicit skriveadgang plus en vedvarende butikspinnet nøgle kan oprette et pending WooCommerce-udkast. Intet betales automatisk.","inputSchema":{"type":"object","properties":{"items":{"type":"array","items":{"type":"string"}},"customer_note":{"type":"string"}},"required":["items"],"additionalProperties":false}},
        {"name":"provenance_pubkey","description":"Hent crypto-agile provenanceprofil, nøgle-id og offentlige nøgleben. Butikken fastlåser nøgle-id'et før en signatur kan autorisere et ordreudkast.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
        {"name":"verify_order","description":"Verificér crypto-agile provenance og skeln mellem integritet og ordreautorisation. Trust kommer kun fra serverens FLUX_MCP_TRUSTED_KEY_ID; expected_key_id er kun en sammenligning. Nye envelopes bruger signed_payload, bundle_hex og profile; legacy SQIsign bruger signature_hex og pubkey_hex.","inputSchema":{"type":"object","properties":{"signed_payload":{"type":"string"},"bundle_hex":{"type":"string"},"profile":{"type":"string","enum":["sqisign-l5-v1","hybrid-sqisign-ed25519-v1"]},"expected_key_id":{"type":"string"},"signature_hex":{"type":"string"},"pubkey_hex":{"type":"string"}},"required":["signed_payload"],"additionalProperties":false}}
    ])
}

fn text_result(v: &Value) -> Value {
    let s = if v.is_string() { v.as_str().unwrap().to_string() } else { serde_json::to_string_pretty(v).unwrap_or_default() };
    json!({"content":[{"type":"text","text": s}]})
}

fn call_tool(name: &str, args: &Value) -> Value {
    match name {
        "list_products" => match list_products() { Ok(c) => text_result(&json!(c)), Err(e) => text_result(&json!(format!("Fejl: {e}"))) },
        "search_products" => {
            let q = args.get("query").and_then(|q| q.as_str()).unwrap_or("");
            match search_products(q) { Ok(c) => text_result(&json!(c)), Err(e) => text_result(&json!(format!("Fejl: {e}"))) }
        }
        "get_product" => {
            let id = args.get("id").and_then(|i| i.as_u64()).unwrap_or(0);
            match store_get(&format!("products/{id}")) {
                Ok(p) if p.get("id").is_some() => text_result(&normalise(&p)),
                Ok(_) => text_result(&json!(format!("Vare med id {id} findes ikke."))),
                // A network/HTTP failure is NOT the same as "not found" — say so, so the
                // assistant doesn't tell the customer the butcher doesn't sell the item.
                Err(e) => text_result(&json!(format!(
                    "Kunne ikke hente vare {id}: {e}. Butikkens API svarede ikke korrekt — det betyder IKKE at varen ikke findes; prøv igen."))),
            }
        }
        "draft_order" => text_result(&draft_order(&str_array(args.get("items")))),
        "create_order" => {
            let note = args.get("customer_note").and_then(|n| n.as_str()).unwrap_or("");
            text_result(&order_flow::create_order(&str_array(args.get("items")), note))
        }
        "provenance_pubkey" => text_result(&prov::public_metadata()),
        "verify_order" => {
            let payload = args.get("signed_payload").and_then(Value::as_str).unwrap_or("");
            let result = if let Some(bundle) = args.get("bundle_hex").and_then(Value::as_str) {
                let profile = args
                    .get("profile")
                    .and_then(Value::as_str)
                    .unwrap_or("hybrid-sqisign-ed25519-v1");
                let expected = args.get("expected_key_id").and_then(Value::as_str);
                prov::verify_envelope(payload, bundle, profile, expected)
            } else if let Some(signature) =
                args.get("signature_hex").and_then(Value::as_str)
            {
                let public_key = args
                    .get("pubkey_hex")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(prov::sqisign_pubkey_hex)
                    .unwrap_or_default();
                prov::verify_legacy(payload, signature, &public_key)
            } else {
                json!({
                    "valid": false,
                    "authorizes_order": false,
                    "error": "bundle_hex (new envelope) or signature_hex (legacy) is required"
                })
            };
            text_result(&result)
        }
        _ => json!({"content":[{"type":"text","text":format!("Ukendt værktøj: {name}")}],"isError":true}),
    }
}

fn str_array(v: Option<&Value>) -> Vec<String> {
    v.and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

// ---------- JSON-RPC dispatch ----------
fn handle(msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    match method {
        "initialize" => Some(json!({"jsonrpc":"2.0","id":id,"result":{
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "serverInfo": {"name":"slagteren-mcp","version":"0.2.0"}}})),
        "notifications/initialized" => None,
        "tools/list" => Some(json!({"jsonrpc":"2.0","id":id,"result":{"tools": tools()}})),
        "tools/call" => {
            let params = msg.get("params").cloned().unwrap_or(json!({}));
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            Some(json!({"jsonrpc":"2.0","id":id,"result": call_tool(name, &args)}))
        }
        _ => id.map(|i| json!({"jsonrpc":"2.0","id":i,"error":{"code":-32601,"message":format!("Method not found: {method}")}})),
    }
}

fn main() {
    let stdin = io::stdin();
    let mut out = io::stdout();
    for line in stdin.lock().lines() {
        let line = match line { Ok(l) => l, Err(_) => break };
        let line = line.trim();
        if line.is_empty() { continue; }
        let msg: Value = match serde_json::from_str(line) { Ok(v) => v, Err(_) => continue };
        if let Some(reply) = handle(&msg) {
            let _ = writeln!(out, "{}", reply);
            let _ = out.flush();
        }
    }
}
