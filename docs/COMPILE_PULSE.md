# Compile pulse — every build judges its own cost

`fluxc` (v0.41+, commits cc43b974 / a8e428e7 and later) measures every rustc unit it wraps and,
after each `build | check | test | combo | self`, prints one line on stderr:

```
  ⏱ ▲ heavier — 3m18s CPU over 9 unit(s); judged work +9.0s vs typical (+29%); sigil-node 42s (typ 30s, +40%) src +6.0 KB
    ↳ lever: sigil-node grew +6.0 KB and got +40% slower — split the module into its own crate, or …
```

Why: compile time creeps. On sigilgraph a cold `check -p sigil-top` went from 241 s to 356 s
CPU between 15 Jul and 3 Oct 2026, via 113 new packages, without anyone deciding it. The pulse
makes each step visible while the change that caused it is still on screen.

## What is measured

| per rustc unit (wrapper) | per invocation (parent) |
|---|---|
| CPU = user+sys of the rustc child (`getrusage`), not wall clock | wall split: wait before first rustc / rustc span / after |
| peak RSS | cargo's own `Blocking waiting for file lock` time: package cache vs target dir |
| shape: crate-type, check vs build, opt-level(+lto), target, feature-set hash | `target/*/.cargo-lock` busy at start |
| local (path) crate vs registry | 1-min load vs physical cores |
| source bytes of the module tree | flux-cache restored vs compiled |

## Verdicts

`steady` · `▲ heavier` · `▼ lighter` · `◔ learning` (a unit needs 3 prior builds of the same shape)
· `◌ idle` (nothing compiled; a slow no-op still speaks up) · `✗ failed`.

Regression per unit = ≥1.25× its rolling median (last 24) **and** ≥300 ms, with the tolerance
widened when the box is more contended than when the baseline was taken (CPU time inflates when
SMT siblings are busy; up to ×1.6). A *new dependency* = a crate in the working `Cargo.lock` but
not in `HEAD:Cargo.lock`.

## Commands (MCP: `flux_compile_pulse view=…`)

```
fluxc pulse                     # last verdict (--json for the whole Pulse)
fluxc pulse line                # ≤60-char status line
fluxc pulse weather             # session-start summary (also opens flux_quickstart)
fluxc pulse drift               # what got slower over days (--days 28 --recent 7)
fluxc pulse tax                 # where dev compile CPU went: builds × cost (--days 7 --all)
fluxc pulse blast <crate>       # what editing it rebuilds + critical-path wall floor (--check, --workspace)
fluxc pulse deps                # Cargo.lock vs HEAD, with measured cost per package
fluxc pulse cost <crate>[@ver]  # what ADDING a dependency would bring in, before you add it
```

## Plumbing

- Store: `~/.flux/cache/pulse/` — `baselines.json`, `units-YYYY-MM-DD.jsonl`, `last.json`.
- Env: `FLUX_PULSE=0` off · `FLUX_PULSE_QUIET=1` no stderr line · `FLUX_PULSE_DIR=<dir>` isolate an
  experiment (always do this for historical / what-if builds, or they pollute live baselines).
- Webhook event `compile_pulse` (the full Pulse JSON); `combo_complete` carries a pulse summary.
  Literal-loopback endpoints (`http://127.0.0.1:…`) work from shell builds too.
- MCP: `flux_combo` shows PULSE rows; any tool that triggered a build gets a one-line footer;
  planning tools that name a crate get its blast radius; cortex output is marked SIMULATED.
- Overhead: a few µs per unit in the wrapper; no-op builds skip the baseline read.

## Measured on the way (2026-10-03)

- sigil-top's `producer` feature = +90 packages / 97 s per cold check; `gpu` = +5 / 3 s. The wgpu
  stack came in through `flux-zk-stark` (now optional behind `gpu-acceleration`, default on).
  Opting SIGIL out removes 29 crates / 34 s per cold check (operator decision).
- cargo resolves under one box-wide exclusive lock on every invocation — held ~30 % of a
  10-minute window on Epsilon. Batch crates into one call.
- Switching `RUSTC_WRAPPER` between two paths to the same binary recompiles nothing (cargo 1.93.1).
