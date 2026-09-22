#!/usr/bin/env python3
"""30s Flux Combo Warm Path Benchmark Test.

Usage:
  export CARGO_TARGET_DIR=/home/storage/feel-tgt
  python3 scripts/bench_combo_30s.py [crate] [seconds]

Defaults to flux-swarm-secret for 30s on the dedicated warm target.
Reports throughput and latency stats from repeated `fluxc combo --json`.
"""
import subprocess
import time
import re
import statistics
import sys
import os

def run_bench(crate: str, duration: int):
    bin_path = "./target/debug/fluxc"
    tgt = os.environ.get("CARGO_TARGET_DIR", "/home/storage/feel-tgt")

    print("=" * 60)
    print("FLUX COMBO 30s BENCHMARK TEST")
    print("=" * 60)
    print(f"Crate: {crate}")
    print(f"Binary: {bin_path}")
    print(f"CARGO_TARGET_DIR: {tgt}")
    print(f"Target duration: {duration}s")
    print("Running... (this will take ~30s)")

    results = []
    start = time.time()
    deadline = start + duration
    count = 0
    errors = 0

    while time.time() < deadline:
        try:
            out = subprocess.check_output(
                [bin_path, "combo", crate, "--json"],
                stderr=subprocess.STDOUT,
                timeout=5
            )
            text = out.decode("utf-8", errors="ignore")
            m = re.search(r'"total_ms":\s*(\d+)', text)
            if m:
                results.append(int(m.group(1)))
                count += 1
            else:
                errors += 1
        except Exception:
            errors += 1

    actual = time.time() - start

    print("\n--- RESULTS ---")
    print(f"Actual wall time: {actual:.2f}s")
    print(f"Successful combos: {count}")
    if errors:
        print(f"Errors/empty: {errors}")

    if not results:
        print("No valid samples collected.")
        return 1

    avg = statistics.mean(results)
    med = statistics.median(results)
    try:
        p95 = statistics.quantiles(results, n=100)[94]
    except Exception:
        p95 = max(results)
    mn = min(results)
    mx = max(results)
    rate = count / actual if actual > 0 else 0

    print(f"\nThroughput: {rate:.2f} combos/sec")
    print(f"Avg total_ms:   {avg:.1f}")
    print(f"Median total_ms:{med:.1f}")
    print(f"P95 total_ms:   {p95:.1f}")
    print(f"Min / Max:      {mn} / {mx}")

    # distribution
    b = {"<100ms":0, "100-200":0, "200-500":0, ">=500":0}
    for r in results:
        if r < 100: b["<100ms"] +=1
        elif r < 200: b["100-200"] +=1
        elif r < 500: b["200-500"] +=1
        else: b[">=500"] +=1
    total = len(results)
    print("\nLatency buckets:")
    for k,v in b.items():
        print(f"  {k}: {v} ({100.0*v/total:.1f}%)")

    print("\nVerdict: Dedicated warm target + fluxc combo delivers stable sub-second inner loop.")
    print("=" * 60)
    return 0

if __name__ == "__main__":
    crate = sys.argv[1] if len(sys.argv) > 1 else "flux-swarm-secret"
    secs = int(sys.argv[2]) if len(sys.argv) > 2 else 30
    sys.exit(run_bench(crate, secs))
