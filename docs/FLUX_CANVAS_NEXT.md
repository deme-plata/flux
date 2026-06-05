# flux-canvas — the Figma-AI experience, the Flux way (UE × vast.ai × fluxc)

> Viktor: *"render stuff using vast.ai + Unreal Engine for a flux IDE — interfaces/UI like Figma but for desktop apps, much better than Electron and Slint — the new Figma AI experience paired with our flux AI / deepseek-codewhale code."*

## Verdict: YES, and it's assembly, not invention

Every tier already exists in the workspace as a real piece. flux-canvas wires them into one loop: **describe a UI → AI emits Flux UI code → Unreal renders it photoreal on a cloud GPU → fluxc compiles it to a 0.5 MB native, `.proof`-signed desktop binary.** No Chromium, no toolkit ceiling.

## The four tiers (all backed by something real today)

```
   ┌───────────────────────────────────────────────────────────────────┐
   │  DESIGNER (any browser / thin client — no local GPU)                │
   │   drags + describes: "a dark trading panel, neon candle chart"      │
   └───────────────┬───────────────────────────────────────────────────┘
                   │ WebRTC frames ↑   design intents ↓
   ┌───────────────▼──────────────┐   ┌──────────────────────────────────┐
   │  RENDER TIER — vast.ai GPU    │   │  AI TIER — flux AI + deepseek      │
   │  Unreal Engine 5, HEADLESS    │   │  flux_iterate / X-Algo / deepseek  │
   │  + Pixel Streaming (WebRTC)   │◀─▶│  emits Flux UI code (a DSL), not   │
   │  renders 3D/shader/anim UI    │   │  throwaway vectors → real widgets  │
   └───────────────┬──────────────┘   └──────────────┬───────────────────┘
                   │  flux-ue-bridge (EXISTING crate) │
                   │  UI commands ↔ UE · build/AI events ↔ editor
   ┌───────────────▼──────────────────────────────────▼───────────────────┐
   │  OUTPUT TIER — fluxc compile-native                                   │
   │  → native desktop binary (ARM/Win/Linux, ~0.5 MB musl) + .proof       │
   │  → ships via flux://  (proof-addressed, hot-update)                    │
   └───────────────────────────────────────────────────────────────────────┘
```

| Tier | What it does | Real piece it stands on |
|---|---|---|
| **Render** | UE5 headless on a rented GPU, **Pixel Streaming** (WebRTC) beams interactive frames to a browser tab — GPU stays in the cloud | UE Pixel Streaming is a shipping UE feature; **flux compute fabric (Vast)** already brokers GPU rental (`setup-flux.sh`, nodeswarm, +10% red line) |
| **Bridge** | UI-design commands ↔ UE (place widget, bind data, style); streams build/AI/webhook events into the editor | **`flux-ue-bridge` crate already exists** (shipped v0.11.2: `/v1/workspace`, `/v1/webhook`) — extend its protocol |
| **AI** | "Figma AI", but the output is *compiled code*: designer describes/drags → flux AI + DeepSeek emit a Flux UI DSL → live preview in UE | flux AI loop (`flux_iterate`, `flux_architect_predict`, X-Algo) + DeepSeek-codewhale model — the dogfooded coding loop |
| **Output** | the design **is** the app: `fluxc compile-native --provenance` → real native binary, signed | the cross-compile prototype (ARM/Win/Linux, musl, `.proof`) + `flux://` distribution |

## Why this genuinely beats Electron and Slint

| | Electron | Slint | **flux-canvas** |
|---|---|---|---|
| Bundle | ~150 MB Chromium | small native | **~0.5 MB musl native + `.proof`** |
| Render ceiling | DOM/CSS (2D) | own toolkit (2D) | **UE-grade: 3D, shaders, photoreal, animation** |
| Design tool | hand-code | hand-code + preview | **Figma-like AI canvas, GPU-cloud rendered** |
| AI | bolt-on | none | **native: design → compiled code, in-loop** |
| Where the GPU is | local | local | **rented on vast.ai (pixel-stream); thin client** |
| Distribution | installer | binary | **`flux://` proof-addressed + hot-update** |
| Provenance | none | none | **every build SQIsign-signed (`.proof`)** |

The killer line: **Figma gives you a picture of an app; flux-canvas gives you the app — compiled, signed, and shippable — and renders the design with a game engine instead of a DOM.**

## Honest scope — real today vs the moonshot

- **Real today:** `flux-ue-bridge` crate, the vast.ai compute fabric, `fluxc compile-native` cross-targeting (ARM/Win/Linux proven this session), `flux://` distribution, UE Pixel Streaming as a known UE capability.
- **The build:** (1) stand up UE5 headless + Pixel Streaming on a vast box (WebRTC signalling + a UE project that renders a widget tree); (2) define the **Flux UI DSL** + the bridge command protocol (the seam fluxc↔UE); (3) the AI design→DSL loop; (4) DSL→native codegen via fluxc. The moonshot is (4) — "the canvas output is a real compiled native app," not a preview.
- **Smallest first lane (FC-1):** extend `flux-ue-bridge` with a `/v1/canvas` command channel + render a static Flux-UI-DSL tree in a UE Pixel-Streaming session on one vast GPU → a browser sees a live UE-rendered panel it can restyle. That's the "is it possible" proof, end-to-end, on real infra.

## Build lanes
- **FC-1** `flux-ue-bridge` `/v1/canvas` + UE Pixel-Streaming POC on vast.ai (one GPU, one panel, restyle live) — the feasibility proof.
- **FC-2** Flux UI DSL + bridge command protocol (place/bind/style/animate).
- **FC-3** AI design loop (flux AI + DeepSeek: prompt/drag → DSL → live UE preview).
- **FC-4** DSL → `fluxc compile-native` → signed native binary (the moonshot) → ship via `flux://`.

## The one line
**flux-canvas is Figma's AI design experience rendered by Unreal on rented GPUs and compiled by fluxc into a half-megabyte signed native app — the design isn't a mockup, it's the binary.**

— rocky, 2026-05-31 · joins flux-knot + flux:// + cross-compile in the "next prototype" family.
