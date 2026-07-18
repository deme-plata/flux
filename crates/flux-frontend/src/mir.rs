// flux-frontend/mir.rs — MIR parser (rustc --emit=mir output)
//
// Parses the text MIR format that rustc --emit=mir produces.
// Extracts function signatures, locals, basic blocks, and statements.
// Feeds into flux-backend for Cranelift codegen.
//
// MIR format example:
//   fn add(_1: i64, _2: i64) -> i64 {
//       debug a => _1;
//       let mut _0: i64;
//       bb0: {
//           _0 = Add(_1, _2);
//           return;
//       }
//   }

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirFunction {
    pub name: String,
    pub params: Vec<MirLocal>,
    pub return_type: String,
    pub locals: Vec<MirLocal>,
    pub blocks: Vec<MirBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirLocal {
    pub index: usize,
    pub name: String,
    pub ty: String,
    pub mutable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirBlock {
    pub label: String,
    pub statements: Vec<MirStmt>,
    pub terminator: Option<MirTerminator>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MirStmt {
    Assign { dst: String, op: String, args: Vec<String> },
    StorageLive(String),
    StorageDead(String),
    Debug { name: String, local: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MirTerminator {
    Return,
    Goto(String),
    Assert { cond: String, target: String },
    /// Function call: `dst = func(args...) -> [return: target, ...]`
    Call { func: String, args: Vec<String>, dst: String, target: String },
    /// `switchInt(operand) -> [value: target, ..., otherwise: fallback]`
    SwitchInt { discr: String, targets: Vec<(String, String)>, otherwise: String },
    /// `unreachable;` — rustc's exhaustive-match `otherwise` arm. Lowers to a Cranelift trap.
    Unreachable,
}

/// FIP-0001 keep-A-open #3: the swap-point between "produce Flux MIR" and the rest of the compiler.
///
/// Today the only Frontend is `RustcMirFrontend`, which parses rustc's `--emit=mir` text (Option B, the
/// version-pinned contracted frontend). A future native frontend (Option A) implements this same trait to
/// emit `MirFunction` directly from a `syn` AST — with zero changes anywhere downstream, because the
/// pipeline depends on `Frontend`, not on where the MIR came from. This is a zero-cost abstraction
/// (static dispatch); it exists to mark the intent and give the native parser a clean injection point.
pub trait Frontend {
    /// Produce Flux MIR for a translation unit from this frontend's source form.
    /// (v0.36: named `parse` per the IR_SPEC contract; was `to_mir`.)
    fn parse(&self, mir_text: &str) -> Result<Vec<MirFunction>, String>;
}

/// The default, contracted frontend: parse rustc's `--emit=mir` textual output.
pub struct RustcMirFrontend;

impl Frontend for RustcMirFrontend {
    fn parse(&self, mir_text: &str) -> Result<Vec<MirFunction>, String> {
        parse_mir(mir_text)
    }
}

// ── v0.36 parse-mir IR cache (phase 3, DeepSeek adapt) ─────────────────────
//
// parse_mir output is a pure function of its input text, so it is cached
// content-addressably: BLAKE3(mir_text) → JSON of Vec<MirFunction> at
// `<flux_cache::cache_dir()>/mir-ir/<2ch>/<rest>-v<IR_VERSION>.json`.
// The IR_VERSION suffix invalidates every entry on an intentional IR bump
// (and serde shape drift falls back to a re-parse anyway). Transparent to
// callers — phase3's compile path (`fluxc run`) just calls parse_mir.
// FLUX_MIR_CACHE=0 disables; FLUX_MIR_CACHE_TRACE=1 prints HIT/MISS/STORE.

static MIR_CACHE_HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static MIR_CACHE_MISSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static MIR_CACHE_STORES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// (hits, misses, stores) for this process — proof hook for the phase-3 gate.
pub fn mir_parse_cache_stats() -> (u64, u64, u64) {
    use std::sync::atomic::Ordering::Relaxed;
    (
        MIR_CACHE_HITS.load(Relaxed),
        MIR_CACHE_MISSES.load(Relaxed),
        MIR_CACHE_STORES.load(Relaxed),
    )
}

fn mir_ir_cache_path(key: &str) -> std::path::PathBuf {
    flux_cache::cache_dir()
        .join("mir-ir")
        .join(&key[..2])
        .join(format!("{}-v{}.json", &key[2..], crate::IR_VERSION))
}

/// Parse MIR text output from rustc --emit=mir.
///
/// v0.36: cached — see the block comment above. The actual parser is
/// `parse_mir_uncached`; behavior is byte-for-byte identical on hit vs miss
/// for every Ok result (parse errors are never cached).
pub fn parse_mir(mir_text: &str) -> Result<Vec<MirFunction>, String> {
    use std::sync::atomic::Ordering::Relaxed;
    if std::env::var("FLUX_MIR_CACHE").as_deref() == Ok("0") {
        return parse_mir_uncached(mir_text);
    }
    let trace = std::env::var("FLUX_MIR_CACHE_TRACE").is_ok();
    let key = blake3::hash(mir_text.as_bytes()).to_hex().to_string();
    let path = mir_ir_cache_path(&key);
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(funcs) = serde_json::from_slice::<Vec<MirFunction>>(&bytes) {
            MIR_CACHE_HITS.fetch_add(1, Relaxed);
            if trace {
                eprintln!("FLUXMIR HIT key={} ({} fns, parse skipped)", &key[..16], funcs.len());
            }
            return Ok(funcs);
        }
        // Unreadable/stale-shape entry: fall through to a real parse (re-stored below).
    }
    MIR_CACHE_MISSES.fetch_add(1, Relaxed);
    if trace {
        eprintln!("FLUXMIR MISS key={}", &key[..16]);
    }
    let funcs = parse_mir_uncached(mir_text)?;
    // Best-effort atomic store (tmp + rename). Any I/O failure is silent —
    // the cache must never break a compile.
    if let Ok(json) = serde_json::to_vec(&funcs) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        if std::fs::write(&tmp, &json).is_ok() {
            if std::fs::rename(&tmp, &path).is_ok() {
                MIR_CACHE_STORES.fetch_add(1, Relaxed);
                flux_cache::add_external_bytes(json.len() as u64);
                if trace {
                    eprintln!("FLUXMIR STORE key={}", &key[..16]);
                }
            } else {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }
    Ok(funcs)
}

/// The raw rustc `--emit=mir` text parser (the single place that knows the
/// dialect — FIP-0001). Public for benchmarks/diagnostics; production code
/// should call `parse_mir` (cached).
/// Rung 8 (closures): rustc renders the closure's synthetic capture type as
/// `{closure@FILE:L:C: L:C}` — braces inside type positions that break the
/// line-oriented parser (a construction's `find('{')` hits the TYPE's brace).
/// Pre-lex every occurrence to a deterministic flat identifier
/// `__closure_<blake3-8>` of the full site string: same site → same name in
/// the local decls, the construction, the `<.. as Fn..>::call` site, and the
/// closure fn's receiver param — which is exactly the agreement the trait
/// canonicalizer needs downstream.
fn flatten_closure_types(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains("{closure@") {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("{closure@") {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        match after.find('}') {
            Some(j) => {
                let site = &after[..=j];
                let h = blake3::hash(site.as_bytes()).to_hex().to_string();
                out.push_str("__closure_");
                out.push_str(&h[..8]);
                rest = &after[j + 1..];
            }
            None => {
                out.push_str(after);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

pub fn parse_mir_uncached(mir_text: &str) -> Result<Vec<MirFunction>, String> {
    let mir_text = flatten_closure_types(mir_text);
    let mut functions = Vec::new();
    let mut lines = mir_text.lines().peekable();

    // Skip the warning header
    while let Some(line) = lines.peek() {
        if line.starts_with("fn ") { break; }
        lines.next();
    }

    while let Some(line) = lines.peek() {
        if line.starts_with("fn ") {
            let func = parse_function(&mut lines)?;
            functions.push(func);
        } else {
            lines.next();
        }
    }

    Ok(functions)
}

fn parse_function<'a, I>(lines: &mut std::iter::Peekable<I>) -> Result<MirFunction, String>
where I: Iterator<Item = &'a str>
{
    let header = lines.next().unwrap().trim().to_string();
    // fn add(_1: i64, _2: i64) -> i64 {
    let (name, params, return_type) = parse_fn_header(&header)?;

    let mut locals = Vec::new();
    let mut blocks = Vec::new();
    // Track brace depth so nested `scope N { ... }` blocks (used by rustc for variable
    // lifetime info) don't prematurely end the function body.
    let mut depth = 1usize;

    while let Some(line) = lines.peek() {
        let trimmed = line.trim();
        if trimmed.starts_with("debug ") {
            let parts: Vec<&str> = trimmed[6..].split("=>").collect();
            if parts.len() == 2 {
                let local_idx = parts[1].trim().trim_end_matches(';').trim_start_matches('_').parse().unwrap_or(0);
                locals.push(MirLocal {
                    index: local_idx,
                    name: parts[0].trim().to_string(),
                    ty: String::new(),
                    mutable: false,
                });
            }
            lines.next();
        } else if trimmed.starts_with("let mut ") || trimmed.starts_with("let ") {
            let is_mut = trimmed.starts_with("let mut ");
            let rest = trimmed.trim_start_matches("let mut ").trim_start_matches("let ");
            if let Some(colon) = rest.find(':') {
                let local_name = rest[..colon].trim().trim_start_matches('_');
                let ty = rest[colon+1..].trim().trim_end_matches(';');
                locals.push(MirLocal {
                    index: local_name.parse().unwrap_or(0),
                    name: format!("_{}", local_name),
                    ty: ty.to_string(),
                    mutable: is_mut,
                });
            }
            lines.next();
        } else if trimmed.starts_with("bb") && trimmed.ends_with('{') {
            // parse_block consumes its own matching close brace, so depth stays balanced.
            let block = parse_block(lines)?;
            blocks.push(block);
        } else if trimmed.ends_with('{') {
            // `scope N {` or any other unhandled open block — track and skip.
            depth += 1;
            lines.next();
        } else if trimmed == "}" {
            depth -= 1;
            lines.next();
            if depth == 0 { break; }
        } else {
            lines.next();
        }
    }

    Ok(MirFunction { name, params, return_type, locals, blocks })
}

fn parse_fn_header(header: &str) -> Result<(String, Vec<MirLocal>, String), String> {
    // fn add(_1: i64, _2: i64) -> i64 {
    let rest = header.trim_start_matches("fn ").trim_end_matches(" {");
    let paren_idx = rest.find('(').ok_or("no (")?;
    let name = rest[..paren_idx].trim().to_string();

    // Match the param-list close paren by NESTING depth — a tuple/struct param type like
    // `(i64, i64)` carries its own parens, so the FIRST `)` is not the list end (that bug parsed
    // `_1: (i64, i64)` as ty `"(i64"` plus a phantom empty param).
    let mut pdepth = 0i32;
    let mut close_paren = None;
    for (j, b) in rest.bytes().enumerate().skip(paren_idx) {
        match b {
            b'(' => pdepth += 1,
            b')' => { pdepth -= 1; if pdepth == 0 { close_paren = Some(j); break; } }
            _ => {}
        }
    }
    let close_paren = close_paren.ok_or("no )")?;
    let params_str = &rest[paren_idx+1..close_paren];
    let return_part = &rest[close_paren+1..];

    // Trim BEFORE stripping the arrow: rustc renders `) -> T` so return_part has a
    // leading space (" -> T"). trim_start_matches("-> ") can't match past that space,
    // which left the arrow glued on ("-> (i64,i64)") and made parse_tuple_type's
    // starts_with('(') fail -> tuple/struct returns silently collapsed to 1 value.
    let return_type = return_part.trim().trim_start_matches("->").trim().to_string();

    let mut params = Vec::new();
    // Split on TOP-LEVEL commas only: a comma inside a tuple/struct/generic param type
    // (`(i64, i64)`, `Wrap<A, B>`) must not start a new param.
    for (i, p) in split_top_level_commas(params_str).iter().enumerate() {
        let p = p.trim();
        if p.is_empty() { continue; }
        let parts: Vec<&str> = p.splitn(2, ':').collect();
        let name = parts[0].trim().to_string();
        let ty = parts.get(1).map(|s| s.trim().to_string()).unwrap_or_default();
        params.push(MirLocal { index: i + 1, name, ty, mutable: false });
    }

    Ok((name, params, return_type))
}

/// Split a comma list on TOP-LEVEL commas only, leaving commas nested inside (), [], <>, {}
/// untouched — so an inline tuple/struct/generic type like `(i64, i64)` stays a single element.
fn split_top_level_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for ch in s.chars() {
        match ch {
            '(' | '[' | '<' | '{' => { depth += 1; cur.push(ch); }
            ')' | ']' | '>' | '}' => { if depth > 0 { depth -= 1; } cur.push(ch); }
            ',' if depth == 0 => out.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    out.push(cur);
    out
}

fn parse_block<'a, I>(lines: &mut std::iter::Peekable<I>) -> Result<MirBlock, String>
where I: Iterator<Item = &'a str>
{
    let header = lines.next().unwrap().trim().to_string();
    // Rung 8: unwind-path blocks render as `bb3 (cleanup): {` — strip the marker.
    let label = header.trim_end_matches(": {")
        .trim_end_matches(" (cleanup)").to_string();

    let mut statements = Vec::new();
    let mut terminator = None;

    while let Some(line) = lines.peek() {
        let trimmed = line.trim();
        if trimmed == "}" {
            lines.next();
            break;
        }

        if trimmed.starts_with("drop(") {
            // Rung 8: `drop(_1) -> [return: bb2, unwind …];` — generic-by-value
            // params get drop glue. Every type in this pipeline is scalar-
            // replaced plain data (no Drop impls, no heap), so the drop is a
            // no-op: lower to a Goto of the return target.
            let target = trimmed.find("return: ")
                .map(|i| trimmed[i + 8..].split([',', ']']).next().unwrap_or("").trim().to_string())
                .unwrap_or_default();
            terminator = Some(MirTerminator::Goto(target));
            lines.next();
        } else if trimmed.starts_with("resume") || trimmed.starts_with("terminate") {
            // Unwind-only exits — unreachable in our no-unwind world.
            terminator = Some(MirTerminator::Unreachable);
            lines.next();
        } else if trimmed.starts_with("return") {
            terminator = Some(MirTerminator::Return);
            lines.next();
        } else if trimmed.starts_with("goto") {
            let target = trimmed[4..].trim()
                .trim_start_matches("->").trim()
                .trim_end_matches(';')
                .to_string();
            terminator = Some(MirTerminator::Goto(target));
            lines.next();
        } else if trimmed.starts_with("switchInt") {
            // switchInt(move _3) -> [0: bb2, otherwise: bb1];
            let paren_open = trimmed.find('(').map(|i| i + 1).unwrap_or(trimmed.len());
            let paren_close = trimmed.find(')').unwrap_or(trimmed.len());
            let discr = trimmed[paren_open..paren_close]
                .trim()
                .trim_start_matches("copy ")
                .trim_start_matches("move ")
                .to_string();
            let arrow_pos = trimmed.find("->").map(|i| i + 2).unwrap_or(trimmed.len());
            let bracket = trimmed[arrow_pos..]
                .trim()
                .trim_end_matches(';')
                .trim_start_matches('[')
                .trim_end_matches(']');
            let mut targets: Vec<(String, String)> = Vec::new();
            let mut otherwise = String::new();
            for part in bracket.split(',') {
                let part = part.trim();
                if let Some(colon) = part.find(':') {
                    let key = part[..colon].trim();
                    let val = part[colon+1..].trim();
                    if key == "otherwise" {
                        otherwise = val.to_string();
                    } else {
                        targets.push((key.to_string(), val.to_string()));
                    }
                }
            }
            terminator = Some(MirTerminator::SwitchInt { discr, targets, otherwise });
            lines.next();
        } else if trimmed.starts_with("assert") {
            // assert(!move (_3.1: bool), ...) -> [success: bb1, ...];
            let target = if let Some(idx) = trimmed.find("success: ") {
                let rest = &trimmed[idx+9..];
                rest.split(',').next().unwrap_or("").trim().to_string()
            } else { String::new() };
            terminator = Some(MirTerminator::Assert { cond: String::new(), target });
            lines.next();
        } else if trimmed.starts_with("_") && trimmed.contains('=') && trimmed.contains("->") {
            // Function call as a terminator:
            //   _1 = double(copy _2) -> [return: bb1, unwind continue];
            //   _3 = funcname(move _1) -> bb2;
            let eq_idx = trimmed.find('=').unwrap();
            let dst = trimmed[..eq_idx].trim().to_string();
            let after_eq = trimmed[eq_idx+1..].trim();
            let arrow_idx = after_eq.find("->").unwrap_or(after_eq.len());
            let call_part = after_eq[..arrow_idx].trim();
            let (func, args) = parse_rhs(call_part);
            // Target block: either "[return: bbN, ...]" or bare "bbN"
            let after_arrow = after_eq[arrow_idx+2..].trim();
            let target = if let Some(idx) = after_arrow.find("return:") {
                let rest = &after_arrow[idx+7..];
                rest.split(',').next().unwrap_or("")
                    .trim()
                    .trim_matches(|c: char| !c.is_ascii_alphanumeric())
                    .to_string()
            } else {
                after_arrow
                    .trim_matches(|c: char| !c.is_ascii_alphanumeric())
                    .to_string()
            };
            terminator = Some(MirTerminator::Call { func, args, dst, target });
            lines.next();
        } else if trimmed.starts_with("_") && trimmed.contains('=') {
            // _0 = Add(_1, _2) — but also _7 = move ((_1 as Rect).0: Point)
            // Route EVERY assignment through parse_rhs so downcast projections,
            // struct construction, casts, and plain copies all decode correctly.
            let eq_idx = trimmed.find('=').unwrap();
            let dst = trimmed[..eq_idx].trim().to_string();
            let rhs = trimmed[eq_idx+1..].trim().trim_end_matches(';');
            let (op, args) = parse_rhs(rhs);
            statements.push(MirStmt::Assign { dst, op, args });
            lines.next();
        } else if trimmed.starts_with("StorageLive") {
            let local = trimmed[13..].trim().trim_end_matches(';').trim_matches('(').trim_matches(')').to_string();
            statements.push(MirStmt::StorageLive(local));
            lines.next();
        } else if trimmed.starts_with("StorageDead") {
            let local = trimmed[13..].trim().trim_end_matches(';').trim_matches('(').trim_matches(')').to_string();
            statements.push(MirStmt::StorageDead(local));
            lines.next();
        } else if trimmed.starts_with("unreachable") {
            // rustc emits `unreachable;` in an exhaustive match's otherwise arm. Previously this fell
            // through to the silent-drop else, leaving the block with no terminator ("block bbN has no
            // terminator"). Capture it so the backend can emit a trap.
            terminator = Some(MirTerminator::Unreachable);
            lines.next();
        } else {
            lines.next();
        }
    }

    Ok(MirBlock { label, statements, terminator })
}

fn parse_rhs(rhs: &str) -> (String, Vec<String>) {
    // Rung 7 (traits): a qualified-path call `<Sq as Area>::area(move _2)`. The ` as `
    // inside the path would otherwise trip the cast handler below (func -> "as"), so
    // recognize the call FIRST: func = the whole `<..>::method`, args from the list after
    // it. normalize_traits() later canonicalizes the func to `Type__method`.
    {
        let t = rhs.trim();
        if t.starts_with('<') {
            if let Some(gp) = t.find(">::") {
                if let Some(pp) = t[gp..].find('(') {
                    let paren = gp + pp;
                    let func = t[..paren].to_string();
                    let args_str = &t[paren + 1..];
                    if let Some(close) = args_str.rfind(')') {
                        let args: Vec<String> = args_str[..close].split(',')
                            .map(|a| a.trim().trim_start_matches("copy ").trim_start_matches("move ").trim().to_string())
                            .filter(|a| !a.is_empty()).collect();
                        return (func, args);
                    }
                }
            }
        }
    }
    // Rung 7 (traits, by-value &self elision): `&_N` / `&mut _N` — a reference to a
    // by-value scalar-replaced local. Flux passes aggregates by value, so a shared
    // reference is identity: treat it as `copy _N`. The whole-aggregate passthrough
    // (flux-backend) then copies every leaf, and a later deref `(*_N).K` reads field K.
    {
        let t = rhs.trim();
        let inner = t.strip_prefix("&mut ").or_else(|| t.strip_prefix("&"));
        if let Some(inner) = inner {
            let inner = inner.trim();
            if inner.starts_with('_') && inner[1..].chars().all(|c| c.is_ascii_digit()) && inner.len() > 1 {
                return ("copy".to_string(), vec![inner.to_string()]);
            }
        }
    }
    // Rung 7: `&self` field read renders as a deref-projection `((*_N).K: T)` (opt
    // copy/move). By-value elision makes `*_N == _N`, so this is field K of the
    // aggregate: emit `_N.K`, reusing the tuple/struct `_N.F` resolver.
    if let Some(proj) = strip_deref_projection(rhs) {
        return ("copy".to_string(), vec![proj]);
    }
    // Data-carrying enum payload extraction: `copy ((_N as Variant).K: T)` — rustc downcasts an enum
    // local to a variant and projects its K-th field. Map it to aggregate field (K+1): field 0 is the
    // discriminant tag, payload slots start at 1. Emitting `copy _N.(K+1)` reuses the existing
    // `_N.F` tuple-projection resolver in the backend. MUST come BEFORE the ` as ` cast check below,
    // which would otherwise split on the inner " as " and mangle the operand.
    if let Some(proj) = strip_downcast_projection(rhs) {
        return ("copy".to_string(), vec![proj]);
    }
    // Rung 8: bare deref `copy (*_5)` — a by-ref capture read. After whole-program
    // by-value elision the local holds the VALUE, so the deref is the identity.
    {
        let s = rhs.trim().trim_start_matches("copy ").trim_start_matches("move ").trim();
        if let Some(digits) = s.strip_prefix("(*_").and_then(|r| r.strip_suffix(')')) {
            if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                return ("copy".to_string(), vec![format!("_{}", digits)]);
            }
        }
    }
    // Cast rvalue: "move _1 as i64 (IntToInt)" / "_1 as f64 (IntToFloat)" — capture operand + target type.
    if let Some(pos) = rhs.find(" as ") {
        let operand = rhs[..pos].trim().to_string();
        let target = rhs[pos + 4..].trim().split_whitespace().next().unwrap_or("").to_string();
        return ("as".to_string(), vec![operand, target]);
    }
    // Struct construction: `P { x: const 3_i64, y: move _2 }` -> aggregate (op="") of field values,
    // positional (declaration order), reusing the tuple scalar-replacement path.
    if let Some(brace) = rhs.find('{') {
        let inner = &rhs[brace + 1..];
        if let Some(close) = inner.rfind('}') {
            let args: Vec<String> = inner[..close].split(',')
                .filter_map(|f| f.split_once(':').map(|(_, v)|
                    v.trim().trim_start_matches("copy ").trim_start_matches("move ").trim().to_string()))
                .filter(|v| !v.is_empty())
                .collect();
            if !args.is_empty() { return (String::new(), args); }
        }
    }
    if let Some(paren) = rhs.find('(') {
        let op = rhs[..paren].trim().to_string();
        let args_str = &rhs[paren+1..];
        if let Some(close) = args_str.rfind(')') {
            let args: Vec<String> = args_str[..close].split(',')
                .map(|a| a.trim().trim_start_matches("copy ").trim_start_matches("move ").to_string())
                .filter(|a| !a.is_empty()) // `mk()` -> [] not [""]; a 0-arg call must pass 0 args
                .collect();
            return (op, args);
        }
    }
    // No paren — handle `_0 = copy _1`, `_0 = move _3`, `_0 = const 42_i64`, `_0 = _4`.
    let trimmed = rhs.trim();
    if let Some(rest) = trimmed.strip_prefix("copy ") {
        return ("copy".to_string(), vec![rest.trim().to_string()]);
    }
    if let Some(rest) = trimmed.strip_prefix("move ") {
        return ("move".to_string(), vec![rest.trim().to_string()]);
    }
    if trimmed.starts_with("const ") {
        // Keep the `const ` prefix so lower_operand can recognise the literal form.
        return ("const".to_string(), vec![trimmed.to_string()]);
    }
    // Bare reference: `_0 = _4` or similar.
    if trimmed.starts_with('_') {
        return ("copy".to_string(), vec![trimmed.to_string()]);
    }
    (trimmed.to_string(), vec![])
}

/// Recognise a by-value `&self` field read `((*_N).K: T)` (optionally `copy `/`move `
/// prefixed) and return the aggregate-field operand `_N.K`. Under Flux's by-value
/// aggregate ABI a shared reference is identity, so a deref-then-project is just field
/// access. Returns None for anything without the `(*_` deref shape.
fn strip_deref_projection(rhs: &str) -> Option<String> {
    let s = rhs.trim().trim_start_matches("copy ").trim_start_matches("move ").trim();
    let rest = s.strip_prefix("((*_")?;                 // ((*_1).0: i64)
    let ndigits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if ndigits.is_empty() { return None; }
    let after = &rest[ndigits.len()..];
    let after = after.strip_prefix(").")?;              // ).0: i64)
    let kdigits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    if kdigits.is_empty() { return None; }
    Some(format!("_{}.{}", ndigits, kdigits))
}

/// Recognise a data-carrying enum downcast-projection `((_N as Variant).K: T)` (optionally prefixed
/// `copy `/`move `) and return the operand encoded as `_N|Variant|K` (raw payload index K — the
/// backend computes the real field offset from the enum layout). Returns None for anything else — crucially
/// for a plain int/float cast (`move _1 as i64 (IntToInt)`), which has ` as ` but no `).` projection,
/// so it falls through to parse_rhs's cast handler untouched.
fn strip_downcast_projection(rhs: &str) -> Option<String> {
    let s = rhs.trim()
        .trim_start_matches("copy ").trim_start_matches("move ").trim();
    let as_pos = s.find(" as ")?;
    // Local immediately before " as " — walk back to its leading `_` and read the digits.
    let before = &s[..as_pos];
    let lstart = before.rfind('_')?;
    let local: String = before[lstart + 1..].chars().take_while(|c| c.is_ascii_digit()).collect();
    if local.is_empty() { return None; }
    // Variant name between ` as ` and `).` — e.g. `Rect` in `((_1 as Rect).0: Point)`
    let after_as = &s[as_pos + 4..];
    let dot = after_as.find(").")?;
    let variant = after_as[..dot].trim().to_string();
    if variant.is_empty() { return None; }
    // Field index after the closing `).` — `).0`, `).1`, …
    let kpos = dot + 2;
    let kdigits: String = after_as[kpos..].chars().take_while(|c| c.is_ascii_digit()).collect();
    let k: usize = kdigits.parse().ok()?;
    // Encode as _N|Variant|K — the backend computes the real field offset from the enum layout.
    Some(format!("_{}|{}|{}", local, variant, k))
}

// ── Monomorphization (FIP-0001 type-complexity ladder, rung 5 part 2) ──
//
// rustc --emit=mir gives us a generic function POLYMORPHICALLY (`fn id(_1: T) -> T`) plus turbofish
// call sites (`id::<i64>`), never the monomorphized instances — so Flux generates them itself. For each
// distinct instantiation reached from a call site, clone the template, substitute its type params with
// the concrete type args, mangle a unique name (`id$i64`), and rewrite call sites to it. Templates
// (functions only ever reached via turbofish) are dropped. A program with no turbofish calls is
// returned UNCHANGED (early-out), so non-generic code is provably unaffected.

/// Split a `<…>` arg list on top-level commas (depth-aware, so `Vec<u8>, i64` → ["Vec<u8>", "i64"]).
fn split_top_commas(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0i32);
    for c in s.chars() {
        match c {
            '<' => { depth += 1; cur.push(c); }
            '>' => { depth -= 1; cur.push(c); }
            ',' if depth == 0 => { out.push(cur.trim().to_string()); cur.clear(); }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() { out.push(cur.trim().to_string()); }
    out
}

/// Parse a turbofish call func string `id::<i64>` / `fst::<i64, bool>` → (base, type-args). Returns
/// None for a non-turbofish call OR an enum-variant ctor path (`MyOpt::<i64>::Some`, which has a `::`
/// segment AFTER the `>` — those construct inline, they aren't generic-fn calls).
fn parse_turbofish(func: &str) -> Option<(String, Vec<String>)> {
    let pos = func.find("::<")?;
    let base = func[..pos].to_string();
    let rest = &func[pos + 3..];
    let close = rest.rfind('>')?;
    if rest[close + 1..].contains("::") { return None; }
    Some((base, split_top_commas(&rest[..close])))
}

/// A unique, symbol-safe name for one instantiation: `id` + `[i64]` → `id$i64`.
fn mangle(base: &str, targs: &[String]) -> String {
    let mut s = base.to_string();
    for t in targs {
        s.push('$');
        for c in t.chars() { s.push(if c.is_ascii_alphanumeric() { c } else { '_' }); }
    }
    s
}

/// A single uppercase ASCII letter — the Rust convention for a type parameter (T, U, A, B, E, K, V).
/// Multi-letter params (`Item`) aren't detected; that's a documented limitation of the MIR-text path
/// (the header carries no explicit `<…>` list to read them from).
fn is_type_param_name(s: &str) -> bool {
    s.len() == 1 && s.chars().next().map_or(false, |c| c.is_ascii_uppercase())
}

/// Type-param names of a template, in first-appearance order across params then the return type.
fn detect_type_params(f: &MirFunction) -> Vec<String> {
    let mut params: Vec<String> = Vec::new();
    let mut scan = |ty: &str, params: &mut Vec<String>| {
        let mut cur = String::new();
        for c in ty.chars().chain(std::iter::once(' ')) {
            if c.is_ascii_alphanumeric() || c == '_' { cur.push(c); }
            else { if is_type_param_name(&cur) && !params.contains(&cur) { params.push(cur.clone()); } cur.clear(); }
        }
    };
    for p in &f.params { scan(&p.ty, &mut params); }
    scan(&f.return_type, &mut params);
    params
}

/// Substitute whole-token type-param names in a type string: `(T, i64)` + {T→u32} → `(u32, i64)`.
/// Token-aware so `TypeName` is never partially rewritten.
fn subst_type(ty: &str, map: &std::collections::HashMap<String, String>) -> String {
    if map.is_empty() { return ty.to_string(); }
    let (mut out, mut cur) = (String::new(), String::new());
    for c in ty.chars().chain(std::iter::once('\0')) {
        if c.is_ascii_alphanumeric() || c == '_' { cur.push(c); }
        else {
            if !cur.is_empty() { out.push_str(map.get(&cur).map(|s| s.as_str()).unwrap_or(&cur)); cur.clear(); }
            if c != '\0' { out.push(c); }
        }
    }
    out
}

/// Rewrite turbofish calls to template functions into their mangled instance names, in place.
fn rewrite_calls(f: &mut MirFunction, templates: &std::collections::HashSet<String>) {
    for b in &mut f.blocks {
        if let Some(MirTerminator::Call { func, .. }) = &mut b.terminator {
            if let Some((base, targs)) = parse_turbofish(func) {
                if templates.contains(&base) { *func = mangle(&base, &targs); }
            }
        }
    }
}

/// Ladder rung 7 part 2 (generic trait dispatch): a generic template's body may call a trait
/// method through the still-generic receiver (`<T as Area>::area`), already canonicalized by
/// `normalize_traits` (which runs before monomorphize) to `T__area`. Once `specialize` pins `T` to
/// a concrete type, the callee must follow — rewrite `T__area` -> `Sq__area` so the instance calls
/// the real, already-canonicalized impl function emitted alongside it, not a name that never
/// exists. A non-trait callee (no `__`, or a prefix that isn't a substituted type param) passes
/// through unchanged.
fn subst_trait_callee(func: &str, map: &std::collections::HashMap<String, String>) -> String {
    if let Some(pos) = func.find("__") {
        if let Some(concrete) = map.get(&func[..pos]) {
            return format!("{}__{}", concrete, &func[pos + 2..]);
        }
    }
    func.to_string()
}

/// Generate a concrete instance of `template` for `targs`: substitute type params, rename to `mangled`.
fn specialize(template: &MirFunction, targs: &[String], mangled: &str) -> MirFunction {
    let tparams = detect_type_params(template);
    let map: std::collections::HashMap<String, String> =
        tparams.iter().cloned().zip(targs.iter().cloned()).collect();
    let mut f = template.clone();
    f.name = mangled.to_string();
    f.return_type = subst_type(&f.return_type, &map);
    for p in &mut f.params { p.ty = subst_type(&p.ty, &map); }
    for l in &mut f.locals { l.ty = subst_type(&l.ty, &map); }
    for b in &mut f.blocks {
        if let Some(MirTerminator::Call { func, .. }) = &mut b.terminator {
            *func = subst_trait_callee(func, &map);
        }
    }
    f
}

/// Canonicalize a trait method name to a receiver-qualified symbol both the impl
/// DEFINITION and the CALL SITE agree on. rustc renders the impl body as
/// `<impl at FILE:L:C: L:C>::method` and the call as `<Type as Trait>::method` — two
/// different strings for one function, which link as an undefined symbol. We map both to
/// `Type__method`: the call encodes the type directly; the impl derives it from the
/// receiver (param 0) type, `&Sq`/`&mut Sq`/`Sq` -> `Sq`.
fn trait_canon_from_call(name: &str) -> Option<String> {
    // <Sq as Area>::area  ->  Sq__area
    let inner = name.strip_prefix('<')?;
    let as_pos = inner.find(" as ")?;
    let ty = inner[..as_pos].trim();
    let method = name.rsplit(">::").next()?.trim();
    if ty.is_empty() || method.is_empty() || method == name { return None; }
    Some(format!("{}__{}", ty, method))
}
fn recv_base(ty: &str) -> String {
    let t = ty.trim().trim_start_matches("&mut ").trim_start_matches('&').trim();
    t.split(|c: char| c == '<' || c == ' ' || c == ',').next().unwrap_or(t).to_string()
}
fn trait_canon_from_impl(f: &MirFunction) -> Option<String> {
    if !f.name.starts_with("<impl") { return None; }
    let method = f.name.rsplit(">::").next()?.trim();
    let recv = f.params.first().map(|p| recv_base(&p.ty)).unwrap_or_default();
    if recv.is_empty() || method.is_empty() { return None; }
    Some(format!("{}__{}", recv, method))
}

/// Rung 7 (static trait dispatch): unify impl-definition and call-site trait method names,
/// and strip `&`/`&mut ` from receiver param types so `&self` methods scalar-replace their
/// by-value receiver. Programs with no trait syntax are returned unchanged.
///
/// Rung 7 part 3 (dyn dispatch, closed-world devirtualization): a `<dyn Trait as Trait>::m`
/// call canonicalizes to `dyn Trait__m` — a symbol no impl ever defines. In this single-unit
/// pipeline the whole world is visible, so when exactly ONE canonicalized impl provides
/// method `m`, the dyn call is provably that impl: rewrite the callee to `Type__m`, rewrite
/// `&dyn Trait`/`dyn Trait` carriers to the concrete type, and collapse the unsize coercion
/// (`_a = copy _b as &dyn Trait (PointerCoercion(Unsize, …))`) to a plain aggregate copy.
/// Rung 7 part 3b (MULTI-impl dyn dispatch, tagged defunctionalization): with several
/// impls the target is runtime-dependent. In a closed single-unit world the honest
/// lowering is a tagged union, not a fat pointer: `dyn Trait` becomes a flat tuple
/// `(tag, payload…)` (payload width = max impl field count, scalar-replaced like every
/// aggregate here), the unsize coercion becomes tuple construction with the source
/// type's tag, and the dyn call becomes a SwitchInt over the tag fanning out to the
/// already-canonicalized static impls. Same call site, target chosen by runtime data —
/// that IS dyn dispatch; indirect calls buy nothing extra without dynamic loading.
/// Impl field counts are inferred from construction sites in the same unit (no layout
/// plumbing). Anything the lowering can't prove (no construction found, unknown shape)
/// is left untouched → the old loud link failure, never silent mis-dispatch.
pub fn normalize_traits(mut funcs: Vec<MirFunction>) -> Vec<MirFunction> {
    use std::collections::HashMap;
    let touches = funcs.iter().any(|f| f.name.starts_with("<impl")
        || f.blocks.iter().any(|b| matches!(&b.terminator,
            Some(MirTerminator::Call { func, .. })
                if (func.starts_with('<') && func.contains(" as "))
                    || func.starts_with("Vec::<")
                    || func.starts_with("String::"))));
    if !touches { return funcs; }

    // Pass 1 — canonicalize impl-definition and call-site names to `Type__method`.
    // Track which names came from impls: only those are devirtualization candidates
    // (a user fn that happens to contain `__` must not be mistaken for one).
    let mut method_impls: HashMap<String, Vec<String>> = HashMap::new(); // method -> concrete types
    for f in &mut funcs {
        // Rung 8: `parent::{closure#N}` bodies are the impl of `call` for their
        // flattened capture type (the receiver param names it) — rename to the
        // same `__closure_X__call` the call-site canonicalizer produces, and the
        // rung-7 name unification does the rest.
        if f.name.contains("::{closure#") {
            if let Some(base) = f.params.first().map(|p| recv_base(&p.ty)) {
                if base.starts_with("__closure_") {
                    method_impls.entry("call".to_string()).or_default().push(base.clone());
                    f.name = format!("{}__call", base);
                }
            }
        }
        if let Some(canon) = trait_canon_from_impl(f) {
            if let Some(pos) = canon.find("__") {
                method_impls.entry(canon[pos + 2..].to_string())
                    .or_default().push(canon[..pos].to_string());
            }
            f.name = canon;
        }
        for b in &mut f.blocks {
            if let Some(MirTerminator::Call { func, .. }) = &mut b.terminator {
                if let Some(canon) = trait_canon_from_call(func) { *func = canon; }
                // Rung 9 (heap): std collection bodies never appear in the local
                // MIR dump — they live precompiled in std. Recognized Vec<i64>
                // operations rewrite to __flux_vec_* runtime shims (Vec = an
                // opaque i64 heap handle; the C runtime links in fluxc run).
                // Anything outside the recognized set keeps its std name and
                // fails LOUD at link — never a guessed lowering.
                let mapped = match func.as_str() {
                    "Vec::<i64>::new" => Some("__flux_vec_new"),
                    "Vec::<i64>::push" => Some("__flux_vec_push"),
                    "Vec::<i64>::len" => Some("__flux_vec_len"),
                    "Vec::<i64>::pop" => Some("__flux_vec_pop"),
                    "Vec<i64>__index" => Some("__flux_vec_index"),
                    // Rung 10: String, same opaque-handle pattern (ASCII chars).
                    "String::new" => Some("__flux_string_new"),
                    "String::push" => Some("__flux_string_push"),
                    "String::len" => Some("__flux_string_len"),
                    // Rung 10: `for x in v` — a heap-handle iterator, so &mut
                    // mutation happens BEHIND the handle (elision-safe).
                    "Vec<i64>__into_iter" => Some("__flux_vec_intoiter"),
                    "std::vec::IntoIter<i64>__next" => Some("__flux_vec_next"),
                    _ => None,
                };
                if let Some(m) = mapped { *func = m.to_string(); }
            }
        }
    }

    // Pass 2 — resolve `dyn Trait__method` callees. Unique impl → devirtualize now.
    // Multiple impls → collect for the pass-4 tagged lowering.
    let mut dyn_map: HashMap<String, String> = HashMap::new(); // "dyn Trait" -> "Type" (unique impl)
    let mut multi_dyn: HashMap<String, Vec<String>> = HashMap::new(); // "dyn Trait" -> sorted impl types
    for f in &mut funcs {
        for b in &mut f.blocks {
            if let Some(MirTerminator::Call { func, .. }) = &mut b.terminator {
                if !func.starts_with("dyn ") { continue; }
                let Some(pos) = func.find("__") else { continue };
                let (dyn_ty, method) = (func[..pos].to_string(), func[pos + 2..].to_string());
                match method_impls.get(&method).map(|v| v.as_slice()) {
                    Some([one]) => {
                        dyn_map.insert(dyn_ty, one.clone());
                        *func = format!("{}__{}", one, method);
                    }
                    Some(many) if many.len() > 1 => {
                        let mut impls = many.to_vec();
                        impls.sort();
                        impls.dedup();
                        multi_dyn.insert(dyn_ty, impls);
                    }
                    _ => {}
                }
            }
        }
    }

    // Pass 3 — by-value elision + dyn carrier substitution + unsize-cast collapse.
    for f in &mut funcs {
        // Whole-program by-value elision: strip &/&mut from EVERY param and local
        // type, so reference-typed carriers ( in a caller, ) become
        // their pointee and get scalar-replaced by value. Consistent caller+callee; the
        // deref/ref-op rewrites in parse_rhs make the bodies agree. Devirtualized
        // `dyn Trait` carriers become the concrete type BEFORE recv_base would
        // otherwise mangle `&dyn Trait` into the meaningless base `dyn`. Multi-impl
        // carriers keep their full `dyn Trait` spelling for the pass-4 lowering.
        for pp in f.params.iter_mut().chain(f.locals.iter_mut()) {
            let bare = pp.ty.trim().trim_start_matches("&mut ").trim_start_matches('&')
                .trim().to_string();
            if let Some(conc) = dyn_map.get(&bare) {
                pp.ty = conc.clone();
            } else if multi_dyn.contains_key(&bare) {
                pp.ty = bare; // preserve "dyn Trait" verbatim for pass 4
            } else if pp.ty.trim_start().starts_with('&') {
                pp.ty = recv_base(&pp.ty);
            }
        }
        if dyn_map.is_empty() { continue; }
        // Collapse unsize casts ONLY for single-impl (devirtualized) dyn types —
        // multi-impl casts become tagged constructions in pass 4. The cast target
        // is truncated to "&dyn" by parse_rhs, so the dst LOCAL's type (already
        // rewritten above) is the discriminator: concrete → devirt'd → collapse.
        let ty_of: HashMap<String, String> = f.params.iter().chain(f.locals.iter())
            .filter(|l| !l.ty.is_empty()) // debug entries shadow real decls with ty:""
            .map(|l| (format!("_{}", l.index), l.ty.clone())).collect();
        for b in &mut f.blocks {
            for s in &mut b.statements {
                if let MirStmt::Assign { dst, op, args } = s {
                    if op == "as" && args.len() >= 2
                        && args[1].trim_start_matches('&').starts_with("dyn")
                        && ty_of.get(dst).map(|t| !t.starts_with("dyn ")).unwrap_or(false) {
                        let operand = args[0].trim()
                            .trim_start_matches("copy ").trim_start_matches("move ")
                            .trim().to_string();
                        *op = "copy".to_string();
                        *args = vec![operand];
                    }
                }
            }
        }
    }

    // Pass 4 — tagged defunctionalization of multi-impl dyn (rung 7 part 3b).
    if !multi_dyn.is_empty() {
        lower_multi_dyn(&mut funcs, &multi_dyn);
    }

    // Pass 5 (rung 8) — tuple-ize closure capture types. `__closure_X` is a
    // synthetic struct that exists in no source file, so the backend has no
    // layout for it; but its construction is already positional and its body
    // reads are `_1.K` projections — exactly a tuple. Rewrite the TYPE to
    // `(i64, …)` of the construction's field count and everything downstream
    // (flattened params, aggregate call args, projections) is existing tuple
    // machinery. A closure type with no construction in the unit (e.g. a
    // non-capturing closure's unit value) is left untouched → loud, not guessed.
    let has_closures = funcs.iter().any(|f| f.params.iter().chain(f.locals.iter())
        .any(|l| l.ty.contains("__closure_")));
    if has_closures {
        let mut nfields: HashMap<String, usize> = HashMap::new();
        for f in funcs.iter() {
            let ty_of: HashMap<String, String> = f.params.iter().chain(f.locals.iter())
                .filter(|l| !l.ty.is_empty())
                .map(|l| (format!("_{}", l.index), l.ty.clone())).collect();
            for b in &f.blocks {
                for s in &b.statements {
                    if let MirStmt::Assign { dst, op, args } = s {
                        if op.is_empty() && !args.is_empty() {
                            if let Some(t) = ty_of.get(dst) {
                                if t.starts_with("__closure_") {
                                    nfields.entry(t.clone()).or_insert(args.len());
                                }
                            }
                        }
                    }
                }
            }
        }
        for f in funcs.iter_mut() {
            for pp in f.params.iter_mut().chain(f.locals.iter_mut()) {
                let base = recv_base(&pp.ty);
                if let Some(&n) = nfields.get(&base) {
                    let slots = vec!["i64"; n];
                    pp.ty = format!("({})", slots.join(", "));
                }
            }
        }
    }

    // Pass 7 (rung 11) — iterator adapter chain FUSION. `v.into_iter()
    // .map(f).sum()` reaches MIR as three std calls whose bodies are hidden
    // generic code (the Map adapter is a lazy struct; sum is its consumer).
    // With the WHOLE chain visible, the honest lowering is deforestation:
    // fuse it into the loop it means, built from parts already proven —
    // the rung-10 handle iterator and a DIRECT static call to the (already
    // canonicalized) closure. The Map struct never materializes at all.
    lower_iter_fusion(&mut funcs);

    // Pass 6 (rung 10) — range for-loop desugar. `for i in a..b` reaches MIR as
    // Range construction + IntoIterator::into_iter (the identity) + repeated
    // Iterator::next calls matched on Option. Range<i64> and Option<i64> both
    // tuple-ize as (i64, i64) — Option's (tag, payload) layout matches the
    // data-enum convention (tag slot 0, payload 1+), so discriminant() and the
    // `(_ as Some).0` downcast resolve through EXISTING machinery. next()'s
    // semantics inline as pure MIR: if start < end { r = (start+1, end);
    // Some(start) } else { None }. No runtime, no hidden std body.
    lower_range_sugar(&mut funcs);
    funcs
}

/// Rung 11: fuse `sum(map(into_iter(v), closure))` into a loop. Guardrails:
/// the map receiver must trace to a `__flux_vec_intoiter` result (a vec
/// handle), and the closure symbol must be extractable from map's turbofish —
/// otherwise the chain is left intact and fails LOUD at link (undefined
/// `Map<..>__sum`), never a guessed fusion.
fn lower_iter_fusion(funcs: &mut Vec<MirFunction>) {
    for f in funcs.iter_mut() {
        // Locate the sum call.
        let Some(sum_bi) = f.blocks.iter().position(|b| matches!(&b.terminator,
            Some(MirTerminator::Call { func, .. })
                if func.starts_with("Map<") && func.contains("__sum")))
        else { continue };
        let Some(MirTerminator::Call { args: sum_args, dst: sum_dst, target: sum_tgt, .. }) =
            f.blocks[sum_bi].terminator.clone() else { continue };
        let map_local = sum_args.first().map(|a| a.trim()
            .trim_start_matches("copy ").trim_start_matches("move ").trim().to_string())
            .unwrap_or_default();
        // Locate the map call producing that local.
        let Some(map_bi) = f.blocks.iter().position(|b| matches!(&b.terminator,
            Some(MirTerminator::Call { func, dst, .. })
                if func.contains("__map::<") && *dst == map_local))
        else { continue };
        let Some(MirTerminator::Call { func: map_fn, args: map_args, target: map_tgt, .. }) =
            f.blocks[map_bi].terminator.clone() else { continue };
        // Closure symbol from map's turbofish: `..__map::<i64, __closure_h>`.
        let Some(cl_name) = map_fn.split("__map::<").nth(1)
            .and_then(|t| t.trim_end_matches('>').split(',').last())
            .map(|c| c.trim().to_string())
            .filter(|c| c.starts_with("__closure_"))
        else { continue };
        let iter_op = map_args.first().map(|a| a.trim()
            .trim_start_matches("copy ").trim_start_matches("move ").trim().to_string())
            .unwrap_or_default();
        let cl_op = map_args.get(1).map(|a| a.trim()
            .trim_start_matches("copy ").trim_start_matches("move ").trim().to_string())
            .unwrap_or_default();
        // The iterator must be a vec handle from the rung-10 shim.
        let is_vec_iter = f.blocks.iter().any(|b| matches!(&b.terminator,
            Some(MirTerminator::Call { func, dst, .. })
                if func == "__flux_vec_intoiter" && *dst == iter_op));
        if !is_vec_iter || iter_op.is_empty() || cl_op.is_empty() { continue; }

        // Fresh locals.
        let mut next_idx = f.params.iter().chain(f.locals.iter())
            .map(|l| l.index).max().unwrap_or(0) + 1;
        let mut fresh = |ty: &str, locals: &mut Vec<MirLocal>| -> String {
            let name = format!("_{}", next_idx);
            locals.push(MirLocal { index: next_idx, name: String::new(), ty: ty.into(), mutable: true });
            next_idx += 1;
            name
        };
        let mut locals_add: Vec<MirLocal> = Vec::new();
        let acc = fresh("i64", &mut locals_add);
        let tag_t = fresh("i64", &mut locals_add);
        let val_t = fresh("i64", &mut locals_add);
        let map_t = fresh("i64", &mut locals_add);

        // The map call vanishes — the adapter never materializes.
        f.blocks[map_bi].terminator = Some(MirTerminator::Goto(map_tgt));

        // The sum block seeds the accumulator and enters the fused loop.
        let l_head = "bbfuse_head".to_string();
        let l_body = "bbfuse_body".to_string();
        let l_body2 = "bbfuse_body2".to_string();
        let l_body3 = "bbfuse_body3".to_string();
        let l_done = "bbfuse_done".to_string();
        f.blocks[sum_bi].statements.push(MirStmt::Assign {
            dst: acc.clone(), op: "const".into(), args: vec!["const 0_i64".into()],
        });
        f.blocks[sum_bi].terminator = Some(MirTerminator::Goto(l_head.clone()));
        f.blocks.push(MirBlock {
            label: l_head.clone(), statements: vec![],
            terminator: Some(MirTerminator::Call {
                func: "__flux_vec_next_tag".into(), args: vec![format!("copy {}", iter_op)],
                dst: tag_t.clone(), target: format!("{}_chk", l_head),
            }),
        });
        f.blocks.push(MirBlock {
            label: format!("{}_chk", l_head), statements: vec![],
            terminator: Some(MirTerminator::SwitchInt {
                discr: tag_t.clone(),
                targets: vec![("0".into(), l_done.clone())],
                otherwise: l_body.clone(),
            }),
        });
        f.blocks.push(MirBlock {
            label: l_body, statements: vec![],
            terminator: Some(MirTerminator::Call {
                func: "__flux_vec_lastval".into(), args: vec![format!("copy {}", iter_op)],
                dst: val_t.clone(), target: l_body2.clone(),
            }),
        });
        f.blocks.push(MirBlock {
            label: l_body2, statements: vec![],
            terminator: Some(MirTerminator::Call {
                func: format!("{}__call", cl_name),
                args: vec![format!("copy {}", cl_op), format!("copy {}", val_t)],
                dst: map_t.clone(), target: l_body3.clone(),
            }),
        });
        f.blocks.push(MirBlock {
            label: l_body3,
            statements: vec![MirStmt::Assign {
                dst: acc.clone(), op: "Add".into(),
                args: vec![format!("copy {}", acc), format!("copy {}", map_t)],
            }],
            terminator: Some(MirTerminator::Goto(l_head)),
        });
        f.blocks.push(MirBlock {
            label: l_done,
            statements: vec![MirStmt::Assign {
                dst: sum_dst, op: "copy".into(), args: vec![acc.clone()],
            }],
            terminator: Some(MirTerminator::Goto(sum_tgt)),
        });
        f.locals.extend(locals_add);
    }
}

fn lower_range_sugar(funcs: &mut Vec<MirFunction>) {
    use std::collections::HashMap;
    const RANGE_T: &str = "std::ops::Range<i64>";
    let is_opt = |t: &str| t == "Option<i64>" || t == "std::option::Option<i64>";
    // NOT recv_base — that strips the `<i64>` generic and nothing matches.
    let bare = |t: &str| t.trim().trim_start_matches("&mut ").trim_start_matches('&').trim().to_string();
    let uses_sugar = funcs.iter().any(|f| f.params.iter().chain(f.locals.iter())
        .any(|l| { let b = bare(&l.ty); b == RANGE_T || is_opt(&b) }));
    if !uses_sugar { return; }

    for f in funcs.iter_mut() {
        // Tuple-ize the carriers.
        for pp in f.params.iter_mut().chain(f.locals.iter_mut()) {
            let b = bare(&pp.ty);
            if b == RANGE_T || is_opt(&b) {
                pp.ty = "(i64, i64)".to_string();
            }
        }
        let mut next_idx = f.params.iter().chain(f.locals.iter())
            .map(|l| l.index).max().unwrap_or(0) + 1;
        let mut locals_add: Vec<MirLocal> = Vec::new();
        let mut fresh = |ty: &str, locals: &mut Vec<MirLocal>| -> String {
            let name = format!("_{}", next_idx);
            locals.push(MirLocal { index: next_idx, name: String::new(), ty: ty.into(), mutable: false });
            next_idx += 1;
            name
        };
        // Ref-elided copies (`_6 = &mut _4` parsed as `_6 = copy _4`) — resolve a
        // next() receiver back to the UNDERLYING range local, or refuse loudly.
        let mut copy_src: HashMap<String, String> = HashMap::new();
        for b in &f.blocks {
            for s in &b.statements {
                if let MirStmt::Assign { dst, op, args } = s {
                    if op == "copy" && args.len() == 1 && args[0].starts_with('_')
                        && !args[0].contains('.') {
                        copy_src.insert(dst.clone(), args[0].clone());
                    }
                }
            }
        }
        let resolve_recv = |a: &str, copy_src: &HashMap<String, String>| -> String {
            let mut r = a.trim().trim_start_matches("copy ").trim_start_matches("move ")
                .trim().to_string();
            for _ in 0..4 { // follow short copy chains
                match copy_src.get(&r) {
                    Some(src) => r = src.clone(),
                    None => break,
                }
            }
            r
        };

        let mut new_blocks: Vec<MirBlock> = Vec::new();
        let mut seq = 0usize;
        for bi in 0..f.blocks.len() {
            let Some(MirTerminator::Call { func, args, dst, target }) = f.blocks[bi].terminator.clone()
            else { continue };
            if func == format!("{}__into_iter", RANGE_T) {
                // IntoIterator for Range is the identity.
                let recv = args.first().cloned().unwrap_or_default();
                let src = recv.trim().trim_start_matches("copy ").trim_start_matches("move ")
                    .trim().to_string();
                f.blocks[bi].statements.push(MirStmt::Assign {
                    dst: dst.clone(), op: "copy".into(), args: vec![src],
                });
                f.blocks[bi].terminator = Some(MirTerminator::Goto(target));
                continue;
            }
            if func == "__flux_vec_next" {
                // Pair-returning shims can't cross the C ABI (Cranelift's
                // multi-return isn't struct-compatible) — chain two
                // single-return calls: tag (advances + stashes) then lastval,
                // then assemble the Option tuple.
                let recv = args.first().cloned().unwrap_or_default();
                let tag_t = fresh("i64", &mut locals_add);
                let val_t = fresh("i64", &mut locals_add);
                let l_val = format!("bbvnext{}_val", seq);
                let l_asm = format!("bbvnext{}_asm", seq);
                seq += 1;
                new_blocks.push(MirBlock {
                    label: l_val.clone(),
                    statements: vec![],
                    terminator: Some(MirTerminator::Call {
                        func: "__flux_vec_lastval".into(),
                        args: vec![recv.clone()],
                        dst: val_t.clone(),
                        target: l_asm.clone(),
                    }),
                });
                new_blocks.push(MirBlock {
                    label: l_asm.clone(),
                    statements: vec![MirStmt::Assign {
                        dst: dst.clone(), op: String::new(),
                        args: vec![format!("copy {}", tag_t), format!("copy {}", val_t)],
                    }],
                    terminator: Some(MirTerminator::Goto(target)),
                });
                f.blocks[bi].terminator = Some(MirTerminator::Call {
                    func: "__flux_vec_next_tag".into(),
                    args: vec![recv],
                    dst: tag_t,
                    target: l_val,
                });
                continue;
            }
            if func == format!("{}__next", RANGE_T) {
                let recv = resolve_recv(args.first().map(String::as_str).unwrap_or(""), &copy_src);
                if !recv.starts_with('_') { continue; } // unresolvable → loud link
                let cmp = fresh("bool", &mut locals_add);
                f.blocks[bi].statements.push(MirStmt::Assign {
                    dst: cmp.clone(), op: "Lt".into(),
                    args: vec![format!("copy {}.0", recv), format!("copy {}.1", recv)],
                });
                let some_l = format!("bbrange{}_some", seq);
                let none_l = format!("bbrange{}_none", seq);
                seq += 1;
                // Some branch: dst = (1, start); range = (start+1, end). All
                // whole-local assignments — the statement path has no projected
                // destinations, so the range tuple is RECONSTRUCTED, not patched.
                let old_start = fresh("i64", &mut locals_add);
                let old_end = fresh("i64", &mut locals_add);
                let new_start = fresh("i64", &mut locals_add);
                new_blocks.push(MirBlock {
                    label: some_l.clone(),
                    statements: vec![
                        MirStmt::Assign { dst: old_start.clone(), op: "copy".into(),
                            args: vec![format!("{}.0", recv)] },
                        MirStmt::Assign { dst: old_end.clone(), op: "copy".into(),
                            args: vec![format!("{}.1", recv)] },
                        MirStmt::Assign { dst: new_start.clone(), op: "Add".into(),
                            args: vec![format!("copy {}", old_start), "const 1_i64".into()] },
                        MirStmt::Assign { dst: recv.clone(), op: String::new(),
                            args: vec![format!("copy {}", new_start), format!("copy {}", old_end)] },
                        MirStmt::Assign { dst: dst.clone(), op: String::new(),
                            args: vec!["const 1_i64".into(), format!("copy {}", old_start)] },
                    ],
                    terminator: Some(MirTerminator::Goto(target.clone())),
                });
                new_blocks.push(MirBlock {
                    label: none_l.clone(),
                    statements: vec![
                        MirStmt::Assign { dst: dst.clone(), op: String::new(),
                            args: vec!["const 0_i64".into(), "const 0_i64".into()] },
                    ],
                    terminator: Some(MirTerminator::Goto(target)),
                });
                f.blocks[bi].terminator = Some(MirTerminator::SwitchInt {
                    discr: cmp,
                    targets: vec![("0".into(), none_l)],
                    otherwise: some_l,
                });
            }
        }
        f.blocks.extend(new_blocks);
        f.locals.extend(locals_add);
    }
}

/// Rung 7 part 3b: lower every multi-impl `dyn Trait` to a tagged flat tuple and
/// every dyn call to a SwitchInt fan-out over the tag. See normalize_traits docs.
fn lower_multi_dyn(
    funcs: &mut Vec<MirFunction>,
    multi_dyn: &std::collections::HashMap<String, Vec<String>>,
) {
    use std::collections::HashMap;

    // Impl payload widths, inferred from construction sites in this unit:
    // `_d = Ty { f: …, g: … }` parses as Assign{op:"", args:[fields…]} with the
    // dst local typed `Ty`. No construction found → shape unknown → skip that
    // dyn type entirely (old loud link failure, never a guessed layout).
    let mut nfields: HashMap<String, usize> = HashMap::new();
    for f in funcs.iter() {
        let ty_of: HashMap<String, String> = f.params.iter().chain(f.locals.iter())
            .filter(|l| !l.ty.is_empty()) // debug entries shadow real decls with ty:""
            .map(|l| (format!("_{}", l.index), l.ty.clone())).collect();
        for b in &f.blocks {
            for s in &b.statements {
                if let MirStmt::Assign { dst, op, args } = s {
                    if op.is_empty() && !args.is_empty() {
                        if let Some(t) = ty_of.get(dst) {
                            nfields.entry(t.clone()).or_insert(args.len());
                        }
                    }
                }
            }
        }
    }
    let lowered: HashMap<String, (Vec<String>, usize)> = multi_dyn.iter()
        .filter_map(|(dyn_ty, impls)| {
            let widths: Option<Vec<usize>> = impls.iter().map(|t| nfields.get(t).copied()).collect();
            widths.map(|w| (dyn_ty.clone(), (impls.clone(), w.into_iter().max().unwrap_or(0))))
        })
        .collect();
    if lowered.is_empty() {
        for (d, i) in multi_dyn {
            eprintln!("flux-frontend: dyn dispatch on `{}` ({} impls) — no construction \
                       site found to infer payload shape; leaving unresolved (loud link \
                       failure, not silent mis-dispatch)", d, i.len());
        }
        return;
    }
    let tuple_ty = |payload: usize| {
        let slots = vec!["i64"; payload + 1];
        format!("({})", slots.join(", "))
    };

    for f in funcs.iter_mut() {
        // Snapshot pre-rewrite local types (concrete cast sources + dyn carriers).
        let orig_ty: HashMap<String, String> = f.params.iter().chain(f.locals.iter())
            .filter(|l| !l.ty.is_empty()) // debug entries shadow real decls with ty:""
            .map(|l| (format!("_{}", l.index), l.ty.clone())).collect();
        let mut next_idx = f.params.iter().chain(f.locals.iter())
            .map(|l| l.index).max().unwrap_or(0) + 1;
        let mut fresh = |locals: &mut Vec<MirLocal>| -> String {
            let name = format!("_{}", next_idx);
            locals.push(MirLocal { index: next_idx, name: String::new(), ty: "i64".into(), mutable: false });
            next_idx += 1;
            name
        };

        // Carrier types → tagged tuple.
        for pp in f.params.iter_mut().chain(f.locals.iter_mut()) {
            if let Some((_, payload)) = lowered.get(pp.ty.trim()) {
                pp.ty = tuple_ty(*payload);
            }
        }

        // Unsize casts → tagged tuple construction (payload extracted through
        // fresh temps so construction args stay simple operands).
        let mut locals_add: Vec<MirLocal> = Vec::new();
        for b in &mut f.blocks {
            let mut out: Vec<MirStmt> = Vec::with_capacity(b.statements.len());
            for s in b.statements.drain(..) {
                match s {
                    MirStmt::Assign { dst, op, args }
                        if op == "as" && args.len() >= 2
                            && args[1].trim_start_matches('&').starts_with("dyn")
                            && orig_ty.get(&dst).map(|t| lowered.contains_key(t.trim())).unwrap_or(false) =>
                    {
                        let dyn_ty = orig_ty.get(&dst).unwrap().trim().to_string();
                        let (impls, payload) = &lowered[&dyn_ty];
                        let src = args[0].trim()
                            .trim_start_matches("copy ").trim_start_matches("move ")
                            .trim().to_string();
                        let src_ty = orig_ty.get(&src).cloned().unwrap_or_default();
                        let (Some(tag), Some(&n)) =
                            (impls.iter().position(|t| *t == src_ty), nfields.get(&src_ty))
                        else {
                            // Unknown source type — keep the cast (loud downstream).
                            out.push(MirStmt::Assign { dst, op, args });
                            continue;
                        };
                        let mut ctor: Vec<String> = vec![format!("const {}_i64", tag)];
                        for k in 0..n {
                            let t = fresh(&mut locals_add);
                            out.push(MirStmt::Assign {
                                dst: t.clone(), op: "copy".into(),
                                args: vec![format!("{}.{}", src, k)],
                            });
                            ctor.push(format!("copy {}", t));
                        }
                        for _ in n..*payload {
                            ctor.push("const 0_i64".into());
                        }
                        out.push(MirStmt::Assign { dst, op: String::new(), args: ctor });
                    }
                    other => out.push(other),
                }
            }
            b.statements = out;
        }

        // Dyn calls → tag switch fanning out to the static impls.
        let mut new_blocks: Vec<MirBlock> = Vec::new();
        let mut seq = 0usize;
        for bi in 0..f.blocks.len() {
            let Some(MirTerminator::Call { func, args, dst, target }) = f.blocks[bi].terminator.clone()
            else { continue };
            if !func.starts_with("dyn ") { continue; }
            let Some(pos) = func.find("__") else { continue };
            let (dyn_ty, method) = (func[..pos].to_string(), func[pos + 2..].to_string());
            let Some((impls, _)) = lowered.get(&dyn_ty) else { continue };
            let recv = args.first().map(|a| a.trim()
                .trim_start_matches("copy ").trim_start_matches("move ").trim().to_string())
                .unwrap_or_default();
            // Tag extraction in the calling block.
            let tag_local = fresh(&mut locals_add);
            f.blocks[bi].statements.push(MirStmt::Assign {
                dst: tag_local.clone(), op: "copy".into(),
                args: vec![format!("{}.0", recv)],
            });
            // One branch block per impl: unpack that impl's receiver fields from the
            // payload slots, call the canonicalized static impl, continue to the
            // original target.
            let mut targets: Vec<(String, String)> = Vec::new();
            let mut otherwise = String::new();
            for (tag, imp) in impls.iter().enumerate() {
                let label = format!("bbdyn{}_{}", seq, tag);
                let n = nfields.get(imp).copied().unwrap_or(0);
                let mut stmts: Vec<MirStmt> = Vec::new();
                let mut call_args: Vec<String> = Vec::new();
                for k in 0..n {
                    let t = fresh(&mut locals_add);
                    stmts.push(MirStmt::Assign {
                        dst: t.clone(), op: "copy".into(),
                        args: vec![format!("{}.{}", recv, k + 1)],
                    });
                    call_args.push(format!("copy {}", t));
                }
                new_blocks.push(MirBlock {
                    label: label.clone(),
                    statements: stmts,
                    terminator: Some(MirTerminator::Call {
                        func: format!("{}__{}", imp, method),
                        args: call_args,
                        dst: dst.clone(),
                        target: target.clone(),
                    }),
                });
                if tag + 1 == impls.len() {
                    otherwise = label; // last impl is the switch fallback
                } else {
                    targets.push((tag.to_string(), label));
                }
            }
            seq += 1;
            f.blocks[bi].terminator = Some(MirTerminator::SwitchInt {
                discr: tag_local, targets, otherwise,
            });
        }
        f.blocks.extend(new_blocks);
        f.locals.extend(locals_add);
    }
}

/// Monomorphize a parsed MIR function list. See the module-section comment above. No turbofish calls →
/// the input is returned unchanged.
pub fn monomorphize(funcs: Vec<MirFunction>) -> Vec<MirFunction> {
    use std::collections::{HashMap, HashSet};
    // Rung 7: unify trait method names + elide &self references BEFORE monomorphization,
    // so generic trait calls (`<T as Tr>::m`) canonicalize alongside concrete ones.
    let funcs = normalize_traits(funcs);
    let mut by_name: HashMap<String, MirFunction> = HashMap::new();
    for f in &funcs { by_name.entry(f.name.clone()).or_insert_with(|| f.clone()); }

    // Collect distinct instantiations from turbofish call sites whose base names a defined function.
    let mut needed: Vec<(String, Vec<String>)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for f in &funcs {
        for b in &f.blocks {
            if let Some(MirTerminator::Call { func, .. }) = &b.terminator {
                if let Some((base, targs)) = parse_turbofish(func) {
                    if by_name.contains_key(&base) && seen.insert(format!("{}|{}", base, targs.join(","))) {
                        needed.push((base, targs));
                    }
                }
            }
        }
    }
    let templates: HashSet<String> = needed.iter().map(|(b, _)| b.clone()).collect();
    if templates.is_empty() { return funcs; } // non-generic program → untouched

    let mut out: Vec<MirFunction> = Vec::new();
    let mut emitted: HashSet<String> = HashSet::new();
    for f in &funcs {
        if templates.contains(&f.name) { continue; }      // drop templates (replaced by instances)
        if !emitted.insert(f.name.clone()) { continue; }  // drop rustc CTFE duplicates
        let mut nf = f.clone();
        rewrite_calls(&mut nf, &templates);
        out.push(nf);
    }
    for (base, targs) in &needed {
        let mut s = specialize(&by_name[base], targs, &mangle(base, targs));
        rewrite_calls(&mut s, &templates); // a generic instance may itself call other generics
        out.push(s);
    }
    out
}

// ── Named-const resolution (ladder rung 6) ──
//
// rustc renders a named integer const as `const HALVING_INTERVAL` in MIR, not its value. Flux's
// operand resolver only understands `const <literal>`, so an unresolved named const parses to 0 — and
// a `Div`/`Rem` by one traps (SIGFPE), as found dogfooding sigil-emission::block_reward. This pass
// substitutes `const NAME` → `const <value>` before codegen, reusing the literal-const path. Empty
// table → input returned unchanged.

fn fix_const(arg: &mut String, consts: &std::collections::HashMap<String, String>) {
    if let Some(name) = arg.strip_prefix("const ") {
        if let Some(lit) = consts.get(name.trim()) { *arg = format!("const {}", lit); }
    }
}

/// Substitute named-const operands in every statement + terminator using the const table.
pub fn resolve_consts(mut funcs: Vec<MirFunction>, consts: &std::collections::HashMap<String, String>) -> Vec<MirFunction> {
    if consts.is_empty() { return funcs; }
    for f in &mut funcs {
        for b in &mut f.blocks {
            for s in &mut b.statements {
                if let MirStmt::Assign { args, .. } = s {
                    for a in args.iter_mut() { fix_const(a, consts); }
                }
            }
            match &mut b.terminator {
                Some(MirTerminator::Call { args, .. }) => for a in args.iter_mut() { fix_const(a, consts); },
                Some(MirTerminator::SwitchInt { discr, .. }) => fix_const(discr, consts),
                _ => {}
            }
        }
    }
    funcs
}

// ── MIR → flux-frontend IR lowering (file-scope, callable from phase3) ──
// Phase 2c+: previously trapped inside `mod tests` so phase3 could not use them.
// Hoisted here, return-value bug fixed, opcode coverage expanded.

pub fn lower_mir_to_ir(mf: &MirFunction) -> crate::FunctionDef {
    let params: Vec<crate::Param> = mf.params.iter().map(|p| crate::Param {
        name: format!("_{}", p.index),
        ty: mir_type_to_ir(&p.ty),
    }).collect();
    let ret = mir_type_to_ir(&mf.return_type);
    let pn: Vec<String> = params.iter().map(|p| p.name.clone()).collect();

    // If the MIR looks like a simple `if cond { x } else { y }` returning a value, lower it
    // to Expr::If so the backend can emit Cranelift branches. Otherwise fall through to the
    // flat per-block lowering used for straight-line code.
    if let Some(if_expr) = try_lower_if_else(mf, &pn) {
        return crate::FunctionDef {
            name: mf.name.clone(),
            visibility: crate::Visibility::Public,
            params,
            return_type: ret,
            body: crate::Expr::Block(vec![crate::Expr::Return(Box::new(if_expr))]),
            is_async: false,
        };
    }

    let mut stmts = Vec::new();
    for b in &mf.blocks {
        for s in &b.statements {
            if let MirStmt::Assign { dst, op, args } = s {
                let val = lower_mir_op(op, args, &pn);
                let val = substitute_locals(val, &stmts);
                stmts.push(crate::Expr::Let { name: dst.clone(), value: Box::new(val) });
            }
        }
        if let Some(MirTerminator::Call { func, args, dst, target: _ }) = &b.terminator {
            // MIR call lowers to a Let binding the destination local to a flux Expr::Call.
            // Args are substituted through the Let chain so the backend sees real values.
            let call_args: Vec<crate::Expr> = args.iter()
                .map(|a| substitute_locals(lower_operand(a, &pn), &stmts))
                .collect();
            let cleaned_func = func.rsplit("::").next().unwrap_or(func).to_string();
            stmts.push(crate::Expr::Let {
                name: dst.clone(),
                value: Box::new(crate::Expr::Call { func: cleaned_func, args: call_args }),
            });
        }
        if let Some(MirTerminator::Return) = &b.terminator {
            // Return value is whatever was last assigned to _0. Follow Variable("_N")
            // chains back through earlier Lets so MIR's overflow-checked path
            // `_3 = AddWithOverflow(_1,_2); _0 = move (_3.0)` resolves to the real BinaryOp.
            let rv = resolve_let_chain("_0", &stmts).unwrap_or(crate::Expr::Empty);
            let rv = substitute_locals(rv, &stmts);
            stmts.push(crate::Expr::Return(Box::new(rv)));
        }
    }
    crate::FunctionDef {
        name: mf.name.clone(),
        visibility: crate::Visibility::Public,
        params,
        return_type: ret,
        body: crate::Expr::Block(stmts),
        is_async: false,
    }
}

/// Detect the diamond pattern:
///   bb0: <cond stmts>; switchInt(discr) -> [0: else_bb, otherwise: then_bb]
///   then_bb: <stmts>; goto join_bb
///   else_bb: <stmts>; goto join_bb
///   join_bb: return
/// If found, return the equivalent Expr::If. Otherwise None.
fn try_lower_if_else(mf: &MirFunction, pn: &[String]) -> Option<crate::Expr> {
    if mf.blocks.len() < 4 { return None; }
    let bb0 = &mf.blocks[0];
    let (discr, then_label, else_label) = match &bb0.terminator {
        Some(MirTerminator::SwitchInt { discr, targets, otherwise }) => {
            let else_lbl = targets.iter().find(|(v, _)| v == "0").map(|(_, l)| l.clone())?;
            (discr.clone(), otherwise.clone(), else_lbl)
        }
        _ => return None,
    };

    // Build bb0's stmts so we can resolve the discriminant value.
    let mut bb0_stmts: Vec<crate::Expr> = Vec::new();
    for s in &bb0.statements {
        if let MirStmt::Assign { dst, op, args } = s {
            let val = lower_mir_op(op, args, pn);
            let val = substitute_locals(val, &bb0_stmts);
            bb0_stmts.push(crate::Expr::Let { name: dst.clone(), value: Box::new(val) });
        }
    }
    let cond_expr = resolve_let_chain(&discr, &bb0_stmts)?;
    let cond_expr = substitute_locals(cond_expr, &bb0_stmts);

    // Find the join block: a block reachable from BOTH branches that ends with Return.
    // This works for simple if/else (where both branches end with Goto join) and for
    // recursive/multi-step branches where one or both branches go through Call→Assert
    // chains before reaching the join.
    let then_reach = reachable_blocks(&then_label, mf);
    let else_reach = reachable_blocks(&else_label, mf);
    let join_label = then_reach.intersection(&else_reach)
        .filter(|l| matches!(
            mf.blocks.iter().find(|b| b.label == ***l).and_then(|b| b.terminator.as_ref()),
            Some(MirTerminator::Return)
        ))
        .min()  // deterministic pick
        .map(|s| s.to_string())?;

    let then_stmts = walk_chain(&then_label, &join_label, mf, pn)?;
    let else_stmts = walk_chain(&else_label, &join_label, mf, pn)?;
    let then_val_raw = resolve_let_chain("_0", &then_stmts)?;
    let else_val_raw = resolve_let_chain("_0", &else_stmts)?;
    let then_val = substitute_locals(then_val_raw, &then_stmts);
    let else_val = substitute_locals(else_val_raw, &else_stmts);

    Some(crate::Expr::If {
        cond: Box::new(cond_expr),
        then_branch: Box::new(then_val),
        else_branch: Some(Box::new(else_val)),
    })
}

/// Compute the set of block labels reachable from `start` by following Goto/Assert/Call/
/// SwitchInt terminators. Used to find the merge point of an if/else's branches.
fn reachable_blocks(start: &str, mf: &MirFunction) -> std::collections::HashSet<String> {
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut stack = vec![start.to_string()];
    while let Some(label) = stack.pop() {
        if !visited.insert(label.clone()) { continue; }
        let bb = match mf.blocks.iter().find(|b| b.label == label) {
            Some(b) => b, None => continue,
        };
        match &bb.terminator {
            Some(MirTerminator::Goto(t)) => stack.push(t.clone()),
            Some(MirTerminator::Assert { target, .. }) => stack.push(target.clone()),
            Some(MirTerminator::Call { target, .. }) => stack.push(target.clone()),
            Some(MirTerminator::SwitchInt { targets, otherwise, .. }) => {
                for (_, t) in targets { stack.push(t.clone()); }
                stack.push(otherwise.clone());
            }
            _ => {}
        }
    }
    visited
}

/// Walk the chain from `start` through Goto/Assert/Call terminators, accumulating
/// statements as Lets. Stops when reaching `stop`. Returns None on cycles or unsupported
/// terminators (e.g. SwitchInt within a branch).
fn walk_chain(start: &str, stop: &str, mf: &MirFunction, pn: &[String]) -> Option<Vec<crate::Expr>> {
    let mut stmts: Vec<crate::Expr> = Vec::new();
    let mut current = start.to_string();
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    loop {
        if current == stop { return Some(stmts); }
        if !visited.insert(current.clone()) { return None; }
        let bb = mf.blocks.iter().find(|b| b.label == current)?;
        for s in &bb.statements {
            if let MirStmt::Assign { dst, op, args } = s {
                let val = lower_mir_op(op, args, pn);
                let val = substitute_locals(val, &stmts);
                stmts.push(crate::Expr::Let { name: dst.clone(), value: Box::new(val) });
            }
        }
        match &bb.terminator {
            Some(MirTerminator::Goto(t)) => current = t.clone(),
            Some(MirTerminator::Assert { target, .. }) => current = target.clone(),
            Some(MirTerminator::Call { func, args, dst, target }) => {
                let call_args: Vec<crate::Expr> = args.iter()
                    .map(|a| substitute_locals(lower_operand(a, pn), &stmts))
                    .collect();
                let cleaned_func = func.rsplit("::").next().unwrap_or(func).to_string();
                let call_expr = crate::Expr::Call { func: cleaned_func, args: call_args };
                stmts.push(crate::Expr::Let { name: dst.clone(), value: Box::new(call_expr) });
                current = target.clone();
            }
            Some(MirTerminator::Return) => return Some(stmts),
            _ => return None,
        }
    }
}

fn extract_branch_value(target: &str, bb: &MirBlock, pn: &[String]) -> Option<crate::Expr> {
    let mut stmts: Vec<crate::Expr> = Vec::new();
    for s in &bb.statements {
        if let MirStmt::Assign { dst, op, args } = s {
            let val = lower_mir_op(op, args, pn);
            let val = substitute_locals(val, &stmts);
            stmts.push(crate::Expr::Let { name: dst.clone(), value: Box::new(val) });
        }
    }
    // If the block ends with a Call terminator that writes to our target local,
    // the call itself is the branch's value (`if cond { f(x) } else { g(x) }`).
    if let Some(MirTerminator::Call { func, args, dst, target: _ }) = &bb.terminator {
        let call_args: Vec<crate::Expr> = args.iter()
            .map(|a| substitute_locals(lower_operand(a, pn), &stmts))
            .collect();
        let cleaned_func = func.rsplit("::").next().unwrap_or(func).to_string();
        let call_expr = crate::Expr::Call { func: cleaned_func, args: call_args };
        if dst == target { return Some(call_expr); }
        stmts.push(crate::Expr::Let { name: dst.clone(), value: Box::new(call_expr) });
    }
    let v = resolve_let_chain(target, &stmts)?;
    Some(substitute_locals(v, &stmts))
}

fn resolve_let_chain(target: &str, stmts: &[crate::Expr]) -> Option<crate::Expr> {
    for stmt in stmts.iter().rev() {
        if let crate::Expr::Let { name, value } = stmt {
            if name == target {
                // If the value points to another local, try to resolve it further —
                // but if the chain dead-ends (e.g. points to a function param), keep the
                // current value rather than returning None.
                if let crate::Expr::Variable(next) = value.as_ref() {
                    if next.starts_with('_') {
                        if let Some(resolved) = resolve_let_chain(next, stmts) {
                            return Some(resolved);
                        }
                    }
                }
                return Some((**value).clone());
            }
        }
    }
    None
}

fn lower_mir_op(op: &str, args: &[String], pn: &[String]) -> crate::Expr {
    use crate::{Expr, BinOp};
    let binop = |bop: BinOp, args: &[String]| -> Expr {
        Expr::BinaryOp {
            op: bop,
            left: Box::new(lower_operand(&args[0], pn)),
            right: Box::new(lower_operand(&args[1], pn)),
        }
    };
    match op {
        "AddWithOverflow" | "Add"            if args.len() >= 2 => binop(BinOp::Add, args),
        "SubWithOverflow" | "Sub"            if args.len() >= 2 => binop(BinOp::Sub, args),
        "MulWithOverflow" | "Mul"            if args.len() >= 2 => binop(BinOp::Mul, args),
        "Div"                                if args.len() >= 2 => binop(BinOp::Div, args),
        "Rem"                                if args.len() >= 2 => binop(BinOp::Rem, args),
        "Eq"                                 if args.len() >= 2 => binop(BinOp::Eq, args),
        "Ne"                                 if args.len() >= 2 => binop(BinOp::Neq, args),
        "Lt"                                 if args.len() >= 2 => binop(BinOp::Lt, args),
        "Gt"                                 if args.len() >= 2 => binop(BinOp::Gt, args),
        "Le"                                 if args.len() >= 2 => binop(BinOp::Le, args),
        "Ge"                                 if args.len() >= 2 => binop(BinOp::Ge, args),
        "BitAnd"                             if args.len() >= 2 => binop(BinOp::And, args),
        "BitOr"                              if args.len() >= 2 => binop(BinOp::Or, args),
        "BitXor"                             if args.len() >= 2 => binop(BinOp::BitXor, args),
        "Shl" | "ShlUnchecked"               if args.len() >= 2 => binop(BinOp::Shl, args),
        "Shr" | "ShrUnchecked"               if args.len() >= 2 => binop(BinOp::Shr, args),
        "Neg"                                if args.len() >= 1 => crate::Expr::Unary { op: crate::UnOp::Neg, operand: Box::new(lower_operand(&args[0], pn)) },
        "Not"                                if args.len() >= 1 => crate::Expr::Unary { op: crate::UnOp::Not, operand: Box::new(lower_operand(&args[0], pn)) },
        "as"                                 if args.len() >= 2 => crate::Expr::Cast { value: Box::new(lower_operand(&args[0], pn)), target: mir_type_to_ir(&args[1]) },
        "copy" | "move" | "Use" | "const"    if args.len() >= 1 => lower_operand(&args[0], pn),
        _ => Expr::Empty,
    }
}

fn lower_operand(op: &str, pn: &[String]) -> crate::Expr {
    let c = op.trim().trim_start_matches("copy ").trim_start_matches("move ").trim();
    // Tuple-projection like `(_3.0: i64)` → use _3 as the carrier; tuple unpacking proper TBD.
    let c = c.trim_start_matches('(').trim_end_matches(')');
    // Strip the projection suffix ONLY for locals (`_3.0` -> `_3`). A const literal like
    // `2.5f64` must keep its decimal point, or it truncates to the integer `2`.
    let c = if c.starts_with('_') {
        if let Some(dot) = c.find('.') { &c[..dot] } else { c }
    } else { c };
    if c.starts_with('_') && c.chars().nth(1).map_or(false, |c| c.is_ascii_digit()) {
        let n: String = c.chars().skip(1).take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(i) = n.parse::<usize>() {
            if i > 0 && i <= pn.len() {
                return crate::Expr::Variable(pn[i - 1].clone());
            }
            // Refer to a local (e.g. _3 from an earlier Assign) by its raw name.
            return crate::Expr::Variable(format!("_{}", i));
        }
        crate::Expr::Variable(c.to_string())
    } else if let Some(rest) = c.strip_prefix("const ") {
        // MIR literals: ints "const 2_i64", "const -5_i32" (underscore + suffix);
        // floats "const 0f64", "const 1.5f64" (suffix attached, NO underscore); "const true".
        let r = rest.trim();
        if r == "true" {
            crate::Expr::Literal(crate::Literal::Bool(true))
        } else if r == "false" {
            crate::Expr::Literal(crate::Literal::Bool(false))
        } else if let Some(num) = r.strip_suffix("f64").or_else(|| r.strip_suffix("f32")) {
            crate::Expr::Literal(crate::Literal::Float(num.trim_end_matches('_').parse().unwrap_or(0.0)))
        } else if let Ok(i) = r.split('_').next().unwrap_or("")
            .trim_end_matches(|c: char| c.is_ascii_alphabetic()).parse::<i64>() {
            crate::Expr::Literal(crate::Literal::Int(i))
        } else {
            crate::Expr::Empty
        }
    } else if let Ok(i) = c.parse::<i64>() {
        crate::Expr::Literal(crate::Literal::Int(i))
    } else {
        crate::Expr::Variable(c.to_string())
    }
}

/// Walk an Expr and replace any `Variable("_N")` with its bound value from `stmts` (recursively).
/// Necessary because the backend only knows about function params, not arbitrary MIR locals.
fn substitute_locals(expr: crate::Expr, stmts: &[crate::Expr]) -> crate::Expr {
    use crate::Expr;
    match expr {
        Expr::Variable(ref name) if name.starts_with('_')
            && name.chars().nth(1).map_or(false, |c| c.is_ascii_digit()) =>
        {
            if let Some(value) = resolve_let_chain(name, stmts) {
                substitute_locals(value, stmts)
            } else {
                expr
            }
        }
        Expr::BinaryOp { op, left, right } => Expr::BinaryOp {
            op,
            left: Box::new(substitute_locals(*left, stmts)),
            right: Box::new(substitute_locals(*right, stmts)),
        },
        Expr::Unary { op, operand } => Expr::Unary {
            op,
            operand: Box::new(substitute_locals(*operand, stmts)),
        },
        Expr::Cast { value, target } => Expr::Cast {
            value: Box::new(substitute_locals(*value, stmts)),
            target,
        },
        Expr::Call { func, args } => Expr::Call {
            func,
            args: args.into_iter().map(|a| substitute_locals(a, stmts)).collect(),
        },
        Expr::Let { name, value } => Expr::Let {
            name,
            value: Box::new(substitute_locals(*value, stmts)),
        },
        Expr::Return(v) => Expr::Return(Box::new(substitute_locals(*v, stmts))),
        Expr::Block(items) => Expr::Block(items.into_iter().map(|i| substitute_locals(i, stmts)).collect()),
        other => other,
    }
}

fn mir_type_to_ir(t: &str) -> crate::TypeRef {
    match t.trim().trim_start_matches("-> ") {
        "i64" => crate::TypeRef::I64,
        "i32" => crate::TypeRef::I32,
        "u64" => crate::TypeRef::U64,
        "u32" => crate::TypeRef::U32,
        "bool" => crate::TypeRef::Bool,
        "f64" => crate::TypeRef::F64,
        "f32" => crate::TypeRef::F32,
        "()" | "" => crate::TypeRef::Unit,
        _ => crate::TypeRef::Named(t.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_add() {
        let mir = r#"fn add(_1: i64, _2: i64) -> i64 {
    debug a => _1;
    let mut _0: i64;
    bb0: {
        _0 = Add(_1, _2);
        return;
    }
}"#;
        let funcs = parse_mir(mir).unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "add");
        assert_eq!(funcs[0].params.len(), 2);
        assert_eq!(funcs[0].blocks.len(), 1);
        // scalar return type must be the clean form, NOT "-> i64"
        assert_eq!(funcs[0].return_type, "i64");
    }

    #[test]
    fn test_parse_fn_header_return_arrow_stripped() {
        // Regression: rustc renders `) -> (i64, i64)` with a leading space before the
        // arrow. The header parser must strip the arrow so return_type is the clean
        // "(i64, i64)" — else parse_tuple_type's starts_with('(') fails and a
        // tuple/struct-returning fn silently collapses to a single return value
        // (the c1=3-not-7 / c2=0-not-42 aggregate-returning-call bug).
        let mir = r#"fn mk() -> (i64, i64) {
    let mut _0: (i64, i64);
    bb0: {
        _0 = (const 3_i64, const 4_i64);
        return;
    }
}"#;
        let funcs = parse_mir(mir).unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "mk");
        assert_eq!(funcs[0].return_type, "(i64, i64)",
            "return_type must be clean, got {:?}", funcs[0].return_type);
        assert!(!funcs[0].return_type.contains("->"),
            "no arrow may survive in return_type");
    }

    #[test]
    fn frontend_trait_default_matches_parse_mir() {
        // FIP-0001 #3: the default RustcMirFrontend must be a transparent wrapper over parse_mir,
        // so swapping in a native (Option A) frontend later changes nothing downstream.
        let mir = "fn add(_1: i64, _2: i64) -> i64 {\n    let mut _0: i64;\n    bb0: {\n        _0 = Add(_1, _2);\n        return;\n    }\n}";
        let via_trait = RustcMirFrontend.parse(mir).unwrap();
        let via_fn = parse_mir(mir).unwrap();
        assert_eq!(via_trait.len(), via_fn.len());
        assert_eq!(via_trait[0].name, "add");
        assert_eq!(via_trait[0].name, via_fn[0].name);
    }

    #[test]
    fn parse_mir_cache_roundtrip_and_hit() {
        // Pin an isolated cache dir if we're first; if another test already
        // resolved it, content-addressing keeps this test correct anyway.
        let _ = flux_cache::set_cache_dir(
            std::env::temp_dir().join(format!("flux-frontend-test-{}", std::process::id())),
        );
        let mir = "fn cache_probe(_1: i64, _2: i64) -> i64 {\n    let mut _0: i64;\n    bb0: {\n        _0 = Add(_1, _2);\n        return;\n    }\n}";
        let (h0, _, _) = mir_parse_cache_stats();
        let a = parse_mir(mir).unwrap();
        let b = parse_mir(mir).unwrap();
        let (h1, _, _) = mir_parse_cache_stats();
        assert!(h1 > h0, "second parse of identical MIR must HIT the IR cache (h0={h0} h1={h1})");
        assert_eq!(a.len(), b.len());
        assert_eq!(a[0].name, "cache_probe");
        assert_eq!(a[0].name, b[0].name);
        assert_eq!(a[0].blocks.len(), b[0].blocks.len());
        // And the cached result must equal a fresh uncached parse.
        let fresh = parse_mir_uncached(mir).unwrap();
        assert_eq!(serde_json::to_string(&fresh).unwrap(), serde_json::to_string(&b).unwrap());
    }

    #[test]
    fn ir_version_frozen() {
        // FIP-0001 #1: this guards the FROZEN IR. If a change to the public IR types is intended,
        // bump crate::IR_VERSION and update this assertion in the same commit — never silently.
        assert_eq!(crate::IR_VERSION, 3,
            "flux-frontend IR changed: bump IR_VERSION intentionally (FIP-0001 frozen-IR contract)");
    }

    #[test]
    fn resolve_named_consts() {
        // The gap dogfooding sigil-emission::block_reward: `Div(copy _1, const HALVING_INTERVAL)`.
        let mir = "fn f(_1: u64) -> u64 {\n    bb0: {\n        _0 = Div(copy _1, const HALVING_INTERVAL);\n        return;\n    }\n}\n";
        let consts: std::collections::HashMap<String,String> =
            [("HALVING_INTERVAL".to_string(), "2100000_u64".to_string())].into_iter().collect();
        let funcs = resolve_consts(parse_mir(mir).unwrap(), &consts);
        let stmt = &funcs[0].blocks[0].statements[0];
        if let MirStmt::Assign { args, .. } = stmt {
            assert_eq!(args, &vec!["_1".to_string(), "const 2100000_u64".to_string()],
                "named const must be substituted with its width-tagged value");
        } else { panic!("expected assign"); }
    }

    #[test]
    fn monomorphize_helpers_and_pass() {
        assert_eq!(parse_turbofish("id::<i64>"), Some(("id".to_string(), vec!["i64".to_string()])));
        assert_eq!(parse_turbofish("fst::<i64, bool>"), Some(("fst".to_string(), vec!["i64".to_string(), "bool".to_string()])));
        assert_eq!(parse_turbofish("plain_call"), None);
        assert_eq!(parse_turbofish("MyOpt::<i64>::Some"), None); // enum ctor, not a generic fn call
        assert_eq!(mangle("id", &["i64".into()]), "id$i64");
        assert_eq!(mangle("fst", &["i64".into(), "bool".into()]), "fst$i64$bool");
        let mut m = std::collections::HashMap::new();
        m.insert("T".to_string(), "u32".to_string());
        assert_eq!(subst_type("T", &m), "u32");
        assert_eq!(subst_type("(T, i64)", &m), "(u32, i64)");
        assert_eq!(subst_type("TypeName", &m), "TypeName"); // whole-token only
        // detect_type_params: first-appearance order across params then return.
        let f = MirFunction { name: "fst".into(), return_type: "A".into(),
            params: vec![MirLocal{index:1,name:String::new(),ty:"A".into(),mutable:false},
                         MirLocal{index:2,name:String::new(),ty:"B".into(),mutable:false}],
            locals: vec![], blocks: vec![] };
        assert_eq!(detect_type_params(&f), vec!["A".to_string(), "B".to_string()]);
        // (end-to-end monomorphization is proven by the compile-native `run()=100` gate)
    }

    #[test]
    fn monomorphize_generic_trait_callee_rewrite() {
        // Ladder rung 7 part 2 (generic trait dispatch): area_of<T> calls `<T as Area>::area`,
        // canonicalized pre-monomorphize to `T__area`. specialize()-ing area_of for T=Sq must
        // rewrite that callee to `Sq__area` (the concrete impl's canon name) -- otherwise the
        // instance calls a function that never exists (silently mis-linked / wrong value).
        let mir = "fn <impl at t.rs:1:1: 1:1>::area(_1: &Sq) -> i64 {
    bb0: {
        _0 = copy ((*_1).0: i64);
        return;
    }
}
fn area_of(_1: &T) -> i64 {
    bb0: {
        _0 = <T as Area>::area(copy _1) -> [return: bb1, unwind continue];
    }
    bb1: {
        return;
    }
}
fn call_generic(_1: Sq) -> i64 {
    bb0: {
        _2 = &_1;
        _0 = area_of::<Sq>(copy _2) -> [return: bb1, unwind continue];
    }
    bb1: {
        return;
    }
}
";
        let funcs = monomorphize(parse_mir(mir).unwrap());
        let names: Vec<&str> = funcs.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"Sq__area"), "impl must canonicalize to Sq__area: {:?}", names);
        assert!(names.contains(&"area_of$Sq"), "generic must specialize to area_of$Sq: {:?}", names);
        let inst = funcs.iter().find(|f| f.name == "area_of$Sq").unwrap();
        let callee = match &inst.blocks[0].terminator {
            Some(MirTerminator::Call { func, .. }) => func.clone(),
            other => panic!("expected a call terminator, got {:?}", other),
        };
        assert_eq!(callee, "Sq__area",
            "specialized area_of$Sq must call the CONCRETE impl (Sq__area), not the still-generic T__area");
    }

    #[test]
    fn vec_ops_rewrite_to_runtime_shims() {
        // Rung 9: recognized Vec<i64> std calls rewrite to __flux_vec_* shims
        // (incl. the Index call, which first canonicalizes through the trait
        // machinery); an UNRECOGNIZED std op must keep its name → loud link.
        let mir = "fn main() -> i64 {
    let mut _0: i64;
    let mut _1: std::vec::Vec<i64>;
    let mut _3: &mut std::vec::Vec<i64>;
    let mut _7: &i64;
    let mut _8: &std::vec::Vec<i64>;
    bb0: {
        _1 = Vec::<i64>::new() -> [return: bb1, unwind continue];
    }
    bb1: {
        _3 = &mut _1;
        _2 = Vec::<i64>::push(move _3, const 3_i64) -> [return: bb2, unwind continue];
    }
    bb2: {
        _8 = &_1;
        _7 = <Vec<i64> as Index<usize>>::index(move _8, const 0_usize) -> [return: bb3, unwind continue];
    }
    bb3: {
        _9 = Vec::<i64>::truncate(move _8, const 1_usize) -> [return: bb4, unwind continue];
    }
    bb4: {
        return;
    }
}
";
        let funcs = normalize_traits(parse_mir(mir).unwrap());
        let callees: Vec<String> = funcs[0].blocks.iter().filter_map(|b| match &b.terminator {
            Some(MirTerminator::Call { func, .. }) => Some(func.clone()),
            _ => None,
        }).collect();
        assert_eq!(callees, vec![
            "__flux_vec_new".to_string(),
            "__flux_vec_push".to_string(),
            "__flux_vec_index".to_string(),
            "Vec::<i64>::truncate".to_string(), // unrecognized → untouched → loud
        ]);
    }

    #[test]
    fn closures_flatten_canonicalize_and_tupleize() {
        // Rung 8: the `{closure@FILE:L:C}` type flattens to a deterministic
        // identifier, `parent::{closure#N}` renames to `<flat>__call` matching
        // the canonicalized `<flat as Fn<..>>::call` site, the capture struct
        // tuple-izes, drop glue becomes a Goto, and `resume` is unreachable.
        let mir = "fn main::{closure#0}(_1: &{closure@t.rs:3:17: 3:25}, _2: i64) -> i64 {
    bb0: {
        _5 = copy ((*_1).0: &i64);
        _3 = copy (*_5);
        _0 = copy _3;
        return;
    }
}
fn apply(_1: F, _2: i64) -> i64 {
    let mut _0: i64;
    let mut _3: &F;
    let mut _4: (i64,);
    bb0: {
        _3 = &_1;
        _4 = (copy _2,);
        _0 = <F as Fn<(i64,)>>::call(move _3, move _4) -> [return: bb1, unwind: bb3];
    }
    bb1: {
        drop(_1) -> [return: bb2, unwind continue];
    }
    bb2: {
        return;
    }
    bb3 (cleanup): {
        resume;
    }
}
fn main() -> i64 {
    let mut _0: i64;
    let _1: i64;
    let mut _3: &i64;
    let _2: {closure@t.rs:3:17: 3:25};
    bb0: {
        _1 = const 3_i64;
        _3 = &_1;
        _2 = {closure@t.rs:3:17: 3:25} { k: move _3 };
        _0 = apply::<{closure@t.rs:3:17: 3:25}>(move _2, const 4_i64) -> [return: bb1, unwind continue];
    }
    bb1: {
        return;
    }
}
";
        let funcs = monomorphize(parse_mir(mir).unwrap());
        let names: Vec<&str> = funcs.iter().map(|f| f.name.as_str()).collect();
        // The closure body canonicalized to <flat>__call and the specialized
        // apply instance's inner `F__call` was rewritten to it.
        let call_fn = names.iter().find(|n| n.starts_with("__closure_") && n.ends_with("__call"))
            .expect("closure body must canonicalize to __closure_<h>__call");
        let inst = funcs.iter().find(|f| f.name.starts_with("apply$")).expect("apply must specialize");
        match &inst.blocks[0].terminator {
            Some(MirTerminator::Call { func, .. }) =>
                assert_eq!(func, call_fn, "specialized apply must call the concrete closure"),
            other => panic!("expected call, got {:?}", other),
        }
        // Drop glue → Goto; cleanup resume → Unreachable; capture struct → 1-tuple.
        assert!(matches!(
            inst.blocks.iter().find(|b| b.label == "bb1").unwrap().terminator,
            Some(MirTerminator::Goto(ref t)) if t == "bb2"), "drop must lower to Goto");
        assert!(matches!(
            inst.blocks.iter().find(|b| b.label == "bb3").unwrap().terminator,
            Some(MirTerminator::Unreachable)), "resume must lower to Unreachable");
        let body = funcs.iter().find(|f| f.name == **call_fn).unwrap();
        assert_eq!(body.params[0].ty, "(i64)", "capture struct must tuple-ize");
        assert!(funcs.iter().all(|f| f.params.iter().chain(f.locals.iter())
            .all(|l| !l.ty.contains("closure@"))), "no raw closure type may survive");
    }

    #[test]
    fn multi_impl_dyn_lowers_to_tagged_switch() {
        // Rung 7 part 3b: TWO impls of `area` → the dyn call must become a
        // SwitchInt over the tag slot fanning out to Rect__area / Sq__area,
        // the `&dyn Area` carrier must become the (tag, p0, p1) tuple, and the
        // unsize casts must become tagged constructions (Sq zero-padded).
        let mir = "fn <impl at t.rs:1:1: 1:1>::area(_1: &Sq) -> i64 {
    bb0: {
        _0 = copy ((*_1).0: i64);
        return;
    }
}
fn <impl at t.rs:2:2: 2:2>::area(_1: &Rect) -> i64 {
    bb0: {
        _0 = copy ((*_1).0: i64);
        return;
    }
}
fn dyn_call(_1: &dyn Area) -> i64 {
    bb0: {
        _0 = <dyn Area as Area>::area(copy _1) -> [return: bb1, unwind continue];
    }
    bb1: {
        return;
    }
}
fn main() -> i64 {
    let mut _0: i64;
    let _1: Sq;
    let _2: Rect;
    let mut _3: &dyn Area;
    let _4: &Sq;
    bb0: {
        _1 = Sq { s: const 4_i64 };
        _2 = Rect { w: const 2_i64, h: const 3_i64 };
        _4 = &_1;
        _3 = copy _4 as &dyn Area (PointerCoercion(Unsize, Implicit));
        _0 = dyn_call(move _3) -> [return: bb1, unwind continue];
    }
    bb1: {
        return;
    }
}
";
        let funcs = normalize_traits(parse_mir(mir).unwrap());
        // dyn_call's param is now the tagged tuple (tag + max(1,2) payload slots).
        let dc = funcs.iter().find(|f| f.name == "dyn_call").unwrap();
        assert_eq!(dc.params[0].ty, "(i64, i64, i64)",
            "multi-impl dyn carrier must become the tagged tuple");
        // Its call became a switch fanning out to both canon impls.
        let (targets, otherwise) = match &dc.blocks[0].terminator {
            Some(MirTerminator::SwitchInt { targets, otherwise, .. }) => (targets.clone(), otherwise.clone()),
            other => panic!("dyn call must lower to SwitchInt, got {:?}", other),
        };
        let mut callees: Vec<String> = targets.iter().map(|(_, l)| l.clone())
            .chain([otherwise]).filter_map(|l| {
                dc.blocks.iter().find(|b| b.label == l).and_then(|b| match &b.terminator {
                    Some(MirTerminator::Call { func, .. }) => Some(func.clone()),
                    _ => None,
                })
            }).collect();
        callees.sort();
        assert_eq!(callees, vec!["Rect__area".to_string(), "Sq__area".to_string()],
            "switch branches must call BOTH canonicalized impls");
        // main's Sq unsize cast became a tagged construction, zero-padded to
        // the Rect width: (const <sq_tag>_i64, copy <field>, const 0_i64).
        let mn = funcs.iter().find(|f| f.name == "main").unwrap();
        let ctor = mn.blocks[0].statements.iter().find_map(|s| match s {
            MirStmt::Assign { op, args, .. } if op.is_empty() && args.len() == 3
                && args[0].starts_with("const ") && args[2] == "const 0_i64" => Some(args.clone()),
            _ => None,
        });
        assert!(ctor.is_some(),
            "Sq's unsize cast must become a zero-padded tagged tuple construction");
        assert!(mn.locals.iter().all(|l| !l.ty.contains("dyn")),
            "no dyn carrier may survive the lowering");
    }

    #[test]
    fn normalize_traits_devirtualizes_unique_dyn_impl() {
        // Ladder rung 7 part 3 (dyn dispatch, closed-world): `<dyn Area as Area>::area`
        // canonicalizes to `dyn Area__area` — a symbol no impl defines. With exactly one
        // impl of `area` visible in the unit, the callee must devirtualize to `Sq__area`,
        // the `&dyn Area` carriers must become the concrete `Sq`, and the unsize coercion
        // must collapse to a plain aggregate copy.
        let mir = "fn <impl at t.rs:1:1: 1:1>::area(_1: &Sq) -> i64 {
    bb0: {
        _0 = copy ((*_1).0: i64);
        return;
    }
}
fn dyn_call(_1: &dyn Area) -> i64 {
    bb0: {
        _0 = <dyn Area as Area>::area(copy _1) -> [return: bb1, unwind continue];
    }
    bb1: {
        return;
    }
}
fn main() -> i64 {
    let mut _0: i64;
    let _1: Sq;
    let mut _2: &dyn Area;
    let _3: &Sq;
    bb0: {
        _1 = Sq { s: const 4_i64 };
        _3 = &_1;
        _2 = copy _3 as &dyn Area (PointerCoercion(Unsize, Implicit));
        _0 = dyn_call(move _2) -> [return: bb1, unwind continue];
    }
    bb1: {
        return;
    }
}
";
        let funcs = normalize_traits(parse_mir(mir).unwrap());
        let dc = funcs.iter().find(|f| f.name == "dyn_call").unwrap();
        match &dc.blocks[0].terminator {
            Some(MirTerminator::Call { func, .. }) =>
                assert_eq!(func, "Sq__area", "dyn callee must devirtualize to the unique impl"),
            other => panic!("expected a call terminator, got {:?}", other),
        }
        assert_eq!(dc.params[0].ty, "Sq",
            "&dyn Area receiver must become the concrete by-value type, not `dyn`");
        let mn = funcs.iter().find(|f| f.name == "main").unwrap();
        assert!(mn.locals.iter().all(|l| !l.ty.contains("dyn")),
            "no dyn carrier may survive devirtualization: {:?}",
            mn.locals.iter().map(|l| &l.ty).collect::<Vec<_>>());
        let unsize_collapsed = mn.blocks[0].statements.iter().any(|s| matches!(s,
            MirStmt::Assign { op, args, .. } if op == "copy" && args == &vec!["_3".to_string()]));
        assert!(unsize_collapsed, "unsize coercion must collapse to a plain copy of the operand");
    }

    #[test]
    fn parse_rhs_data_carrying_enum_forms() {
        // Ladder rung 4 (part 2): construction keeps the variant path + payload args.
        assert_eq!(parse_rhs("Opt::Some(const 42_i64)"),
            ("Opt::Some".to_string(), vec!["const 42_i64".to_string()]));
        assert_eq!(parse_rhs("Opt::None"), ("Opt::None".to_string(), vec![]));
        // Payload extraction `((_1 as Some).0: i64)` → `_N|Variant|K` (raw K; the backend
        // computes the real field offset from the enum layout).
        assert_eq!(parse_rhs("copy ((_1 as Some).0: i64)"),
            ("copy".to_string(), vec!["_1|Some|0".to_string()]));
        assert_eq!(parse_rhs("move ((_3 as Box).1: i64)"),
            ("copy".to_string(), vec!["_3|Box|1".to_string()]));
        // A PLAIN cast must still reach the cast handler, NOT be eaten as a projection.
        assert_eq!(parse_rhs("move _1 as i64 (IntToInt)").0, "as");
        assert_eq!(strip_downcast_projection("move _1 as i64 (IntToInt)"), None);
    }

    #[test]
    fn parse_clike_enum() {
        // Ladder rung 4: C-like enums parse into unit.enums with running discriminants, overridable
        // by an explicit `= N` (then resuming previous+1).
        let u = crate::parse_source("enum Color { Red, Green, Blue }\nenum E { A = 10, B }", "t").unwrap();
        assert_eq!(u.enums.len(), 2);
        assert_eq!(u.enums[0].name, "Color");
        assert_eq!(u.enums[0].variants[0].discriminant, 0);
        assert_eq!(u.enums[0].variants[1].name, "Green");
        assert_eq!(u.enums[0].variants[1].discriminant, 1);
        assert_eq!(u.enums[0].variants[2].discriminant, 2);
        assert_eq!(u.enums[1].variants[0].discriminant, 10); // A = 10
        assert_eq!(u.enums[1].variants[1].discriminant, 11); // B = previous+1
    }

    #[test]
    fn test_parse_add_with_overflow() {
        let mir = r#"fn add(_1: i64, _2: i64) -> i64 {
    let mut _0: i64;
    let mut _3: (i64, bool);
    bb0: {
        _3 = AddWithOverflow(copy _1, copy _2);
        assert(!move (_3.1: bool), "...") -> [success: bb1, unwind continue];
    }
    bb1: {
        _0 = move (_3.0: i64);
        return;
    }
}"#;
        let funcs = parse_mir(mir).unwrap();
        assert_eq!(funcs.len(), 1);
        assert!(funcs[0].blocks.len() >= 1);
    }

    #[test]
    fn test_lower_add_returns_binop() {
        let mir = "fn add(_1: i64, _2: i64) -> i64 {\n    bb0: {\n        _0 = Add(copy _1, copy _2);\n        return;\n    }\n}";
        let funcs = parse_mir(mir).unwrap();
        let ir = lower_mir_to_ir(&funcs[0]);
        if let crate::Expr::Block(stmts) = &ir.body {
            let ret = stmts.last().expect("at least one stmt");
            assert!(matches!(ret, crate::Expr::Return(rv) if matches!(**rv, crate::Expr::BinaryOp { op: crate::BinOp::Add, .. })),
                "expected Return(BinaryOp(Add, ...)), got {:?}", ret);
        } else { panic!("expected Block body"); }
    }

    #[test]
    fn test_lower_mul_returns_correct_op() {
        let mir = "fn mul(_1: i64, _2: i64) -> i64 {\n    bb0: {\n        _0 = Mul(copy _1, copy _2);\n        return;\n    }\n}";
        let funcs = parse_mir(mir).unwrap();
        let ir = lower_mir_to_ir(&funcs[0]);
        if let crate::Expr::Block(stmts) = &ir.body {
            let ret = stmts.last().expect("at least one stmt");
            assert!(matches!(ret, crate::Expr::Return(rv) if matches!(**rv, crate::Expr::BinaryOp { op: crate::BinOp::Mul, .. })));
        } else { panic!("expected Block body"); }
    }
}
