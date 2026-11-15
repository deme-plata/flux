#!/bin/bash
# ⚡ Flux — AI-Native Build Orchestrator · one-line setup
#
#   curl -fsSL https://fluxapp.xyz/setup-flux.sh | bash
#   curl -fsSL https://quillon.xyz/setup-flux.sh | bash     (mirror)
#
# Installs the Flux toolchain and wires its MCP into your AI client
# (Claude Code / Cursor / Codex). Verifies the MCP handshake before it
# tells you it's done. After setup, just say:
#   "compile this crate with flux"
#   "rent a GPU box on vast and run a chronos sim"
#   "spawn a 100-node swarm and measure throughput"
#   "sign this build with a provenance proof"
set -e

# ── palette ──────────────────────────────────────────────────────────────
B=$'\033[1m'; D=$'\033[2m'; R=$'\033[0m'
VIO=$'\033[38;5;141m'; GLD=$'\033[38;5;220m'; GRN=$'\033[38;5;43m'; CYN=$'\033[38;5;51m'

banner() {
cat <<BANNER

   ${VIO}${B}⚡  F L U X${R}
   ${D}════════════════════════════════════════════════════${R}
   ${B}AI-native build orchestrator${R} ${D}+ agent compute fabric${R}
   ${D}fluxc compiler · 180+ MCP tools · provenance proofs${R}
   ${D}════════════════════════════════════════════════════${R}

BANNER
}
banner

echo "  ${B}What this installs:${R}"
echo "    ${GLD}•${R} ${B}fluxc${R}     — MIR-direct Rust compiler · content-hash cache · ${VIO}.proof${R} signing"
echo "    ${GLD}•${R} ${B}MCP tools${R} — build/test/bench · chronos network sims · ${CYN}node swarms${R}"
echo "                 ${CYN}Vast.ai GPU gateway${R} · version control · ZK 10ms verify"
echo ""

# ── Windows (Git-Bash/MSYS) → use WSL ────────────────────────────────────
case "$(uname -s 2>/dev/null)" in
  MINGW*|MSYS*|CYGWIN*)
    echo "  ⚠ Detected Git Bash / MSYS on Windows."
    echo "  Flux runs on Windows via WSL. In a WSL (Ubuntu) terminal, run:"
    echo "      ${B}curl -fsSL https://fluxapp.xyz/setup-flux.sh | bash${R}"
    exit 1 ;;
esac

OS="$(uname -s)"
FLUX_HOME="$HOME/.flux"
SRC_DIR="$FLUX_HOME/src/flux"
mkdir -p "$FLUX_HOME"

# ── 1. detect AI clients ─────────────────────────────────────────────────
HAS_CLAUDE=0; HAS_CURSOR=0; HAS_CODEX=0
command -v claude &>/dev/null && { HAS_CLAUDE=1; echo "  ${GRN}✓${R} Claude Code found"; }
{ [ -d "$HOME/.cursor" ] || command -v cursor &>/dev/null; } && { HAS_CURSOR=1; echo "  ${GRN}✓${R} Cursor found"; }
{ [ -f "$HOME/.codex/config.toml" ] || command -v codex &>/dev/null; } && { HAS_CODEX=1; echo "  ${GRN}✓${R} Codex found"; }
if [ $((HAS_CLAUDE+HAS_CURSOR+HAS_CODEX)) -eq 0 ]; then
  echo "  ${B}No AI client detected.${R} Install one, then re-run:"
  echo "    Claude Code:  https://claude.com/claude-code"
  echo "    Cursor:       https://cursor.sh"
  echo "  (Continuing — fluxc will still be installed; configure the MCP later.)"
fi
echo ""

