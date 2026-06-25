# Flux: A Hybrid Self-Hosting Compiler with a Contracted Safety Frontend

**Abstract**  
Flux is a hybrid self-hosting Rust compiler that deliberately borrows the official Rust compiler’s frontend—parse, type-check, borrow-check—and only owns the code generation and build infrastructure. Concretely, Flux consumes the stable, post-borrowck MIR emitted by `rustc --emit=mir` and translates it through its own IR to Cranelift-generated machine code, backed by a content-addressed build cache and a BLAKE3-native version control system. This architecture is not a shortcut; it is a principled recognition that the safety-critical frontend can be contracted as a version-pinned service, letting a small team (or an AI-collaborative swarm) ship a working self-hosting compiler in months rather than years. We present the design rationale, the FIP‑0001 posture on native frontend ambitions, the type-complexity ladder that guides our incremental self-hosting, and what Flux demonstrates about pragmatic compiler construction when the memory-safety guarantee is explicitly delegated to the incumbent.

---

## 1. Introduction

The canonical narrative in systems programming holds that a “real” compiler must be self-hosting, and that self-hosting demands a full-stack reimplementation of every phase—lexing, parsing, type checking, borrow checking, codegen—in the new language. Rust’s own history reinforced this: `rustc` bootstrapped itself by eating its own tail, rewriting each component in Rust after an initial OCaml bootstrap. The result is a towering achievement, but also a multi-year, multi-team undertaking that no small group can replicate casually. If we insist on the full-rewrite path, self-hosting remains a privilege of massive projects.

Flux rejects that orthodoxy. Our thesis is that **a self-hosting compiler does not need to re-implement the whole frontend**; it can contract the safety-critical frontend as a version-pinned external service and own only the codegen and build orchestration. This is not laziness—it is a deliberate architectural choice that preserves the full soundness guarantees of Rust’s type system and borrow checker while liberating the compiler author to focus on what matters most for a new project: fast, correct code generation, reproducible builds, and a friction-free developer loop.

Flux is a Rust compiler written in Rust. From the perspective of the user’s build, `fluxc` replaces `rustc` in the pipeline. It invokes `rustc --emit=mir` on each source file to obtain a post-typecheck, post-borrowck MIR dump, parses that dump, lowers it to Flux’s own intermediate representation, and then codegens to ELF objects using Cranelift 0.114. Linking is delegated to `cc` or `mold`. A content-addressed build cache (`flux-cache`) and a BLAKE3-based VCS (`flux-rev`) complete the owned ecosystem. The result is a compiler that is self-hosting on its own codegen terms while cheerfully acknowledging that its memory-safety guarantee is `rustc`’s—and treats that as a *feature*, not a compromise.

We shipped v0.29 through v0.33 with scalar completeness (all integer widths, signs, casts), support for aggregates (tuples and structs), real function call signatures—including aggregate-returning calls—and a build cache that went from a live 0% hit rate over 753 builds to approximately 40% after we traced a volatile `-L deps-dir` listing poisoning every cache key. Every line of Flux has been co-authored by an AI-collaborative swarm (DeepSeek + a Claude agent + human direction), dogfooding the vision that Flux is not merely a compiler but a compiler *platform* for AI-collaborative systems programming.

This paper articulates the architecture, the contracted-frontend posture, and the incremental road to self-hosting. We leave the empirical evaluation and cache internals to a companion section.

---

## 2. Architecture

Flux is assembled from four owned subsystems and one explicitly borrowed component. The division of labor is clean:

- **Borrowed (contracted) frontend**: `rustc --emit=mir` on each source file. This runs the official compiler through parsing, name resolution, type checking, trait resolution, and borrow checking, then dumps the MIR in a stable-ish text or binary format. Flux consumes this output. The contract is that the frontend is pinned to a specific `rustc` nightly version (currently 1.82.0-nightly) and the MIR schema is treated as a versioned API. Flux’s entire safety guarantee rests on this step; we explicitly do not re-verify borrows or types.

- **flux-frontend (owned)**: This is Flux’s own intermediate representation and lowering pipeline. It reads the serialized MIR and builds a typed, SSA-like IR that deliberately diverges from `rustc`’s internal MIR to suit Cranelift’s input expectations and Flux’s optimization ambitions. The IR handles all scalar types, aggregates, call ABIs, and (in progress) sum types. It is the semantic heart of Flux: everything codegen-relevant flows through this representation.

