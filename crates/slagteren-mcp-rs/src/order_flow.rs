//! Validated order state machine.
//!
//! Unpriced products become signed quote requests. Dry-runs become signed,
//! non-authorizing drafts. Only fixed-price input plus a persistent pinned key
//! can produce the authorization used for a live pending WooCommerce draft.

use serde_json::{json, Value};

use super::{
    agent, b64, list_products, ore_to_dkk, prov, resolve_all, scrub, total_note,
};

pub(super) fn create_order(items: &[String], note: &str) -> Value {
    let catalog = match list_products() {
        Ok(c) => c,
        Err(e) => return json!({"mode": "ERROR", "error": e}),
    };
    let (lines, unmatched, fixed_ore, variable, lead_notes) = resolve_all(items, &catalog);
    let line_items = lines
        .iter()
        .map(|line| {
            json!({
                "product_id": line.get("id"),
                "quantity": line.get("quantity")
            })
        })
        .collect::<Vec<_>>();
    let note = if note.is_empty() {
        "Oprettet som uforpligtende udkast via AI-assistent (MCP). Afventer bekræftelse."
    } else {
        note
    };
    let base_payload = json!({
        "status": "pending",
        "set_paid": false,
        "line_items": line_items,
        "customer_note": note
    });
    let lead_time_warning = if lead_notes.is_empty() {
        Value::Null
    } else {
        json!(format!(
            "Bemærk leveringstid — ikke i aften: {}",
            lead_notes.join("; ")
        ))
    };
    let budgets = budget_constraints(items);

    // Commercial validation deliberately happens before credential handling.
    // The previous flow signed an unknown-price weight item as a pending order
    // whenever the server was in dry-run mode.
    if line_items.is_empty() || !unmatched.is_empty() || !variable.is_empty() {
        let intent_body = json!({
            "shop": "Slagteren på Suensonsvej",
            "requested_items": items,
            "requested_budgets": budgets,
            "resolved": lines,
            "unmatched": unmatched,
            "variable_items": variable,
            "fixed_total_ore": fixed_ore,
            "note": note
        });
        let provenance = sign_or_error("quote_request", false, intent_body);
        return json!({
            "mode": if !variable.is_empty() { "QUOTE_REQUIRED" } else { "REFUSED" },
            "authorizes_order": false,
            "error": "Ordren er ikke prisfast og entydig nok til at blive oprettet.",
            "requested_budgets": budgets,
            "unmatched": unmatched,
            "variable_items": variable,
            "resolved": lines,
            "fixed_total_dkk": ore_to_dkk(fixed_ore),
            "total_note": total_note(fixed_ore, &variable),
            "lead_time_warning": lead_time_warning,
            "provenance": provenance,
            "note": "Dette er en signeret forespørgsel om pris/mængde — ikke en ordre. Butikken skal afklare varen først."
        });
    }

    let url = std::env::var("WOO_URL")
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/')
        .to_string();
    let key = std::env::var("WOO_KEY").unwrap_or_default().trim().to_string();
    let secret = std::env::var("WOO_SECRET").unwrap_or_default().trim().to_string();
    let allow = std::env::var("WOO_ALLOW_WRITE").unwrap_or_default().trim() == "1";
    let order_body = json!({
        "shop": "Slagteren på Suensonsvej",
        "payload": base_payload,
        "resolved": lines,
        "fixed_total_ore": fixed_ore,
        "requested_budgets": budgets
    });

    if url.is_empty() || key.is_empty() || secret.is_empty() || !allow {
        let provenance = sign_or_error("order_draft", false, order_body);
        return json!({
            "mode": "DRY-RUN",
            "authorizes_order": false,
            "reason": "Butikkens skriveadgang er ikke aktiveret.",
            "note": "Intet er sendt til butikken. Provenance attesterer kun dette uforpligtende udkast.",
            "preview": {
                "method": "POST",
                "payload": base_payload,
                "resolved": lines,
                "fixed_total_dkk": ore_to_dkk(fixed_ore),
                "total_note": total_note(fixed_ore, &variable),
                "lead_time_warning": lead_time_warning
            },
            "provenance": provenance
        });
    }
    if !url.starts_with("https://") {
        return json!({
            "mode": "REFUSED",
            "authorizes_order": false,
            "error": "Butikkens ordre-endpoint skal bruge HTTPS."
        });
    }
    if !prov::can_authorize_orders() {
        let provenance = sign_or_error("order_draft", false, order_body);
        return json!({
            "mode": "REFUSED",
            "authorizes_order": false,
            "error": "Live-ordre kræver en vedvarende nøgle, der matcher butikkens server-side nøglepin.",
            "note": "Sæt FLUX_MCP_TRUSTED_KEY_ID til nøgle-id'et fra provenance_pubkey efter en separat butikskontrol. Intet er sendt.",
            "provenance": provenance
        });
    }

    let provenance = match prov::sign_record("order_authorization", true, order_body) {
        Ok(value) => value,
        Err(e) => {
            return json!({
                "mode": "REFUSED",
                "authorizes_order": false,
                "error": format!("Kunne ikke oprette ordreautorisation: {e}")
            })
        }
    };
    let mut payload = base_payload;
    payload["meta_data"] = json!([
        {"key": "_flux_provenance_profile", "value": provenance["profile"]},
        {"key": "_flux_provenance_key_id", "value": provenance["key_id"]},
        {"key": "_flux_provenance_bundle_hex", "value": provenance["bundle_hex"]},
        {"key": "_flux_provenance_signed_payload", "value": provenance["signed_payload"]}
    ]);

    let endpoint = format!("{url}/wp-json/wc/v3/orders");
    let cred = b64(format!("{key}:{secret}").as_bytes());
    let auth = format!("Basic {cred}");
    let body = payload.to_string();
    let response = agent()
        .post(&endpoint)
        .set("Authorization", &auth)
        .set("Content-Type", "application/json")
        .send_string(&body);
    let secrets = [cred.as_str(), key.as_str(), secret.as_str()];
    match response {
        Ok(r) => match r.into_json::<Value>() {
            Ok(created) => json!({
                "mode": "LIVE (draft order created)",
                "authorizes_order": true,
                "order_id": created.get("id"),
                "status": created.get("status"),
                "total": created.get("total"),
                "note": "Udkast oprettet i WooCommerce. Butik og kunde bekræfter; intet er betalt.",
                "resolved": lines,
                "provenance": provenance
            }),
            Err(e) => json!({
                "mode": "ERROR",
                "authorizes_order": false,
                "error": scrub(&e.to_string(), &secrets)
            }),
        },
        Err(e) => json!({
            "mode": "ERROR",
            "authorizes_order": false,
            "error": scrub(&e.to_string(), &secrets)
        }),
    }
}

