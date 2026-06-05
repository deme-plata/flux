#!/bin/bash
# ─────────────────────────────────────────────
# fluxfood — Enhanced Flux Dogfood Workflow
# ─────────────────────────────────────────────
# Replaces the basic dogfood sequence from the handoff.
# Runs: verify → diagnose → benchmark → optimize → deploy → report
#
# Usage: ./fluxfood.sh [--quick]
#   --quick  Skip benchmark (for fast iteration)
# ─────────────────────────────────────────────

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

BIN="./target/debug/fluxc"
MCP="timeout 10 $BIN mcp 2>/dev/null"
PASS=0
FAIL=0
TOTAL=0

green() { echo -e "\033[32m$1\033[0m"; }
red()   { echo -e "\033[31m$1\033[0m"; }
bold()  { echo -e "\033[1m$1\033[0m"; }

report() {
    local name="$1"; shift
    TOTAL=$((TOTAL + 1))
    if "$@"; then
        green "  ✓ $name"
        PASS=$((PASS + 1))
    else
        red "  ✗ $name"
        FAIL=$((FAIL + 1))
    fi
}

echo ""
bold "🥩 fluxfood v0.9.6 — Enhanced Dogfood Workflow"
echo "═══════════════════════════════════════════════"
echo ""

# ── Phase 1: Core Health ──
bold "1. CORE HEALTH"
report "Binary exists"        test -x "$BIN"
report "Version check"       $BIN version 2>&1 | grep -q "fluxc 0.9.6"

