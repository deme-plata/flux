use flux_study::{day_number, civil_from_days, Study};

#[test]
fn dates_round_trip_and_order_across_a_year_boundary() {
    assert_eq!(civil_from_days(day_number("2026-09-05").unwrap()), "2026-09-05");
    assert!(day_number("2027-01-06").unwrap() > day_number("2026-11-30").unwrap());
    assert_eq!(day_number("2027-01-06").unwrap() - day_number("2026-09-05").unwrap(), 123);
    assert!(day_number("2026-13-01").is_none());
}

#[test]
fn the_seed_orders_by_the_nearest_deadline_and_marks_done() {
    let mut st = Study::default();
    st.seed_viktor();
    assert_eq!(st.goals.len(), 6);
    let q = st.queue("2026-09-05");
    // GMAT (2026-11-30) comes before the bachelor (2027-06-30) — the loop works the nearest date first.
    assert!(q[0].1.title.starts_with("GMAT"), "first in queue: {}", q[0].1.title);
    let plan = st.plan("2026-09-05", 180);
    assert!(!plan.is_empty() && plan.iter().map(|p| p.2.minutes).sum::<u32>() <= 180 || plan.len() == 1);
    let (g, t) = (plan[0].1.id, plan[0].2.id);
    st.done(g, t, "2026-09-05").unwrap();
    assert!(st.goals.iter().find(|x| x.id == g).unwrap().tasks.iter().find(|x| x.id == t).unwrap().done);
    let after = st.plan("2026-09-05", 180);
    assert!(!(after[0].1.id == g && after[0].2.id == t), "a done task must leave the plan");
}

#[test]
fn the_store_survives_a_save_and_load() {
    let dir = std::env::temp_dir().join(format!("flux-study-{}", std::process::id()));
    let path = dir.join("study.json");
    let mut st = Study::default();
    st.seed_viktor();
    st.credit("2026-09-05", "Modul 4", 10, "drew the note/anchor/nullifier flow");
    st.save(&path).unwrap();
    let back = Study::load(&path).unwrap();
    assert_eq!(back, st);
    assert_eq!(back.university_points(), 10);
    let _ = std::fs::remove_dir_all(dir);
}
