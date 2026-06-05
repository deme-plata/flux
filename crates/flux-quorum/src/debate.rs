//! debate — PROOF-OF-DEBATE: a 2-of-N **LLM verified-execution money quorum**,
//! built on top of [`verify_quorum`](crate::quorum::verify_quorum).
//!
//! This is the **merge** of two things this swarm built in parallel (and shouldn't
//! have built twice — see the box-registry / lane-lock lesson):
//!   - rocky's **2-of-2 Qwen trade gate** — a proposer LLM proposes an agentic-money
//!     action; an INDEPENDENT auditor LLM red-teams it; only 2-of-2 agreement passes.
//!     Demonstrated: a greedy qwen2.5:32b proposer picked the MOON honeypot
//!     (95.25% round-trip loss), the auditor BLOCKED it.
//!   - rocky-lite's **PROOF-OF-DEBATE** — K=2 → P(bad)=f²: two independent models'
//!     errors must coincide for a bad action to settle; only 2-of-2 settles on-chain.
//!
//! A money action SETTLES only when BOTH gates hold:
//!   1. **Debate agreement** — ≥1 independent auditor and NONE veto (the LLM layer).
//!   2. **Crypto quorum** — ≥M of N parties sign the exact action ([`verify_quorum`],
//!      scheme-injected for crypto agility).
//!
//! Two LLMs must AGREE *and* two keys must SIGN. Either gate alone is insufficient.

use crate::quorum::{verify_quorum, QuorumOutcome, QuorumPolicy, QuorumVerifier, SignedShare};

/// An agentic-money action proposed by the proposer LLM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub action: String,
    pub pool: String,
    pub size: u128,
    pub reason: String,
}

/// An independent auditor LLM's verdict on a proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Approve,
    Veto(String),
}

/// One auditor's verdict, tagged by auditor identity (different model / box).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Audit {
    pub auditor_id: u32,
    pub verdict: Verdict,
}

/// The result of running proof-of-debate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebateOutcome {
    /// Agreement + crypto quorum → the action may execute on-chain.
    Settle { signers: Vec<u32> },
    /// An auditor vetoed (e.g. honeypot) — blocked before signing even matters.
    Vetoed(String),
    /// LLMs agreed but the signed shares didn't reach the M-of-N quorum.
    NoQuorum,
}

/// Canonical bytes the quorum signs — binds action+pool+size so a signed share
/// attests THIS exact action and cannot be malleated to another pool/size.
pub fn proposal_msg(p: &Proposal) -> Vec<u8> {
    format!("flux-debate/v1|{}|{}|{}", p.action, p.pool, p.size).into_bytes()
}

/// Run proof-of-debate. Settles iff (1) there is ≥1 independent auditor and NONE
/// veto, AND (2) the signed shares form the policy's M-of-N quorum over the action.
pub fn run_debate<V: QuorumVerifier>(
    proposal: &Proposal,
    audits: &[Audit],
    policy: &QuorumPolicy,
    shares: &[SignedShare],
    verifier: &V,
) -> DebateOutcome {
    // Gate 1 — DEBATE: need an independent auditor; any veto blocks (honeypot-catch).
    if audits.is_empty() {
        return DebateOutcome::Vetoed("no independent auditor (K<2 — single agent can't self-approve)".into());
    }
    for a in audits {
        if let Verdict::Veto(why) = &a.verdict {
            return DebateOutcome::Vetoed(why.clone());
        }
    }
    // Gate 2 — CRYPTO QUORUM over the exact action.
    let report = verify_quorum(policy, &proposal_msg(proposal), shares, verifier);
    match report.outcome {
        QuorumOutcome::Reached { signers, .. } => DebateOutcome::Settle { signers },
        _ => DebateOutcome::NoQuorum,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quorum::{blake3_mac, Blake3MacVerifier, QuorumMember};

    fn member(id: u32) -> QuorumMember {
        QuorumMember { id, pubkey: format!("key-{id}").into_bytes() }
    }
    fn share(id: u32, msg: &[u8]) -> SignedShare {
        SignedShare { member: id, sig: blake3_mac(format!("key-{id}").as_bytes(), msg) }
    }
    fn policy2() -> QuorumPolicy {
        QuorumPolicy::new(2, vec![member(1), member(2)])
    }
    fn hodl() -> Proposal {
        Proposal { action: "swap".into(), pool: "HODL".into(), size: 50, reason: "lowest round-trip loss 9.21%".into() }
    }
    fn moon() -> Proposal {
        Proposal { action: "swap".into(), pool: "MOON".into(), size: 50, reason: "highest %".into() }
    }

    #[test]
    fn honeypot_proposal_is_vetoed_even_with_signatures() {
        // greedy proposer picks the MOON honeypot; the independent auditor vetoes.
        let p = moon();
        let audits = vec![Audit { auditor_id: 2, verdict: Verdict::Veto("MOON 95.25% loss > threshold + honeypot".into()) }];
        let msg = proposal_msg(&p);
        let shares = vec![share(1, &msg), share(2, &msg)]; // even WITH a full quorum of sigs…
        let out = run_debate(&p, &audits, &policy2(), &shares, &Blake3MacVerifier);
        // …the debate gate vetoes it before the crypto quorum is honored.
        assert!(matches!(out, DebateOutcome::Vetoed(_)), "honeypot must be vetoed, got {out:?}");
    }

    #[test]
    fn agreed_and_quorum_signed_settles() {
        let p = hodl();
        let audits = vec![Audit { auditor_id: 2, verdict: Verdict::Approve }];
        let msg = proposal_msg(&p);
        let shares = vec![share(1, &msg), share(2, &msg)];
        match run_debate(&p, &audits, &policy2(), &shares, &Blake3MacVerifier) {
            DebateOutcome::Settle { signers } => assert_eq!(signers.len(), 2),
            other => panic!("expected Settle, got {other:?}"),
        }
    }

    #[test]
    fn agreed_but_one_signature_is_no_quorum() {
        let p = hodl();
        let audits = vec![Audit { auditor_id: 2, verdict: Verdict::Approve }];
        let msg = proposal_msg(&p);
        let shares = vec![share(1, &msg)]; // only 1 of 2 signed
        assert_eq!(run_debate(&p, &audits, &policy2(), &shares, &Blake3MacVerifier), DebateOutcome::NoQuorum);
    }

    #[test]
    fn no_auditor_means_no_debate_no_settle() {
        // a lone agent cannot self-approve — K<2 → vetoed regardless of its own sig.
        let p = hodl();
        let msg = proposal_msg(&p);
        let shares = vec![share(1, &msg), share(2, &msg)];
        assert!(matches!(run_debate(&p, &[], &policy2(), &shares, &Blake3MacVerifier), DebateOutcome::Vetoed(_)));
    }

    #[test]
    fn signature_over_a_different_action_does_not_settle() {
        // sigs that attest a DIFFERENT pool can't authorize this action (anti-malleation).
        let p = hodl();
        let audits = vec![Audit { auditor_id: 2, verdict: Verdict::Approve }];
        let wrong = proposal_msg(&moon()); // signed the MOON action, not HODL
        let shares = vec![share(1, &wrong), share(2, &wrong)];
        assert_eq!(run_debate(&p, &audits, &policy2(), &shares, &Blake3MacVerifier), DebateOutcome::NoQuorum);
    }
}
