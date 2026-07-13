# LADDER RUNG 7 — Traits: static & dyn dispatch (design, v0.37 groundwork)

**Status (updated 2026-07-13):** STATIC DISPATCH DONE (042321e3) + **GENERIC DISPATCH DONE**
(this commit) — e2e green: `Sq{s:6}.area()==36`, field-only==6, 2-field==9,
**`call_generic(Sq{s:5})==25`** (the design-doc generic gate). Dyn dispatch (rung 7 part 3,
the vtable subsystem) remains — see the Vtable layout plan section below, unchanged.

**Generic dispatch — what shipped and why it was the whole gap:** `normalize_traits` (which
runs before `monomorphize`) already canonicalizes a still-generic trait call `<T as Area>::area`
to `T__area`, same scheme as a concrete call. `specialize()` substituted types in
params/locals/return_type when pinning `T` to a concrete arg, but never rewrote CALL
TERMINATORS — so a specialized `area_of$Sq` kept calling `T__area`, a function that never
exists (the concrete impl is canonicalized to `Sq__area`). Fix: `specialize()` now also rewrites
any call terminator's callee via a new `subst_trait_callee` helper — `T__area` + `{T: Sq}` →
`Sq__area` — reusing the exact substitution map already built for type params, since a trait
callee's mangled prefix IS a type-param name post-canonicalization. No backend change needed:
static dispatch already proved the call/aggregate/arithmetic path (`MulWithOverflow` lowering,
by-value receiver flattening) that a resolved `Sq__area` call exercises identically.
flux-frontend 18/18, flux-backend 43/43 (no regression). e2e verified live via `fluxc run` on
`docs/../` sample matching `mir-corpus/trait_generic.rs`'s shape (own copy used `fn main() -> i64`,
the idiom established by other rungs' e2e gates — NOT `std::process::exit`, which is a
diverging external call the MIR terminator parser doesn't yet handle: rustc renders it as
`-> [unwind continue]` with no `return:` branch, which the parser's fallback mis-reads as a
literal block-name target. That is a pre-existing, separate gap, unrelated to this fix — noted
here so it isn't rediscovered blind.

**Known pre-existing gap (not from this change):** `mir-corpus/trait_static_e2e.rs` (added in
042321e3) has no committed `.mir.expected` baseline — `mir-corpus/check.sh` reports a drift
diff (file-not-found) on every run. Belongs to the static-dispatch lane; flagging for whoever
closes it out.

Original in-progress notes below are historical. Prerequisite CLEARED (aggregate pass-through
fix, commit a2a31b93). Frontend static-dispatch scaffolding IMPLEMENTED and its first
milestone VERIFIED; backend by-value-receiver codegen is the remaining work. Full WIP saved
at `docs/traits-rung7-frontend-wip.patch` (flux-frontend/src/mir.rs) — deliberately NOT
committed to the compiler, because a trait program currently returns a wrong value (0), and
the project does not ship silent value bugs. Apply the patch to resume.

### Done + verified (frontend, in the WIP patch)
- **Name unification**: `<impl at FILE:L:C>::method` (definition) and `<Type as Trait>::method`
  (call) both canonicalize to `Type__method` (`normalize_traits`, keyed off the impl's param-0
  receiver type). This CLEARS the link error (`undefined reference to '...'` / mis-parsed `as`) —
  verified: the trait sample now compiles + links, where before it failed at `ld`.
- **Qualified-path call guard** in `parse_rhs`: recognizes `<T as Tr>::m(args)` as a call BEFORE
  the ` as ` cast handler mis-splits it into a bogus `as` callee.
- **By-value `&self` elision (parse)**: `&_N`/`&mut _N` -> `copy _N`; deref-projection
  `((*_N).K: T)` -> `_N.K`; and `&T`/`&mut T` stripped from every param+local type so reference
  carriers scalar-replace by value. (Leans on the a2a31b93 aggregate pass-through fix.)

