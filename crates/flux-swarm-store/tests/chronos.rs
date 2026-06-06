//! chronos.rs — deterministic proof that the JSON→flux-db migration preserves the
//! swarm state, and ESPECIALLY the money ledger (completed count + Σ QUG), exactly.
//! Seeded corpus ⇒ identical every run. No filesystem source — the corpus is built
//! in-memory via the SwarmStore interface, then imported into a real flux-db.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use flux_swarm_store::import::import;
use flux_swarm_store::{
    Activity, Agent, Claim, Completed, FileClaim, FluxDbStore, JsonStore, Message, SwarmStore,
};

static N: AtomicU64 = AtomicU64::new(0);
fn tmp_db() -> std::path::PathBuf {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let n = N.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("flux-swarm-store-test-{}-{}-{}", std::process::id(), now, n))
}

/// A deterministic corpus that mirrors the live shapes:
///  - 3 agents with KNOWN earned totals (gemini 132.5, grok 0.5, rocky 0.0)
///  - `n_completed` settlement records with a KNOWN Σ QUG
///  - messages at known timestamps, activity, and a file claim
fn corpus(n_completed: u64) -> (JsonStore, f64) {
    let s = JsonStore::new_empty();
    let agent = |id: &str, earned: f64| Agent {
        id: id.into(),
        wallet_address: format!("qnk_{id}"),
        registered_at: 1000,
        status: "Idle".into(),
        current_crates: vec![],
        total_earned_qug: earned,
    };
    s.put_agent(&agent("test_gemini_roundtrip", 132.5)).unwrap();
    s.put_agent(&agent("grok-viktor", 0.5)).unwrap();
    s.put_agent(&agent("rocky-ashwalker", 0.0)).unwrap();

    s.put_claim(&Claim {
        task_id: "codex-rocky-445".into(),
        crates: vec!["sigil-ashwalker".into()],
        agent: "codex-rocky".into(),
        claimed_at: 1780648337,
        priority: 2,
        estimated_qug: 0.5,
    })
    .unwrap();

    let mut sum = 0.0;
    for i in 0..n_completed {
        let q = if i % 2 == 0 { 0.5 } else { 1.0 }; // deterministic, non-uniform
        sum += q;
        s.append_completed(&Completed {
            task_id: format!("t{i}"),
            agent_id: "rocky".into(),
            crates: vec!["flux-x".into()],
            success: true,
            qug_earned: q,
            completed_at: 1_780_000_000 + i,
        })
        .unwrap();
    }

    // messages at increasing ts (range-scan fodder)
    for i in 0..10u64 {
        s.append_message(&Message {
            id: i + 1,
            from: "rocky".into(),
            to: if i % 3 == 0 { "*".into() } else { "codex".into() },
            ts_ms: 1_780_283_925_000 + i * 1000,
            payload: format!("m{i}"),
            reply_to: None,
        })
        .unwrap();
    }
    // several activity entries sharing the SAME second (tiebreaker stress)
    for i in 0..5u64 {
        s.append_activity(&Activity {
            at: 1_780_040_537,
            agent: "rocky".into(),
            kind: "registered".into(),
            detail: format!("d{i}"),
        })
        .unwrap();
    }
    s.put_file_claim(&FileClaim {
        path: "/x/flux-moe/src/bin/flux-moe.rs".into(),
        agent: "rocky-vision-jobs".into(),
        claimed_at: 1780488291,
        note: "stream mode".into(),
    })
    .unwrap();

    (s, sum)
}

#[test]
fn import_preserves_completed_count_and_qug_exactly() {
    let (src, sum) = corpus(445); // the live count
    let dst = FluxDbStore::open(tmp_db()).unwrap();
    let r = import(&src, &dst).unwrap();

    assert!(r.all_ok(), "ledger must be preserved: {r:?}");
    assert_eq!(r.completed, 445);
    assert_eq!(dst.completed_count().unwrap(), 445);
    assert!((dst.sum_qug_earned().unwrap() - sum).abs() < 1e-9);
    assert!((dst.sum_qug_earned().unwrap() - src.sum_qug_earned().unwrap()).abs() < 1e-9);
}

#[test]
fn per_agent_earned_survives_the_move() {
    let (src, _) = corpus(10);
    let dst = FluxDbStore::open(tmp_db()).unwrap();
    import(&src, &dst).unwrap();
    // the real attribution (gemini 132.5) must come back byte-for-byte
    assert_eq!(dst.get_agent("test_gemini_roundtrip").unwrap().unwrap().total_earned_qug, 132.5);
    assert_eq!(dst.get_agent("grok-viktor").unwrap().unwrap().total_earned_qug, 0.5);
    assert_eq!(dst.list_agents().unwrap().len(), 3);
}

#[test]
fn all_record_kinds_round_trip() {
    let (src, _) = corpus(20);
    let dst = FluxDbStore::open(tmp_db()).unwrap();
    let r = import(&src, &dst).unwrap();
    assert_eq!(r.agents, 3);
    assert_eq!(r.claims, 1);
    assert_eq!(r.completed, 20);
    assert_eq!(r.messages, 10);
    assert_eq!(r.activity, 5); // all 5 same-second entries survive (seq tiebreaker)
    assert_eq!(r.files, 1);
}

#[test]
fn messages_since_is_a_correct_range_scan() {
    let (src, _) = corpus(1);
    let dst = FluxDbStore::open(tmp_db()).unwrap();
    import(&src, &dst).unwrap();
    // messages were at 1_780_283_925_000 + i*1000, i in 0..10
    let cutoff = 1_780_283_925_000 + 5 * 1000;
    let got = dst.messages_since(cutoff).unwrap();
    assert_eq!(got.len(), 5, "ts >= cutoff should yield i in 5..10");
    assert!(got.iter().all(|m| m.ts_ms >= cutoff));
    // chronological
    assert!(got.windows(2).all(|w| w[0].ts_ms <= w[1].ts_ms));
}

#[test]
fn activity_tail_is_chronological_last_n() {
    let s = JsonStore::new_empty();
    for i in 0..8u64 {
        s.append_activity(&Activity {
            at: 1_780_000_000 + i, // distinct seconds, increasing
            agent: "a".into(),
            kind: "k".into(),
            detail: format!("{i}"),
        })
        .unwrap();
    }
    let dst = FluxDbStore::open(tmp_db()).unwrap();
    import(&s, &dst).unwrap();
    let tail = dst.activity_tail(3).unwrap();
    assert_eq!(tail.len(), 3);
    assert_eq!(tail[0].detail, "5");
    assert_eq!(tail[2].detail, "7"); // newest last, chronological
}

#[test]
fn empty_source_is_clean() {
    let src = JsonStore::new_empty();
    let dst = FluxDbStore::open(tmp_db()).unwrap();
    let r = import(&src, &dst).unwrap();
    assert!(r.all_ok());
    assert_eq!(r.completed, 0);
    assert_eq!(r.sum_qug, 0.0);
}
