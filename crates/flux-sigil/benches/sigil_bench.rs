//! Benchmarks for flux-sigil vs raw BLAKE3 + SQIsign baselines.
//!
//! Goal: quantify the overhead of (a) SQIsign-derived keying on top of raw
//! BLAKE3 keyed_hash, and (b) auto-signing the BLAKE3 root in SignedHasher.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use flux_sigil::{keyed::SigilKey, streaming::SignedHasher};

fn payload(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i & 0xFF) as u8).collect()
}

fn bench_keyed_hash(c: &mut Criterion) {
    let mut group = c.benchmark_group("keyed_hash");
    let raw_key = [0x42u8; 32];

    for &size in &[64usize, 1024, 1024 * 1024] {
        let msg = payload(size);
        group.throughput(Throughput::Bytes(size as u64));

        // Baseline: raw BLAKE3 keyed_hash
        group.bench_function(format!("blake3_keyed/{}B", size), |b| {
            b.iter(|| blake3::keyed_hash(black_box(&raw_key), black_box(&msg)));
        });

        // flux-sigil: SigilKey::hash (one keyed_hash, key already derived)
        let key = SigilKey::from_raw_key(raw_key);
        group.bench_function(format!("sigil_keyed/{}B", size), |b| {
            b.iter(|| key.hash(black_box(&msg)));
        });
    }
    group.finish();
}

fn bench_key_derivation(c: &mut Criterion) {
    let fake_sig: Vec<u8> = (0..177u8).collect();
    c.bench_function("derive_key_from_sqisign_sig", |b| {
        b.iter(|| SigilKey::from_sqisign_signature(black_box(&fake_sig)));
    });
}

fn bench_signed_hasher(c: &mut Criterion) {
    let mut group = c.benchmark_group("signed_hasher");
    group.sample_size(20); // SQIsign sign is expensive; keep samples reasonable

    let (sk, pk) = flux_sqisign::keygen();

    for &size in &[64usize, 1024, 1024 * 1024] {
        let msg = payload(size);
        group.throughput(Throughput::Bytes(size as u64));

        // Baseline: raw BLAKE3 hash of the same payload, no signature
        group.bench_function(format!("blake3_only/{}B", size), |b| {
            b.iter(|| blake3::hash(black_box(&msg)));
        });

        // flux-sigil: streaming hash + SQIsign sign of the 32B root
        group.bench_function(format!("sigil_signed/{}B", size), |b| {
            b.iter(|| {
                let mut h = SignedHasher::new(&sk, &pk);
                h.update(black_box(&msg));
                h.finalize().unwrap()
            });
        });
    }
    group.finish();
}

fn bench_sqisign_baselines(c: &mut Criterion) {
    let mut group = c.benchmark_group("sqisign_baseline");
    group.sample_size(10);

    group.bench_function("keygen", |b| {
        b.iter(|| flux_sqisign::keygen());
    });

    let (sk, pk) = flux_sqisign::keygen();
    let digest = [0u8; 32];
    group.bench_function("sign_32B_digest", |b| {
        b.iter(|| flux_sqisign::sign(black_box(&digest), &sk, &pk).unwrap());
    });

    let sig = flux_sqisign::sign(&digest, &sk, &pk).unwrap();
    group.bench_function("verify_32B_digest", |b| {
        b.iter(|| flux_sqisign::verify(black_box(&digest), &sig, &pk).unwrap());
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_keyed_hash,
    bench_key_derivation,
    bench_signed_hasher,
    bench_sqisign_baselines
);
criterion_main!(benches);
