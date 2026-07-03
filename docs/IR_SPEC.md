# Flux IR Specification — the frozen frontend contract

**IR_VERSION = 3** (frozen; `flux_frontend::IR_VERSION`)
Status: normative for v0.36+. Guarded by the `ir_version_frozen` snapshot test in
`crates/flux-frontend/src/mir.rs` and the MIR-drift contract
(`crates/flux-frontend/tests/mir_drift.rs`, `mir-corpus/check.sh`,
`.github/workflows/mir-diff.yml`).

## Why this document exists (FIP-0001)

FIP-0001 (accepted 2026-06-26, Option B) makes rustc a **contracted frontend**: Flux consumes
the *textual* `--emit=mir` output of one pinned rustc (`flux_driver::RUSTC_VERSION`, currently
`1.93.1`) and parses it in exactly ONE place — `flux_frontend::mir::parse_mir`. Everything
downstream (flux-backend Cranelift codegen, the JIT, the ladder rungs) consumes the IR types
below and never learns where the MIR text came from.

A future native frontend (Option A, the north star) emits this same IR directly. That swap is
contained if and only if the IR below stays frozen — hence:

- **Any breaking change to these types MUST bump `IR_VERSION`** and update the
  `ir_version_frozen` test in the same commit. Never silently.
- Additive, non-breaking evolution (a new enum variant consumed behind a match `_` arm, a new
  field with a serde default) still deserves a minor note here, but not necessarily a bump.
  When in doubt, bump.

## The swap-point: `trait Frontend`

```rust
// crates/flux-frontend/src/mir.rs (re-exported at the crate root)
pub trait Frontend {
    fn parse(&self, mir_text: &str) -> Result<Vec<MirFunction>, String>;
}

/// The default, contracted frontend (FIP-0001 Option B): wraps `parse_mir`,
/// the single function that knows rustc's --emit=mir dialect.
pub struct RustcMirFrontend;
```

`RustcMirFrontend.parse(text)` ≡ `parse_mir(text)` — guaranteed by the
`frontend_trait_default_matches_parse_mir` test. Pipeline code takes a `&dyn Frontend` (or a
generic `F: Frontend`); a native Option-A frontend implements `parse` from its own AST and
nothing downstream changes.

## Layer 1 — MIR-level IR (what the backend consumes)

Produced by `parse_mir` from rustc `--emit=mir` text. All types are
`serde::{Serialize, Deserialize}`, so IR can be persisted/cached (the BLAKE3-keyed parse cache
stores exactly this shape).

### `MirFunction`

| field         | type              | meaning                                        |
|---------------|-------------------|------------------------------------------------|
| `name`        | `String`          | function name as printed by rustc (`fn NAME(`) |
| `params`      | `Vec<MirLocal>`   | `_1..=_n` declared in the fn header            |
| `return_type` | `String`          | textual type after `->` (`""`/`()` for unit)   |
| `locals`      | `Vec<MirLocal>`   | `let [mut] _N: TY;` declarations               |
| `blocks`      | `Vec<MirBlock>`   | `bbN: { ... }` basic blocks, in source order   |

`MirLocal { index: usize, name: String, ty: String, mutable: bool }` — `_0` is the return
place. Types stay TEXTUAL at this layer (`"i64"`, `"(i64, i64)"`, `"Shape"`); the backend
lowers them.

`MirBlock { label: String, statements: Vec<MirStmt>, terminator: Option<MirTerminator> }`

### `MirStmt` (frozen shape)

```rust
pub enum MirStmt {
    Assign { dst: String, op: String, args: Vec<String> },
    StorageLive(String),
    StorageDead(String),
    Debug { name: String, local: String },
}
```

`Assign.op` carries the rvalue's operator/constructor spelling (`Add`, `CheckedAdd`, `Const`,
struct/tuple aggregate spellings, …); `args` its operand spellings. The dialect of these
spellings is exactly what the pinned rustc prints — that is the point of the drift contract.

### `MirTerminator` (frozen shape)

```rust
pub enum MirTerminator {
    Return,
    Goto(String),
    Assert { cond: String, target: String },
    Call { func: String, args: Vec<String>, dst: String, target: String },
    SwitchInt { discr: String, targets: Vec<(String, String)>, otherwise: String },
    Unreachable,
}
```

## Layer 2 — High-Level IR (`TranslationUnit`, syn-derived)

Produced by `flux_frontend::parse_source/parse_file` (the embryonic Option-A path). Same
freeze discipline.

```rust
pub struct TranslationUnit {
    pub file_path: String,
    pub functions: Vec<FunctionDef>,   // name, visibility, params, return_type, body: Expr, is_async
    pub structs:   Vec<StructDef>,     // name, fields: Vec<FieldDef>, derives
    pub enums:     Vec<EnumDef>,       // name, variants: Vec<EnumVariant>, derives
    pub imports:   Vec<String>,
}
```

Supporting shapes (see `crates/flux-frontend/src/lib.rs` for authoritative definitions):

- `EnumVariant { name, discriminant: i64, fields: Vec<TypeRef> }` — v3 added `fields`
  (data-carrying enum payloads; empty = C-like variant). Layout contract: tagged union
  `[i64 tag, payload…]`, payload slots = max `fields.len()` across variants.
- `TypeRef` — `Unit | I32 | I64 | U32 | U64 | Bool | F32 | F64 | String | Named(String) |
  Ref(Box) | Option(Box) | Vec(Box) | Result(Box, Box) | Unknown`.
- `Expr` — `Literal | Variable | BinaryOp | Unary | Cast | Call | Return | Let | Block | If |
  Empty`; `BinOp` covers `Add..Shr` (16 ops), `UnOp` is `Neg | Not`.

## The drift contract (operational half)

- `flux_driver::RUSTC_VERSION` (`1.93.1`) is the ONLY place the pinned toolchain is named.
- `mir-corpus/*.rs` + `*.mir.expected` are the dialect baselines. Normalization: strip
  trailing whitespace, drop `// MIR for` banner lines, collapse absolute toolchain paths to
  `<path>`.
- Enforced three ways, all diffing the same corpus:
  1. `mir-corpus/check.sh` (dev loop; `--update` regenerates baselines on an intended bump),
  2. `crates/flux-frontend/tests/mir_drift.rs` (runs under `fluxc test -p flux-frontend` and
     flux_combo; skips only when no rustc is on PATH, hard-fails on a wrong-version rustc),
  3. `.github/workflows/mir-diff.yml` (CI).

## Version history

| IR_VERSION | date       | change                                                            |
|-----------:|------------|-------------------------------------------------------------------|
| 3          | 2026-06-28 | `EnumVariant.fields` — data-carrying enum payloads (ladder rung 4) |
| 2          | —          | pre-freeze iterations                                              |
| 1          | —          | initial HIR                                                        |