fn sign_or_error(intent: &str, authorizes_order: bool, body: Value) -> Value {
    prov::sign_record(intent, authorizes_order, body)
        .unwrap_or_else(|e| json!({"error": e, "authorizes_order": false}))
}

fn budget_constraints(items: &[String]) -> Vec<Value> {
    items
        .iter()
        .filter_map(|requested| {
            budget_ore(requested).map(|ore| {
                json!({
                    "requested": requested,
                    "budget_ore": ore,
                    "budget_dkk": ore_to_dkk(ore)
                })
            })
        })
        .collect()
}

fn budget_ore(text: &str) -> Option<i64> {
    let words = text.split_whitespace().collect::<Vec<_>>();
    for pair in words.windows(2) {
        let currency = pair[1]
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_ascii_lowercase();
        if currency != "kr" {
            continue;
        }
        let raw = pair[0].trim_matches(|c: char| !c.is_ascii_digit() && c != ',' && c != '.');
        if raw.is_empty() {
            continue;
        }
        let (whole, cents) = if let Some((whole, decimals)) = raw.rsplit_once(',') {
            let cents = match decimals.len() {
                0 => 0,
                1 => decimals.parse::<i64>().ok()? * 10,
                2 => decimals.parse::<i64>().ok()?,
                _ => return None,
            };
            (whole.replace('.', "").parse::<i64>().ok()?, cents)
        } else {
            (raw.replace('.', "").parse::<i64>().ok()?, 0)
        };
        return whole.checked_mul(100)?.checked_add(cents);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_danish_budget_constraints() {
        assert_eq!(budget_ore("leverpostej for cirka 100 kr."), Some(10_000));
        assert_eq!(budget_ore("maks 99,95 kr"), Some(9_995));
        assert_eq!(budget_ore("budget 1.250 kr"), Some(125_000));
        assert_eq!(budget_ore("3 leverpostejer"), None);
    }

    #[test]
    fn unknown_weight_price_is_not_rendered_as_zero() {
        let note = total_note(0, &["Leverpostej".to_string()]);
        assert_eq!(note, "pris og mængde skal bekræftes for Leverpostej");
        assert!(!note.contains("0 kr"));
    }
}
