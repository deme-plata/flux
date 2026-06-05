//! flux-advisor — webhook server: POST comms/trade events → propose-only advice.
//!
//!   flux-advisor [port]        # default 8799
//!   POST /comms  {"message":"..."}              → {"flags":[...]}
//!   POST /trade  {"price":..,"trend":..,"fng":..,"arb_pct":..} → {"advice":"BuyDip"}
//! Never sends or spends — it only advises.

use flux_advisor::{comms_advice, trade_advice, CommsFlag};
use tiny_http::{Server, Response, Method};

fn main() {
    let port: u16 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(8799);
    let server = Server::http(("0.0.0.0", port)).expect("bind");
    eprintln!("flux-advisor (propose-only) on :{port} — POST /comms or /trade");
    for mut req in server.incoming_requests() {
        let mut body = String::new();
        use std::io::Read;
        let _ = req.as_reader().read_to_string(&mut body);
        let url = req.url().to_string();
        let out = if req.method() == &Method::Post && url.starts_with("/comms") {
            let msg = serde_json::from_str::<serde_json::Value>(&body).ok()
                .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(String::from))
                .unwrap_or_default();
            let flags: Vec<&str> = comms_advice(&msg).iter().map(|f| match f {
                CommsFlag::FabricationRisk => "FabricationRisk: a metric without read-from-file evidence",
                CommsFlag::IncludeOthers => "IncludeOthers: reference the swarm / @an agent",
                CommsFlag::NotActionable => "NotActionable: add a next step or ask",
            }).collect();
            serde_json::json!({"flags": flags}).to_string()
        } else if req.method() == &Method::Post && url.starts_with("/trade") {
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let g = |k| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
            let a = trade_advice(g("price"), g("trend"), g("fng") as u8, g("arb_pct"));
            serde_json::json!({"advice": format!("{a:?}"), "propose_only": true}).to_string()
        } else {
            "{\"flux-advisor\":\"POST /comms or /trade\"}".to_string()
        };
        let _ = req.respond(Response::from_string(out).with_header(
            "Content-Type: application/json".parse::<tiny_http::Header>().unwrap()));
    }
}
