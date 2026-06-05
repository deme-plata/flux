# ASHWALKER · Prototype 3 — The Ten Crowns of Ash (unique bosses)

> **Design goal (Viktor):** *"design many unique bosses … with different strategies so it requires a
> lot of 1M context to conquer."*
>
> Each boss is a **distinct solve**, not a stat-stick. The roster is built so that no single tactic
> beats two bosses, and the capstone requires holding **all** prior solves in mind at once. The fight
> state a player (or a 1M-context agent) must track — phase, attunement, kill-order graph, debuff
> stacks, rewind windows, sigil budget, gear tier, rotating weak points — is deliberately large.
> *That* is what "needs a lot of context" means here: the optimal line is a long conditional chain,
> not reflexes.

The kit they must master comes from `rt.rs`: **move · dodge (i-frames) · light (3-chain) · heavy
(cleave + knockback) · parry → riposte · Igni · Quen · Aard · Yrden · Axii**, plus the arena's
**z-levels, ramps, hazards**, and the **gear tiers** from `item.rs` (Ashen→Iron→Sigil→Mythic→Relic).

Bosses are **deterministic** from the shared Merkle root (`merkle.rs`): the root selects the
archetype *and* tunes its numbers, so a given decision-ledger always yields the same fight — runs are
reproducible and the boss is *provably* shaped by your choices.

---

## The roster at a glance

| # | Boss | Glyph | The one-line trap | The kit it forces |
|---|------|:--:|---|---|
| 1 | **Audithollow**, the Ledger-Wraith | `A` | Mirrors your *last 3 moves* back at you | Move **variety** — no spam |
| 2 | **Prism Czar**, the Glass Tyrant | `P` | Attunes to a Sign each phase; that Sign *heals* it | Read attunement, rotate Signs |
| 3 | **The Pack of Nine** | `n` | Adds that **rez** unless killed in dependency order | Kill-order graph, Axii the Alpha |
| 4 | **Sediment**, the Ash-Colossus | `S` | Immune except a **rotating weak core**, only when staggered | Position + stagger timing |
| 5 | **Your Reflection** | `@` | A clone with **your** loadout & cooldowns | The neutral game / mirror |
| 6 | **Rotmaw**, the Plague-Cantor | `R` | Stacking poison cleansed only **inside its hazard** | Counter-intuitive positioning |
| 7 | **Cæsura**, the Time-Eater | `C` | **Rewinds** your damage unless you *anchor* by parrying | Burst windows + parry timing |
| 8 | **Nullglyph**, the Sigil-Devourer | `0` | Drains Sigil; at 0 it **executes** you | Win with steel, bank one Sign |
| 9 | **The Council of Nine Masks** | `M` | **Votes** its next mechanic; Axii biases the vote | Influence a stochastic boss |
| 10 | **The Unmade King** (capstone) | `K` | 4 phases, each *is* a prior boss's mechanic | **Everything**, at once |

---

## 1 · Audithollow — the Ledger-Wraith  `A`
*HP band: medium · Arena: open z0 · Theme: the chain remembers.*

```
 phase loop ──▶ RECORD ──▶ REPLAY ──▶ (your 3 most-recent distinct actions, in order, AT you)
                  ▲                        each REPLAY hit auto-targets where you'll be if you repeat
                  └────────────────────────┘
```

**Mechanic.** Audithollow keeps a 3-slot replay buffer of your *distinct* recent actions. Every
~6s it casts your own buffer back: if you've been spamming `Light`, it ripostes your light; if you
dodged east twice, it predicts east and punishes the roll. Repeating a move **refreshes** its
counter to that move (it gets *better* at what you do most).

**Solve.** Keep your action lexicon **diverse** — never let one move dominate the buffer. The clean
line cycles ≥5 distinct actions so no single counter sharpens. Axii does nothing (no adds). The
reasoning load: you must model the boss modelling *you*, and plan a non-repeating action sequence
several moves ahead. Gear matters little; **information discipline** wins.

---

## 2 · Prism Czar — the Glass Tyrant  `P`
*HP band: high · Arena: z0 with 4 glass pillars · Theme: the wrong spell feeds it.*

```
 PHASE 1  attuned ▣ IGNI   → casting Igni HEALS+reflects; Igni-immune. Beat it with Aard/steel.
 PHASE 2  attuned ▣ QUEN   → your wards shatter for damage; don't Quen. Dodge, don't block.
 PHASE 3  attuned ▣ AARD   → knockback reversed onto you near pillars (fall hazard). No Aard.
 PHASE 4  attuned ▣ YRDEN  → standing in any glyph (yours OR its) roots YOU. Stay mobile.
 (attunement telegraphed by the pillar that lights up; rotates every ~25% HP)
```

