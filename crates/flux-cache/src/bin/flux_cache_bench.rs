//! Hand-rolled microbench for flux-cache.
//!
//! Why not criterion? The cost of the benchmark machinery dwarfs what we're
//! measuring (sub-millisecond LRU lookups). A coarse-grained "do N operations
//! and divide" gives the right signal with less setup.
//!
//! Measures:
//!   1. cold store          — bincode encode + atomic_write + LRU put
//!   2. mem-LRU hit          — best case, no I/O
//!   3. mmap'd disk hit      — LRU miss, .bin file read + bincode decode
//!   4. current_disk_bytes   — full walk of cache dir (cap-eviction cost)
//!   5. legacy JSON fallback — pre-v0.11 entry read
//!
//! Run:  cargo run --release --bin flux-cache-bench -p flux-cache

use std::collections::HashMap;
use std::time::Instant;

use flux_cache::{
    self, current_disk_bytes, lookup, set_disk_capacity, stats_v11, store, CacheEntry,
};

fn fresh_entry(tag: u64) -> CacheEntry {
    let mut outputs = HashMap::new();
    outputs.insert("rmeta".into(), format!("target/release/deps/lib{tag}-abcdef.rmeta"));
    outputs.insert("rlib".into(), format!("target/release/deps/lib{tag}-abcdef.rlib"));
    CacheEntry {
        source_hash: format!("blake3-source-{tag:016x}"),
        args_hash: format!("blake3-args-{tag:016x}"),
        outputs,
        rustc_version: "rustc 1.85.0 (a28077b28 2025-01-07)".into(),
        timestamp: tag,
    }
}

fn time<F: FnOnce()>(label: &str, n: usize, f: F) -> f64 {
    let t0 = Instant::now();
    f();
    let total_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let per_op_us = (total_ms * 1000.0) / n as f64;
    println!("  {label:<40} {total_ms:>8.2} ms total   {per_op_us:>8.2} µs/op  ({n} ops)");
    per_op_us
}

fn main() {
    println!("flux-cache microbenchmark v1");
    println!("============================\n");

    // Use a private bench prefix so we don't trample real caches.
    let prefix = "_bench_flux_cache_v11_";
    let n = 5_000;
    let keys: Vec<String> = (0..n).map(|i| format!("{prefix}{i:08x}_abcdef0123456789abcdef")).collect();

    // Disable cap during pure store/lookup measurement.
    set_disk_capacity(0);

    println!("phase 1: cold store ({n} fresh entries)");
    let store_us = time("  store(key, entry)", n, || {
        for (i, k) in keys.iter().enumerate() {
            store(k, &fresh_entry(i as u64));
        }
    });

    println!("\nphase 2: mem-LRU hit (all keys still in 4096-cap LRU)");
    let mem_keys = &keys[(n.saturating_sub(4096))..];
    let mem_us = time("  lookup(key) [warm LRU]", mem_keys.len(), || {
        for k in mem_keys {
            let _ = lookup(k);
        }
    });

    println!("\nphase 3: disk-hit (cold LRU; keys not in mem)");
    // Force LRU clear by filling it with throwaway keys.
    for i in 0..4096 {
        let k = format!("__lru_evict__{i:08x}");
        store(&k, &fresh_entry(0));
    }
    // Now the original keys are out of LRU but still on disk.
    let disk_keys = &keys[..1000];
    let disk_us = time("  lookup(key) [LRU miss → disk]", disk_keys.len(), || {
        for k in disk_keys {
            let _ = lookup(k);
        }
    });

    println!("\nphase 4: cache-capacity walk");
    let walk_us = time("  current_disk_bytes()", 50, || {
        for _ in 0..50 {
            let _ = current_disk_bytes();
        }
    });
    let bytes_now = current_disk_bytes();
    println!("  → current_disk_bytes = {} KiB", bytes_now / 1024);

    println!("\nphase 5: stats snapshot");
    let stats_us = time("  stats_v11()", 100_000, || {
        for _ in 0..100_000 {
            let _ = stats_v11();
        }
    });

    println!("\n— summary —");
    let (mem_hits, disk_hits, misses, stores, _ttl, disk_evict, total_bytes) = stats_v11();
    println!("  stats: mem_hits={mem_hits}  disk_hits={disk_hits}  misses={misses}");
    println!("         stores={stores}  disk_evictions={disk_evict}  bytes={total_bytes}");
    println!();
    println!("  ratio disk-hit / mem-hit  = {:.1}×",  disk_us / mem_us.max(0.001));
    println!("  ratio store    / mem-hit  = {:.1}×",  store_us / mem_us.max(0.001));
    println!("  walk cost is {:.1}× a single mem hit", walk_us / mem_us.max(0.001));
    println!("  stats_v11() throughput:    {:.0} M/s", 1.0 / (stats_us / 1000.0));

    // Cleanup.
    println!("\ncleaning up bench prefix…");
    flux_cache::clean();
    println!("done.");
}
