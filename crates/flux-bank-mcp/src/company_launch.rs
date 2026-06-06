//! Company launch combo — Wickes CMS site proposal + Sigil Bank ledger (proposal-first).

use flux_bank_core::{Ledger, TransferProposal, NATIVE};
use serde_json::{json, Value};

const SIGIL_API: &str = "https://sigilgraph.fluxapp.xyz/api/v1";

/// Propose a Wickes CMS site skeleton (content types + page blocks).
/// Mirrors flux-cck + flux-pagebuilder + wickes-cms patterns without cross-workspace deps.
pub fn flux_wickes_site_propose(company_name: &str, slug: &str, template: Option<&str>) -> String {
    let tpl = template.unwrap_or("saas-landing");
    let hero = format!("Welcome to {company_name}");
    let body = format!(
        "{company_name} runs on Wickes CMS + Sigil Bank. Slug: {slug}. API: {SIGIL_API}"
    );

    let content_types = json!([
        {
            "id": "company_profile",
            "name": "Company Profile",
            "fields": [
                {"name": "legal_name", "kind": "text", "required": true},
                {"name": "slug", "kind": "text", "required": true},
                {"name": "treasury_wallet", "kind": "text", "required": true},
                {"name": "tagline", "kind": "text", "required": false}
            ]
        },
        {
            "id": "page",
            "name": "Page",
            "fields": [
                {"name": "title", "kind": "text", "required": true},
                {"name": "body", "kind": "richtext", "required": true},
                {"name": "seo_slug", "kind": "text", "required": true}
            ]
        }
    ]);

    let pages = json!([
        {
            "type_id": "page",
            "id": "home",
            "state": "draft",
            "blocks": [
                {"kind": "hero", "content": hero},
                {"kind": "text", "content": body},
                {"kind": "cta", "content": "Launch treasury (proposal-first)"}
            ]
        },
        {
            "type_id": "company_profile",
            "id": slug,
            "state": "draft",
            "fields": {
                "legal_name": company_name,
                "slug": slug,
                "tagline": format!("{company_name} on Sigil Graph")
            }
        }
    ]);

    json!({
        "ok": true,
        "mode": "proposal_only",
        "module": "wickes-cms",
        "template": tpl,
        "company_name": company_name,
        "slug": slug,
        "api": SIGIL_API,
        "content_types": content_types,
        "pages": pages,
        "workflow": ["draft", "review", "published"],
        "mcp_tools": ["wickes_page_create", "wickes_page_publish", "wickes_dashboard"],
        "note": "CMS apply requires operator approval; no auto-publish."
    })
    .to_string()
}

/// Full company launch: CMS site proposal + treasury seed transfer (dry-run) + bank status.
pub fn flux_company_launch_propose(
    company_name: &str,
    slug: &str,
    founder_wallet: &str,
    treasury_wallet: &str,
    seed_capital_uqug: u128,
    bank_endpoint: Option<&str>,
) -> String {
    let endpoint = bank_endpoint.unwrap_or("quillon");
    let cms = flux_wickes_site_propose(company_name, slug, Some("company-launch"));

    let transfer = TransferProposal {
        from: founder_wallet.into(),
        to: treasury_wallet.into(),
        token: NATIVE.into(),
        amount_uqug: seed_capital_uqug,
        memo: Some(format!("seed treasury for {company_name} ({slug})")),
        dry_run: true,
    };

    let ledger = Ledger::new();
    let transfer_body: Value = match ledger.simulate_transfer(&transfer) {
        Ok(()) => json!({
            "ok": true,
            "mode": "dry_run",
            "from": transfer.from,
            "to": transfer.to,
            "token": transfer.token,
            "amount_uqug": transfer.amount_uqug,
            "memo": transfer.memo,
            "note": "proposal only — execute requires SignedIntent + 2-of-2"
        }),
        Err(e) => json!({
            "ok": false,
            "mode": "dry_run",
            "error": e.to_string()
        }),
    };

    let bank_status = flux_bank_bridge::bank_status(endpoint);
    let bank_json =
        serde_json::to_value(&bank_status).unwrap_or(json!({"error": "bank_status serialize failed"}));
    let cms_parsed: Value = serde_json::from_str(&cms).unwrap_or(json!({"raw": cms}));

    json!({
        "ok": true,
        "combo": "flux_company_launch_combo",
        "mode": "proposal_only",
        "company": {
            "name": company_name,
            "slug": slug,
            "domain": "sigilgraph.fluxapp.xyz",
            "api": SIGIL_API
        },
        "cms": cms_parsed,
        "bank": {
            "endpoint": endpoint,
            "status": bank_json,
            "treasury_wallet": treasury_wallet,
            "founder_wallet": founder_wallet,
            "seed_transfer": transfer_body,
            "spend_gate": "SignedIntent + 2-of-2 operator co-sign"
        },
        "steps": [
            {"step": 1, "tool": "flux_wickes_site_propose", "status": "proposed"},
            {"step": 2, "tool": "flux_bank_propose_transfer", "status": "dry_run"},
            {"step": 3, "tool": "flux_bank_status", "status": "read"},
            {"step": 4, "action": "operator_approve", "note": "SignedIntent to materialize CMS + credit treasury"}
        ],
        "next_tools": ["flux_bank_propose_transfer", "flux_bifrost_run"]
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_propose_has_blocks() {
        let out = flux_wickes_site_propose("Acme Corp", "acme", None);
        assert!(out.contains("hero"));
        assert!(out.contains("sigilgraph.fluxapp.xyz"));
    }

    #[test]
    fn company_launch_combo_proposal_only() {
        let out = flux_company_launch_propose(
            "Acme Corp",
            "acme",
            "founder-wallet",
            "treasury-wallet",
            1_000_000,
            Some("epsilon"),
        );
        assert!(out.contains("proposal_only"));
        assert!(out.contains("flux_company_launch_combo"));
        assert!(out.contains("dry_run"));
    }
}