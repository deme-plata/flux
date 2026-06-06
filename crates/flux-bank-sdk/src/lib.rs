//! flux-bank-sdk — typed client (Stainless-aligned surface).

use flux_bank_api::flux;
use flux_bank_bridge::bank_status;

pub struct FluxBankClient {
    pub base: String,
}

impl FluxBankClient {
    pub fn quillon() -> Self {
        Self { base: "https://quillon.xyz/api/v1".into() }
    }

    pub fn with_base(base: impl Into<String>) -> Self {
        Self { base: base.into() }
    }

    /// GET /flux-bank/status (local bridge until HTTP route ships)
    pub fn status(&self, endpoint_alias: &str) -> flux_bank_api::BankStatusResponse {
        let _ = &self.base;
        bank_status(endpoint_alias)
    }

    pub fn propose_transfer_dry(
        &self,
        from: &str,
        to: &str,
        amount_uqug: u128,
    ) -> serde_json::Value {
        let _ = &self.base;
        let raw = flux_bank_mcp::flux_bank_propose_transfer(from, to, amount_uqug, None, None);
        serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({"raw": raw}))
    }

    pub fn status_path() -> &'static str {
        flux::STATUS
    }

    pub fn propose_path() -> &'static str {
        flux::PROPOSE_TRANSFER
    }
}
