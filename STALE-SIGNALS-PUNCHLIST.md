# Stale-signal de-drift punch-list

**Goal:** remove the misleading static strings that make every fresh agent/human misread Flux. Adding `AGENTS.md`
matters; *removing the landmines* matters as much. Each item below is a concrete, low-risk edit. Verify every
"after" value against live introspection (`fluxc version`, `fluxc status --json`, `flux_health_report`) at the
time you apply it — don't hardcode a new number that will just drift again; prefer build-time reads from `Cargo.toml`.

> Canonical at audit time (2026-06-24): **version 0.28.0 · 143 crates · ~162 tools (help) / ~185 (live)**.

> **Update 2026-06-24:** the base build is now GREEN (`fluxc self` exits 0). The original "active jq filter"
> root cause was wrong — it was a **poisoned `.rustc_info.json` cargo cache** (cargo replayed a cached failed
> rustc probe). Quarantined + `self_build` env restored → fixed. The doc fixes below (P0/P1) are still pending.

## P0 — version strings (highest confusion-per-byte)

1. **`crates/fluxc/src/mcp.rs:571` — hardcoded `"fluxc 0.9.6 … MCP: 30 tools"`**
   - Problem: a hardcoded version/tool-count string, 19 minor versions stale.
   - Fix: replace the literal with `env!("CARGO_PKG_VERSION")` (→ `0.28.0`) and derive the tool count from the
     actual registered-tools list length rather than a literal. Grep for other hardcoded `0.9.x`/`0.2x` strings
     in the same file (e.g. the `--version`/MCP banner around `mcp.rs:47,571`) and fix them the same way.

2. **`README.md` badges — `status-beta_v0.22`, `crates-111`, `MCP_tools-130+`**
   - Fix: regenerate from `Cargo.toml` + a `fluxc status`/`tools/list` count in CI (a tiny `scripts/gen-badges`
     step), or at minimum bump to `0.28.0 / 143 / ~185`. Better: a CI check that fails if the badge ≠ `Cargo.toml`.

3. **`docs/PLATFORM_STATUS_2026-06-06.md` — `Flux Platform v0.25`**
   - Fix: it's a dated status doc, so leave the historical body but add a top banner: "superseded — current
     platform version is `0.28.0`; see live `fluxc xray --json`."

4. **`CHANGELOG.md` top — `v0.6.0 — Self-Hosting Compiler`**
   - Problem: the *title* asserts self-hosting as a delivered state; fluxc is still built by cargo+rustc.
   - Fix: keep the historical entry, but ensure the **latest** changelog entry reflects `0.28.0`, and reword any
     present-tense "self-hosting compiler" claim to "self-hosting *target* / Phase-1 dogfooding (cargo+wrapper)."

## P1 — false "green/real" status claims

5. **`README.md` "Honest status" table — `Compiler self-builds ✅ Real — fluxc self is green, ~2 min, dogfooded`**
   - Problem: NOW FALSE — `fluxc self` exits 101 (jq-contaminated cargo probe), reproduced 2026-06-24.
   - Fix: change to `🔴 RED — fluxc self currently fails (cargo rustc target-probe contaminated); see FLUXC build notes`.
     Make this row auto-driven by CI status if possible, so it can't silently rot again.

6. **`README.md` "Honest status" table — `Cache + prediction ✅ Real — measured speedups`**
   - Problem: misleading — live wrapper-cache hit rate is 0.0% (`docs/FLUXC_CACHE_GAP.md`: populates, never hits).
   - Fix: `🟡 populates but 0% hit — apply restores a marker, not rmeta/rlib bytes; Patches 2+3 pending`.

7. **`docs/FLUX_WHITEPAPER.tex` — "distinguishing claim is self-hosting"**
   - Fix: reframe as the *goal/roadmap*, not a realized property (or gate the claim behind the Phase-3 self-host
     milestone). Note it owns Cranelift codegen but borrows rustc's frontend semantics.

## P2 — argv bug + junk (correctness, not just docs)

8. **Argv-parse bug creating literal dirs `./--help/` and `./--message/`** (the `flux-rev` / hooks entrypoint
   mis-parses flags into directory names).
   - Fix: find the entrypoint that does `mkdir`/path-join on an arg without flag-stripping (start at
     `crates/flux-rev/src/` and any `fluxc rev`-adjacent dispatch); strip/validate `--flags` before treating an
     arg as a path. Then delete the junk dirs `./--help` `./--message` and the stray `&1` file at repo root.

9. **Wire `flux-rev` as `fluxc rev`** — today `fluxc rev` prints top-level help; the real VCS is a separate
   `flux-rev` binary. Surfacing it as a `fluxc` subcommand is what makes the "Flux accounts for itself" story true.

## P3 — keep it from drifting again

10. **CI guard:** add a check that fails the build when (a) any badge/banner version ≠ `Cargo.toml` version, or
    (b) a "Honest status" row marked ✅ has a failing CI job behind it. Cheap insurance against re-drift.
11. **Regenerate `FLUX_FACTS.json`** (in `flux-repo-onboarding/`, also commit to repo root) from live
    introspection on each release, so programmatic analyzers always read truth.

*All of the above are surgical and reversible. None changes runtime behavior except #8 (argv fix) and #9 (subcommand wiring).*
