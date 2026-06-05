//! Tiny CLI for the scaffold-chain engine (the `fluxc scaffold-chain` surface).
//! usage: scaffold <name> <tag> <p2p_port> <api_port> <out_dir>
use flux_extension::{scaffold_chain, ChainParams};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 6 {
        eprintln!("usage: scaffold <name> <tag> <p2p_port> <api_port> <out_dir>");
        std::process::exit(2);
    }
    let params = ChainParams::new(&a[1], &a[2], a[3].parse()?, a[4].parse()?)?;
    let chain = scaffold_chain(&params);
    let written = chain.write_to(std::path::Path::new(&a[5]))?;
    print!("{}", chain.manifest());
    println!("\nwrote {} files under {}", written.len(), a[5]);
    Ok(())
}
