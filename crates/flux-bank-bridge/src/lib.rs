//! flux-bank-bridge — read Quillon bank metrics; pin endpoints.

use flux_bank_api::quillon;
use flux_bank_api::BankStatusResponse;
use flux_bank_core::BankStatus;

pub fn resolve_endpoint(alias: &str) -> String {
    match alias.trim().to_lowercase().as_str() {
        "epsilon" => "http://89.149.241.126:8080/api/v1".into(),
        "delta" => "http://5.79.79.158:8080/api/v1".into(),
        _ => "https://quillon.xyz/api/v1".into(),
    }
}

pub fn fetch_quillon_bank_metrics(endpoint: &str) -> Result<serde_json::Value, String> {
    let url = format!("{}{}", endpoint.trim_end_matches('/'), quillon::METRICS);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let res = client.get(&url).send().map_err(|e| format!("GET {url}: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("GET {url} -> {}", res.status()));
    }
    res.json().map_err(|e| e.to_string())
}

pub fn bank_status(endpoint_alias: &str) -> BankStatusResponse {
    let endpoint = resolve_endpoint(endpoint_alias);
    let mut status = BankStatus::default();
    status.endpoint = endpoint.clone();
    let quillon_raw = match fetch_quillon_bank_metrics(&endpoint) {
        Ok(v) => {
            status.quillon_metrics_ok = true;
            status.notes.push("quillon-bank/metrics OK".into());
            Some(v)
        }
        Err(e) => {
            status.notes.push(format!("quillon metrics: {e}"));
            None
        }
    };
    BankStatusResponse { status, quillon_raw }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LinkScore {
    pub peer: String,
    pub latency_ms: u32,
    pub throughput_mbps: f64,
    pub cost_score: f64,
}

pub fn pick_best_link(scores: &[LinkScore]) -> Option<usize> {
    scores
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            let sa = a.latency_ms as f64 - a.throughput_mbps + a.cost_score;
            let sb = b.latency_ms as f64 - b.throughput_mbps + b.cost_score;
            sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
}
