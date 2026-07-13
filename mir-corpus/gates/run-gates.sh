#!/usr/bin/env bash
# FIP-0001 ladder rung 7 — trait-dispatch e2e gates.
#
# Each gate compiles a whole program through the REAL pipeline
# (rustc --emit=mir → parse_mir → normalize/monomorphize → Cranelift → cc link)
# via `fluxc run`, and the program's exit code IS the computed value. A wrong
# value (mis-dispatch, bad receiver flattening, broken devirtualization) fails
# the gate — this is outcome verification, not "it compiled".
#
#   static  : Sq{s:6}.area()              == 36   (rung 7 part 1)
#   generic : area_of::<Sq>(&Sq{s:5})     == 25   (rung 7 part 2, monomorphize)
#   dyn     : dyn_call(&Sq{s:4} as &dyn)  == 16   (rung 7 part 3, closed-world devirt)
#
#   ./run-gates.sh [path-to-fluxc]        # default: ../../target/debug/fluxc
set -uo pipefail
cd "$(dirname "$0")"
FLUXC="${1:-../../target/debug/fluxc}"
[ -x "$FLUXC" ] || { echo "fluxc not found/executable at $FLUXC" >&2; exit 2; }

rc=0
check() { # $1=gate file  $2=expected exit code
  # fluxc run caches objects by source BLAKE3 in $TMPDIR/flux_jit — use a
  # private TMPDIR so gates always exercise the current binary, not a cache.
  local tmp; tmp="$(mktemp -d)"
  TMPDIR="$tmp" "$FLUXC" run "$1" >/dev/null 2>&1
  local got=$?
  rm -rf "$tmp"
  if [ "$got" -eq "$2" ]; then
    echo "ok: $1 == $2"
  else
    echo "GATE FAIL: $1 exited $got, expected $2" >&2
    rc=1
  fi
}

check gate_static.rs  36
check gate_generic.rs 25
check gate_dyn.rs     16
exit $rc
