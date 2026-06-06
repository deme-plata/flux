//! flux-swarm-migrate — run the non-destructive JSON→flux-db import against the
//! live `/tmp` swarm files and print the verify table. Touches NOTHING in `/tmp`;
//! writes a fresh flux-db and reports whether the money ledger is preserved.
//!
//!   flux-swarm-migrate [src_dir=/tmp] [db_path=/tmp/flux-swarm-db-proof-<ts>]

use std::time::{SystemTime, UNIX_EPOCH};

use flux_swarm_store::import::{import, verify_table};
use flux_swarm_store::{FluxDbStore, JsonStore};

fn main() {
    let mut args = std::env::args().skip(1);
    let src_dir = args.next().unwrap_or_else(|| "/tmp".to_string());
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let db_path = args.next().unwrap_or_else(|| format!("/tmp/flux-swarm-db-proof-{now}"));

    eprintln!("source : {src_dir}/flux-swarm*.json[l]");
    eprintln!("dest   : {db_path}  (fresh flux-db)\n");

    let src = match JsonStore::load_dir(std::path::Path::new(&src_dir)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("load source failed: {e}");
            std::process::exit(1);
        }
    };
    if src.parse_errors() > 0 {
        eprintln!("⚠ {} JSONL line(s) failed to parse (skipped)\n", src.parse_errors());
    }

    let dst = match FluxDbStore::open(&db_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("open flux-db failed: {e}");
            std::process::exit(1);
        }
    };

    let report = match import(&src, &dst) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("import failed: {e}");
            std::process::exit(1);
        }
    };

    match verify_table(&src, &report) {
        Ok(t) => print!("{t}"),
        Err(e) => {
            eprintln!("verify failed: {e}");
            std::process::exit(1);
        }
    }

    std::process::exit(if report.all_ok() { 0 } else { 2 });
}