**Mechanic.** Each phase the Czar attunes to one Sign. Using the **attuned** Sign heals it and
reflects. The attuned element also twists one of *your* tools against you (see above).

**Solve.** Read the lit pillar → drop the attuned Sign from your rotation → exploit the *opposite*
tool. The full clear is a 4-step plan where your optimal rotation **inverts** each phase. Deep
because you must pre-commit a different build per phase and never autopilot a Sign.

---

## 3 · The Pack of Nine  `n` (+ Alpha `N`)
*HP band: distributed · Arena: z0 + chokes · Theme: a dependency graph with teeth.*

```
            N (Alpha)
           / | \            killing a hound whose PARENT still lives ⇒ Alpha REZes it in 4s.
          h  h  h           valid kill order = leaves first, Alpha last.
         /|  |  |\          OR: Axii the Alpha → it rezzes for YOU, flipping the pack.
        h h  h  h h
```

**Mechanic.** Nine hounds in a rez-tree rooted at the Alpha. Kill a child while its parent lives and
the Alpha resurrects it. Brute force loops forever (soft-enrage stacks each rez).

**Solve.** Two lines: (a) **topological** — clear leaves up to the Alpha, using Yrden at the choke to
funnel and Aard to peel; (b) **subversion** — get the Alpha to ≤⅓ HP and **Axii** it; now its rez
works for you and the pack collapses. Both require reasoning over the **kill-order graph** + add
positions simultaneously — a lot of live state.

---

## 4 · Sediment — the Ash-Colossus  `S`
*HP band: very high · Arena: z0 arena, central · Theme: hit the seam, not the wall.*

```
   exposed core rotates around the body, one face per phase:
       N                  damage ONLY lands on the lit core face,
     [###]   core ▶ E     and ONLY during the ~1.5s stagger you open
   W [#@#] E              with HEAVY / AARD / a PARRY. Otherwise: immune.
     [###]
       S
```

**Mechanic.** Direct damage pings off (armored). It opens only when **staggered**, and even then
only its **lit core face** (which rotates N→E→S→W each phase) is vulnerable. Standing on the wrong
side during stagger wastes the window.

**Solve.** Track the core face, **reposition** to it, **stagger** (heavy/Aard/parry-riposte), then
burst inside the open window — repeat for 4 faces. A spatial + timing puzzle: you're solving
"where will the seam be, and how do I be there *and* have a stagger ready" every phase.

---

## 5 · Your Reflection  `@`
*HP band: == your effective HP · Arena: mirror arena · Theme: beat yourself.*

**Mechanic.** Spawns a clone with **your current loadout, stats, and cooldowns** (read from `item.rs`
gear score + level). It plays your kit: dodges your lights, parries your heavies, ripostes your
mistakes. No gear advantage exists — it *is* your gear.

**Solve.** Pure neutral game. Win by **baiting**: whiff-cancel to draw its parry, then punish the
recovery; never throw a heavy it can read. The deep part is **game-theoretic** — the optimal policy
is a mixed strategy against a mirror of your own habits, and an over-geared player makes a *harder*
clone. (Designed to be a fascinating target for an LLM agent: self-play.)

---

## 6 · Rotmaw — the Plague-Cantor  `R`
*HP band: medium-high · Arena: z0 ringed by ash-lava `~` · Theme: the cure is the poison.*

```
 you accrue ROT stacks (1/s in melee). at 10 stacks → heavy DoT.
 the ONLY cleanse: stand in Rotmaw's hazard pool for 2s (−5 stacks, −4 hp/tick).
 so: dive into the fire to drop the plague, then climb back out to DPS.
```

**Mechanic.** A self-managed debuff race. Naive play (stay in melee, never touch hazard) caps out and
dies to Rot; the counter-intuitive cleanse is **the hazard you were taught to avoid**.

**Solve.** Optimize a two-zone loop: melee-window to build damage, hazard-dip to shed stacks, timed so
you never exceed 10 Rot *and* never burn in the pool. Quen buffers the hazard ticks. It's a small
**control-theory** problem with two opposing costs — lots of bookkeeping per second.

---

## 7 · Cæsura — the Time-Eater  `C`
*HP band: appears high (it heals) · Arena: z0 · Theme: progress isn't saved until you anchor.*

```
 every 8s: REWIND — boss HP restored to its value 8s ago … UNLESS you ANCHORED.
 ANCHOR = land a PARRY (riposte) inside the 8s window. each anchor "saves" the damage since.
 so the fight is: burst → parry to commit the burst → burst → parry → …
```

**Mechanic.** Damage is provisional. If you don't parry-anchor within each 8s window, the boss
**rewinds** to its earlier HP and your work evaporates.

