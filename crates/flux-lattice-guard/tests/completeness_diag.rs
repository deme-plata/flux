use flux_lattice_guard::{ArithmeticCircuit, LatticeGuard, LatticeGuardSRS, SecurityLevel};
use rand::{rngs::StdRng, SeedableRng};
#[test]
fn round_trip_diag() {
    let _ = tracing_subscriber::fmt().with_max_level(tracing::Level::WARN).with_test_writer().try_init();
    let lg = LatticeGuard::new(SecurityLevel::PQ128).unwrap();
    let mut rng = StdRng::seed_from_u64(1);
    let mut c = ArithmeticCircuit::new(1, 2);
    c.add_multiplication_gate(vec![(0,1)], vec![(1,1)], vec![(2,1)]);
    let public = vec![3u64];
    let witness = vec![4u64, 12u64];
    let srs = LatticeGuardSRS::generate(lg.params().clone(), 4, &mut rng).unwrap();
    let p = lg.prove(&c, &witness, &public, &srs, &mut rng).expect("prove");
    eprintln!("DIAG prove ok: evals={:?} commitments={}", p.evaluations, p.commitments.len());
    let v = lg.verify(&c, &public, &p, &srs).expect("verify call");
    eprintln!("DIAG verify => {v}");
    assert!(v, "honest proof must verify");
}
