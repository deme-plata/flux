# FLUX-DROID v0 — the wickedest Android dev AI workflow for Flux

> Viktor's call (2026-05-31): *"invent the wickedest and coolest android development ai workflow for flux taking expo plus dead-on and also kotlin and rust."*
>
> **Interpretation of the stack:** **Expo** (RN/TS — the fast UI lane) · **Kotlin/Compose** (native lane) · **Rust** (the one core, fluxc-compiled). **"dead-on"** = byte-for-byte **reproducible builds** so N agents produce an identical APK → that's the trust substrate the whole thing rides on (Verifiable Build Consensus). If you meant a specific tool by "dead-on" (Detox? Dioxus? Tauri-mobile?), say so and I'll fold it in.

---

## The one-sentence thesis

**One Rust core, three heads, one AI loop, byte-identical output.**
Write the logic once in Rust (fluxc-compiled, `.proof`-signed), surface it through *whichever* UI lane fits the screen (Expo for speed, Compose for native polish, Slint for pure-Rust), and drive the whole thing with the same Flux agent loop we already use on the web — **predict → build → SEE-it-on-device → swarm-settle** — where "SEE-it" is the **flux eye extended to a real Android device**.

```
              ┌──────────────────────────────────────────────┐
              │           flux-droid-core  (Rust)            │
              │  SIGIL wallet · keys · consensus · tip-verify │
              │  one crate · fluxc-built · .proof per ABI     │
              └───────────────┬──────────────┬───────────────┘
                      UniFFI / │  JSI/Turbo   │  JNI
                  ┌───────────▼──┐  ┌─────────▼────┐  ┌────────▼────┐
                  │  EXPO (TS)   │  │ KOTLIN/Compose│  │ SLINT (Rust)│
                  │ fast UI lane │  │ native lane   │  │ pure-rust   │
                  └──────┬───────┘  └──────┬────────┘  └─────┬───────┘
                         └─────────────────┼─────────────────┘
                                  ┌─────────▼─────────┐
                                  │   THE FLUX EYE    │  adb screencap +
                                  │  on a real device │  uiautomator dump
                                  └─────────┬─────────┘  → AI sees the app
                                  ┌─────────▼─────────┐
                                  │  AI agent loop    │ predict→build→verify
                                  │  fluxc + MCP combo│ →.proof→VBC→SIGIL
                                  └───────────────────┘
```

---

## Why one Rust core (not three codebases)