# ── fast path: prefer the prebuilt musl binary (x86_64 Linux) — ~2s vs a 10-30min source build ──
GOT_PREBUILT=0
ARCH="$(uname -m 2>/dev/null)"
if [ "$OS" = "Linux" ] && { [ "$ARCH" = "x86_64" ] || [ "$ARCH" = "amd64" ]; }; then
  echo "  ${B}[1/2]${R} Fetching prebuilt fluxc ${D}(musl static — no build, ~2s)${R}…"
  mkdir -p "$FLUX_HOME/bin"
  # 2026-10-23: installed ONLY if the signed release manifest names it — its Ed25519 signature is
  # checked against the Flux release key pinned below, then the binary's sha256 against the
  # manifest. Any failure → source build. (Before this the binary was fetched with no check.)
  FLUX_RELEASE_PUB=e9b6aecec3531889a49d39e148663187b3bf728ec178429aa75c50580d20d78a
  MF_URL="https://quillon.xyz/downloads/fluxc-latest.json"
  PT="$(mktemp -d)"
  hex2bin() { printf "$(printf '%s' "$1" | sed 's/../\\x&/g')"; }
  verify_ed25519() { # <body-file> <sig-hex-file> <pubkey-hex>
    if command -v python3 >/dev/null 2>&1 && python3 -c 'import cryptography' 2>/dev/null; then
      python3 -c 'import sys; from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey as K; K.from_public_bytes(bytes.fromhex(sys.argv[3])).verify(bytes.fromhex(open(sys.argv[2]).read().strip()), open(sys.argv[1],"rb").read())' "$1" "$2" "$3"
    elif openssl version 2>/dev/null | grep -qE '^OpenSSL [3-9]'; then
      hex2bin "302a300506032b6570032100$3" > "$PT/pub.der" && hex2bin "$(tr -d ' \n' < "$2")" > "$PT/sig.bin" \
        && openssl pkeyutl -verify -pubin -keyform DER -inkey "$PT/pub.der" -rawin -in "$1" -sigfile "$PT/sig.bin" >/dev/null
    else
      echo "    ${D}(no python3 'cryptography' and no OpenSSL 3 — cannot check the release signature)${R}"; return 1
    fi
  }
  if curl -fsSL "$MF_URL" -o "$PT/m.json" && curl -fsSL "$MF_URL.sig" -o "$PT/m.sig" \
     && verify_ed25519 "$PT/m.json" "$PT/m.sig" "$FLUX_RELEASE_PUB" 2>/dev/null; then
    BIN_URL=$(sed -n 's/^ *"url": *"\([^"]*\)".*/\1/p' "$PT/m.json" | head -1)
    BIN_SHA=$(sed -n 's/^ *"sha256_hex": *"\([0-9a-f]\{64\}\)".*/\1/p' "$PT/m.json" | head -1)
    if [ -n "$BIN_URL" ] && [ -n "$BIN_SHA" ] && curl -fsSL "$BIN_URL" -o "$PT/fluxc" \
       && echo "$BIN_SHA  $PT/fluxc" | sha256sum -c --quiet - >/dev/null 2>&1 \
       && chmod +x "$PT/fluxc" && "$PT/fluxc" version >/dev/null 2>&1 && mv -f "$PT/fluxc" "$FLUX_HOME/bin/fluxc"; then
      FLUXC="$FLUX_HOME/bin/fluxc"; GOT_PREBUILT=1
      echo "  ${GRN}✓${R} signed release verified (key ${FLUX_RELEASE_PUB:0:8}…) · $("$FLUXC" version 2>/dev/null | head -c 60) ${D}($(du -h "$FLUXC" 2>/dev/null | cut -f1))${R} → ${VIO}$FLUXC${R}"
    else
      echo "    ${D}(prebuilt binary missing or does not match the signed manifest — falling back to source build)${R}"
    fi
  else
    echo "    ${D}(release manifest unsigned or signature invalid — falling back to source build)${R}"
  fi
  rm -rf "$PT"
fi

# ── source-build fallback (only if the prebuilt path didn't land) ──────────
if [ "$GOT_PREBUILT" != "1" ]; then

# ── 2. build prerequisites ───────────────────────────────────────────────
echo "  ${B}[1/4]${R} Build prerequisites…"
if [ "$OS" = "Linux" ] && command -v apt-get &>/dev/null; then
  SUDO=""; [ "$(id -u)" -ne 0 ] && SUDO="sudo"
  $SUDO apt-get update -qq >/dev/null 2>&1 || true
  $SUDO apt-get install -y -qq build-essential pkg-config libssl-dev libudev-dev clang cmake curl git >/dev/null 2>&1 || \
    echo "    ${D}(apt step skipped — install build-essential/libssl-dev/clang manually if the build fails)${R}"
