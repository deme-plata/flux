# Flux × Hunyuan3D — End-to-End Pipeline (FH3D)

**Status:** v1 design (2026-06-09). Iteration 1 of `/loop`.
**Goal:** image (or text) → textured 3D mesh, provisioned + verified + reaped through Flux primitives, test-run on a rented Vast 3090.

Hunyuan3D-2 (Tencent) is a two-stage image-to-3D model:
- **Hunyuan3D-DiT** — flow-based diffusion on a 3D shape latent → untextured mesh. ~10–16 GB VRAM (mini/turbo less).
- **Hunyuan3D-Paint** — multiview texture synthesis → textured `.glb`. A 3090's 24 GB holds both comfortably.

The bet: don't write a bespoke harness. **Every stage maps onto a Flux crate that already exists**, so FH3D is mostly wiring + a thin runner, and it inherits Flux's spend-discipline, content-addressing, and divergence-detection for free.

---

## Stage map (the whole pipeline on one screen)

| # | Stage | What happens | Flux primitive | Artifact (content-addressed) |
|---|-------|--------------|----------------|------------------------------|
| 0 | **Provision** | Pick + rent a 24 GB box under a hard $/hr ceiling | `flux-gpu-market` `Need{vram:24,disk:80,down:200}` → `flux_vast_recommend` (spend-gate in the type) → `flux_vast_create` | `box_id`, pricing breakdown |
| 1 | **Bootstrap** | cuda-12.4 image; `pip install` Hunyuan3D-2 + rembg + trimesh; pull weights | ssh runner + `flux-torrent` (weights as a blob, not re-downloaded per box) | `env_hash` |
| 2 | **Ingest input** | Image in (or text→image first) | `flux-aether` ingest | `input_cid` |
| 3 | **Preprocess** | rembg background removal → clean RGBA, square pad | runner step | `clean_cid` |
| 4 | **Shape gen** | Hunyuan3D-DiT, **seed pinned**, steps configurable | runner (GPU) | `shape_cid` (`.obj`) |
| 5 | **Texture gen** | Hunyuan3D-Paint multiview → textured mesh | runner (GPU) | `glb_cid` (`.glb`) |
| 6 | **Postprocess** | decimate / weld / normals (trimesh) | runner step | `final_cid` |
| 7 | **Verify** | N≥2 workers run identical `(weights_hash, seed, clean_cid)` → compare meshes | **`flux-burst` VBC**, generalized (see below) | `vbc_report` |
| 8 | **Provenance** | Stamp `(input_cid, weights_hash, seed, final_cid, box_id, ts)` | `flux-rev` | `.proof` |
| 9 | **Reap** | Idle-autostop / explicit destroy; release box-registry claim | `flux-gpu-market` autostop → `flux_vast_destroy` | — |

```
 image ──▶[2 aether]──▶[3 rembg]──▶[4 DiT shape]──▶[5 Paint texture]──▶[6 clean]──┐
                                       │ seed,weights_hash                          │
                       [0 market]──▶[1 boot box]                                    ▼
                                                                          [7 VBC verify]──▶[8 rev .proof]──▶[9 reap]
                                                                                   ▲
                                       (worker B runs 4–6 with same seed) ─────────┘
```

---

## Stage 7 — verification: porting VBC from builds to 3D gen (the interesting part)

`flux-burst` already proves *build* divergence is impossible to hide: N workers compile the same unit; if artifact hashes disagree, one tampered and the quorum exposes it (`verify_consensus`, `BuildClaim`). FH3D reuses the **same primitive** for generation:

> N workers run Hunyuan3D over the **same** `(weights_hash, seed, clean_cid, step_count)`. Each emits a `BuildClaim`-shaped `GenClaim{ inputs_hash, mesh_hash }`. The coordinator runs `verify_consensus`.

**Honest caveat — byte-exact hashing will NOT hold for diffusion across heterogeneous GPUs.** CUDA float nondeterminism + different SM counts → meshes that are *perceptually identical* but not bit-identical. So FH3D needs **two verify modes**, chosen per use-case:

1. **`strict` (byte-exact)** — only valid when all workers share GPU model + driver + `torch.use_deterministic_algorithms(True)` + fixed seed + fp32. Then `flux-burst` VBC works unchanged. This is the *reproducibility* claim.
2. **`tolerant` (geometric)** — Chamfer distance between meshes `< ε` (e.g. 1e-3 of bbox diag) → "same shape, different bits". This is the *anti-tamper* claim against a worker that swaps in a different model. Needs a tiny `mesh_distance` extension to the verifier; VBC's quorum logic is reused, only the equality predicate changes.

v1 ships `strict` on a **single GPU model** (rent 2× same-host-class 3090) to get a real consensus result; `tolerant` is the v2 follow-up. Don't claim cross-GPU determinism we haven't measured.

---

## The runner

One small binary, `fh3d-run`, lives on the box (pushed via ssh, or baked into a `flux-torrent` image). It is a dumb executor of stages 2–6 + emits a `GenClaim`. Orchestration (0,1,7,8,9) stays on the controlling Flux side so the box holds no secrets and no money logic. Stage I/O is content-addressed end-to-end, so a re-run with the same `input_cid`+`seed` is a cache hit, not recompute.

```
fh3d-run --input <path|cid> --seed 42 --steps 30 --texture on \
         --emit-claim --out /work/out.glb
# prints: {"input_cid":...,"clean_cid":...,"shape_cid":...,"glb_cid":...,"mesh_hash":...,"secs":...}
```

---

## Test plan on the rented 3090 (10 min → 2 h budget)

**Smoke (≤10 min) — prove the wire end-to-end:**
1. `flux_vast_recommend` (vram≥24) → `flux_vast_create` a single 3090.
2. ssh bootstrap: pull cuda image, `pip install` Hunyuan3D-2 **turbo/mini** (fast weights), rembg, trimesh.
3. One image → `fh3d-run --steps 20 --texture off` (shape only). Record wall-clock + `mesh_hash` + VRAM peak.
4. `flux_vast_destroy`. Confirm burn = (secs/3600)·dph, well under budget.

**Full (up to 2 h) — quality + consensus:**
5. Full DiT steps (30–50) **+ texture on** → a real textured `.glb`, pulled back via `scp`/aether.
6. Rent a **2nd identical 3090**, run the same `(seed, clean_cid)`, collect both `GenClaim`s, run `flux-burst verify_consensus` in `strict` mode → expect AGREE (or a measured, reported disagreement → that's the determinism finding).
7. Provenance-stamp the winning mesh with `flux-rev`; ingest to `flux-aether`.
8. Reap both boxes; print final burn + runway.

**Spend ceiling:** 3090 ≈ $0.20–0.30/hr on Vast. Smoke ≈ $0.05. Full 2-box 2 h ≈ < $1.20. Hard-gated by `FLUX_VAST_BUDGET_DPH` — the recommend tool won't return a create-id over budget.

---

## Known open items / honesty section

- **BLOCKER (live):** `VAST_API_KEY` is not set — both the direct `vast-ai` MCP (400s) and the Flux gateway (`flux_vast_*`) refuse. Renting is impossible until the operator sets the key server-side. Everything above stages 0–9 is designed; nothing has run on real hardware yet.
- **Determinism is a claim, not a measurement** until step 6 runs. The `strict`/`tolerant` split exists precisely because cross-GPU bit-exactness is unproven.
- **`fh3d-run` does not exist yet** — it's the one new piece of code; everything else is existing crates.
- **Hunyuan3D model variant** (full vs turbo vs mini) is a quality/speed/VRAM knob to pick empirically on the 10-min smoke, not from the spec sheet.
- Weights are large (several GB). First box pays the HF download; `flux-torrent` is the mechanism to stop every subsequent box re-paying it, but that caching is v2.

## Iteration log
- **iter 1 (2026-06-09):** stage map + VBC-for-gen design + test plan written. Vast rent blocked on missing API key. Next: unblock key → run the ≤10-min smoke.
