//! flux-context-watch — Task 3 daemon driver.
//!
//! Usage:
//!   flux-context-watch [root]            run the watch loop (Ctrl-C to stop)
//!   flux-context-watch [root] --once     do a single scan and exit
//!   flux-context-watch [root] --status   print the last watch status JSON
//!   flux-context-watch [root] --poll N   poll interval seconds (default 2)

use flux_context::watch::{self, WatchConfig};
use std::path::PathBuf;
use std::time::Duration;

fn main() {
    let mut root = std::env::current_dir().expect("cwd");
    let mut mode = "run";
    let mut poll = 2u64;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--status" => mode = "status",
            "--once" => mode = "once",
            "--poll" => {
                if let Some(v) = it.next() {
                    poll = v.parse().unwrap_or(2);
                }
            }
            other => root = PathBuf::from(other),
        }
    }
    let ctx_dir = root.join(".whale/context");

    match mode {
        "status" => match watch::read_status(&ctx_dir) {
            Some(s) => println!("{}", serde_json::to_string_pretty(&s).unwrap_or_default()),
            None => {
                eprintln!("flux-context-watch: no status at {}", ctx_dir.display());
                std::process::exit(1);
            }
        },
        "once" => {
            let cfg = WatchConfig::new(root.clone());
            let mut sig = 0u64;
            let mut st = watch::read_status(&ctx_dir).unwrap_or_default();
            match watch::tick(&cfg, &mut sig, &mut st) {
                Ok(changed) => eprintln!(
                    "scan: {} · v{} · {} chunks · ~{} tok · {}",
                    if changed { "CHANGED" } else { "no-change" },
                    st.version, st.crate_count, st.total_tokens, st.last_diff_summary
                ),
                Err(e) => {
                    eprintln!("flux-context-watch: {e}");
                    std::process::exit(1);
                }
            }
        }
        _ => {
            let mut cfg = WatchConfig::new(root.clone());
            cfg.poll = Duration::from_secs(poll);
            eprintln!(
                "flux-context-watch: polling {} every {}s · L1 {}",
                root.display(),
                poll,
                watch::l1_dir(&ctx_dir).display()
            );
            if let Err(e) = watch::run(&cfg) {
                eprintln!("flux-context-watch: {e}");
                std::process::exit(1);
            }
        }
    }
}
