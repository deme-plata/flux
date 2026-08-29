//! The building's MCP/HTTP surface — declared the flux way.
//!
//! Every handler below is registered at COMPILE TIME via `flux-api`'s
//! `#[api]` macro into the inventory registry, which is what
//! `flux_api::discover_endpoints_static()` / OpenAPI generation / the SDK
//! generators read. So the humans' interface to the tower ("the greatest work
//! culture" starts with never having to ask where anything is) comes for free
//! from the same spec the machines use — one truth, many renderings.
//!
//! Handlers are pure: state in, JSON-serializable answer out. Whatever hosts
//! them (fluxc serve route, an MCP tool, a kiosk in the lobby) does transport;
//! this module does meaning.

use crate::bank::QuillonBank;
use crate::cortex::BuildingCortex;
use crate::culture::OperatingReport;
use crate::elevator::{CarKind, ElevatorBank, HallCall, TransportMetrics};
use crate::tower::TowerSpec;
use crate::vault::Vault;
use flux_api::api;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResponse {
    pub tower: String,
    pub floors: usize,
    pub building_health: f64,
    pub vault_bars: usize,
    pub treasury_uqug: u128,
}

#[api(GET, "/v1/skyskraber/status", summary = "One-glance building status: health, vault, treasury")]
pub fn get_status(
    spec: &TowerSpec,
    cortex: &BuildingCortex,
    vault: &Vault,
    bank: &QuillonBank,
) -> StatusResponse {
    StatusResponse {
        tower: spec.name.clone(),
        floors: spec.floors.len(),
        building_health: cortex.building_health(),
        vault_bars: vault.bar_count(),
        treasury_uqug: bank.balance(crate::bank::TREASURY),
    }
}

#[api(GET, "/v1/skyskraber/blueprint", summary = "The full validated tower blueprint (floors + zones)")]
pub fn get_blueprint(spec: &TowerSpec) -> TowerSpec {
    spec.clone()
}

#[derive(Debug, Clone, Deserialize)]
pub struct ElevatorRequest {
    pub from: i32,
    pub to: i32,
    pub robot: bool,
    pub tick: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ElevatorResponse {
    pub assigned_car: Option<u32>,
}

#[api(POST, "/v1/skyskraber/elevator/request", summary = "Hall call: request robot freight or human cab transport")]
pub fn post_elevator_request(bank: &mut ElevatorBank, req: ElevatorRequest) -> ElevatorResponse {
    let kind = if req.robot { CarKind::RobotFreight } else { CarKind::HumanCab };
    let assigned_car =
        bank.request(HallCall { from: req.from, to: req.to, kind, requested_tick: req.tick });
    ElevatorResponse { assigned_car }
}

#[api(GET, "/v1/skyskraber/elevator/metrics", summary = "Transport metrics: served, waits, robot share")]
pub fn get_elevator_metrics(bank: &ElevatorBank) -> TransportMetrics {
    bank.metrics()
}

#[derive(Debug, Clone, Serialize)]
pub struct VaultAuditResponse {
    pub bars: usize,
    pub holdings_kg: f64,
    pub chain_head: String,
    pub chain_intact: bool,
    pub audit_entries: usize,
}

#[api(GET, "/v1/skyskraber/vault/audit", summary = "Vault custody proof: holdings + BLAKE3 chain verification")]
pub fn get_vault_audit(vault: &Vault) -> VaultAuditResponse {
    let verify = vault.verify_chain();
    VaultAuditResponse {
        bars: vault.bar_count(),
        holdings_kg: vault.holdings_kg(),
        chain_head: vault.head_hex(),
        chain_intact: verify.is_ok(),
        audit_entries: vault.audit_log().len(),
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PayRequest {
    pub from: String,
    pub to: String,
    pub amount_uqug: u128,
    pub memo: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PayResponse {
    pub ok: bool,
    pub error: Option<String>,
}

#[api(POST, "/v1/skyskraber/bank/pay", summary = "Quillon Bank transfer (propose→simulate→execute)")]
pub fn post_bank_pay(bank: &mut QuillonBank, req: PayRequest) -> PayResponse {
    match bank.pay(&req.from, &req.to, req.amount_uqug, &req.memo) {
        Ok(()) => PayResponse { ok: true, error: None },
        Err(e) => PayResponse { ok: false, error: Some(e.to_string()) },
    }
}

#[api(GET, "/v1/skyskraber/operating-index", summary = "The Building Operating Index: five measured components + verdict")]
pub fn get_operating_index(report: &OperatingReport) -> OperatingReport {
    report.clone()
}

#[derive(Debug, Clone, Serialize)]
pub struct StateRootResponse {
    pub root: String,
    pub commitment: String,
    pub twin_version: String,
}

#[api(GET, "/v1/skyskraber/state-root", summary = "Merkle-rooted tower state + the commitment the full node publishes")]
pub fn get_state_root(b: &crate::Building) -> StateRootResponse {
    let c = crate::state_root::tower_state(b);
    StateRootResponse {
        root: hex::encode(c.root),
        commitment: hex::encode(c.commitment),
        twin_version: c.twin_version.to_string(),
    }
}

/// Every endpoint this crate registered, straight from the inventory registry.
pub fn registered_endpoints() -> Vec<&'static flux_api::ApiEndpointDescriptor> {
    inventory::iter::<flux_api::ApiEndpointDescriptor>()
        .into_iter()
        .filter(|d| d.crate_name == "flux-skyskraber")
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_eight_endpoints_are_registered_at_compile_time() {
        let eps = registered_endpoints();
        assert_eq!(eps.len(), 8, "expected 8 registered endpoints, got {}", eps.len());
        let paths: Vec<&str> = eps.iter().map(|e| e.path).collect();
        assert!(paths.contains(&"/v1/skyskraber/status"));
        assert!(paths.contains(&"/v1/skyskraber/vault/audit"));
        assert!(paths.contains(&"/v1/skyskraber/bank/pay"));
        assert!(paths.contains(&"/v1/skyskraber/operating-index"));
        assert!(paths.contains(&"/v1/skyskraber/state-root"));
        let posts = eps.iter().filter(|e| e.method == "POST").count();
        assert_eq!(posts, 2);
    }

    #[test]
    fn state_root_endpoint_folds_the_building() {
        let b = crate::Building::quillon_default().unwrap();
        let r = get_state_root(&b);
        assert_eq!(r.root.len(), 64);
        assert_eq!(r.commitment.len(), 64);
    }

    #[test]
    fn status_handler_reports_the_building() {
        let b = crate::Building::quillon_default().unwrap();
        let s = get_status(&b.spec, &b.cortex, &b.vault, &b.bank);
        assert_eq!(s.tower, "Quillon Graph Skyskraber");
        // 4 basements + lobby + 45 shared plates + 43 spire-A + 33 spire-B + 2 bridge decks
        assert_eq!(s.floors, 128);
        assert!(s.treasury_uqug > 0);
    }

    #[test]
    fn elevator_endpoint_assigns_a_car() {
        let mut b = crate::Building::quillon_default().unwrap();
        let resp = post_elevator_request(
            &mut b.transport,
            ElevatorRequest { from: -1, to: 30, robot: true, tick: 0 },
        );
        assert!(resp.assigned_car.is_some());
    }
}