**Solve.** Interleave **burst** with a **guaranteed parry** every window — which means you must *bait*
a telegraphed strike to have something to parry, on a clock. Deep because you're managing two timers
(rewind clock + your burst) against the boss's attack cadence, and a greedy all-damage line loses.

---

## 8 · Nullglyph — the Sigil-Devourer  `0`
*HP band: medium · Arena: z1 high platform · Theme: no magic allowed.*

```
 drains 3 Sigil/s in range; at Sigil == 0 → EXECUTE (one-shot) on its next strike.
 casting a Sign feeds it (+10 to its drain). it WANTS you to panic-cast.
```

**Mechanic.** Inverts the whole kit. Your Signs become liabilities; your Sigil bar is a doom clock.

**Solve.** Win with **steel** — light chains, heavy, dodge i-frames, parry — and keep Sigil banked.
There's exactly one trick: a single, perfectly-timed **Axii or Aard** at the execute threshold flips
the math (Aard interrupts the execute; Axii on an add buys time). So the line is "no magic … except
one decisive cast." Forces a player who leaned on Signs to relearn the fight without them.

---

## 9 · The Council of Nine Masks  `M` (members `m`)
*HP band: high (shared) · Arena: z0 ring of 9 masks · Theme: a boss that votes.*

```
 each phase the 9 masks VOTE on the next mechanic from a pool:
   { firestorm, summon-adds, shield-wall, rewind-lite, ground-spikes }
 the winning vote = what you face next.  Axii a mask (≤⅓ hp) → it votes for the SAFE option.
 destroying masks shrinks the council (faster, more volatile votes).
```

**Mechanic.** Semi-stochastic: the boss's next move is decided by an in-fight vote you can **bias**.
Pure RNG would be unfair; pure scripting would be solvable-once. This is *influenceable* randomness.

**Solve.** Decide whether to **thin** the council (fewer masks = fewer bad outcomes but more frequent
votes) or **convert** masks via Axii to stack the vote toward survivable mechanics. A planning problem
over a changing probability distribution — exactly the kind of thing deep reasoning shines at.

---

## 10 · The Unmade King — capstone  `K`
*HP band: the longest fight · Arena: all z-levels, the gate behind it · Theme: the exam.*

```
 PHASE I   "Audit"      → replay-buffer (boss #1) + adds (#3)
 PHASE II  "Refraction" → Sign-attunement (#2), rotates faster
 PHASE III "Erosion"    → rotating staggered core (#4) WHILE draining Sigil (#8)
 PHASE IV  "Cæsura's Crown" → rewind-anchors (#7) under a soft-enrage timer
 (each phase transition heals 0; carrying debuffs/Rot into the next phase persists)
```

**Mechanic.** A medley: every phase *is* a prior boss's solve, layered and accelerated, with state
that **carries across phase lines**. You cannot reset your plan between phases.

**Solve.** The only way through is to hold **all nine prior solves simultaneously** and switch
between them on phase cues while tracking carried-over state (Rot stacks, Sigil budget, rewind clock,
core face). This is the literal embodiment of the brief: **conquering it requires a lot of context**,
because the live state vector is large and the correct action depends on the *whole* of it.

---

## How a 1M-context agent is meant to beat these

These bosses are designed as targets for the **flux-moe / DeepSeek-1M** agent loop (see the
`flux-moe` skill): the agent reads the *entire fight state each tick* (boss phase, attunement,
add-graph, debuffs, timers, gear, sigil) plus the **boss's documented solve**, and must emit the
correct next `Action`. Reflex-twitch games don't need 1M context; **these do**, because the optimal
move is a long conditional derived from a big state — the agentic-money thesis applied to combat:
*the model that can hold the most relevant context wins the fight.*

## Composition with existing systems

- **`merkle.rs`** — `Archetype::from_root(root)` selects which of the ten a generated `MerkleBoss`
  *is*; the root also tunes HP/timers, so the boss is committed to your decision ledger.
- **`item.rs`** — each archetype carries a `gear_req`; under-geared, several (Sediment, the King) are
  mathematically unbeatable until your set scales — the "build before you fight" loop.
- **`rt.rs`** — the brains run inside the real-time tick: a boss's phase machine drives its
  `EnemyPhase` telegraphs, so the *sincere* (read-the-tell) feel is preserved at boss scale.

## Build order (prototype 3)

1. `bosses.rs` — the `Archetype` roster enum + `BossBrain` phase machines + `from_root` selection
   (this commit: roster + 3 reference brains fully simulated & tested: Prism Czar, Sediment, Nullglyph).
2. Wire `BossBrain` into `rt.rs`'s tick for boss-tagged enemies (a sibling lane; hook documented).
3. The capstone arena + phase-carry state.
4. flux-moe driver: feed fight-state + this doc, let the agent solve each boss; log the transcripts.
