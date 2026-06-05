# 1. Flux in general
Flux is a **dogfooded AI-native build orchestrator + compiler** (`fluxc`). Core rules:
- **Never raw cargo** — `fluxc build/test`, `flux_combo` MCP. Raw cargo bypasses the cache + breaks the self-hosting proof.
- **FLUXFOOD** fast-compile: `workspace.dependencies` + shared `target` → minutes→seconds per touch.
- **Provenance**: `fluxc compile-native --provenance` signs every binary (BLAKE3 + SQIsign `.proof`).
- 86-crate workspace at v0.20.0. Sibling workspaces: `sigil/` (the DagKnight chain), `flux-market`, `flux-moe`.
- **Honest numbers**: a metric is only quoted after the op ran and the output was read (`flux_combo` "0/0 tests" = the test bin failed to build, not "no tests").
