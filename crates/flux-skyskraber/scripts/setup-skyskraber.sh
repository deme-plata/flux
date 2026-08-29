#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════════════════
#  Quillon Graph Skyskraber — digital-twin installer
#  curl -fsSL https://quillon.xyz/downloads/setup-skyskraber.sh | bash
#
#  Installs the `skyskraber` twin binary (linux x86_64) and shows you around.
#  Honest labeling: this installs the runnable digital twin (blueprint, the
#  deterministic day, the 8-endpoint API surface, the secret). MCP *server*
#  mode — these endpoints served live over MCP/HTTP — is a SPECIFIED lane,
#  not yet in this binary.
# ═══════════════════════════════════════════════════════════════════════════
set -euo pipefail

BASE="https://quillon.xyz/downloads"
DEST="${SKYSKRABER_HOME:-$HOME/.skyskraber}"
BIN="$DEST/bin/skyskraber"

echo "═══ Quillon Graph Skyskraber — twin installer ═══"
mkdir -p "$DEST/bin"

echo "▸ fetching twin binary…"
curl -fsSL "$BASE/skyskraber-linux-x86_64" -o "$BIN"
chmod +x "$BIN"

echo "▸ verifying the twin actually runs (not just downloads)…"
"$BIN" blueprint >/dev/null
echo "  ✓ blueprint validates"

echo "▸ fetching the whitepaper…"
curl -fsSL "$BASE/skyskraber-whitepaper.pdf" -o "$DEST/skyskraber-whitepaper.pdf" \
  && echo "  ✓ $DEST/skyskraber-whitepaper.pdf" \
  || echo "  (whitepaper fetch failed — non-fatal)"

echo
echo "═══ installed. the tower is yours to run ═══"
echo
echo "  $BIN blueprint          # the validated floor program (twin spires, vault, bank)"
echo "  $BIN day --seed 42      # live one deterministic day, BLAKE3-fingerprinted"
echo "  $BIN api                # the 8 compile-time-registered endpoints"
echo
echo "  add to PATH:  export PATH=\"$DEST/bin:\$PATH\""
echo
echo "  whitepaper:   $BASE/skyskraber-whitepaper.pdf"
echo "  source:       flux workspace, crates/flux-skyskraber (52 tests green)"
echo
echo "  …and towers keep secrets. this one answers to a word."
