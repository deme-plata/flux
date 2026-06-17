//! Thin blocking HTTP layer (ureq) connecting the standalone miner to a live
//! Quillon node. The PROTOCOL (parse challenge / build submission) lives in the
//! pure, unit-tested lib; this file is only the network I/O around it, kept
//! small and deliberately NOT unit-tested (it talks to a live endpoint —
//! validated with a smoke run instead, so the test suite never flakes).

use quillon_gpu_miner::{parse_challenge_response, ParsedChallenge};

/// GET `/api/v1/mining/challenge?wallet=...` and parse it into [`ParsedChallenge`].
/// `server` is a base URL like `https://quillon.xyz` (the node's own
/// server_notice asks miners to use HTTPS, not direct IP:8080).
pub fn fetch_challenge(server: &str, wallet: &str) -> Result<ParsedChallenge, String> {
    let url = format!("{}/api/v1/mining/challenge?wallet={}", server.trim_end_matches('/'), wallet);
    let body = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|e| format!("challenge request failed: {e}"))?
        .into_string()
        .map_err(|e| format!("challenge body read failed: {e}"))?;
    parse_challenge_response(&body)
        .ok_or_else(|| format!("could not parse challenge response: {}", &body[..body.len().min(200)]))
}

/// POST a `MiningSubmission` JSON body to `/api/v1/mining/submit`. Returns the
/// node's raw response string (accept/reject + reason). Build `payload_json`
/// with `quillon_gpu_miner::submit_payload_json`.
pub fn submit_solution(server: &str, payload_json: &str) -> Result<String, String> {
    let url = format!("{}/api/v1/mining/submit", server.trim_end_matches('/'));
    ureq::post(&url)
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .send_string(payload_json)
        .map_err(|e| format!("submit request failed: {e}"))?
        .into_string()
        .map_err(|e| format!("submit body read failed: {e}"))
}