TOOL_COUNT=$(echo '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | $MCP | python3 -c "import sys,json; print(len(json.load(sys.stdin)['result']['tools']))" 2>/dev/null || echo "0")
if [ "$TOOL_COUNT" -ge 33 ]; then
    green "  ✓ MCP tools: $TOOL_COUNT (expected ≥ 33)"
    PASS=$((PASS + 1))
else
    red "  ✗ MCP tools: $TOOL_COUNT (expected ≥ 33)"
    FAIL=$((FAIL + 1))
fi
TOTAL=$((TOTAL + 1))
echo ""

# ── Phase 2: Systemd Services ──
bold "2. SYSTEMD SERVICES"
report "fluxc-serve unit exists"   test -f /etc/systemd/system/fluxc-serve.service
report "fluxc-benchdog unit exists" test -f /etc/systemd/system/fluxc-benchdog.service
report "fluxc-benchdog timer exists" test -f /etc/systemd/system/fluxc-benchdog.timer
echo ""

# ── Phase 3: Diagnostics ──
bold "3. DIAGNOSTICS"
DIAG=$(echo '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"flux_diagnose","arguments":{"package":"fluxc"}}}' | timeout 15 $BIN mcp 2>/dev/null | python3 -c "import sys,json; print(json.load(sys.stdin)['result']['content'][0]['text'])" 2>/dev/null || echo "FAILED")
if echo "$DIAG" | grep -q "Architecture"; then
    green "  ✓ flux_diagnose returned architecture data"
    echo "$DIAG" | head -4
    PASS=$((PASS + 1))
else
    red "  ✗ flux_diagnose failed"
    FAIL=$((FAIL + 1))
fi
TOTAL=$((TOTAL + 1))
echo ""

# ── Phase 4: Tests ──
bold "4. TESTS (quick: flux-search + flux-science only)"
TESTS=$(echo '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"flux_test","arguments":{"package":"flux-search"}}}' | timeout 30 $BIN mcp 2>/dev/null | python3 -c "import sys,json; print(json.load(sys.stdin)['result']['content'][0]['text'][:200])" 2>/dev/null || echo "FAILED")
if echo "$TESTS" | grep -q "test "; then
    green "  ✓ flux-search tests ran"
    echo "     $(echo "$TESTS" | head -1)"
    PASS=$((PASS + 1))
else
    red "  ✗ flux-search tests failed"
    FAIL=$((FAIL + 1))
fi
TOTAL=$((TOTAL + 1))
echo ""

# ── Phase 5: Benchmark (skip with --quick) ──
if [ "${1:-}" != "--quick" ]; then
    bold "5. BENCHMARK (flux_benchmark — all crates, Q-Spec + X-Algo)"
    BM=$(echo '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"flux_benchmark","arguments":{"format":"text"}}}' | timeout 120 $BIN mcp 2>/dev/null | python3 -c "import sys,json; print(json.load(sys.stdin)['result']['content'][0]['text'][:600])" 2>/dev/null || echo "FAILED")
    if echo "$BM" | grep -q "CRATE\|HLTH"; then
        green "  ✓ flux_benchmark ran"
        echo "$BM" | head -20
        PASS=$((PASS + 1))
    else
        red "  ✗ flux_benchmark failed or timed out"
        FAIL=$((FAIL + 1))
    fi
    TOTAL=$((TOTAL + 1))
    echo ""
    
    bold "6. OPTIMIZATION PATH (flux_optimize)"
    OPT=$(echo '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"flux_optimize","arguments":{"limit":5}}}' | timeout 60 $BIN mcp 2>/dev/null | python3 -c "import sys,json; print(json.load(sys.stdin)['result']['content'][0]['text'][:500])" 2>/dev/null || echo "FAILED")
    if echo "$OPT" | grep -q "Optimization\|#"; then
        green "  ✓ flux_optimize returned suggestions"
        echo "$OPT" | head -15
        PASS=$((PASS + 1))
    else
        red "  ✗ flux_optimize failed"
        FAIL=$((FAIL + 1))
    fi
    TOTAL=$((TOTAL + 1))
else
    bold "5-6. BENCHMARK SKIPPED (--quick)"
    echo ""
fi
echo ""

# ── Phase 7: Dashboard + Deploy ──
bold "7. DEPLOY"
HTTP=$(curl -s -o /dev/null -w "%{http_code}" https://quillon.xyz/dashboard.html 2>/dev/null || echo "000")
if [ "$HTTP" = "200" ]; then
    green "  ✓ Dashboard HTTP $HTTP"
    PASS=$((PASS + 1))
else
    red "  ✗ Dashboard HTTP $HTTP"
    FAIL=$((FAIL + 1))
fi
TOTAL=$((TOTAL + 1))
echo ""

# ── Phase 8: Health Report ──
bold "8. HEALTH REPORT (JSON)"
HR=$(echo '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"flux_health_report","arguments":{"format":"json"}}}' | timeout 10 $BIN mcp 2>/dev/null | python3 -c "import sys,json; d=json.load(sys.stdin); r=json.loads(d['result']['content'][0]['text']); print(f\"score={r['health_score']:.1f}% crates={r['architecture']['crates']} loc={r['architecture']['loc']}\")" 2>/dev/null || echo "FAILED")
if echo "$HR" | grep -q "score="; then
    green "  ✓ Health report: $HR"
    PASS=$((PASS + 1))
else
    red "  ✗ Health report failed"
    FAIL=$((FAIL + 1))
fi
TOTAL=$((TOTAL + 1))
echo ""

# ── Phase 9: Auto-Tune ──
bold "9. AUTO-TUNE"
TUNE=$(echo '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"flux_tune","arguments":{"auto":true,"context":"benchmark and optimize the flux workspace"}}}' | timeout 10 $BIN mcp 2>/dev/null | python3 -c "import sys,json; print(json.load(sys.stdin)['result']['content'][0]['text'][:200])" 2>/dev/null || echo "FAILED")
if echo "$TUNE" | grep -q "Auto-Equip\|Loadout"; then
    green "  ✓ Auto-tune: $(echo "$TUNE" | head -1)"
    PASS=$((PASS + 1))
else
    red "  ✗ Auto-tune failed"
    FAIL=$((FAIL + 1))
fi
TOTAL=$((TOTAL + 1))
echo ""

# ── Summary ──
echo "═══════════════════════════════════════════════"
if [ "$FAIL" -eq 0 ]; then
    green "🥩 fluxfood complete: $PASS/$TOTAL checks passed"
else
    echo -e "\033[33m🥩 fluxfood: $PASS/$TOTAL passed, $FAIL failed\033[0m"
fi
echo ""

# Status line for webhook
echo "{\"fluxfood\":{\"passed\":$PASS,\"failed\":$FAIL,\"total\":$TOTAL,\"timestamp\":$(date +%s)}}"
