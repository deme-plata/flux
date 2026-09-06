#!/usr/bin/env bash
# prewarm-when-idle.sh — keep the SIGIL crates compile-warm, but ONLY when the box has
# cores to spare.
#
# The prewarm script itself has warned since 2026-09-02 that a busy box "will manufacture
# whatever conclusion you were hoping for": its own 48.8 s / 131 s / 138 s numbers were
# eight saturated cores, not a fingerprint effect. The same is true of the warming itself
# — prewarming a box that is already oversubscribed does not make anything warm, it just
# takes cores from whoever is using them and lands a cold reading in the cache stats.
#
# So this refuses to run rather than degrading. Measured on Epsilon 2026-09-06: 48 cores
# with 49.9 consumed by long-running processes, load 91-107. Under that, a prewarm is
# pure harm; the guard is the feature.
set -uo pipefail

# ── WHY NOT LOAD AVERAGE ──────────────────────────────────────────────────────
# The first version gated on /proc/loadavg < 12 and would have NEVER fired. Load counts
# every runnable task equally, including ones that yield the instant anything else wants
# the CPU. Measured on Epsilon 2026-09-06, load 52.66:
#
#   qli-worker (Qubic)  nice  0   ~18 cores   ← real competition
#   sigil-miner         nice  5    ~4 cores
#   sigil-rpcd          nice 19    ~2 cores   ← yields
#   q-api-server        nice 19    ~1 core    ← yields
#   rustc (this build)  nice 19              ← yields
#
# Half of that load is nice-19 batch work that the kernel preempts the moment a build
# asks for CPU. Blocking on it means never prewarming on a box that is, for practical
# purposes, available. So the gate counts only what actually competes: CPU consumed by
# processes at nice <= 0. That is the number that decides whether a prewarm will get
# scheduled or merely thrash.
COMPETING_MAX="${PREWARM_MAX_COMPETING_CORES:-34}"
SCRIPT="${PREWARM_SCRIPT:-/home/storage/deepseek-codewhale/sigil/scripts/prewarm-sigil.sh}"
LOG="${PREWARM_LOG:-/home/storage/sigil-scratch/prewarm-when-idle.log}"
LOCK="/tmp/prewarm-when-idle.lock"

load=$(cut -d' ' -f1 /proc/loadavg)
cores=$(nproc)
# Cores consumed by processes at normal-or-higher priority. `ps` %CPU is per-process and
# can exceed 100 on a multithreaded one, which is exactly what we want to add up.
# 🪤 `ps` reports %CPU as CPU-time / LIFETIME, so a process that has existed for two
# seconds and used two seconds of CPU reads as ~100 % — and a freshly spawned one can read
# far higher across threads. Measured here: counting everything gave 41.1 cores, of which
# 8 were processes younger than ten seconds, INCLUDING the `ps` doing the measuring. That
# is the difference between refusing (>34) and proceeding, so the guard would have refused
# on the strength of its own measurement. Only established processes count.
competing=$(ps -eo etimes,ni,pcpu --no-headers 2>/dev/null \
  | awk '$1 > 10 && $2 <= 0 { s += $3 } END { printf "%.1f", s/100 }')
competing="${competing:-0}"
ts=$(date -Is)

exec 9>"$LOCK" || exit 0
flock -n 9 || { echo "$ts skip: another prewarm holds the lock" >> "$LOG"; exit 0; }

# ONE BUILD OWNER AT A TIME. The nice<=0 test above deliberately ignores nice-19 work,
# because such work yields — but OUR OWN builds are nice-19 too, so the test happily let a
# prewarm start on top of a running release build. Observed 2026-09-06: a prewarm launched
# beside an ARM64 release cross-build (17 rustc), and the two simply halved each other.
# A build already in flight is the one thing a prewarm must never join.
if pgrep -x rustc >/dev/null 2>&1 || pgrep -f "cargo (build|check|test)" >/dev/null 2>&1; then
  echo "$ts skip: a build is already running — one build owner at a time" >> "$LOG"
  exit 0
fi

if awk "BEGIN{exit !($competing > $COMPETING_MAX)}"; then
  echo "$ts skip: ${competing} of ${cores} cores taken by nice<=0 work (limit ${COMPETING_MAX}; load ${load}) — a prewarm would only thrash" >> "$LOG"
  exit 0
fi
[ -x "$SCRIPT" ] || { echo "$ts skip: $SCRIPT not executable" >> "$LOG"; exit 0; }

echo "$ts run: ${competing}/${cores} cores competing (load ${load}) — warming" >> "$LOG"
# Niced hard: even when the box looks idle, a real build arriving mid-prewarm must win.
# WARM_S is left at the script's own default. I raised it to 10 on the theory that
# sigil-top's `lib` cell bottoms out at 9s and an 8s threshold marks it cold forever,
# burning a third pass every run. MEASURED, and the theory was wrong: with the threshold
# at 10 the run still took three passes, because the lib cells do not have a 9s floor —
# they landed at 16s and 13s that run. They vary with contention, and sit above both
# thresholds either way. Two observations of 9s were a favourable case, not a limit.
timeout 3600 ionice -c3 nice -n19 "$SCRIPT" >> "$LOG" 2>&1
echo "$ts done: rc=$? load now $(cut -d' ' -f1 /proc/loadavg)" >> "$LOG"