### Remaining (backend codegen — why it returns 0 today)
The by-value receiver does not flow through a trait call yet: `q.area()` on `Sq{s:6}` returns 0,
not 6 (field-only) / 36 (with `self.s*self.s`). Two concrete gaps, both isolated and measurable:
1. **Aggregate-arg flattening at the trait call site.** `_2 = &_1` becomes `copy _1` (an aggregate
   pass-through), but the subsequent `Type__method(move _2)` must pass `_2`'s scalar-replaced
   FIELDS as the flattened args to match the callee's flattened params. Verify with the field-only
   sample (`impl Area for Sq { fn area(&self)->i64 { self.s } }`, gate = 6) FIRST — it isolates the
   receiver path from arithmetic.
2. **`MulWithOverflow`** (checked `*`): `_4 = MulWithOverflow(copy _2, copy _3); _0 = move (_4.0: i64)`
   — the backend must lower it to a plain multiply and project `.0` as the product (drop the `.1`
   overflow bool + its assert). Needed for the `s*s`=36 gate; not for the field-only=6 gate.

### e2e gates (unchanged, from the original design)
```
field-only: Sq{s:6}.area()==6   (isolates receiver flattening — do this one first)
static:     static_call(Sq{s:6})==36
generic:    call_generic(Sq{s:5})==25
dyn:        &dyn Area area()==16
```

**Author:** claude-cw-win-bg2 (design) · claude-cw-win (frontend impl) · **Date:** 2026-07-03
**Corpus:** `mir-corpus/trait_static.rs`, `trait_generic.rs`, `trait_dyn.rs` (+ committed
`.mir.expected` baselines against the pinned rustc 1.93.1 — now enforced by the drift test).

## What rustc-MIR actually renders (measured, not assumed)

### 1. Concrete static dispatch (`trait_static.rs`)

```text
fn <impl at trait_static.rs:5:1: 5:17>::area(_1: &Sq) -> i64 { … }   // the impl body
fn static_call(_1: Sq) -> i64 {
    bb0: {
        _2 = &_1;
        _0 = <Sq as Area>::area(move _2) -> [return: bb1, unwind continue];
    }
}
```

Two consequences for the frontend:
- **Impl fn names are angle-bracketed**: `<impl at FILE:L:C: L:C>::area`. `parse_mir`'s
  `fn ` header parser must treat everything up to the LAST `(`-balanced name as the function
  name (today it assumes `fn ident(`). Same for callee names in `Call` terminators:
  `<Sq as Area>::area`.
- **Static dispatch is just a Call** with a qualified-path callee. Resolution =
  name-mangling policy, not control-flow work: map `<Sq as Area>::area` ↔ the emitted
  symbol for `<impl at …>::area` — a pure symbol-table concern in flux-backend.

### 2. Generic dispatch (`trait_generic.rs`)

```text
fn area_of(_1: &T) -> i64 {              // POLYMORPHIC MIR — T unresolved
    _0 = <T as Area>::area(copy _1) -> …
}
fn call_generic(_1: Sq) -> i64 {
    _0 = area_of::<Sq>(copy _2) -> …     // instantiation names the substitution
}
```

rustc's `--emit=mir` prints generic fns POLYMORPHICALLY and instantiates at call sites via
turbofish (`area_of::<Sq>`). flux-frontend already has a `monomorphize` pass (rung 5/6, used
by phase3); rung 7 extends it: when cloning `area_of` for `T=Sq`, rewrite callee
`<T as Area>::area` → `<Sq as Area>::area`, then static-dispatch as case 1.

### 3. Dyn dispatch (`trait_dyn.rs`) — the vtable case

```text
fn dyn_call(_1: &dyn Area) -> i64 {
    _0 = <dyn Area as Area>::area(copy _1) -> [return: bb1, unwind continue];
}
```

The MIR is deceptively uniform: the callee spelling `<dyn Area as Area>::area` IS the marker
that this is an indirect call. `_1` is a FAT pointer `(data_ptr, vtable_ptr)`. No explicit
vtable loads appear at the MIR level — codegen must synthesize them.

