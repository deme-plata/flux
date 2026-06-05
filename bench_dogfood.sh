#!/bin/bash
set -e
BIN=./target/debug/fluxc
echo "═══════════════════════════════════════"
echo "  Flux v0.8.0 Dogfood Benchmark"
echo "  $(date)"
echo "═══════════════════════════════════════"

echo ""; echo "--- 1. COLD BUILD ---"; cargo clean -p fluxc --quiet 2>/dev/null; START=$(date +%s%3N); cargo build --package fluxc --quiet 2>&1; COLD=$(( $(date +%s%3N) - START )); echo "  Cold: ${COLD}ms"

echo ""; echo "--- 2. INCREMENTAL (no change) ---"; START=$(date +%s%3N); cargo build --package fluxc --quiet 2>&1; INCR1=$(( $(date +%s%3N) - START )); echo "  Incr(0): ${INCR1}ms"

echo ""; echo "--- 3. INCREMENTAL (touch) ---"; touch crates/fluxc/src/main.rs; START=$(date +%s%3N); cargo build --package fluxc --quiet 2>&1; INCR2=$(( $(date +%s%3N) - START )); echo "  Incr(1): ${INCR2}ms"

echo ""; echo "--- 4. TESTS ---"; START=$(date +%s%3N); cargo test --package flux-search --package flux-science --package flux-hotswap --package flux-db --quiet 2>&1; TESTS=$(( $(date +%s%3N) - START )); echo "  Tests: ${TESTS}ms"

echo ""; echo "--- 5. MCP ---"; printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"bench","version":"1"}}}\n{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"flux_version","arguments":{}}}\n{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"flux_bench","arguments":{"suite":"search"}}}\n' | timeout 5 $BIN mcp 2>/dev/null | grep -o '"text":"[^"]*"' | head -2 | sed 's/"text":"//;s/"//'

echo ""; echo "--- 6. STATS ---"; cat ~/.flux/stats.json 2>/dev/null | python3 -m json.tool 2>/dev/null || echo "  none"

SPD=$(( ${INCR2:-1} > 0 ? COLD / INCR2 : COLD ))
echo ""; echo "═══════════════════════════════════════"
echo "  Cold:${COLD}ms  Incr(0):${INCR1}ms  Incr(1):${INCR2}ms  Tests:${TESTS}ms  Speedup:${SPD}x"
echo "═══════════════════════════════════════"
