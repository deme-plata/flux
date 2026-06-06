# Flux Platform v0.25 — status (2026-06-06)

> Hurtig handoff: hvad der er lavet, hvad der kører, hvad der mangler.  
> Host: **Epsilon** `89.149.241.126` · repo: `/home/storage/deepseek-codewhale/flux` (branch `_public`, ~8 commits ahead)

---

## Mål (fra planen)

- **Ingen UI** i v0.25-gate — kun MCP combos + audit-webhooks
- **P0 tools:** Aether (ingest/retrieve/sync), fleet search, compile-error combo
- **`flux_compile_error_combo`:** rustc-fejl → fil:linje + kode-snippet → webhook `:4178` → agent kan rette hurtigt
- **Build/test via `fluxc`** — ikke `flux-cargo-wrapper` (flux-dev skill)

---

## ✅ Færdigt

### MCP handlers (fluxc-mcp)

| Tool | Fil | Status |
|------|-----|--------|
| `flux_aether_ingest` | `handlers/aether.rs` | Deployed, roundtrip-test |
| `flux_aether_retrieve` | `handlers/aether.rs` | Deployed, BLAKE3 verify |
| `flux_aether_sync` | `handlers/aether.rs` | Deployed, `sync` default **false** |
| `flux_fleet_search` | `handlers/fleet.rs` | Deployed, SSH fan-out + dedup |
| `flux_compile_error_combo` | `handlers/compile_error.rs` | Deployed, parser + snippets |

Alle registreret i `crates/fluxc-mcp/src/lib.rs`.

### Webhooks & audit

- **`handlers/platform_webhook.rs`** — POST til `http://127.0.0.1:4178/api/mcp-webhook` + append `target/flux-platform-captures/<event>.jsonl`
- Wired i **aether** (3 steder), **fleet** (1), **compile_error** (1)
- Swarm-feed via `fluxc_core::webhook::auto_dispatch` bevares parallelt

### Sikkerhed

- **`handlers/platform_security.rs`** — query-sanitering, 64-hex `content_root`, base64-cap, strict JSON, node-validering

### Dokumentation (opdateret på Epsilon)

- `docs/FLUX_MCP_COMBOS.md` — platform-sektion + arkitektur-diagram
- `docs/FLUX_PLATFORM_INDEX.md` — v0.25 gate-række, fjernet Excel/UI fra gate-beskrivelse

### Gate-script (omskrevet)

- `tools/v25-release-gate.sh` — **6 gates**, combo-first:
  1. fluxc-mcp tests
  2. aether ingest+retrieve roundtrip
  3. flux-aether crate tests
  4. compile_error parser + handler
  5. sigil-vm wasmi
  6. promote-gate + fluxc 0.25.x
- Bruger nu **`./target/debug/fluxc test`** (ikke flux-cargo-wrapper)

### fluxc-disciplin (seneste fix)

- **`fluxc_cmd()`** tilføjet i `handlers/mod.rs` → kalder `target/debug/fluxc` med `/root/.cargo/bin` på PATH
- **`compile_error_combo`** skiftet fra `cargo check` → **`fluxc build --package`**
- **`cargo_cmd()`** tilbage til `cargo` + PATH (ikke wrapper) — legacy handlers uændret i adfærd

### Tests (verificeret tidligere)

- **59/59** fluxc-mcp unit tests grønne (kørsel før seneste rebuild)
- **sigil-vm** 5/5 wasmi tests grønne
- Commit på Epsilon: `d93e0049` — *feat(platform): flux_aether_* + flux_fleet_search + flux_compile_error_combo webhooks*

---

## 🔄 Igangværende / afbrudt

| Opgave | Tilstand |
|--------|----------|
| Rebuild `fluxc` efter seneste `compile_error.rs`-fix | Afbrudt — ikke kørt færdig |
| Live MCP-test af `flux_compile_error_combo` med `fluxc build` | Ikke bekræftet efter match-block-fix |
| Kør `bash tools/v25-release-gate.sh` med fluxc (6/6) | Ikke kørt færdig med nyt script |
| Git commit af seneste ændringer | **Ucommittet** på Epsilon (`platform_webhook.rs`, gate, `fluxc_cmd`, docs, patches) |

---

## ⏳ Mangler (P0 / næste skridt)

1. **Rebuild + verify:** `PATH=/root/.cargo/bin:$PATH ./target/debug/fluxc build --package fluxc`
2. **Live test:** `flux_compile_error_combo` på kendt fejl-pakke → tjek snippet + `target/flux-compile-errors/latest.json` + `captures/compile_error.jsonl`
3. **Gate grøn:** `bash tools/v25-release-gate.sh` → 6/6
4. **Commit + push** `_public` (alle platform-filer)
5. **DeepSeek audit** (valgfri) på final diff før promote

### P2 (ikke v0.25)

- P2P Delta (0 peers), TLS SAN `sigilgraph.fluxapp.xyz`
- VM-2 host imports
- `flux_source_edit_combo`, `flux_platform_full_combo`
- Opdater `FLUX_SOURCE_EDITING.md` fuldt (combo-first, ingen UI)

---

## Vigtige filer

```
flux/crates/fluxc-mcp/src/handlers/
  aether.rs
  fleet.rs
  compile_error.rs
  platform_webhook.rs   # NY
  platform_security.rs
  mod.rs                # fluxc_cmd() + cargo_cmd PATH-fix

flux/tools/v25-release-gate.sh   # combo-only, fluxc test
flux/docs/FLUX_MCP_COMBOS.md
flux/docs/FLUX_PLATFORM_INDEX.md

flux/target/flux-compile-errors/latest.json    # compile_error snapshot
flux/target/flux-platform-captures/*.jsonl     # audit trail
```

---

## Kendte gotchas

- **`fluxc` kræver cargo på PATH** — MCP sætter `/root/.cargo/bin` automatisk via `fluxc_cmd()`
- **Gammel gate (Excel + flux_ui_*)** kørte 5/5 grøn — er **erstattet**; ny gate ikke endnu verificeret
- **`fluxc build`** tager ~3–5 min cold på fluxc-mcp — forvent lang gate Gate 1
- Brug **`flux_combo`** / **`fluxc test`** / **`fluxc build`** — aldrig `flux-cargo-wrapper` i nye scripts

---

## Hurtig genoptagelse (3 kommandoer)

```bash
ssh root@89.149.241.126
cd /home/storage/deepseek-codewhale/flux
export PATH="/root/.cargo/bin:$PATH"
./target/debug/fluxc build --package fluxc && bash tools/v25-release-gate.sh
```