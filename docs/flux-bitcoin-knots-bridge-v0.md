# flux-bitcoin-knots-bridge v0 — native Bitcoin + Lightning for Flux chains

> **One-line:** Port Quillon's `q-bitcoin-bridge` to a chain-agnostic `flux-bitcoin-bridge` crate; add **Bitcoin Knots** backend (from Delta), **LNbits** Lightning UI (from Quillon), and an auto-BIP-generation tool that emits BIP-style markdown + LaTeX PDF + Flux API spec from one source. Inherits the FIP standards process (see `flux-standards-v0.md`).

## Why now

Both chains we care about have Bitcoin presence already, but **siloed**:
- **Delta server** runs Bitcoin Knots (the privacy-flavored Core fork) for steganographic peer discovery + transaction-graph privacy
- **Quillon Graph** runs the `q-bitcoin-bridge` crate at `/home/orobit/q-narwhalknight/q-bitcoin-bridge/` (16 modules: api, atomic_swap, blockstamp, bridge, cytoplasmic_gateway, discovery, encoding, free_bitcoin_discovery, header_beacon, real_bitcoin_client, solana_bridge, steganography, zcash, zcash_memo_optimizer, zmq_monitor) and LNbits for Lightning custody / payment links

Flux (and therefore SIGIL) doesn't natively speak Bitcoin yet. Porting unifies the substrate so every future chain inherits BTC + LN for free.

## Inventory (what to port from)

| Source | Origin | What we take | What we leave |
|---|---|---|---|
| Bitcoin Knots (binary) | Delta server | Backend variant: prefer Knots over Bitcoin Core for privacy-sensitive deploys; otherwise Core is fine | Don't fork Knots — wrap its RPC |
| `q-bitcoin-bridge` | `/home/orobit/q-narwhalknight/q-bitcoin-bridge/` | bridge.rs, discovery.rs, encoding.rs, header_beacon.rs, real_bitcoin_client.rs, steganography.rs, zmq_monitor.rs | beda.rs (Quillon-internal), zcash*.rs (separate port), solana_bridge.rs (separate port) |
| LNbits | Quillon ops | Lightning custody/payment UI hosted side-car to flux-bitcoin-bridge | LNbits's own DB layer (we wrap, don't fork) |
| `rust-bitcoin` crate | Already in `cargo-target/` cache | Reuse as the Bitcoin types layer | — |

## Dep graph

```
flux-bitcoin-bridge (chain-agnostic core)
├── bitcoin              (rust-bitcoin — types + scripts)
├── bitcoin_hashes
├── bitcoincore-rpc      (drives either Knots or Core via the same RPC)
├── tokio + reqwest
├── tracing
├── flux-api-standards/FIP-XXXX  (the FIP that defines the conformance surface)
└── modules/
    ├── backend.rs       trait BtcBackend { rpc(...), zmq(...), block_at(...) }
    ├── backend_knots.rs Knots-flavored backend impl
    ├── backend_core.rs  vanilla Core backend impl
    ├── discovery.rs     port of q-bitcoin-bridge::discovery
    ├── encoding.rs      port of q-bitcoin-bridge::encoding
    ├── steganography.rs port of q-bitcoin-bridge::steganography
    ├── zmq_monitor.rs   port of q-bitcoin-bridge::zmq_monitor
    ├── bridge.rs        port of q-bitcoin-bridge::bridge (peer announce/listen)
    └── header_beacon.rs port of q-bitcoin-bridge::header_beacon (chain header proofs)

flux-bitcoin-bridge-lightning (Lightning sidecar)
├── lnbits-client        new — speaks LNbits REST API
├── lnurl                rfc paths
└── modules/
    ├── invoice.rs       create/pay/lookup invoices
    ├── wallet.rs        per-Flux-chain-account LNbits wallet provisioning
    └── routing.rs       channel route hints (read from BOLT-7)

flux-bitcoin-bridge-ui (sigil-node + future chains pin this)
├── api.rs               REST surface — adopted from q-bitcoin-bridge::api
├── topics/              gossipsub topic constants for chain-specific BTC events
└── auto-bip/            BIP-gen tooling (see below)
```

## What `flux-api/standards/FIP-XXXX/` looks like

The bridge's conformance surface is itself a FIP — `FIP-0010 — Bitcoin Bridge Conformance` (suggested number). Excerpt of the API trait it exposes:

```rust
#[fip(0010)]
pub trait BitcoinBridge {
    fn current_tip(&self) -> Result<BlockHeader>;
    fn submit_anchor_tx(&self, payload: &[u8], options: AnchorOptions) -> Result<Txid>;
    fn watch_anchors(&self, subscriber: impl FnMut(AnchorEvent)) -> Subscription;
    fn lightning_invoice(&self, amount_sat: u64, memo: &str) -> Result<Invoice>;
    fn redeem_invoice(&self, preimage: &[u8; 32]) -> Result<RedeemReceipt>;
}
```

SIGIL implements this trait; a future Flux-substrate chain implements it independently. `flux-api-test fip-0010 sigil-node` runs the parametric suite against the SIGIL binary; green = conformant.

## Auto-BIP generation

`fluxc bip new` is the BIP-specific cousin of `fluxc standard new`:

```bash
fluxc bip new \
    --title "OP_FLUXANCHOR — chain anchor opcode for flux-substrate chains" \
    --author rocky-sigil \
    --type Standards
```

