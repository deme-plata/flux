//! DILITHIUM IN-CIRCUIT COST — is a SNARK-native (lattice) signature the lever
//! that ed25519-in-circuit could not be?
//!
//! eddsa_air.rs measured the prover wall: proving ed25519 validity in-circuit is
//! ~290,000× slower than verifying it natively, because ed25519's 255-bit curve
//! arithmetic needs non-native field emulation (~2–4M R1CS constraints).
//!
//! Dilithium is LATTICE-based: all arithmetic is mod q = 8,380,417 (a 23-bit
//! prime) over degree-256 polynomials. That field is tiny and SNARK-native — no
//! emulation. This harness drives the REAL `DilithiumVerifierGadget::synthesize`
//! and counts the constraints it actually emits, per security level. The count is
//! the soundness scale that decides prover cost.

use flux_recursive_proofs::gadgets::dilithium::{
    DilithiumLevel, DilithiumParams, DilithiumPublicKeyWires, DilithiumSignatureWires,
    DilithiumVerifierGadget,
};
use flux_recursive_proofs::ConstraintBuilder;

/// Build the verification circuit for `level` and return (constraints, wires).
fn build(level: DilithiumLevel) -> (usize, usize) {
    let p = DilithiumParams::new(level);
    let mut b = ConstraintBuilder::new(p.q);
    let _one = b.allocator.alloc_public_input(); // wire 0 = constant 1

    let mut rho = [0usize; 8];
    for r in rho.iter_mut() { *r = b.allocator.alloc_witness(); }
    let t1: Vec<Vec<usize>> = (0..p.k).map(|_| b.allocator.alloc_witness_array(p.n)).collect();
    let z: Vec<Vec<usize>> = (0..p.l).map(|_| b.allocator.alloc_witness_array(p.n)).collect();
    let h: Vec<Vec<usize>> = (0..p.k).map(|_| b.allocator.alloc_witness_array(p.omega)).collect();
    let mut c_tilde = [0usize; 8];
    for c in c_tilde.iter_mut() { *c = b.allocator.alloc_witness(); }
    let message = b.allocator.alloc_witness_array(8);

    let pk = DilithiumPublicKeyWires { t1, rho };
    let sig = DilithiumSignatureWires { z, h, c_tilde };
    let _valid = DilithiumVerifierGadget::new(level).synthesize(&mut b, &pk, &sig, &message);

    (b.constraint_count(), b.allocator.wire_count())
}

fn main() {
    println!("\n  DILITHIUM IN-CIRCUIT COST — SNARK-native lattice sig vs ed25519\n");
    println!("  {:>11} | {:>12} | {:>9} | {:>12}", "level", "constraints", "wires", "≈trace 2^k");
    println!("  {}", "-".repeat(54));
    let mut d5 = 0usize;
    for (nm, lv) in [
        ("Dilithium2", DilithiumLevel::Level2),
        ("Dilithium3", DilithiumLevel::Level3),
        ("Dilithium5", DilithiumLevel::Level5),
    ] {
        let (c, w) = build(lv);
        if matches!(lv, DilithiumLevel::Level5) { d5 = c; }
        let k = (c.max(1) as f64).log2().ceil() as u32;
        println!("  {nm:>11} | {c:>12} | {w:>9} | 2^{k:<2} = {}", 1usize << k);
    }

    // ed25519 reference (non-native 255-bit curve, from the ZK literature).
    let ed = 3_000_000usize; // ~2–4M; use 3M midpoint
    println!("\n  ed25519 in-circuit (non-native 255-bit curve): ~2,000,000–4,000,000 constraints");
    println!("  → Dilithium5 is {:.1}× FEWER constraints than ed25519 (and post-quantum).",
             ed as f64 / d5.max(1) as f64);

    // Map constraint count → trace rows → prover cost via the eddsa_air curve:
    //   measured: 2^14 rows ≈ 363 ms (commitment floor, ~linear in rows × log).
    let prove_ms = |constraints: usize| -> f64 {
        let rows = constraints.next_power_of_two().max(16384) as f64;
        363.0 * (rows / 16384.0)  // linear floor; log factor folded into the ~
    };
    let d5_ms = prove_ms(d5);
    let ed_ms = prove_ms(ed);
    println!("\n  ── prover-cost map (from eddsa_air's measured 2^14≈363ms floor) ──");
    println!("    Dilithium5 proof:  ~{:.0} ms/sig  →  {:.0} sound sigs/s/box", d5_ms, 1000.0 / d5_ms);
    println!("    ed25519 proof:     ~{:.0} ms/sig  →  {:.1} sound sigs/s/box", ed_ms, 1000.0 / ed_ms);
    println!("    Dilithium5 prover is ~{:.0}× faster than ed25519 — but still", ed_ms / d5_ms);
    let d5_sigs = 1000.0 / d5_ms;
    println!("    {:.0} sigs/s/box → 500M needs {:.0} CPU prover boxes ({:.0}–{:.0} with GPU).",
             d5_sigs, 500e6 / d5_sigs, 500e6 / d5_sigs / 50.0, 500e6 / d5_sigs / 10.0);
}
