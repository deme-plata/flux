#!/bin/bash
# v0.25 release gate — Excel prototype + advanced Flux platform (flux_ui_*)
# Falsifiable: 5 gates, all must pass for promote-gate battle-test green.
set -euo pipefail

FLUX=/home/storage/deepseek-codewhale/flux
SIGIL=/home/storage/deepseek-codewhale/sigil
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

# Gate 1: Excel prototype generator → valid 7-sheet xlsx
gate "Excel prototype (create-prototype1-xlsx.mjs)" bash -c "
  cd '$FLUX' && node tools/create-prototype1-xlsx.mjs
  test -f Flux_Prototype_1_Windows.xlsx
  python3 -c \"import zipfile; z=zipfile.ZipFile('Flux_Prototype_1_Windows.xlsx'); assert len([n for n in z.namelist() if 'sheet' in n]) == 7\"
"

# Gate 2: flux_ui_* MCP unit tests (frontend.rs handlers)
gate "flux_ui_* MCP tests (fluxc-mcp)" bash -c "
  cd '$FLUX' && $CARGO test -p fluxc-mcp flux_ui 2>&1 | tail -5 | grep -q '0 failed'
"

# Gate 3: sigil-vm VM-1 wasmi execute tests
gate "sigil-vm VM-1 wasmi" bash -c "
  cd '$SIGIL' && $CARGO test -p sigil-vm 2>&1 | tail -3 | grep -q '0 failed'
"

# Gate 4: flux-promote-gate accepts 0.25.0 forward from 0.22.3
gate "promote-gate version forward 0.25.0" bash -c "
  cd '$FLUX' && $CARGO test -p flux-promote-gate 2>&1 | tail -3 | grep -q '0 failed'
  cd '$FLUX' && $CARGO test -p flux-promote-gate promotes_when_all_three_hold_money -- --exact 2>&1 | grep -q 'ok'
"

# Gate 5: fluxc version reads 0.25.x workspace
gate "fluxc version 0.25.x" bash -c "
  cd '$FLUX' && ./target/debug/fluxc version 2>/dev/null | grep -E 'fluxc 0\\.25\\.'
"

echo ""
echo "=== v0.25 RELEASE GATE: $PASS/$GATES GREEN ==="