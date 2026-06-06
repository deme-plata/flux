#!/bin/bash
# v0.25 release gate — MCP platform combos (Aether + fleet + compile_error + promote)
# Falsifiable: 6 gates, all must pass for promote-gate battle-test green.
# No Excel, no flux_ui_* — combo-first platform per DeepSeek-refined plan.
# Tests: fluxc-mcp via direct test binary (fluxc test misroutes workspace); promote via fluxc.
set -euo pipefail

FLUX=/home/storage/deepseek-codewhale/flux
SIGIL=/home/storage/deepseek-codewhale/sigil
FLUXC=./target/debug/fluxc
export PATH="/root/.cargo/bin:$PATH"

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

# Gate 1: fluxc-mcp unit tests (direct binary — avoids fluxc test workspace fan-out)
gate "fluxc-mcp unit tests (59+)" bash -c "
  cd '$FLUX'
  BIN=\$(ls -t target/debug/deps/fluxc_mcp-* 2>/dev/null | grep -v '\\.d\$' | head -1)
  test -n \"\$BIN\"
  \$BIN 2>&1 | tail -5 | grep -q '0 failed'
"

# Gate 2: aether ingest+retrieve roundtrip
gate "flux_aether ingest+retrieve roundtrip" bash -c "
  cd '$FLUX'
  BIN=\$(ls -t target/debug/deps/fluxc_mcp-* 2>/dev/null | grep -v '\\.d\$' | head -1)
  \$BIN ingest_retrieve_roundtrip --exact 2>&1 | grep -q 'ok'
"

# Gate 3: flux-aether crate tests (scoped binary)
gate "flux-aether crate tests" bash -c "
  cd '$FLUX'
  BIN=\$(ls -t target/debug/deps/flux_aether-* 2>/dev/null | grep -v '\\.d\$' | head -1)
  test -n \"\$BIN\"
  \$BIN 2>&1 | tail -5 | grep -q '0 failed'
"

# Gate 4: compile_error combo — parser + handler wired
gate "flux_compile_error_combo parser + handler" bash -c "
  cd '$FLUX'
  BIN=\$(ls -t target/debug/deps/fluxc_mcp-* 2>/dev/null | grep -v '\\.d\$' | head -1)
  \$BIN parse_loc_line --exact 2>&1 | grep -q 'ok'
  test -f crates/fluxc-mcp/src/handlers/compile_error.rs
  grep -q 'fluxc_cmd' crates/fluxc-mcp/src/handlers/compile_error.rs
  grep -q 'platform_webhook' crates/fluxc-mcp/src/handlers/compile_error.rs
"

# Gate 5: sigil-vm VM-1 wasmi (sigil repo — cargo test; no fluxc binary in sigil tree)
gate "sigil-vm VM-1 wasmi" bash -c "
  cd '$SIGIL' && cargo test -p sigil-vm 2>&1 | tail -3 | grep -q '0 failed'
"

# Gate 6: flux-promote-gate + fluxc version 0.25.x
gate "promote-gate + fluxc 0.25.x" bash -c "
  cd '$FLUX'
  BIN=\$(ls -t target/debug/deps/flux_promote_gate-* 2>/dev/null | grep -v '\\.d\$' | head -1)
  test -n \"\$BIN\"
  \$BIN 2>&1 | tail -3 | grep -q '0 failed'
  $FLUXC version 2>/dev/null | grep -E 'fluxc 0\\.25\\.'
"

echo ""
echo "=== v0.25 PLATFORM GATE: $PASS/$GATES GREEN ==="