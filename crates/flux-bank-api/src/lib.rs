//! flux-bank-api — route table + DTOs for Stainless/OpenAPI codegen.

pub use flux_bank_core::{BankStatus, SignedIntent, TransferProposal};

pub const API_PREFIX: &str = "/api/v1";

pub mod quillon {
    pub const METRICS: &str = "/quillon-bank/metrics";
    pub const STABLECOIN: &str = "/stablecoin/status";
    pub const TREASURY: &str = "/treasury/reserves";
}

pub mod flux {
    pub const STATUS: &str = "/flux-bank/status";
    pub const PROPOSE_TRANSFER: &str = "/flux-bank/transfer/propose";
    pub const SETTLE: &str = "/flux-bank/settle";
    pub const LINK_BENCH: &str = "/flux-bank/links/bench";
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BankStatusResponse {
    pub status: BankStatus,
    pub quillon_raw: Option<serde_json::Value>,
}
