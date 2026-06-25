#!/usr/bin/env bash
# FIP-0001 keep-A-open #2 — MIR-drift contract.
# Regenerate normalized MIR for every corpus .rs against the pinned rustc and diff it against the
# committed *.mir.expected baseline. Any drift in rustc's --emit=mir dialect (the dialect that
# flux-frontend::mir::parse_mir consumes) fails — turning the rustc frontend dependency into an
# enforced contract. The pinned version must match flux_driver::RUSTC_VERSION.
#
#   ./check.sh           # verify (CI): exit 1 on drift
#   ./check.sh --update  # regenerate the baselines (run on the pinned toolchain after an intended bump)
set -euo pipefail
cd "$(dirname "$0")"
mode="${1:-check}"
rc=0

# Normalize away noise that is not part of the MIR *shape* we parse:
# the leading "// MIR for ..." banner, absolute paths, and trailing whitespace.
normalize() { sed -E 's/[[:space:]]+$//; /^\/\/ MIR for/d; s#/[^ ]*/(rustc|toolchains)/[^ ]*#<path>#g'; }

for f in *.rs; do
  base="${f%.rs}"
  got="$(rustc --crate-type lib --emit=mir -o /dev/stdout "$f" 2>/dev/null | normalize)"
  if [ "$mode" = "--update" ]; then
    printf '%s\n' "$got" > "$base.mir.expected"
    echo "updated $base.mir.expected"
  else
    if ! diff -u "$base.mir.expected" <(printf '%s\n' "$got"); then
      echo "MIR DRIFT in $f vs pinned rustc — flux's parse_mir contract may be broken." >&2
      rc=1
    else
      echo "ok: $f"
    fi
  fi
done
exit $rc
