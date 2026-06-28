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
    fn to_mir(&self, source: &str) -> Result<Vec<MirFunction>, String>;
}

/// The default, contracted frontend: parse rustc's `--emit=mir` textual output.
pub struct RustcMirFrontend;

impl Frontend for RustcMirFrontend {
    fn to_mir(&self, mir_text: &str) -> Result<Vec<MirFunction>, String> {
        parse_mir(mir_text)
    }
}

/// Parse MIR text output from rustc --emit=mir.
pub fn parse_mir(mir_text: &str) -> Result<Vec<MirFunction>, String> {
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

    let close_paren = rest.find(')').ok_or("no )")?;
    let params_str = &rest[paren_idx+1..close_paren];
    let return_part = &rest[close_paren+1..];

    // Trim BEFORE stripping the arrow: rustc renders `) -> T` so return_part has a
    // leading space (" -> T"). trim_start_matches("-> ") can't match past that space,
    // which left the arrow glued on ("-> (i64,i64)") and made parse_tuple_type's
    // starts_with('(') fail -> tuple/struct returns silently collapsed to 1 value.
    let return_type = return_part.trim().trim_start_matches("->").trim().to_string();

    let mut params = Vec::new();
    for (i, p) in params_str.split(',').enumerate() {
        let p = p.trim();
        if p.is_empty() { continue; }
        let parts: Vec<&str> = p.splitn(2, ':').collect();
        let name = parts[0].trim().to_string();
        let ty = parts.get(1).map(|s| s.trim().to_string()).unwrap_or_default();
        params.push(MirLocal { index: i + 1, name, ty, mutable: false });
    }

    Ok((name, params, return_type))
}

fn parse_block<'a, I>(lines: &mut std::iter::Peekable<I>) -> Result<MirBlock, String>
where I: Iterator<Item = &'a str>
{
    let header = lines.next().unwrap().trim().to_string();
    let label = header.trim_end_matches(": {").to_string();

    let mut statements = Vec::new();
    let mut terminator = None;

    while let Some(line) = lines.peek() {
        let trimmed = line.trim();
        if trimmed == "}" {
            lines.next();
            break;
        }

        if trimmed.starts_with("return") {
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
            // _0 = Add(_1, _2);
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
        } else if trimmed.starts_with("_") && trimmed.contains("= move") {
            let eq_idx = trimmed.find('=').unwrap();
            let dst = trimmed[..eq_idx].trim().to_string();
            let rhs = trimmed[eq_idx+1..].trim().trim_end_matches(';').trim_start_matches("move (");
            statements.push(MirStmt::Assign { dst, op: "move".into(), args: vec![rhs.to_string()] });
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
    // Data-carrying enum payload extraction: `copy ((_N as Variant).K: T)` — rustc downcasts an enum
    // local to a variant and projects its K-th field. Map it to aggregate field (K+1): field 0 is the
    // discriminant tag, payload slots start at 1. Emitting `copy _N.(K+1)` reuses the existing
    // `_N.F` tuple-projection resolver in the backend. MUST come BEFORE the ` as ` cast check below,
    // which would otherwise split on the inner " as " and mangle the operand.
    if let Some(proj) = strip_downcast_projection(rhs) {
        return ("copy".to_string(), vec![proj]);
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

/// Recognise a data-carrying enum downcast-projection `((_N as Variant).K: T)` (optionally prefixed
/// `copy `/`move `) and return the equivalent aggregate-field operand `_N.(K+1)`. The +1 skips the
/// discriminant tag, which the backend stores as field 0. Returns None for anything else — crucially
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
    // Field index after the closing `).` — `).0`, `).1`, …
    let dot = s[as_pos..].find(").")?;
    let kpos = as_pos + dot + 2;
    let kdigits: String = s[kpos..].chars().take_while(|c| c.is_ascii_digit()).collect();
    let k: usize = kdigits.parse().ok()?;
    Some(format!("_{}.{}", local, k + 1))
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
    f
}

/// Monomorphize a parsed MIR function list. See the module-section comment above. No turbofish calls →
/// the input is returned unchanged.
pub fn monomorphize(funcs: Vec<MirFunction>) -> Vec<MirFunction> {
    use std::collections::{HashMap, HashSet};
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
        let via_trait = RustcMirFrontend.to_mir(mir).unwrap();
        let via_fn = parse_mir(mir).unwrap();
        assert_eq!(via_trait.len(), via_fn.len());
        assert_eq!(via_trait[0].name, "add");
        assert_eq!(via_trait[0].name, via_fn[0].name);
    }

    #[test]
    fn ir_version_frozen() {
        // FIP-0001 #1: this guards the FROZEN IR. If a change to the public IR types is intended,
        // bump crate::IR_VERSION and update this assertion in the same commit — never silently.
        assert_eq!(crate::IR_VERSION, 3,
            "flux-frontend IR changed: bump IR_VERSION intentionally (FIP-0001 frozen-IR contract)");
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
    fn parse_rhs_data_carrying_enum_forms() {
        // Ladder rung 4 (part 2): construction keeps the variant path + payload args.
        assert_eq!(parse_rhs("Opt::Some(const 42_i64)"),
            ("Opt::Some".to_string(), vec!["const 42_i64".to_string()]));
        assert_eq!(parse_rhs("Opt::None"), ("Opt::None".to_string(), vec![]));
        // Payload extraction `((_1 as Some).0: i64)` → aggregate field K+1 (tag is field 0).
        assert_eq!(parse_rhs("copy ((_1 as Some).0: i64)"),
            ("copy".to_string(), vec!["_1.1".to_string()]));
        assert_eq!(parse_rhs("move ((_3 as Box).1: i64)"),
            ("copy".to_string(), vec!["_3.2".to_string()]));
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
