//! flux-kv CLI — flux-db key/value store for app persistence over SSH.
//!
//!   flux-kv put <key> [value]    # value from arg, or stdin if omitted
//!   flux-kv get <key>            # prints value to stdout (exit 1 if absent)
//!   flux-kv del <key>
//!   flux-kv list <prefix>
//!
//! DB path: $FLUX_KV_DB or /home/orobit/flux-vision-db

use flux_kv::Kv;
use std::io::Read;

fn db_path() -> std::path::PathBuf {
    std::env::var("FLUX_KV_DB")
        .map(Into::into)
        .unwrap_or_else(|_| "/home/orobit/flux-vision-db".into())
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let die = |m: &str| -> ! { eprintln!("flux-kv: {m}"); std::process::exit(2) };
    let cmd = a.first().map(|s| s.as_str()).unwrap_or("");
    let kv = Kv::open(db_path()).unwrap_or_else(|e| die(&format!("open: {e}")));

    match cmd {
        "put" => {
            let key = a.get(1).unwrap_or_else(|| die("put needs <key>"));
            let val = if let Some(v) = a.get(2) {
                v.clone().into_bytes()
            } else {
                let mut b = Vec::new();
                std::io::stdin().read_to_end(&mut b).ok();
                b
            };
            kv.put(key, &val).unwrap_or_else(|e| die(&e));
            println!("ok {} bytes", val.len());
        }
        "get" => {
            let key = a.get(1).unwrap_or_else(|| die("get needs <key>"));
            match kv.get(key).unwrap_or_else(|e| die(&e)) {
                Some(v) => {
                    use std::io::Write;
                    std::io::stdout().write_all(&v).ok();
                }
                None => std::process::exit(1),
            }
        }
        "del" => {
            let key = a.get(1).unwrap_or_else(|| die("del needs <key>"));
            kv.delete(key).unwrap_or_else(|e| die(&e));
            println!("deleted");
        }
        "list" => {
            let prefix = a.get(1).map(|s| s.as_str()).unwrap_or("");
            for k in kv.list(prefix) {
                println!("{k}");
            }
        }
        _ => die("usage: flux-kv put|get|del|list <key|prefix> [value]"),
    }
}