Mobile wallets rot because the same crypto/consensus logic gets reimplemented in Swift, Kotlin, and JS — three places to get a signature wrong. Flux already has the core in Rust (`sigil-*`, `flux-*`, the wallet's `libp2p/`, PQ crypto). So:

- `flux-droid-core` = a thin Rust facade over the existing SIGIL crates: `keypair_from_mnemonic`, `sign_tx`, `tip_verify`, `balance`, `bip39`. **The exact BIP39 + tip-verify we just shipped in the web wallet, reused — not rewritten.**
- Exposed via **UniFFI** (auto-generates Kotlin + Swift bindings) and a **JSI/Turbo-module** shim for Expo. One `.udl`, three language surfaces, zero hand-written FFI.
- Cross-compiled with **cargo-ndk** to `aarch64-linux-android`, `armv7-linux-androideabi`, `x86_64-linux-android` — driven by fluxc so it inherits the cache + `.proof`.

---

## The three heads — when to use which

| Lane | Tech | Use it for | AI iteration speed |
|---|---|---|---|
| **Expo** | Expo Router + RN + TS, Dev Client embedding the Rust Turbo module | 90% of screens. Fast Refresh = the AI writes TSX and the eye verifies on-device in **seconds**. EAS for cloud builds. | ⚡⚡⚡ (hot reload) |
| **Kotlin/Compose** | Jetpack Compose, Rust via JNI/UniFFI | platform-API-heavy screens (biometrics, NFC, secure enclave, widgets), 120fps lists | ⚡⚡ (gradle incremental) |
| **Slint** | pure-Rust UI (already runs on Android via `slint-android`) | the lowest-dependency build; same `.slint` as flux-desktop reskinned for touch | ⚡ (native rebuild) |

The agent picks the lane per-screen from a `lane:` hint in the task, exactly like the swarm picks crates today.

---

## 🔭 The killer feature — **Eye-on-Device**

Today I redesigned the wallet by *rendering it headless and looking at the screenshot* before claiming it worked. **That exact loop, but on a phone.** This is the piece nobody else has.

```
flux_droid_eye:
  1. adb exec-out screencap -p           → PNG of the live app
  2. adb exec-out uiautomator dump        → the view hierarchy (the "DOM")
  3. POST both to the eye-server (the same /home/orobit/sigil-eye one)
  4. the AI agent READS the screenshot + hierarchy → sees exactly what the user sees
  5. iterate: edit TSX/Compose/Slint → Fast-Refresh / install → re-eye → converge
```

- Works on a **real device over USB/ADB-over-WiFi** and on a **headless emulator** (`avd … -no-window`) farmed on **Vast GPU** boxes.
- The agent gets *ground truth pixels*, not "it compiled" — the same thing that finally cracked the wallet theme today (mauve-looks-like-violet was only catchable by looking).
- Bonus: feed the screenshot to an on-device/Vast VLM (Gemma/Qwen) for "does this match the design?" scoring → **automated visual regression on every build**.

---

## 🤖 The AI loop (MCP surface — `flux_droid_*`)

Mirrors the web loop, mobile-native:

| MCP tool | What it does |
|---|---|
| `flux_droid_predict` | predict gradle/NDK build time + APK size before building (avoid the 8-minute surprise — same predict-before-build discipline that saves us on cold Rust builds) |
| `flux_droid_build` | fluxc → cargo-ndk cross-compile Rust (all ABIs) → bundle into Expo/Kotlin/Slint → APK/AAB, cached + `.proof`-signed per ABI |
| `flux_droid_combo` | build + unit test + **boot emulator + smoke** + eye-screenshot, in one call (the `flux_combo` of mobile) |
| `flux_droid_eye` | screencap + uiautomator dump → eye-server (above) |
| `flux_droid_install` | adb install -r to device/emulator, Fast-Refresh push for Expo |
| `flux_droid_ship` | EAS submit / Play internal track, gossipsub auto-update channel (like the slint-wallet updater) |

All gated by the **Honest Checklist** + **predict-before-build** from VarFlow.

---

## 🧱 "dead-on" — byte-for-byte reproducible APKs (the trust layer)

This is what makes the whole agent-economy story work on mobile:

- **Reproducible builds**: pinned NDK + `SOURCE_DATE_EPOCH` + sorted zip entries + deterministic R8 → two different agents on two different boxes build the **same APK hash**.
- That feeds **Verifiable Build Consensus (VBC)** — `flux-burst` already does this for crates; extend it to APKs: M-of-N agents build, agree on the artifact hash, **`flux-quorum` SQIsign-attests** it, settle in SIGIL. A user installing the APK can verify it was built by quorum from *this* source — no trusting a single CI box or Play Store.
- The `.proof` rides *inside* the APK (a signed `assets/build.proof`) so the running app can prove its own provenance to the chain. **A wallet that can attest its own binary.**

---

## 🛰️ Infra it rides on (all already exists)

- **Vast.ai compute fabric** (`project_flux_compute_fabric_vast`) — headless emulator farm + NDK/gradle build farm + on-device-VLM inference, idle-autostop, +10% red line.
- **flux-burst / flux-quorum** — VBC + the one M-of-N quorum, retrofit onto APK hashes.
- **The eye-server** (`/home/orobit/sigil-eye`) — already running; add an `/android-snapshot` route.
- **fluxc cross-compile + .proof** — already cross-compiles musl; add the android NDK targets.
- **gossipsub auto-update** — the slint-wallet updater pattern, for OTA Expo/APK updates.

---

## Build order (proposed)

1. **DROID-1** — `flux-droid-core` crate: UniFFI facade over existing SIGIL wallet crates (bip39, sign, tip-verify). cargo-ndk builds the 3 ABIs. `.proof` per ABI.
2. **DROID-2** — Expo Dev Client + Turbo-module shim → the wallet's login + balance screen in TS, reusing the cyan theme tokens. First Fast-Refresh loop.
3. **DROID-3** — `flux_droid_eye` MCP + eye-server `/android-snapshot`: screencap a real screen, AI reads it. **Prove the see-on-device loop end-to-end** (the make-or-break, like P3 was for SIGIL).
4. **DROID-4** — Kotlin/Compose lane for one native screen (biometric unlock).
5. **DROID-5** — reproducible APK + VBC over `flux-burst`, SIGIL-settled.
6. **DROID-6** — `flux_droid_combo` + predict + Vast emulator farm.

---

## Honest checklist (what's pretend right now)

- **Pretend:** every `flux_droid_*` tool — none built yet; this is the design.
- **Pretend:** reproducible-APK determinism is *claimed*; Android's R8/zipalign are notoriously non-deterministic — DROID-5 has to *measure* two-agent hash equality, not assume it.
- **Real:** the Rust core (exists), the eye (running, used today), Vast fabric (live), flux-burst/quorum (shipped), fluxc cross-compile (does musl today).
- **Measurement gate for DROID-3:** an agent edits one TSX color, and *without a human describing it*, reads back the on-device screenshot and confirms the pixel changed. If that loop closes, the workflow is real.

---

*Companion to the web loop proven today: the SIGIL wallet was themed cyan by render-verifying with playwright-core. FLUX-DROID is that same discipline, on glass you hold.*
