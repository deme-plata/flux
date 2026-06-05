# FluxOS v0 — chain-agnostic port of QuillonOS

> **One-line:** Fork the QuillonOS browser-first WASI userspace into chain-agnostic `flux-os-*` crates; SIGIL OS becomes a thin skin on top later.

## Scope

QuillonOS today (`flux/crates/quillonos-{init,coreutils,claude}` + `dist-final/os.html`) is **branded as Quillon's chain skin** even though the substrate is chain-neutral. FluxOS v0 is the rename + skin extraction so:
- **FluxOS** = browser-WASI substrate, no chain assumptions in the OS layer.
- **QuillonOS** = thin skin: ships with `qnk-*` wallet tools preinstalled, branding = Quillon teal/blue.
- **SIGIL OS** = sibling skin: ships with `sigil-node` tools preinstalled, branding = obsidian/violet (matches sigil.html + verify-tip.html).

User outcome unchanged: `quillon.xyz/os.html` keeps working, plus `quillon.xyz/flux-os.html` boots into the unbranded substrate, plus `sigilgraph.com/os.html` (future) boots into the SIGIL skin.

## Dep graph

```
flux-os-substrate (the OS proper)
├── flux-os-init           (PID 1 — process spawner, fs mount, signal dispatch)
├── flux-os-coreutils      (ls, sh, cat, ps, kill, env, ...)
├── flux-os-fs             (in-memory FS with localStorage / IndexedDB persistence)
├── flux-os-net            (fetch + WebSocket socket layer)
├── flux-os-wm             (window manager — moves dragging logic out of dist-final HTML)
├── flux-os-shell          (terminal emulator + xterm.js binding)
└── flux-os-ai             (Claude / DeepSeek / GPT bus, replaces quillonos-claude)

flux-os-skins (skins layer)
├── quillonos-skin         (ports + repackages today's quillonos-* with the Quillon brand)
└── sigilos-skin           (FUTURE — sigil-node + sigil-tx tools preinstalled, obsidian/violet)

flux-os-browser (delivery)
├── dist-final/flux-os.html        (unbranded substrate, debug-flavored)
├── dist-final/os.html             (alias → loads quillonos-skin)
└── dist-final/skins/sigil/        (later — when SIGIL skin lands)
```

## Concrete v0 line list (target ~1200 LOC moved + 400 LOC new)

```
crates/flux-os-init/                   moved from quillonos-init       ~300
crates/flux-os-coreutils/              moved from quillonos-coreutils  ~400
crates/flux-os-fs/                     new                             ~200
crates/flux-os-shell/                  new                             ~150
crates/flux-os-wm/                     new                             ~100
crates/flux-os-ai/                     moved from quillonos-claude     ~300
crates/quillonos-skin/                 new wrapper (brand css/json)    ~80
gui/quantum-wallet/dist-final/
└── flux-os.html                       new (clones os.html, strips brand) ~600
```

`flux-os.html` boots from the same `init.wasm` but loads a different `manifest.json` per skin:
```json
{
  "name": "FluxOS",
  "version": "0.0.1",
  "boot_modules": ["flux-os-fs", "flux-os-wm", "flux-os-shell", "flux-os-ai"],
  "brand": { "name": "Flux", "primary": "#22d3ee", "accent": "#a78bfa" },
  "preinstall": []
}
```

For the SIGIL skin (P2):
```json
{
  "name": "SIGIL OS",
  "version": "0.0.1",
  "boot_modules": ["flux-os-fs", "flux-os-wm", "flux-os-shell", "flux-os-ai"],
  "brand": { "name": "SIGIL", "primary": "#8b5cf6", "accent": "#fbbf24" },
  "preinstall": ["sigil-node", "sigil-tip-verify", "verify-tip.html"]
}
```

The `verify-tip.html` I shipped today becomes a **first-class SIGIL OS app** — runs as a browser-WASI module inside the OS desktop, not a standalone page.

## What this v0 deliberately does NOT include

- **Renaming `quillonos-claude` → `flux-os-ai` at the API level** in one go. Keep `pub use flux_os_ai as quillonos_claude;` shims so dependent dist-final HTML doesn't break.
- **Removing the `quillonos-*` crates** — they become skin shims, not deleted.
- **Multi-tenant sandbox isolation** — each skin runs in the same WASI instance for v0. Sandbox-per-skin lands when we have real user multi-tenancy.
- **Persistent volumes across page reloads** — `flux-os-fs` v0 is in-memory only; localStorage backing is a follow-up.

## Open Qs

1. **Rename vs alias** — do we physically `mv` the `quillonos-*` crate dirs to `flux-os-*` (breaking history) or add new `flux-os-*` crates that re-export quillonos types (faster, dirtier)? Suggest `mv` since rocky's broadcast on flux/ vs sigil/ separation set the precedent.
2. **Skin manifest schema** — JSON or a tiny TOML? JSON loads directly in browser without a parser dep; TOML matches Cargo style. Suggest JSON.
3. **`flux-os` namespace conflicts** — there's no existing `flux-os` prefix in the workspace; safe to use. Confirm with `grep -r "flux-os" flux/` to be sure.
4. **Compile Garden integration** — should the existing garden.html be reborn as a FluxOS app inside the desktop? Big payoff for the "FluxOS is the demo + the IDE host" pitch. Probably P2 once flux-ide lands.
5. **`fluxc os-stage` MCP tool** — already exists per task #30. Does it stage the SUBSTRATE or the SKIN? May need a `--skin <quillon|sigil>` flag to disambiguate post-port.

## Sequencing

Depends on whether flux-ide ships first:
- **If flux-ide first:** FluxOS hosts the IDE as its flagship app. Clean story.
- **If FluxOS first:** flux-ide ships as a FluxOS module from day one, no separate URL.

Suggest FluxOS first (this v0) since it's a 1.5-session rename + extraction; flux-ide on top in the next 1-2 sessions becomes natural.

— rocky-sigil 🟣
