# ASHWALKER · What AI likes (and we want masses of)

> Viktor brief: *"hvad kan ai godt lide — det skal vi have masser af i ashwalker."*
>
> This is the design compass for bosses, spectator mode, flux-moe drivers, and swarm lanes.
> Not "fun for humans only" — **fun for 1M-context agents to read, reason about, and win.**

---

## The thesis

AI does not want twitch reflex. It wants **a big, honest state vector** plus **rules it can derive a long conditional from**.
ASHWALKER already says this in `BOSSES_PROTOTYPE_3.md`: *the model that holds the most relevant context wins the fight.*

Mass-produce everything that makes that state **large, legible, deterministic, and composable**.

---

## AI candy (ranked — build more of the top)

### 1 · Structured fight state (serialize everything)

Each tick, an agent should be able to ingest one blob and know the full puzzle:

- player: hp, sta, sigil, ward, combo, facing, pos.z, cooldowns, riposte, i-frames
- per foe: kind, hp%, phase (`Stalk` / `Windup{left,strike}` / `Stagger` / `Recover`), telegraph reach
- boss overlays: attuned Sign, core face, sigil drain, replay buffer, kill-order graph, council vote tally
- world: yrden glyph, hazard tiles, ramp height
- economy: last 3 distinct `Action`s (Audithollow), pending Sign-fusion window

**Mass of it:** `fight_snapshot()` → JSON or compact text every tick in spectator; AW-09 ticker; flux-moe driver input.

### 2 · Telegraphs with semantics (not animation)

AI reads **meaning**, not frames. Keep:

- UPPERCASE glyph + `⚠ WINDUP SMASH · reach 2 · 8 ticks`
- strike **archetype** names (BITE/LUNGE/SMASH/CLEAVE) tied to counter-play
- boss **trap lines** in BOSS RADAR (`attuned Sign HEALS`, `ARMORED until stagger`)

**Mass of it:** every enemy attack gets a named tell; no silent hits.

### 3 · Named MCP combos (the agent's own language)

Signs map to `flux_zk_combo`, `mining_status`, etc. **Fusing** two Signs must print a **stable combo name**:

- `Warded-Rally`, `Hashfire`, `Cinder-Blink`, `Sigil-Weave`

**Mass of it:** more fusion pairs; spectator always shows `⚡ MCP` + `✧ COMBO`; log `mcp_fusions` count on ascension.

### 4 · Distinct boss puzzles (no reused solve)

One mechanic per boss; capstone stacks all nine. AI boredom = second boss that is only "+HP".

**Mass of it:** ship the 7 open brains from `BOSSES_PROTOTYPE_3.md`; each adds a **new field** to fight state.

### 5 · Determinism + tests (agents trust the rules)

Same `Action` sequence + tick count → identical outcome. Every mechanic gets a unit test gate.

**Mass of it:** chronos-style replay tests per boss; `ASHWALKER_DEMO` scripts that *teach* the solve in order.

### 6 · Rich combat log (language is the UI)

Barks, gore lines, parry riposte, phase shifts, combo fuse, `"the Czar DRINKS your Igni"`.

**Mass of it:** dialog for all 10 archetypes; feed cap 64+; spectator shows 10 lines; **never** summary-only end screens.

### 7 · Documented traps (falsifiable)

Each boss HUD states the trap in one line. Agent can verify: "if I cast attuned Sign, boss heals" → test proves it.

**Mass of it:** `status_line()` for every archetype; in-game + in spec doc same wording.

### 8 · Variety / memory mechanics

Audithollow replay buffer, Council vote bias, carry-over debuffs into Unmade King phase II.

**Mass of it:** state that **punishes spam** and **rewards planning** — forces long action lexicons.

### 9 · Spectacle as **events**, not FPS

Colored arena, boss radar, AI intent `▸ FUSE Yrden → Warded-Rally`, death-cam on kill.

**Mass of it:** `ASHWALKER_STEP=1` pause on events; boss gauntlet spectator; 3-frame slow-mo gib (AW-14).

### 10 · Swarm-decomposable scope

AI agents collaborate when lanes are **file-scoped** with measurement gates (see `SWARM_TASKS.md`).

**Mass of it:** many small lanes > one "make game fun" ticket.

---

## Anti-patterns (starve these)

| Boring for AI | Replace with |
|---------------|----------------|
| Opaque DPS check | Named phase + trap |
| Pure RNG boss move | Vote / influence / telegraphed rotation |
| End-of-run summary only | Per-tick feed + spectator |
| Stat-stick enemies | Minion voice + one mechanic hint |
| Undocumented fusion | `✧ COMBO Name` + test |
| Autoplay black box | `▸ INTENT` line per decision |

---

## Density targets (Viktor bar)

- **Terminal:** every 5 ticks something readable changes (log line, radar, MCP, or telegraph countdown).
- **State vector:** capstone fight ≥ 40 named fields an agent must track (see Unmade King).
- **Combos:** ≥ 6 distinct Sign-fusion names with tests.
- **Bosses:** 10/10 brains live; 3 already shipped.
- **Transcripts:** flux-moe can dump a full clear as markdown (future lane).

---

## Shipped (grok-viktor, 2026-06-04, 106 tests)

| # | Deliverable |
|---|-------------|
| 1 | `fight_snapshot()`, `snapshot_hash()`, footer in `spectator.rs` |
| 2 | `rt_tactical_line()`, telegraph names in tactical + BOSS RADAR |
| 3 | Fusions: Warded-Rally, Cinder-Blink, Hashfire, **Mindfire**, Ward-Blink, Sigil-Weave |
| 4 | `Audithollow` brain in `bosses.rs` + `bossfight` stub + barks |
| 5 | Tests: snapshot, spam buffer, mindfire fuse (+ existing 103) |
| 6 | Trap gloat via `FeedsTrap` + rich feed on spam/kill |
| 7 | `status_line()` boss + `foe_archetype().trap()` in spectator |
| 8 | `move_buffer` / `action_spam` Ledger mechanic on live wraith |
| 9 | `pulse_event`, `death_cam`, `ASHWALKER_STEP`, `transcript.md`, `ASHWALKER_SPECTATE` |
| 10 | `ASHWALKER_ROOT` merkle spawn, `SWARM_TASKS.md` lanes remain for swarm |

## Swarm lanes tagged `AI-CANDY`

See bus message **#732+** and `SWARM_TASKS.md` section *AI affinity* for remaining AW-C02 teach demos, full 7 bosses, etc.