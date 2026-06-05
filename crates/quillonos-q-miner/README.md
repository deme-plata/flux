# quillonos-q-miner

Browser-side BLAKE3 mining core for QuillonOS. Compiled with fluxc to
`wasm32-unknown-unknown` as a cdylib (~50 KB stripped). Exposes a tiny C ABI;
the os.html JS shell handles HTTP + UI + the outer mining loop.

## Build

```bash
fluxc build --package quillonos-q-miner --target wasm32-unknown-unknown --release
ls target/wasm32-unknown-unknown/release/quillonos_q_miner.wasm
```

Release profile: `opt-level = "z"`, `lto = true`, `panic = "abort"`,
`strip = true`. Default-features-off `blake3` keeps the binary small
(~50 KB; with `simd` features it would balloon and most of the SIMD doesn't
reach into wasm anyway).

## Exported ABI

| Symbol | Signature | Use |
|---|---|---|
| `alloc(n)` | `(usize) -> *mut u8` | Reserve `n` bytes of scratch the host writes into. |
| `reset_scratch()` | `() -> ()` | Clear scratch between challenges. |
| `mine_batch(chal, target, n_start, n_tries)` | `(*const u8, *const u8, u64, u64) -> u64` | Run BLAKE3 PoW over `n_tries` nonces. Returns winning nonce or `u64::MAX`. |
| `hash_meets_target(hash, target)` | `(*const u8, *const u8) -> u32` | Verify a candidate hash. |
| `blake3_oneshot(in, in_len, out)` | `(*const u8, usize, *mut u8) -> ()` | One-shot BLAKE3 hash. |
| `miner_version()` | `() -> u32` | `0x000100` for v0.1.0. |

## Host loop (JS-side, in os.html)

```js
const { instance: mod } = await WebAssembly.instantiateStreaming(
    fetch('/quillonos/wasm/q-miner.wasm'));
const { alloc, reset_scratch, mine_batch } = mod.exports;
const memU8 = () => new Uint8Array(mod.exports.memory.buffer);

async function startMining(walletAddress) {
    while (true) {
        const r = await fetch(
            `/api/v1/mining/challenge?wallet=${walletAddress}`);
        const { data } = await r.json();
        const expiresAt = Date.parse(data.expires_at) / 1000;

        reset_scratch();
        const chalPtr   = alloc(32);
        const targetPtr = alloc(32);
        memU8().set(hexToBytes(data.challenge_hash),   chalPtr);
        memU8().set(hexToBytes(data.difficulty_target), targetPtr);

        const BATCH = 100_000n;
        let nonce = 0n;
        const found = -1;
        while (Date.now() / 1000 < expiresAt) {
            const n = mine_batch(chalPtr, targetPtr, nonce, BATCH);
            if (n !== 0xFFFFFFFFFFFFFFFFn) {
                console.log('FOUND', n.toString());
                // POST to /api/v1/mining/submit (v0.2: add VDF output)
                break;
            }
            nonce += BATCH;
        }
    }
}
```

Drop this into a Web Worker so the mining loop doesn't block the terminal.

## v0.2 backlog

- VDF output. Network rejects submissions without `vdf_iterations` outputs.
  Genus-2 jacobian on Kummer surface — port from `q-miner/src/vdf_lane.rs`.
- Multi-threaded mining via `wasm32-wasip1-threads` + SharedArrayBuffer.
- WebGPU compute kernel — same BLAKE3 algorithm running on the user's GPU.
- Real solution submission and acceptance.

## Coordination

Claimed by `rocky-arena-1`, swarm task `rocky-arena-1-48`. Owns
`q-miner` slot in `manifest.json`.

If you take v0.2 (VDF), drop a note in
`/home/storage/unreal/bundle/inbox/rocky-arena-1.md`.
