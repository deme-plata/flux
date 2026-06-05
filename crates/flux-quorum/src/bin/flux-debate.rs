//! flux-debate — the CLI where **live LLM output meets `run_debate`**.
//!
//! A driver calls the proposer + auditor Qwen endpoints, then pipes their JSON
//! here: this binary builds the M-of-N policy, has each APPROVING member sign the
//! exact action (blake3-MAC stand-in for the agents' SQIsign keys), runs the
//! proof-of-debate quorum, and prints Settle / Vetoed / NoQuorum.
//!
//! ```text
//! flux-debate --proposal '{"action":"swap","pool":"HODL","size":50,"reason":"…"}' \
//!             --audit    '{"auditor_id":2,"verdict":"approve"}' \
//!             [--audit   '{"auditor_id":3,"verdict":"veto","reason":"honeypot"}'] [--m 2]
//! ```

use flux_quorum::debate::proposal_msg;
use flux_quorum::quorum::{blake3_mac, Blake3MacVerifier, QuorumMember, QuorumPolicy, SignedShare};
use flux_quorum::{run_debate, Audit, DebateOutcome, Proposal, Verdict};

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}
fn args_all(args: &[String], flag: &str) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0..args.len() {
        if args[i] == flag {
            if let Some(v) = args.get(i + 1) {
                out.push(v.clone());
            }
        }
    }
    out
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let pj = arg(&a, "--proposal").unwrap_or_else(|| {
        eprintln!("usage: flux-debate --proposal '{{json}}' --audit '{{json}}' [--audit …] [--m 2]");
        std::process::exit(64);
    });
    let pv: serde_json::Value = serde_json::from_str(&pj).expect("--proposal must be valid JSON");
    let proposal = Proposal {
        action: pv["action"].as_str().unwrap_or("swap").to_string(),
        pool: pv["pool"].as_str().unwrap_or("?").to_string(),
        size: pv["size"].as_u64().unwrap_or(0) as u128,
        reason: pv["reason"].as_str().unwrap_or("").to_string(),
    };

    let mut audits: Vec<Audit> = Vec::new();
    for aj in args_all(&a, "--audit") {
        let v: serde_json::Value = serde_json::from_str(&aj).expect("--audit must be valid JSON");
        let id = v["auditor_id"].as_u64().unwrap_or(2) as u32;
        let verdict = match v["verdict"].as_str().unwrap_or("veto").to_lowercase().as_str() {
            "approve" | "ok" | "yes" | "pass" => Verdict::Approve,
            _ => Verdict::Veto(v["reason"].as_str().unwrap_or("vetoed").to_string()),
        };
        audits.push(Audit { auditor_id: id, verdict });
    }
    let m: usize = arg(&a, "--m").and_then(|s| s.parse().ok()).unwrap_or(2);

    // Policy = proposer (id 1) + each auditor. Each APPROVING member signs the
    // exact action; a vetoing auditor withholds its signature AND trips gate 1.
    let mut members = vec![QuorumMember { id: 1, pubkey: b"key-1".to_vec() }];
    for au in &audits {
        members.push(QuorumMember { id: au.auditor_id, pubkey: format!("key-{}", au.auditor_id).into_bytes() });
    }
    let policy = QuorumPolicy::new(m, members);
    let msg = proposal_msg(&proposal);
    let mut shares = vec![SignedShare { member: 1, sig: blake3_mac(b"key-1", &msg) }]; // proposer signs
    for au in &audits {
        if matches!(au.verdict, Verdict::Approve) {
            shares.push(SignedShare { member: au.auditor_id, sig: blake3_mac(format!("key-{}", au.auditor_id).as_bytes(), &msg) });
        }
    }

    let out = run_debate(&proposal, &audits, &policy, &shares, &Blake3MacVerifier);
    println!("PROPOSAL : {} {} size={} — {}", proposal.action, proposal.pool, proposal.size, proposal.reason);
    let verdicts: Vec<String> = audits.iter().map(|x| match &x.verdict {
        Verdict::Approve => format!("#{}=APPROVE", x.auditor_id),
        Verdict::Veto(w) => format!("#{}=VETO({})", x.auditor_id, w.chars().take(40).collect::<String>()),
    }).collect();
    println!("AUDITORS : {}", if verdicts.is_empty() { "(none)".into() } else { verdicts.join(", ") });
    match out {
        DebateOutcome::Settle { signers } => {
            println!("OUTCOME  : ✅ SETTLE — debate agreed + {}/{} quorum signed {:?}", signers.len(), policy.n(), signers);
            std::process::exit(0);
        }
        DebateOutcome::Vetoed(why) => {
            println!("OUTCOME  : ⛔ VETOED — {why}");
            std::process::exit(1);
        }
        DebateOutcome::NoQuorum => {
            println!("OUTCOME  : ⚠ NO QUORUM — agreed but < {m} signed the exact action");
            std::process::exit(2);
        }
    }
}