elif [ "$OS" = "Darwin" ]; then
  command -v brew &>/dev/null && brew install pkg-config openssl cmake >/dev/null 2>&1 || true
  xcode-select -p >/dev/null 2>&1 || xcode-select --install 2>/dev/null || true
fi

# ── 3. Rust toolchain ────────────────────────────────────────────────────
if ! command -v cargo &>/dev/null; then
  echo "  ${B}[2/4]${R} Installing Rust (rustup)…"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y >/dev/null 2>&1
  source "$HOME/.cargo/env"
else
  echo "  ${B}[2/4]${R} ${GRN}✓${R} Rust $(rustc --version 2>/dev/null | awk '{print $2}')"
fi
source "$HOME/.cargo/env" 2>/dev/null || true

# ── 4. fetch + compile fluxc from source ─────────────────────────────────
# The tarball ships BOTH the flux and sigil trees (flux/ has ../sigil path deps).
echo "  ${B}[3/4]${R} Fetching Flux source ${D}(flux + sigil trees)${R}…"
curl -fsSL "https://fluxapp.xyz/downloads/flux-src.tar.gz" -o /tmp/flux-src.tar.gz
mkdir -p "$FLUX_HOME/src"
tar xzf /tmp/flux-src.tar.gz -C "$FLUX_HOME/src" && rm -f /tmp/flux-src.tar.gz
echo "      source → $SRC_DIR"

echo "  ${B}[4/4]${R} Compiling fluxc ${D}(first build pulls libp2p + ZK + PQ-crypto — 10–30 min, grab coffee)${R}…"
( cd "$SRC_DIR" && cargo build --release -p fluxc </dev/null 2>&1 | tail -1 )
FLUXC="$SRC_DIR/target/release/fluxc"
if [ ! -x "$FLUXC" ]; then
  echo "  ${B}✗ build failed.${R} Re-run inside $SRC_DIR with: cargo build --release -p fluxc"
  exit 1
fi
# expose the built binary at the canonical path too
mkdir -p "$FLUX_HOME/bin" && ln -sf "$FLUXC" "$FLUX_HOME/bin/fluxc"
echo "  ${GRN}✓${R} fluxc built → ${VIO}$($FLUXC version 2>/dev/null | head -c 60)${R}"
echo ""

fi   # end source-build fallback

# ── 5. PATH — make `fluxc` a real command in new shells ──────────────────
PATH_LINE='export PATH="$HOME/.flux/bin:$PATH"'
for RC in "$HOME/.bashrc" "$HOME/.zshrc"; do
  [ -f "$RC" ] || continue
  grep -qs '\.flux/bin' "$RC" || echo "$PATH_LINE" >> "$RC"
done
export PATH="$FLUX_HOME/bin:$PATH"
echo "  ${GRN}✓${R} PATH: ~/.flux/bin wired into your shell rc ${D}(new shells get \`fluxc\` directly)${R}"

# ── 6. wire the MCP into each client ─────────────────────────────────────
configure_json() { # $1=config file  $2=client label
  node -e "
    const fs=require('fs'),p='$1';
    let c={}; try{c=JSON.parse(fs.readFileSync(p,'utf8'))}catch(e){}
    c.mcpServers=c.mcpServers||{};
    c.mcpServers.flux={command:'$FLUXC',args:['mcp']};
    fs.mkdirSync(require('path').dirname(p),{recursive:true});
    fs.writeFileSync(p,JSON.stringify(c,null,2));
  " 2>/dev/null && echo "  ${GRN}✓${R} $2 configured" || echo "  ${D}(could not auto-configure $2; add fluxc mcp manually)${R}"
}

if [ $HAS_CLAUDE -eq 1 ]; then
  if claude mcp add-json flux "{\"command\":\"$FLUXC\",\"args\":[\"mcp\"]}" >/dev/null 2>&1; then
    echo "  ${GRN}✓${R} Claude Code configured (claude mcp add)"
  else
    configure_json "$HOME/.claude/settings.json" "Claude Code"
  fi
