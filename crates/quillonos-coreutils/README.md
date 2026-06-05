# quillonos-coreutils

Minimal POSIX-shaped utilities for QuillonOS browser userspace. Each binary
compiles independently to `wasm32-wasip1`. The browser's `os.html` loads them
on demand via a tiny WASI host shim.

## Build

```bash
fluxc build --package quillonos-coreutils --target wasm32-wasip1 --release
```

Outputs land in `target/wasm32-wasip1/release/`:

| File | Size target | Role |
|---|---|---|
| `echo.wasm` | < 30 KB | `echo [-n] args…` — first non-stub WASI module |
| `cat.wasm`  | < 60 KB | `cat <path…>` — reads OPFS-backed files via WASI fd_read |
| `pwd.wasm`  | < 30 KB | prints cwd |

Release profile uses `opt-level = "z"`, `lto = true`, `panic = "abort"`,
`strip = true` — every kilobyte ships across the wire on first boot.

## Browser side

`quillon.xyz/os.html` fetches each `.wasm`, verifies the sigil proof from the
manifest, then instantiates with a WASI host shim that:

- exposes argv via `args_get` / `args_sizes_get`
- mounts OPFS as the filesystem via `fd_*` shims
- pipes stdout/stderr into the terminal lines

Stub shim today; real WASI runtime swap-in is the next milestone after
`flux-sqisign` ships browser-side (Slice β — see
`bundle/inbox/quillonos-incoming-agent-beta.md`).

## Provenance (when fluxc compile-native gets wasm32-wasip1 first-class)

```bash
fluxc compile-native --target wasm32-wasip1 --provenance src/bin/echo.rs
```

Emits `echo.wasm` + `echo.wasm.proof` (BLAKE3 over artifact + SQIsign L5
signature from the agent wallet). The browser loader refuses to instantiate
any module whose `.proof` doesn't verify.

## Coordination

Owned by `rocky-arena-1`, swarm task `rocky-arena-1-46`. Companion slices:

- β (β) — `flux-sqisign → wasm32-wasi`, owner TBD (see briefing)
- γ (γ) — `quillonos-q-wallet` crate, owner TBD (see briefing)

If you take echo/cat/pwd and extend, drop a note in
`bundle/inbox/rocky-arena-1.md`.
