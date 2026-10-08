use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};
use flux_irontunnel::{
    crypto::CryptoEngine,
    zk::ZkEngine,
    wireguard::generate_keypair,
};
use tokio::runtime::Runtime;

fn crypto_benchmarks(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    c.bench_function("crypto_engine_creation", |b| {
        b.iter(|| {
            let _crypto = CryptoEngine::new().unwrap();
        })
    });
    
    c.bench_function("ephemeral_keypair_generation", |b| {
        b.iter(|| {
            let _keypair = CryptoEngine::generate_ephemeral_keypair();
        })
    });
    
    let mut group = c.benchmark_group("packet_encryption");
    for size in [64, 256, 1024, 4096, 16384].iter() {
        group.bench_with_input(BenchmarkId::new("encrypt_decrypt", size), size, |b, &size| {
            let mut crypto = CryptoEngine::new().unwrap();
            let data = vec![0u8; size];
            
            // Set up session first
            rt.block_on(async {
                let (_, peer_public) = CryptoEngine::generate_ephemeral_keypair();
                let _exchange = crypto.perform_key_exchange(peer_public.as_bytes()).await.unwrap();
            });
            
            b.iter(|| {
                rt.block_on(async {
                    let encrypted = crypto.encrypt_packet(&data).await.unwrap();
                    let _decrypted = crypto.decrypt_packet(&encrypted).await.unwrap();
                })
            });
        });
    }
    group.finish();
}

fn zk_benchmarks(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    c.bench_function("zk_engine_creation", |b| {
        b.iter(|| {
            let _engine = ZkEngine::new().unwrap();
        })
    });
    
    c.bench_function("zk_identity_generation", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut engine = ZkEngine::new().unwrap();
                let _identity = engine.generate_identity().unwrap();
            })
        })
    });
    
    let mut group = c.benchmark_group("zk_proof_operations");
    for challenge_size in [32, 64, 128, 256].iter() {
        group.bench_with_input(BenchmarkId::new("proof_generation", challenge_size), challenge_size, |b, &size| {
            let mut engine = ZkEngine::new().unwrap();
            let identity = engine.generate_identity().unwrap();
            let challenge = vec![0xAA; size];
            
            b.iter(|| {
                let _proof = engine.create_proof(&identity, &challenge).unwrap();
            });
        });
    }
    group.finish();
    
    // Benchmark proof verification
    let mut engine = ZkEngine::new().unwrap();
    let identity = engine.generate_identity().unwrap();
    let challenge = b"benchmark_challenge";
    let proof = engine.create_proof(&identity, challenge).unwrap();
    
    c.bench_function("zk_proof_verification", |b| {
        b.iter(|| {
            let _verified = engine.verify_proof(&proof, &identity.commitment).unwrap();
        })
    });
}

fn wireguard_benchmarks(c: &mut Criterion) {
    c.bench_function("wireguard_keypair_generation", |b| {
        b.iter(|| {
            let _keypair = generate_keypair().unwrap();
        })
    });
}

criterion_group!(benches, crypto_benchmarks, zk_benchmarks, wireguard_benchmarks);
criterion_main!(benches);