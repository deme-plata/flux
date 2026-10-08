//! Generate an IronTunnel v1 static keypair. Writes the PRIVATE key to `--out`
//! (mode 0600, never overwrites) and prints the PUBLIC key, which is what you hand to
//! the other side (relay: clients pin it; client: the relay's authorized list holds it).

use clap::Parser;
use flux_irontunnel::secure::StaticKeypair;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "irontunnel-keygen", about = "Generate an IronTunnel v1 x25519 keypair")]
struct Args {
    /// Private key file to create
    #[arg(long)]
    out: PathBuf,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let kp = StaticKeypair::generate()?;
    kp.save(&args.out)?;
    println!("{}", kp.public().to_hex());
    eprintln!("private key written to {} (keep it secret); public key above", args.out.display());
    Ok(())
}
