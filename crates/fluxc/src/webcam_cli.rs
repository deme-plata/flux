//! `fluxc webcam` — operator-side consent control for flux-webcam.
//!
//! # Why this is a CLI and not an MCP tool
//!
//! `grant` is the only operation in the whole system that *widens* access to
//! the camera. It is reachable exclusively from here: a human, at a shell, on
//! the box. The MCP surface an agent can reach carries `status`, `capture`
//! (gated), `revoke` and `panic_stop` — every one of which either observes
//! access or reduces it. There is no verb an agent can utter that increases its
//! own permission.
//!
//! `clear` is likewise operator-only: an agent may slam the kill switch on, but
//! it cannot lift it.

use flux_webcam::ConsentGate;

pub fn run(args: &[String]) -> i32 {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("status");
    let gate = ConsentGate::resolve();

    match sub {
        "grant" => {
            let seconds = flag_u64(args, "--seconds").unwrap_or(300);
            let captures = flag_u64(args, "--captures").unwrap_or(1) as u32;
            let reason = flag_str(args, "--reason").unwrap_or_else(|| "unspecified".into());
            match gate.grant(seconds, captures, &reason) {
                Ok(g) => {
                    println!("✅ consent granted");
                    println!("   captures : {}", g.max_captures);
                    println!("   window   : {seconds}s");
                    println!("   reason   : {reason}");
                    println!("   store    : {}", gate.paths.grant().display());
                    println!();
                    println!("Capture is now permitted until the window closes or the budget");
                    println!("runs out, whichever comes first. Revoke at any time with:");
                    println!("   fluxc webcam revoke");
                    0
                }
                Err(e) => {
                    eprintln!("❌ could not write grant: {e}");
                    1
                }
            }
        }
        "revoke" => match gate.revoke("operator") {
            Ok(()) => {
                println!("🔒 consent revoked — capture is denied again");
                0
            }
            Err(e) => {
                eprintln!("❌ revoke failed: {e}");
                1
            }
        },
        "stop" | "panic" => match gate.engage_kill_switch("operator") {
            Ok(()) => {
                println!("🛑 kill switch ENGAGED — all capture refused, overriding any grant");
                println!("   clear it with: fluxc webcam clear");
                0
            }
            Err(e) => {
                eprintln!("❌ {e}");
                1
            }
        },
        "clear" => match gate.clear_kill_switch("operator") {
            Ok(()) => {
                println!("✅ kill switch cleared");
                // Report the ACTUAL resulting decision rather than assuming one.
                // Clearing the switch does not itself grant anything, but a
                // grant issued before the switch went on may still be live — and
                // printing "still denied" in that case would be a lie about the
                // security state, which is the worst kind of message to get
                // wrong in this tool.
                let d = gate.evaluate();
                if d.is_allowed() {
                    println!("   ⚠ capture is NOW ALLOWED — a grant issued earlier is still live.");
                    println!("     {}", d.reason_str());
                    println!("     Run `fluxc webcam revoke` if you did not intend that.");
                } else {
                    println!("   capture remains denied: {}", d.reason_str());
                    println!("   Clearing the switch does not restore a grant — issue one with");
                    println!("   `fluxc webcam grant` if you want to permit capture.");
                }
                0
            }
            Err(e) => {
                eprintln!("❌ {e}");
                1
            }
        },
        "audit" => {
            match gate.verify_audit() {
                Ok(n) => println!("✅ audit chain intact — {n} entries"),
                Err(seq) => println!("🚨 audit chain BROKEN at entry {seq} — log was edited"),
            }
            let log = std::fs::read_to_string(gate.paths.audit()).unwrap_or_default();
            for line in log.lines().filter(|l| !l.trim().is_empty()).rev().take(30).collect::<Vec<_>>().into_iter().rev() {
                println!("  {line}");
            }
            0
        }
        "status" => {
            let s = gate.status_json();
            println!("{}", serde_json::to_string_pretty(&s).unwrap_or_default());
            0
        }
        "help" | "--help" | "-h" => {
            help();
            0
        }
        other => {
            eprintln!("unknown subcommand: {other}\n");
            help();
            2
        }
    }
}

fn help() {
    println!("fluxc webcam — operator consent control for flux-webcam\n");
    println!("  fluxc webcam status                     show the current decision + grant");
    println!("  fluxc webcam grant [opts]               PERMIT capture (operator only)");
    println!("      --seconds N     window length (default 300, max 3600)");
    println!("      --captures N    budget (default 1, max 500)");
    println!("      --reason TEXT   recorded in the audit log");
    println!("  fluxc webcam revoke                     end the grant immediately");
    println!("  fluxc webcam stop                       kill switch ON (deny everything)");
    println!("  fluxc webcam clear                      kill switch OFF (operator only)");
    println!("  fluxc webcam audit                      verify the hash-chained log\n");
    println!("An agent can call status / capture / revoke / panic_stop over MCP.");
    println!("It can NEVER call grant — that is why grant lives here and not there.");
}

fn flag_str(args: &[String], key: &str) -> Option<String> {
    args.iter().position(|a| a == key).and_then(|i| args.get(i + 1)).cloned()
}

fn flag_u64(args: &[String], key: &str) -> Option<u64> {
    flag_str(args, key).and_then(|v| v.parse().ok())
}
