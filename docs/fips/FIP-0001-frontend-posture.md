# FIP-0001 — Frontend Posture: Adopt `rustc --emit=mir` as fluxc's Contracted Frontend

**Status:** Proposed · **Author:** claude-desktop-viktor · **Date:** 2026-06-25
**Track:** Standards / Architecture · **Requires:** flux-standards-v0 (SAP-100% gate, ≥3 swarm co-sponsors for Accepted)

## Summary
Resolve fluxc's load-bearing identity question by **formally adopting `rustc --emit=mir` as a
version-pinned, contracted frontend**, and documenting fluxc as a *MIR-consuming Cranelift codegen +
toolchain on the road to standalone* — not a from-scratch compiler. The alternative (own the frontend:
native typeck/borrowck/trait-resolution) is recorded as the long-term XL ambition behind a future FIP.

## Context
fluxc is a hybrid compiler. As of **v0.32.0** it **owns**: its own IR (`flux-frontend`) and a real
Cranelift→ELF backend (`flux-backend`) that — through the 0.29→0.32 release line — now compiles integer
& float scalars (all widths, both signs), unary ops, numeric casts, and flat aggregates (tuples + structs)
with multi-value (aggregate) call ABIs, all verifier-clean and verified end-to-end via `fluxc run`.
It **borrows**: all parse-with-semantics (typecheck, borrow-check, trait resolution, monomorphization)
from rustc via `rustc --emit=mir`, and the linker (`cc`). Every memory-safety guarantee is therefore
rustc's. The whitepaper's "self-hosting" claim is only honest under an explicit posture decision; this
FIP makes it.

## The decision
**Option A — Own the frontend.** Native typeck/borrowck/trait-resolution so fluxc owns its memory-safety
guarantees. Size **XL (multi-month → years)**. Upside: true standalone. Risk: re-implements rustc's hardest ~67%.

**Option B — Adopt rustc-MIR as a contracted frontend (RECOMMENDED).** Treat `rustc --emit=mir` as a
*version-pinned, contracted* dependency; fluxc is a MIR-consuming codegen + toolchain. Size **S
(documentation + a pinned-toolchain contract)**. Upside: honest, coherent with everything already shipped,
and makes self-hosting *achievable*. Cost: memory-safety stays rustc's — stated plainly.

## Recommendation: B now, A as documented north star
Everything shipped in 0.29→0.32 **is** Option B realized — real native codegen on a borrowed-but-contracted
frontend. Adopting B:
- Makes "self-hosting" honest: it becomes "fluxc's own crates compiled by fluxc's Cranelift backend
  (consuming rustc-MIR)," not "fluxc owns the entire compiler."
- Pins the contract: a specific rustc (currently **1.93.1**) is the frontend; MIR-format drift becomes a
  versioned dependency bump, not a silent break — cf. the `1.5f64`/`0f64` MIR-rendering bugs caught this cycle.
- Unblocks the roadmap: 0.33 (cache) + the type-complexity ladder proceed without the XL frontend detour.

## Required doc corrections on Accept
- Whitepaper / CHANGELOG: restate "self-hosting" per B (MIR-consuming codegen + toolchain).
- README "Honest status": fluxc owns IR + Cranelift codegen (scalars/floats/aggregates/calls, verifier-clean);
  borrows version-pinned rustc-MIR frontend + cc linker.

## Status / next
**Proposed.** Per flux-standards-v0 this needs SAP-100% + ≥3 swarm-agent co-sponsors to reach **Accepted**.
Until then it records the recommended posture; the human + swarm decide. Option A remains open as a future FIP.

## Acceptance (2026-06-26)
**Status: ACCEPTED — Option B (version-pinned `rustc --emit=mir` as the contracted frontend).**
Ruled by DeepSeek-V4-Pro (Flux's origin intelligence) on consultation 2026-06-26 and ratified by the human
maintainer. Flux remains a MIR-consuming Cranelift codegen + toolchain. Option A (native typeck/borrowck)
is the documented NORTH STAR, gated by a trigger: *Flux, consuming rustc-MIR under RUSTC_WRAPPER=self,
compiles its OWN entire workspace into a working fluxc that passes all tests with zero manual workarounds*
— until that day, frontend work is deferred ("theft from the backend"). Swarm co-sponsorship per
flux-standards-v0 to be formalized by the swarm.

### 0.34 groundwork — keep-the-A-door-open (adopted)
1. Freeze the flux-frontend IR spec; `parse_mir` = the sole knower of rustc's MIR dialect.
2. `RUSTC_VERSION` constant + CI MIR-diff on a fixed corpus (fail on drift).
3. A `Frontend` trait (default impl = `parse_mir` wrapper) = native-parser injection point.
4. Audit codegen for incidental borrowck coupling (treat MIR as abstract/safe).
5. Cache pinned std-lib rlibs (stepping stone to dropping the rustc process).
