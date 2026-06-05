//! Live DRY-RUN of P10 stability → P12 remediation. Read-only probe; executes NOTHING
//! (apply_auto dry_run=true only prints the commands it WOULD run).
//!   flux-cargo-wrapper run -p flux-legacy --example remediate_dryrun
use flux_legacy::{remediate, stability};

fn main() {
    let signals = stability::probe(
        "q-api-server",                              // systemd unit (journalctl)
        "q-api-server",                              // process match
        "data-mainnet-genesis",                      // authoritative-DB marker
        "http://localhost:8080/api/v1/status",       // serving probe
    );
    let report = stability::assess(&signals);
    print!("{}", stability::render(&report));

    let plan = remediate::plan_remediation(&report, &remediate::Policy::default());
    println!();
    print!("{}", remediate::render_remediation(&plan));

    println!("\n── apply_auto (DRY-RUN — executes nothing) ──");
    for o in remediate::apply_auto(&plan, true) {
        println!("  ledger · {:<10} ran={} · {}", o.signal, o.ran, o.result);
    }
}
