#!/bin/bash
# combo-regression.sh — lock in `fluxc combo` GREEN + fast + scope-clean.
#
# Guards against the three bugs we hit getting the combo to <10s:
#   1. fast-FAILURES counted as speed   → GREEN gate (verdict GREEN + passed>=N)
#   2. over-scoping (builds flux-p2p…)  → SCOPE gate (0 unrelated crates compiled)
#   3. silent slowdown                  → SPEED gate (median warm total_ms <= cap)
#
# Run before any release, and after any change to combo_v2.rs or the `fluxc test`
# cargo invocation:   bash scripts/combo-regression.sh
#
# Targets live on /home/storage (80TB) — NEVER /tmp (Epsilon root is 40GB).
set -u

# ── tunable thresholds ────────────────────────────────────────────────────
CRATE="${CRATE:-flux-swarm-secret}"
RUNS="${RUNS:-20}"
MAX_MEDIAN_MS="${MAX_MEDIAN_MS:-800}"
MIN_PASSED="${MIN_PASSED:-25}"
UNRELATED="${UNRELATED:-flux-p2p}"     # this crate must NOT be compiled for $CRATE
TGT="${TGT:-/home/storage/combo-reg-tgt}"

cd "$(dirname "$0")/.." || exit 2
FLUXC="./target/debug/fluxc"
[ -x "$FLUXC" ] || { echo "FAIL: $FLUXC not built"; exit 2; }

# clear any D-state find/du metadata storm (it stalls cargo in uninterruptible I/O)
for p in $(pgrep -x find 2>/dev/null) $(pgrep -x du 2>/dev/null); do
  [ "$p" != "$$" ] && kill -9 "$p" 2>/dev/null
done

export CARGO_TARGET_DIR="$TGT"
mkdir -p "$TGT"

# ── SCOPE gate: a FRESH-target combo must not compile $UNRELATED ───────────
rm -rf "$TGT"
SCOPE_LOG="$(mktemp /home/storage/combo-reg-scope.XXXXXX)"
"$FLUXC" combo "$CRATE" --json > "$SCOPE_LOG" 2>&1 </dev/null
# grep -c always prints a count (0 on no match); don't append a second 0.
SCOPE_HITS=$(grep -c "Compiling ${UNRELATED} " "$SCOPE_LOG" 2>/dev/null)
SCOPE_HITS=${SCOPE_HITS:-0}
rm -f "$SCOPE_LOG"
if [ "${SCOPE_HITS:-0}" -ne 0 ]; then
  echo "FAIL [scope]: combo $CRATE compiled $UNRELATED ${SCOPE_HITS}x — over-scoping regressed"; exit 1
fi

# ── warm up (the cold seed is excluded from timing) ───────────────────────
"$FLUXC" combo "$CRATE" --json >/dev/null 2>&1

# ── GREEN + SPEED gates over $RUNS warm runs ──────────────────────────────
times=()
for i in $(seq 1 "$RUNS"); do
  J=$("$FLUXC" combo "$CRATE" --json 2>/dev/null)
  verdict=$(printf '%s' "$J" | grep -oE '"verdict": *"[A-Z]+"' | grep -oE '[A-Z]+$' | head -1)
  passed=$(printf '%s'  "$J" | grep -oE '"passed": *[0-9]+'   | grep -oE '[0-9]+'  | head -1)
  failed=$(printf '%s'  "$J" | grep -oE '"failed": *[0-9]+'   | grep -oE '[0-9]+'  | head -1)
  ms=$(printf '%s'      "$J" | grep -oE '"total_ms": *[0-9]+' | grep -oE '[0-9]+'  | head -1)
  if [ "${verdict:-RED}" != "GREEN" ] || [ "${passed:-0}" -lt "$MIN_PASSED" ] || [ "${failed:-1}" -ne 0 ]; then
    echo "FAIL [green]: run $i verdict=${verdict:-?} passed=${passed:-?} failed=${failed:-?} (expected GREEN, >=$MIN_PASSED passed, 0 failed)"; exit 1
  fi
  times+=("${ms:-999999}")
done

# median + p95 + combos/sec
sorted=$(printf '%s\n' "${times[@]}" | sort -n)
n=${#times[@]}
median=$(printf '%s\n' "$sorted" | awk -v n="$n" 'NR==int((n+1)/2){print; exit}')
p95=$(printf '%s\n' "$sorted" | awk -v n="$n" 'NR==int(n*0.95+0.5){print; exit}')
sum=0; for t in "${times[@]}"; do sum=$((sum + t)); done
cps=$(awk -v s="$sum" -v n="$n" 'BEGIN{ if(s>0) printf "%.2f", n/(s/1000.0); else print "inf" }')

if [ "${median:-999999}" -gt "$MAX_MEDIAN_MS" ]; then
  echo "FAIL [speed]: median ${median}ms > cap ${MAX_MEDIAN_MS}ms (p95 ${p95}ms, ${cps} combos/sec)"; exit 1
fi

echo "COMBO REGRESSION: PASS — crate=$CRATE runs=$RUNS median ${median}ms p95 ${p95}ms ${cps} combos/sec, scope clean (0x $UNRELATED), all GREEN >=$MIN_PASSED passed"
exit 0
