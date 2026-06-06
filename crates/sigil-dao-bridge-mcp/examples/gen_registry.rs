//! Generate dao-vm-dex-registry.json for sigilgraph.fluxapp.xyz publish.

fn main() {
    let deployer = std::env::args().nth(1).unwrap_or_else(|| "0x".to_string() + &"42".repeat(32));
    let json = sigil_dao_bridge_mcp::flux_sigil_dao_vm_dex_registry_json(&deployer);
    println!("{json}");
}