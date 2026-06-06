//! cargo run --example gen_registry -p flux-agora-stargate-mcp
fn main() {
    let deployer = std::env::args().nth(1).unwrap_or_else(|| "42".repeat(64));
    print!("{}", flux_agora_stargate_mcp::flux_agora_stargate_registry_json(&deployer));
}