fi
[ $HAS_CURSOR -eq 1 ] && configure_json "$HOME/.cursor/mcp.json" "Cursor"
if [ $HAS_CODEX -eq 1 ]; then
  CF="$HOME/.codex/config.toml"; mkdir -p "$HOME/.codex"
  grep -q "\[mcp_servers.flux\]" "$CF" 2>/dev/null || printf '\n[mcp_servers.flux]\ncommand = "%s"\nargs = ["mcp"]\n' "$FLUXC" >> "$CF"
  echo "  ${GRN}✓${R} Codex configured ($CF)"
fi

# ── 7. MCP smoke test — prove the server answers BEFORE claiming success ──
echo ""
echo "  ${B}[verify]${R} MCP handshake…"
MCP_PROBE='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"setup-flux","version":"1.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
MCP_OUT="$(printf '%s\n' "$MCP_PROBE" | timeout 20 "$FLUXC" mcp 2>/dev/null || true)"
MCP_VER="$(printf '%s' "$MCP_OUT" | grep -o '"version":"[0-9.]*"' | head -1 | cut -d'"' -f4)"
MCP_TOOLS="$(printf '%s' "$MCP_OUT" | grep -o '"name":"flux_' | wc -l | tr -d ' ')"
if [ -n "$MCP_VER" ] && [ "${MCP_TOOLS:-0}" -gt 0 ]; then
  echo "  ${GRN}✓ MCP OK${R} — flux-mcp v$MCP_VER answering with ${B}$MCP_TOOLS tools${R}"
else
  echo "  ${B}✗ MCP handshake failed.${R} Try manually: $FLUXC mcp"
  echo "    (then restart your AI client and check its MCP panel)"
fi

# ── 8. Flux dev skills (SKILL.md packs — flux-dev, flux-platform, …) ──
echo "  ${B}[skills]${R} Installing Flux dev skills…"
if curl -fsSL "https://fluxapp.xyz/flux-skills.tar.gz" -o /tmp/flux-skills.tar.gz 2>/dev/null; then
  INSTALLED=0
  for BASE in "$HOME/.claude" "$HOME/.grok" "$HOME/.cursor" "$HOME/.codex"; do
    [ -d "$BASE" ] || continue
    mkdir -p "$BASE/skills" && tar xzf /tmp/flux-skills.tar.gz -C "$BASE/skills" 2>/dev/null && \
      { echo "  ${GRN}✓${R} skills → ${VIO}$BASE/skills${R}"; INSTALLED=1; }
  done
  if [ $INSTALLED -eq 0 ]; then
    mkdir -p "$HOME/.claude/skills" && tar xzf /tmp/flux-skills.tar.gz -C "$HOME/.claude/skills" 2>/dev/null && \
      echo "  ${GRN}✓${R} skills → ~/.claude/skills"
  fi
  rm -f /tmp/flux-skills.tar.gz
else
  echo "  ${D}(skills fetch skipped — later: curl -fsSL https://fluxapp.xyz/flux-skills.tar.gz | tar xz -C ~/.claude/skills)${R}"
fi

# ── welcome / next steps ─────────────────────────────────────────────────
cat <<DONE

   ${GLD}${B}⚡ Flux is ready.${R}  ${D}Restart your AI client so the MCP loads.${R}

   ${B}First words in your AI client:${R}
     ${VIO}"run flux_mcp_status"${R}                     ${D}→ health check + fixes${R}
     ${VIO}"load the flux-dev skill"${R}                 ${D}→ the operational guide${R}
     ${VIO}"compile this crate with flux"${R}            ${D}→ MIR-direct build + cache${R}
     ${VIO}"run a 100-node chronos sim"${R}              ${D}→ deterministic network test${R}

   ${B}Stay current:${R}  ${CYN}fluxc self-update${R}   ${D}(pulls the latest prebuilt)${R}
   ${B}Docs:${R}   https://fluxapp.xyz/garden.html      ${D}(live Compile Garden)${R}
   ${B}fluxc:${R}  $FLUXC $([ -n "$MCP_VER" ] && echo "· v$MCP_VER · MCP verified")

DONE
