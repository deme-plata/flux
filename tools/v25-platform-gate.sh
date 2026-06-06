#!/bin/bash
# v0.25 platform gate — MCP combo-only (Aether + fleet search + compile_error + promote)
set -euo pipefail
FLUX=/home/storage/deepseek-codewhale/flux
CARGO=/usr/local/bin/flux-cargo-wrapper
GATES=0
PASS=0

gate() {
  local name="$1"
  shift
  GATES=$((GATES + 1))
  echo "--- Gate $GATES: $name ---"
  if "$@"; then
    PASS=$((PASS + 1))
    echo "✓ PASS"
  else
    echo "✗ FAIL"
    exit 1
  fi
}

gate "fluxc-mcp unit tests" bash -c "
  cd '$FLUX' && $CARGO test -p fluxc-mcp 2>&1 | tail -5 | grep -q '0 failed'
"

gate "aether ingest+retrieve roundtrip" bash -c "
  cd '$FLUX' && $CARGO test -p fluxc-mcp ingest_retrieve_roundtrip -- --exact 2>&1 | grep -q 'ok'
"

gate "compile_error parser" bash -c "
  cd '$FLUX' && $CARGO test -p fluxc-mcp parse_loc_line -- --exact 2>&1 | grep -q 'ok'
"

gate "flux-aether crate tests" bash -c "
  cd '$FLUX' && $CARGO test -p flux-aether 2>&1 | tail -5 | grep -q '0 failed'
"

gate "flux-promote-gate + fluxc 0.25.x" bash -c "
  cd '$FLUX' && $CARGO test -p flux-promote-gate 2>&1 | tail -3 | grep -q '0 failed'
  ./target/debug/fluxc version | grep -E 'fluxc 0\\.25\\.'
"

echo ""
echo "=== v0.25 PLATFORM GATE: $PASS/$GATES GREEN ==="