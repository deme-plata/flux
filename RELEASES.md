# ⚡ Flux — Release Ledger

Commercial-grade releases. Every release is a **GPG-signed (Verified)** commit, and the released
code carries a **content-addressed flux-rev provenance hash** — re-snapshot the crate with
`flux-rev snapshot <crate>` and verify the hash matches. Verify, don't trust.

| Version | Date | Type | git (signed) | flux-rev provenance |
|---|---|---|---|---|
| **v0.22.3** | 2026-06-05 | 🐛 fixes | `44003e8` | — |
| **v0.22.2** | 2026-06-05 | ✨ feature | `3e1975b` | flux-0x `c55d39bfce4d3b4f…` |
| **v0.22.0** | 2026-06-05 | 🚀 initial public release (111 crates) | `5b560cc` | — |

## v0.22.3 — fixes
- **fix(flux-cmc):** `RetryConfig::from_env` now reads `FLUX_CMC_BASE_BACKOFF_MS` / `FLUX_CMC_CAP_BACKOFF_MS` — backoff was un-tunable via env (parity with flux-0x).
- **fix(flux-0x):** CLI exposes `price-ah` / `quote-ah` — the AllowanceHolder methods were unreachable from the command line.

## v0.22.2 — feature
- **feat(flux-0x):** AllowanceHolder swap flow — `swap_price_ah` / `swap_quote_ah`, the alternative to Permit2 for integrations that prefer a plain ERC-20 approval. 7/7 green.

## v0.22.0 — initial public release
- The AI-native, self-hosting Rust build orchestrator. 111 crates incl. the agentic-money stack (flux-0x / flux-cmc / flux-trade / flux-agent-trade).
