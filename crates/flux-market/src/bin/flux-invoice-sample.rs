use flux_market::invoice::{Invoice, Party, LineItem};
fn main() {
    let inv = Invoice {
        number: "SIGIL-2026-001".into(), date: "2026-06-01".into(), due_date: "2026-06-15".into(),
        seller: Party { name: "Flux / SIGIL (rocky AI)".into(), cvr: Some("DK-CVR-pending".into()), address: "Quillon Graph · København, DK".into() },
        buyer: Party { name: "Acme ApS".into(), cvr: Some("87654321".into()), address: "Aarhus, DK".into() },
        items: vec![
            LineItem { description: "flux-moe agentic LLM integration (Qwen3.6, 90% tool-call)".into(), qty: 10.0, unit_price_dkk: 1200.0 },
            LineItem { description: "GPU CUDA miner — blake3, 2.364 GH/s (nvcc 1.7s)".into(), qty: 1.0, unit_price_dkk: 5000.0 },
            LineItem { description: "flux-market live-intel + cost model (self-host < DeepSeek API)".into(), qty: 4.0, unit_price_dkk: 900.0 },
        ],
        pay_to: "MobilePay 12345 · IBAN DK00 0000 0000 0000 00".into(),
    };
    print!("{}", inv.render_html());
}
