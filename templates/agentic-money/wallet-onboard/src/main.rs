//! wallet-onboard — bootstrap a brand-new, SPENDABLE agent wallet, then ask a
//! sigil-rpcd to onboard it (student + nation account + starter funding).
//!
//! Fixes the `create_wallet` no-mnemonic trap: the seed is generated and kept
//! LOCALLY, so the wallet can actually sign/spend. The address is derived
//! deterministically from that seed, so you can hand it to a funder before the
//! agent ever signs anything.
//!
//! Run:  wallet-onboard [RPC_URL]
//!   e.g. wallet-onboard http://127.0.0.1:8099
//!
//! Prints the address + seed. ⚠️ The seed is the secret — capture it.

use agentic_money_kit::{wallet, Rpc};

fn main() {
    let rpc_url = std::env::args().nth(1).unwrap_or_else(|| "http://127.0.0.1:8099".into());

    // 1. local entropy → spendable wallet (seed kept, address derived)
    let w = match wallet::bootstrap() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("entropy error: {e}");
            std::process::exit(1);
        }
    };
    println!("🪪 new wallet");
    println!("   address : {}", w.address);
    println!("   seed    : {}   ⚠️ SECRET — store it; losing it loses the wallet", w.seed_hex);

    // 2. ask the chain to onboard it (student + nation + starter funds).
    //    sigil-rpcd's /onboard mints the starter grant and registers the wallet
    //    as a student AND a nation citizen in one shot.
    let rpc = Rpc::new(&rpc_url);
    let body = format!("{{\"wallet\":\"{}\"}}", w.address);
    match rpc.post("/onboard", &body) {
        Ok(resp) if !resp.trim().is_empty() => {
            println!("🎓 onboarded → {}", resp.trim());
        }
        Ok(_) => println!("ℹ️  /onboard returned empty — is sigil-rpcd up at {rpc_url}? wallet still valid locally."),
        Err(e) => println!("ℹ️  /onboard transport error ({e}) — wallet still valid locally; fund {} when the node is reachable.", w.address),
    }

    // 3. show how to re-derive the same wallet later from the seed.
    let same = wallet::Wallet::from_seed_hex(&w.seed_hex);
    debug_assert_eq!(same.address, w.address);
    println!("\nre-derive anytime: Wallet::from_seed_hex(\"{}…\") → {}", &w.seed_hex[..8], same.address);
}