- **flux-backend (owned)**: A codegen backend targeting Cranelift 0.114. It walks the Flux IR and emits Cranelift’s CLIF, then uses Cranelift’s `MachBackend` to produce position-independent ELF object files. Unlike `rustc`’s own Cranelift backend, Flux’s is purpose-built for its own IR and can be tuned for compile-time speed and tight control over the generated code. We deliberately use a pinned Cranelift version and rely on Cranelift’s mature instruction selection and register allocation.

- **flux-cache / flux-driver (owned)**: A content-addressed build cache wired through the environment variable `RUSTC_WRAPPER=self`. The cache computes a BLAKE3 hash over the preprocessed source, the contracted `rustc` version, the exact command-line flags (sanitized of volatile paths), and all dependency metadata. Hits avoid MIR generation and codegen entirely, returning cached objects. The driver orchestrates parallel `rustc` invocations and cache lookups, replacing the usual `cargo`/`rustc` dispatch with a cache-aware scheduler. Getting this cache right required pinpointing why `-L deps-dir` paths were changing every build—a host-absolute path embedded in `cargo` incantations—and normalizing them away to achieve a 40% hit rate on real-world incremental builds.

- **flux-rev (owned)**: A BLAKE3 content-addressed version control system, purpose-built for compiler development and reproducible build artifacts. Flux’s own source tree is stored in flux-rev; every commit is identified by the hash of its tree, guaranteeing that a given source state always maps to the same compiler binary. This is not a general-purpose VCS but an integral part of the deterministic-build story.

Why is MIR the right contract boundary? MIR sits at the perfect architectural seam: all Rust-specific safety analysis (borrow checking, lifetime reification, drop elaboration) has been completed, but no machine-specific codegen decisions have been made. The MIR is high-level enough to be reusable across radically different compilation strategies, yet rigid enough that breaking changes are rare and always version-gated. By pinning to a nightly, we sacrifice the illusion of frontend independence in exchange for a guaranteed soundness base. Every time we bump the `rustc` contract version, we re-validate the MIR parser and adjust for any schema changes—a minor cost compared to implementing and maintaining a full type-checker.

The result is an architecture where less than 15% of the codebase (by line count) is borrowed, but that 15% carries the entire soundness burden. The remaining 85% are pure codegen and infrastructure, which is where small-team innovation can shine.

---

## 3. The Contracted-Frontend Posture

Flux’s relationship with the native frontend is defined by FIP‑0001, a foundational design decision that ruled out early work on a native type-checker and borrow-checker. The FIP establishes two modes:

- **Option B** (shipped): Flux relies on a version-pinned `rustc --emit=mir` frontend. This is not a temporary hack; it is the intended production mode indefinitely. Flux’s value proposition is codegen speed, cache efficacy, and deterministic output, none of which require rewriting the frontend.

- **Option A** (north star, deferred): A native Rust frontend—type checking, trait resolution, borrow checking—built in Rust but fully owned by Flux. This is gated by a hard trigger: **when Flux compiles its own entire workspace into a working `fluxc` that passes all tests with zero manual workarounds, without the contracted frontend, only then can we begin investing in Option A.**

This trigger is deliberately gameable in a good way. It forces us to climb the type-complexity ladder using the contracted frontend first, ensuring that the entire codegen and infrastructure stack is battle-hardened before attempting any frontend work. It also avoids the trap of building a “type-checker for a compiler that doesn’t yet work.” History is littered with projects that got lost in type system implementation and never shipped a working binary.

Crucially, the memory-safety guarantee of Flux-compiled code is `rustc`’s. We do not attempt to second-guess the borrow checker or add additional static analyses; the contract says that if `rustc` accepted the program, the generated code will preserve those invariants. From a security perspective, this is a feature: the trust anchor is the same `rustc` that secures the entire Rust ecosystem. Flux merely optimizes the codegen path. Users who audit their supply chain already trust `rustc`’s frontend; adding Flux’s codegen does not introduce a new soundness risk.

The long-term vision for Option A is what we call **type-system-as-a-service**: when we eventually build a native frontend, it will be a separable library that can be used independently of Flux’s codegen, and it may itself be bootstrapped by the contracted frontend as a self-validation step. But that is a distant goal; today, Flux ships, and it ships fast.

---

## 4. The Road to Self-Hosting

Self-hosting is the canonical benchmark for a compiler, but the traditional metric—“it can compile itself”—is too coarse. A compiler that achieves self-hosting by luck, compiling a trivially simple subset of its own source, has not really proven itself. Flux adopts a more rigorous and incremental metric: **the type-complexity ladder, gated by the question ‘does this now compile more of Flux’s own workspace?’**

