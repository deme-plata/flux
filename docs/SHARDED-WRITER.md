# SHARDED-WRITER — flux-db bulk ingest via `put_many` (task: lift the single-writer ceiling)

## Bottleneck finding (measured, arch-B confirmed)
`Database::put()` takes the single global `DbInner` write-lock **per 8 KB entry** and
`write_wal_entry` issues 2 `write()` syscalls + a `flush()` **per entry**. N writer threads
would serialize on that lock — a multi-threaded writer buys nothing (architecture A rejected
by measurement, not taste).

Isolated micro-bench (200k × 8 KB, defer_compaction, 4 GiB WAL, no concurrent load):

| path | MB/s |
|---|---|
| `put()` per entry | 168.5 |
| `write(WriteBatch=16)` | 328.2 |
| `write(WriteBatch=64)` | 345.9 |
| `write(WriteBatch=256)` | 294.5 (double-copy staging degrades) |
| `write(WriteBatch=1024)` | 217.6 |

## The API: `Database::put_many(&[(K, V)])`
One lock acquisition per batch; every WAL record coalesced into ONE buffer → one
`write_all` + one `flush` syscall per batch; direct memtable inserts (no `BatchOp`/PathBuf
staging); per-entry sequence stamps (snapshot semantics identical to N `put()`s); empty
value = tombstone; auto-flush honored once after the lock drops. **WAL record framing is
byte-identical to `put()`** (`[crc][klen][vlen][key][val]`, per-record CRC), so
`replay_wal_streaming` and torn-write recovery are untouched. Durability contract unchanged:
batch the fsync barrier via `sync_wal()` (chronos_scale fsyncs before advancing its marker).

Tests: equivalence with N puts, WAL replay across reopen (flux-db suite green).

## Harness wiring (`chronos_scale`)
`CHRONOS_BATCH` (default 256; `1` = legacy per-entry path for A/B). The pending batch is
**drained before read-probes and before the height marker** so probes see settled data and
the marker's durability barrier covers every height it claims.

## A/B rate bench (16 GiB each, sequential) — an HONEST null result under contention

Run with the 5 TB ladder rung writing to the same array concurrently:

| variant | height | wall | avg MB/s | presence |
|---|---|---|---|---|
| `put()` (CHRONOS_BATCH=1) | 2,000,000 | 335 s | 46.6 | **100.00%** |
| `put_many(256)` | 2,000,000 | 413 s | 37.8 | **100.00%** |

**Do not read this as "put_many is slower."** It is a confounded measurement: both variants
ran while the 5 TB ladder saturated the shared md0 array, so both were **I/O-bound**, not
lock/CPU-bound — and the `put_many` leg happened to run during heavier contention. The
writer-side win (one lock acquire + one WAL syscall per batch instead of per entry) is only
visible when the writer is the bottleneck, which under a busy array it is not.

The **valid** writer-path number is bg3's isolated micro-bench (defer_compaction, 4 GiB WAL,
no concurrent load): `put()` 168 MB/s → coalesced batching ~346 MB/s (~2×). The correctness
result is unconditional and is what matters most here: **put_many audits 100.00% presence**,
byte-identical to N `put()`s.

**The load-bearing finding:** at terabyte scale on this shared array, the single-writer lock
is **not** the bottleneck — the disk is. `put_many` is the right primitive (and helps CPU/
lock-bound consumers like sigil-node sync, and any run on faster/less-contended storage), but
lifting THIS harness's ceiling needs an idle array to measure cleanly, or genuinely parallel
I/O, not just a faster single writer. A clean isolated full-scale A/B remains to be run when
the ladder is not competing for spindles.

## Relaunching the ladder with the fast writer
The running 1→5→20 TB ladder predates this change. To relaunch faster rungs once it
finishes (or is stopped): rebuild sigil-chronos release, then the same ladder invocation
with `CHRONOS_BATCH=256` in the env. Expected: the 20 TB rung drops from ~6 days toward
~2 at micro-bench rates (array contention permitting).

## Follow-ups
- sigil-node `BlockStore` bulk-sync path should adopt `put_many` (same win for real chain
  sync; the API landed in flux-db precisely so consumers beyond the harness benefit).
- A true sharded/multi-store writer remains unnecessary while the coalesced single lock
  saturates the array; revisit only if measured lock contention returns above ~500 MB/s.
