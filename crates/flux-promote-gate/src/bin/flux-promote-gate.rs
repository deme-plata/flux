//! flux-promote-gate CLI — the battle-test gate the SSH auto-updater runs
//! BEFORE flux_hot_swap. Exit 0 = PROMOTE (proceed to mainnet), 1 = HOLD.
//!
//!   flux-promote-gate --candidate 0.22.0 --manifest fluxc-latest.json \
//!     --scope money --gates 5/5 --soaked --approve rocky --approve deepseek
//!
//!   # published version may be given directly instead of a manifest:
//!   flux-promote-gate --candidate 0.22.0 --published 0.17.0 --scope low-risk \
//!     --gates 5/5 --soaked --approve rocky

use flux_promote_gate::{evaluate, published_version_from_manifest, BattleTest, Scope};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |k: &str| -> Option<String> {
        args.iter().position(|a| a == k).and_then(|i| args.get(i + 1).cloned())
    };
    let has = |k: &str| args.iter().any(|a| a == k);
    if args.is_empty() || has("-h") || has("--help") {
        eprintln!("flux-promote-gate --candidate X.Y.Z (--published X.Y.Z | --manifest <file>)\n  --scope money|low-risk --gates P/T --soaked --approve <agent> [--approve <agent>]");
        std::process::exit(2);
    }

    let candidate = get("--candidate").unwrap_or_else(|| die("--candidate required"));

    let published = if let Some(p) = get("--published") {
        p
    } else if let Some(mf) = get("--manifest") {
        let body = std::fs::read_to_string(&mf).unwrap_or_else(|e| die(&format!("read {mf}: {e}")));
        published_version_from_manifest(&body).unwrap_or_else(|| die("manifest has no .version"))
    } else {
        die("need --published or --manifest")
    };

    // --gates P/T  (e.g. 5/5)
    let (passed, total) = get("--gates")
        .and_then(|g| {
            let mut it = g.split('/');
            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
        })
        .unwrap_or((0u32, 0u32));
    let battle = BattleTest { gates_total: total, gates_passed: passed, soaked_on_testnet: has("--soaked") };

    let scope = match get("--scope").as_deref() {
        Some("money") | Some("consensus") | Some("money-consensus") => Scope::MoneyConsensus,
        Some("low-risk") | Some("low") | None => Scope::LowRisk,
        Some(other) => die(&format!("unknown --scope '{other}' (money|low-risk)")),
    };

    // collect every --approve <agent>
    let approvals: Vec<String> = args
        .iter()
        .enumerate()
        .filter(|(_, a)| a.as_str() == "--approve")
        .filter_map(|(i, _)| args.get(i + 1).cloned())
        .collect();

    let d = evaluate(&candidate, &published, &battle, scope, &approvals);

    println!("╔══ FLUX PROMOTE GATE ══════════════════════════════╗");
    println!("  candidate : {candidate}");
    println!("  published : {published} (mainnet)");
    println!("  scope     : {}", scope.label());
    for r in &d.reasons {
        println!("  {r}");
    }
    println!("╠═══════════════════════════════════════════════════╣");
    println!("  VERDICT   : {}", if d.promote { "✅ PROMOTE → mainnet" } else { "⛔ HOLD" });
    println!("╚═══════════════════════════════════════════════════╝");

    std::process::exit(if d.promote { 0 } else { 1 });
}

fn die(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(2);
}