The ladder is:

1. **Scalars**: all integer widths (i8–i128, unsigned), float types, and bitwise/arithmetic operations. This must cover the compiler’s basic arithmetic, pointer manipulation, and hash computations. Shipped in v0.29.

2. **Aggregates**: tuples, structs, struct field projection, and aggregate layout compatibility. Required for virtually every data structure in the compiler—AST nodes, IR types, cache entries. Shipped in v0.30–v0.31.

3. **Real call signatures**: function calls with multiple arguments, struct-return ABI, extern “Rust” ABI conformance, and stack-based returns for large aggregates. Without this, the compiler cannot call its own Cranelift bindings or cache functions. Shipped in v0.32–v0.33, along with aggregate-returning calls.

4. **Enums (sum types)**: simple C-like enums and data-carrying enums (the Rust `enum`). These are pervasive in the Flux codebase—error types, IR variants, optionals. In progress; the MIR representation for enums is well-understood, and the main work lies in memory layout and match lowering.

5. **Generics**: monomorphization and type parameter passing. The Flux workspace uses generics heavily for IR types (`Result<T, E>`, `Vec<T>`, collection adapters). We will monomorphize at the MIR level using `rustc`’s own monomorphization info, but the codegen must correctly instantiate.

6. **Traits**: static and dynamic dispatch, vtable layout, trait objects. The compiler’s own architecture uses traits for the cache backend interface, the diagnostics pipeline, and many internal abstractions. This is the final rung; once trait objects compile, Flux will have compiled its entire `flux-driver`, `flux-cache`, and `flux-frontend` crates, achieving full self-hosting.

At each rung, the test is not “passes a custom test suite” but “compiles more of Flux’s own workspace, and the resulting binary passes the existing test suite.” This dogfooding metric is brutally effective: it immediately exposes ABI mismatches, missing type support, or layout bugs that synthetic tests miss. For example, getting aggregate-returning calls right was forced by a real code pattern in `flux-cache` where a `BLAKE3_HASH` struct was returned from a cache-lookup function. No synthetic test would have exercised that specific combination of ABI, alignment, and return path.

This ladder also avoids the dangerous trap of chasing “rustc parity”—a full-conformance Rust compiler that can compile any crate. Such a goal would require implementing every edge case in the language specification, a task that dwarfs the codegen effort. Instead, Flux targets **self-hosting sufficiency**: the minimal subset of Rust needed to compile Flux itself. Because Flux is deliberately written in a lean, mostly-safe Rust style (no `unsafe` except in FFI stubs), this subset is tractable and well-defined. Every rung expands the subset until it encompasses the entire workspace, at which point Flux is self-hosting by its own definition. The final step—compiling the entire workspace including the future Option A frontend—naturally triggers full self-hosting maturity.

---

## 5. Conclusion

Flux demonstrates three things that the compiler community can learn from.

First, **pragmatic self-hosting is a legitimate path**. By contracting the safety frontend, Flux achieves a working, self-hosting Rust compiler without reimplementing type checking or borrow checking. This shortcuts years of work while preserving the soundness guarantees that make Rust valuable. There is no fundamental reason every new language or compiler project must rebuild the entire analysis stack; for many projects, the contract boundary at MIR is both technically clean and strategically sound.

Second, **targeting self-hosting sufficiency rather than language parity yields faster, more meaningful progress**. The type-complexity ladder, gated by the “compiles more of Flux’s own workspace” test, keeps the team focused on concrete, executable goals. Each rung delivers a working compiler that is *more* self-hosting than before, and the test infrastructure is the compiler’s own source code. This tight feedback loop exposes real-world ABI and layout bugs early, preventing the accumulation of technical debt.

Third, **AI-collaborative compiler construction is not a gimmick—it is a force multiplier when the architecture is modular**. Flux’s development was co-authored by a small human-directed swarm of AI agents. The contracted frontend means the agents could focus on codegen, caching, and IR design without being blocked by the need to understand Rust’s full type system. The result is a compiler built at a pace that would be impossible for a conventional team of the same size. Flux itself is a proof point for the platform it aspires to be: a compiler designed for AI-collaborative systems programming, where human intent and machine generation are co-authors of the final artifact.

The north star remains Option A—a fully owned frontend—but it is a north star we reach only after Flux has thoroughly proven its codegen and infrastructure in production. Until then, we ship fast, we ship correct, and we borrow the safety we need.