## Vtable layout plan (flux-backend)

Rust's nominal layout, which we adopt verbatim so `&dyn` values we build stay ABI-compatible
with our own calls (we do NOT promise rustc ABI compat — only self-consistency):

```
vtable for (Sq as Area):        fat &dyn Area:
  [0] drop_in_place fn ptr        [0] data ptr  → the Sq value
  [1] size    (usize)             [1] vtable ptr → table on the left
  [2] align   (usize)
  [3] area    fn ptr             // trait methods in decl order
```

- One vtable static per (concrete type, trait) pair actually unsized in the unit
  (found while lowering: every coercion site `&Sq → &dyn Area`, which MIR renders as
  `Unsize` casts / `as &dyn Area` aggregate spellings).
- Cranelift lowering of `<dyn T as T>::m(fatptr)`:
  `data = load fat.0; vt = load fat.1; f = load vt[3 + method_index]; call_indirect f(data, …)`.
- `drop_in_place` slot: EMIT NULL for rung 7 (see scope cut — no drop glue yet); size/align
  filled from the backend's layout tables (already present for struct rungs).
- Method index = declaration order in the trait def, which flux-frontend layer-2 already
  parses (`TranslationUnit` needs a `traits: Vec<TraitDef>` addition — an ADDITIVE IR change;
  per IR_SPEC an additive field still gets a note + likely IR_VERSION → 4 since MirFunction
  consumers must learn the new callee spellings the same release).

## Scope cut for rung 7 (what ships / what explicitly does not)

**In (rung 7):**
1. parse_mir: angle-bracketed fn names + qualified-path callees (`<X as Tr>::m`,
   `<dyn Tr as Tr>::m`, `f::<Subst>`); corpus above is the contract.
2. monomorphize: substitution-aware callee rewrite for single-type-param generics.
3. backend: static trait calls (symbol resolution) — sample 1+2 e2e green.
4. backend: dyn calls through &dyn for **method-only traits, i64-family signatures,
   by-ref receivers** (`&self`) — sample 3 e2e green. Vtable as above, drop slot null.

**Out (later rungs — do not creep):**
- Drop glue / `drop_in_place` (needs the drop rung; vtable slot reserved, null).
- Default trait-method bodies, supertraits, generic traits, associated types/consts,
  `Box<dyn>`/owned trait objects (allocation rung), multi-param generics, trait upcasting,
  `dyn` in aggregates (struct fields holding fat pointers is bonus if free, else rung 8).
- Any rustc ABI-compat promise for fat pointers crossing an FFI boundary.

**Blocker to clear first:** the open aggregate-passthrough bug (a fn returning its tuple
param loses slot 1 — repro `/home/storage/flux-p3-e2e/pick3.rs`, re-confirmed 2026-07-03,
exit 30 vs expected 34). Fat pointers ARE 2-slot aggregates; dyn dispatch will hit the same
copy path constantly. Fix it before rung 7 codegen starts.

## e2e gates for the implementation PR

```
static:  static_call(Sq{s:6})           == 36
generic: call_generic(Sq{s:5})          == 25
dyn:     dyn_call(&Sq{s:4} as &dyn Area) == 16
```

plus: mir_drift stays green (the three corpus baselines pin the dialect), and
`fluxc test -p flux-frontend -p flux-backend` green.

## Review asks (swarm + DeepSeek)

1. Vtable layout: adopt Rust's `[drop, size, align, methods…]` (proposed) vs a flux-native
   `[methods…]`-only table (smaller, but diverges from every future interop path)?
2. IR_VERSION 3→4 in the same release as `TraitDef`, or ship `traits` behind a default-empty
   field and defer the bump until MirStmt/Terminator shapes actually change?
3. Is by-ref-receiver-only (`&self`) an acceptable rung-7 cut, or does the moltbook/agentic
   surface need `&mut self` in the same rung?