Generates a BIP-formatted Markdown skeleton (with the conventional sections: Preamble, Abstract, Motivation, Specification, Rationale, Backwards Compatibility, Reference Implementation, Copyright). Then `fluxc bip build BIP-XXXX` emits:

- `out/BIP-XXXX.html` — Bitcoin-style web view
- `out/BIP-XXXX.pdf` — LaTeX-rendered with the same pandoc + xelatex pipeline as FIPs (so a single LaTeX template `templates/proposal.tex` serves both)
- `out/BIP-XXXX.api.json` — extracted Flux API the proposal references
- `out/BIP-XXXX.fip.md` — companion FIP that imports the BIP's behavior into the Flux substrate surface (so a Flux operator can cite the BIP without leaving the FIP ecosystem)

The PDF goes to the Bitcoin BIP repo as a proposal; the FIP goes to the Flux standards library. Both come from one source — author edits the Markdown, the rest regenerates.

## LNbits UI ("Bitcoin bridge UI features")

LNbits is the cleanest off-the-shelf Lightning UI in 2026. Strategy:
- Run LNbits as a sidecar process under `flux-bitcoin-bridge-lightning`.
- Provision a per-Flux-account LNbits wallet on first use; store the wallet key in flux-db.
- Expose LNbits's web UI at `node-host:5000` (Knots node) or `flux-os://app/lightning` (future FluxOS module).
- For SIGIL: `sigil-node lightning invoice 50000` creates an invoice via the bridge; UI is the standard LNbits one with an obsidian+violet skin overlay.

## Bitcoin source code (already on disk)

Found build artifacts for `rust-bitcoin`, `bitcoin_hashes`, `bitcoin-internals`, `bitcoincore-rpc`, `bip39` in `cargo-target/` caches. Suggests the dependency footprint is already vendored. No need to download anything; cargo's offline mode against the existing cache should build the bridge.

The actual **Bitcoin Knots source code** (the `bitcoin/bitcoin` C++ fork) doesn't need to live in this repo — we drive it over RPC. If the operator wants to compile Knots from source, that's `/home/orobit/bitcoin-knots/` as a sibling tree, outside the Rust workspace.

## What this v0 deliberately does NOT include

- **Re-implementing rust-bitcoin** — we use it, not replace it.
- **A new wallet** — LNbits is the wallet for Lightning; flux-bitcoin-bridge handles on-chain UTXO management via Knots/Core RPC. No third wallet impl.
- **Cross-chain atomic swap to Solana** — Quillon's `solana_bridge.rs` doesn't get ported in v0. Separate task.
- **Zcash port** — `zcash.rs` + `zcash_memo_optimizer.rs` are out of scope. Separate task once shielded-flow design lands.
- **A custom Bitcoin node** — neither Knots nor Core is forked; both are run as-is, drive over RPC.

## SIGIL implication

After `flux-bitcoin-bridge` lands, SIGIL gets:
- **On-chain Bitcoin anchor proofs** — every SIGIL block can embed a BTC tx-hash anchor for additional time-stamp + double-spend resistance. New SigilTx variant: `BitcoinAnchor { btc_txid: [u8; 32], block_height_at_anchor: u32 }`.
- **Lightning-paid services** — a SIGIL validator can advertise a paid RPC behind a LN invoice (e.g. "tip-proof streaming for 100 sat / month").
- **Cross-chain peer discovery** — same steganographic peer-discovery Quillon uses, SIGIL inherits free.

All gated behind the FIP-0010 conformance suite.

## Open Qs

1. **Bitcoin Knots vs Core default** — which backend does a fresh `flux-bitcoin-bridge` install ship with? Suggest: Knots by default (privacy posture matches Flux brand) with `--backend core` flag for compat.
2. **LNbits hosted vs self-hosted** — `legend.lnbits.com` hosted is faster to demo; self-hosted is the long-term posture. Suggest: self-hosted required for the standard; hosted is "convenience mode" with a loud warning.
3. **Lightning custody model** — does flux-bitcoin-bridge hold user keys for invoices, or do users sign? Suggest: invoice creation is custodial (the bridge holds), redemption requires user signature (non-custodial spend). Mirrors LNbits's own posture.
4. **PSBT integration** — flux-bitcoin-bridge needs PSBT (BIP-174) support for hardware-wallet flows. `rust-bitcoin` has it. Plan: include in v0.
5. **LaTeX pipeline shared with FIPs** — yes; one `templates/proposal.tex` serves both, parameterized on `--variant fip|bip`. Saves us from duplicating typography work.

## Sequencing

```
FIP-0010 (this proposal as a FIP) ──┐
                                    │
flux-standards-v0 (the process)  ───┴──► flux-bitcoin-bridge (core port)
                                          │
                                          ├──► lightning sidecar
                                          ├──► auto-bip CLI
                                          └──► SIGIL adoption (BitcoinAnchor tx kind)
```

Stack with the other in-flight asks:
```
codewhale-gate      ──► flux-ide (agent dock)
                  └──► codewhale-deck (cost-stream)
flux-standards      ──► FIP-0010 (this) ──► flux-bitcoin-bridge
flux-chronos        ──► tourbillon (shipped) ──► MCP (CHRONOS-D)
FluxOS              ──► flux-ide hosts as FluxOS app
```

— rocky-sigil 🟣
