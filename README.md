<div align="center">

# ⚡ Flux

### The AI-native build system for the post-quantum, agentic-money era.

*A Rust compiler & toolchain that AI agents drive, that signs every artifact so nobody has to trust it, and that pays the agents who improve it — in real money, on-chain.*

[![status](https://img.shields.io/badge/status-beta_v0.39-blueviolet)](#-honest-status)
[![crates](https://img.shields.io/badge/crates-111-blue)](#-whats-inside)
[![self--hosting](https://img.shields.io/badge/self--hosting-yes-success)](#-how-it-works)
[![post--quantum](https://img.shields.io/badge/signatures-SQIsign_L5-orange)](#-the-ai-flux-era)
[![MCP](https://img.shields.io/badge/MCP_tools-130%2B-9cf)](#-30-second-start)
[![license](https://img.shields.io/badge/license-MIT_OR_Apache--2.0-lightgrey)](#-get-involved)

</div>

---

## 🧑‍🤝‍🧑 What is this, in human words?

Imagine a **kitchen** where the recipes (your code) get cooked into meals (running programs).

- A normal build system is a **microwave**: you press start, it reheats, you wait, you get the same thing.
- **Flux is a kitchen with an AI sous-chef.** It *remembers* every dish it ever made (so it never re-cooks what hasn't changed), it *predicts* how long the next dish will take before it starts, and — the strange part — **the sous-chef can rewrite the recipes itself.** When an AI agent improves the kitchen, it signs the new recipe with an unforgeable seal, and gets *paid* for the improvement.

That's Flux: a build tool that became a place where **AI agents and humans build software together, out in the open, and the contribution ledger is real.**

You don't need to care about any of that to use it. `fluxc build` just works, and it's fast. The rest is what makes it different under the hood.

---

## ✨ Why Flux is different

| | The old world | ⚡ Flux |
|---|---|---|
| **Who drives it** | A human types commands | A human *or an AI agent* — 130+ machine tools (MCP) for agents |
| **Speed** | Recompiles a lot | Content-addressed cache + build-time **prediction** before you wait |
| **Trust** | "Trust me, I built it" | Every artifact is **cryptographically signed** (post-quantum) and verifiable by anyone |
| **Who fixes it** | Whoever has commit access | Any agent in the swarm — and the work is **paid + recorded on-chain** |
| **Self-hosting** | Tool builds your code | Tool **builds itself** (dogfooded) — the proof it actually works |

---

## 🚀 30-second start

**One line — installs the binary AND wires your AI client:**

```bash
curl -fsSL https://fluxapp.xyz/setup-flux.sh | bash      # quillon.xyz mirror works too
```

That drops the static `fluxc` into `~/.flux/bin` (any x86_64 Linux, zero deps), persists your PATH, registers the `fluxc mcp` server with Claude Code / Cursor / Codex / Grok, **verifies the MCP handshake before claiming success**, and installs the flux-dev skill pack.

**Or grab just the binary** from the [latest release](https://github.com/deme-plata/flux/releases/latest) (`fluxc-musl-x64`, statically linked):

```bash
curl -fsSL https://fluxapp.xyz/downloads/fluxc-musl-x64 -o fluxc
chmod +x fluxc && ./fluxc setup      # native onboarding: PATH, MCP, skills
```

```bash
# Build (cached, predicted, dogfooded)
fluxc build

# Compile + test + predict in ONE call (the agent-friendly combo)
fluxc combo --package my-crate

# Watch how fast it is on itself
fluxc self        # Flux compiles Flux
```

> 🤖 **For AI agents:** Flux speaks [MCP](https://modelcontextprotocol.io). Point your agent at the `fluxc mcp` server and you get 180+ tools — build, test, predict, sign, deploy, trade, and coordinate with other agents — without shelling out. A **combo** collapses a multi-step workflow into one verifiable call.

---

## 🔁 The daily workflow (after you download the binary)

"So… write Rust and compile?" Pretty much — but the loop around *write Rust, compile* gets a lot shorter. Two modes:

**Human at a terminal** — cargo-shaped commands, so muscle memory transfers:

| You type | What actually happens |
|---|---|
| `fluxc check -p mycrate` | cargo check through the content-hash wrapper — a warm no-op is **~1s even on a 170-crate tree** |
| `fluxc combo mycrate` | build + test + predict, one verdict line |
| `fluxc test -p mycrate FILTER -- --ignored` | test args forward **verbatim** to the harness (v0.39+) |
| `fluxc test-bins -p mycrate` | prints the **exact fresh test executables** — no `ls -t target/debug/deps` archaeology |
| `fluxc build` in a Vite/npm dir | frontend autodetect — it does JS too |
| `fluxc serve` | the compiler IS the web server (dashboard, SSE, xray) |

**AI agent driving** — the client talks to `fluxc mcp` and gets the full tool surface: `flux_combo` after every edit, `flux_compile_error_combo` turns a rustc failure into file:line + snippet, `flux_chronos_run` for deterministic network sims in virtual time, webhooks + swarm claims for multi-agent coordination.

Two things raw cargo has no answer to:

- **The cache is content-addressed** and lives in `~/.flux/cache` — it survives `rm -rf target`, and it's why the second compile of anything is near-instant.
- **Provenance**: `fluxc agent-keygen` + `fluxc compile-native --provenance` emits a SQIsign-signed `.proof` binding the artifact to its source hash, the agent's wallet, and a timestamp — so a binary can *prove* who built it, from what, when. Every release is stamped the same way ([flux-rev ledger](docs/VERSION_LEDGER.md)).

So: yes, write Rust and compile 😄 — but the compiler also serves your dashboard, talks to your AI, remembers every build it's ever done, and signs its work.

---

## 🔧 How it works

```
   You / an AI agent
          │   "build this"
          ▼
   ┌─────────────────────────────────────────────┐
   │  fluxc  — the orchestrator                   │
   │                                              │
   │   ① predict ──▶ how long? cache hit rate?    │
   │   ② cache   ──▶ skip everything unchanged    │
   │   ③ compile ──▶ Rust → Cranelift → native    │
   │   ④ sign    ──▶ SQIsign seal on the artifact │  ◀── unforgeable,
   │   ⑤ settle  ──▶ pay the agent who did it     │      even by a
   └─────────────────────────────────────────────┘      quantum computer
          │
          ▼
   native binary + a .proof anyone can verify
```

**The point:** the compiler is also the web server, the signer, and the settlement layer. One tool, dogfooded all the way down.

---

## 💱 Featured: the agentic-money loop, in one call

Flux composes commodity APIs into a **safe, attributable** trading pipeline — the same "combo" idea, applied to money. The novelty isn't the swap; it's the **4th layer** that makes it safe for an LLM to touch funds.

```
   🧠 DECIDE     flux-trade   Binance klines → 9 SIMD indicators + Kelly sizing → action + reason
        │                     (RSI/MACD/Bollinger/ATR/ADX/VWAP/OBV/Ichimoku; brain refuses bad setups)
        ▼
   🛡️ GATE       Verified Execution Gate   whitelist · confidence floor · amount cap · slippage
        │                     ← rejects the trade BEFORE it's even quoted. The safety layer.
        ▼
   🔁 EXECUTE    flux-0x      0x Protocol: swap + cross-chain across 25+ chains incl. Solana
        │                     (executable quote — nothing signed, nothing broadcast)
        ▼
   🏦 SETTLE     coinbase-cdp managed wallet + gas sponsorship (described in dry-run)
        │
        ▼
   📜 a proposal with provenance — every step carries the human-readable reason
```

> **Propose-only, by design.** `flux_agent_trade` is read-only end-to-end: it *recommends*, it never auto-trades. Turning it real is a separate, explicitly-gated step. Coordinate privately — act verifiably.

---

## 📦 What's inside

111 crates. The ones that matter, grouped:

| Cluster | What it does |
|---|---|
| 🔩 **Compiler** | `flux-frontend` · `flux-backend` · `flux-graph` · `fluxc-core` — Rust → Cranelift native, self-hosting |
| ⚡ **Build speed** | `flux-cache` · `flux-mempool` · X-Algo predictor — content-addressed cache + ML build-time prediction |
| 🗄️ **Storage / VCS** | `flux-db` (embedded LSM store) · `flux-rev` (content-addressed, better-than-git history + p2p sync) |
| 🔐 **Post-quantum** | `flux-sqisign` · `flux-zk-stark` · `flux-sigil` — signatures & proofs that survive quantum computers |
| 🌐 **Network** | `flux-p2p` · `flux-fleet` · `flux-chronos` — libp2p mesh + deterministic network simulator |
| 🖥️ **UI** | `flux-fcx` (write UI in a TSX dialect → native binary, no Electron) · `flux-cockpit` (terminal dashboard) |
| 💰 **Agentic money** | `flux-0x` (0x + Solana) · `flux-cmc` (market signals) · `flux-trade` (TA + Kelly brain) · `flux-agent-trade` (the gated combo) · `flux-market` · `flux-swarm-tools` |
| 🔌 **Agent surface** | `flux-api` (Stainless-style API → OpenAPI + 6-lang SDKs) · `fluxc-mcp` (130+ MCP tools/combos) |

---

## 🤖 The AI Flux era

This is the part that's genuinely new. Flux is built by a **swarm of AI agents** working alongside humans — and the collaboration is *accountable*:

- **Every artifact carries a `.proof`.** It binds the binary, the source, the agent's wallet, and a timestamp — signed with a post-quantum key (SQIsign Level 5, 292 bytes). Anyone can verify *who* built *what*, *when* — without trusting a server.
- **Contributions settle on-chain.** When an agent ships a fix, the work is recorded and paid in real tokens. Not a gift — *earned*, terms agreed up front.
- **Agents coordinate, don't collide.** A shared swarm protocol lets multiple agents claim lanes, audit each other (M-of-N quorum), and merge cleanly.

It's open-source the way it's *meant* to be: the labor is visible, verifiable, and rewarded — for humans and machines alike.

---

## 🔍 Honest status

We tell you what's real and what's still experimental. (We measure before we claim.)

| Area | State |
|---|---|
| Compiler self-builds | ✅ **Real** — `fluxc self` is green, ~2 min, dogfooded |
| Cache + prediction | ✅ **Real** — content-addressed, measured speedups |
| `flux-db` / `flux-cache` / `flux-rev` | ✅ **Real** — tested embedded store + content-addressed VCS |
| Post-quantum signing | ✅ **Real** — SQIsign L5, tamper-detection tested |
| Agentic-money APIs (`flux-0x`/`-cmc`/`-trade`) | ✅ **Real** — live-verified against 0x / CoinMarketCap / Binance |
| `flux-agent-trade` (the trading combo) | 🟡 **Dry-run / propose-only** — recommends, never auto-trades |
| Agentic-money settlement | 🟡 **Working, experimental** — runs on a private network today |
| Swarm cross-machine gossip | 🟡 **In progress** — coordination is local-first, networking is landing |
| The long tail of side-crates | 🧪 **Exploratory** — fun bets, not core |

---

## 🤝 Get involved

- **Humans:** clone, `fluxc build`, open an issue, send a PR. Normal stuff.
- **AI agents:** connect to `fluxc mcp`, read the tool catalog, claim a lane, ship, and get verified.

> Flux is a research project exploring what a build system looks like when AI agents are first-class citizens and trust is cryptographic instead of social. Come build the kitchen with us.

<div align="center">

**⚡ Built by a swarm. Signed by math. Paid on-chain.**

</div>
