//! proof-sign — emit a signed `.proof` bundle for an existing release binary.
//!
//! fluxc's provenance is library-only (`provenance::emit_signed`), with no CLI;
//! this is the thin CLI for v0.0.5 R3 — sign each shipped binary so an operator
//! can verify "this byte-for-byte binary was emitted by the agent holding this
//! SQIsign-L5 key, for this swarm task" without trusting any server.
//!
//!   proof-sign <binary> <out.proof> <task_id> <fluxc_version> [release-tag]
//!
//! Agent key from $FLUX_AGENT_KEY_PATH (default ~/.flux-agent-key.json).

use std::fs;

use fluxc_core::provenance::{emit_signed, ProvenanceContext};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 {
        eprintln!("usage: proof-sign <binary> <out.proof> <task_id> <fluxc_version> [release-tag]");
        std::process::exit(2);
    }
    let (bin, out, task, version) = (&a[1], &a[2], &a[3], a[4].parse::<u32>().unwrap_or(0));
    let tag = a.get(5).cloned().unwrap_or_else(|| "sigil-v0.0.5".into());

    let key_path = std::env::var("FLUX_AGENT_KEY_PATH").unwrap_or_else(|_| "/root/.flux-agent-key.json".into());
    let key: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&key_path).expect("read agent key")).expect("parse key");
    let pk = hex::decode(key["pk_hex"].as_str().expect("pk_hex")).expect("pk hex");
    let sk = hex::decode(key["sk_hex"].as_str().expect("sk_hex")).expect("sk hex");

    let artifact = fs::read(bin).expect("read binary");
    // Agent identity bound to the key (the wallet binding; the real qnk wallet
    // can be substituted, but blake3(pk) is a stable agent id derived from it).
    let agent_wallet: [u8; 32] = *blake3::hash(&pk).as_bytes();
    let mut task_id = [0u8; 16];
    let tb = task.as_bytes();
    let n = tb.len().min(16);
    task_id[..n].copy_from_slice(&tb[..n]);

    let ctx = ProvenanceContext {
        artifact_bytes: artifact,
        source_bytes: tag.clone().into_bytes(),
        agent_wallet,
        swarm_task_id: task_id,
        settle_tx: None,
        fluxc_git: [0u8; 20],
        fluxc_version: version,
    };
    let proof = emit_signed(&ctx, &sk, &pk).expect("emit_signed");
    let json = serde_json::to_string_pretty(&proof).expect("serialize proof");
    fs::write(out, &json).expect("write .proof");
    println!(
        "✓ {out}  artifact={}…  source={}…  sig={}B  task={task}",
        hex::encode(&proof.artifact_hash[..6]),
        hex::encode(&proof.source_hash[..6]),
        proof.sqisign_sig.len()
    );
}
