// flux-backend — Phase 3d: Cranelift CLIF generation
// Takes flux-frontend IR, generates Cranelift IR text.
// Runs on any architecture. Native codegen via clif-util or future JIT.

use flux_frontend::{FunctionDef, TypeRef, Expr, Literal, BinOp, TranslationUnit};
use flux_frontend::mir::{MirFunction, MirStmt, MirTerminator};
use cranelift_codegen::ir::{types, Type, AbiParam, Block, UserFuncName, Function, Signature, InstBuilder, Value};
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{Linkage, Module, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};

pub fn compile_unit(unit: &TranslationUnit) -> Result<Vec<String>, String> {
    unit.functions.iter().map(|f| compile_to_clif(f)).collect()
}

/// Phase 2 object emission: compile a TranslationUnit to a native object file (ELF on Linux).
pub fn compile_unit_to_object(unit: &TranslationUnit, out_path: &std::path::Path) -> Result<(), String> {
    compile_unit_to_object_with_mir(unit, &std::collections::HashMap::new(), out_path)
}

/// As above, but functions whose names appear in `mir_overrides` are compiled via the
/// MIR-direct path (using Cranelift Variables) instead of the Expr-based path. This is
/// how loops, mutable locals, and arbitrary CFG shapes get supported.
pub fn compile_unit_to_object_with_mir(
    unit: &TranslationUnit,
    mir_overrides: &std::collections::HashMap<String, MirFunction>,
    out_path: &std::path::Path,
) -> Result<(), String> {
    use std::collections::HashMap;
    use cranelift_module::FuncId;

    let isa_builder = cranelift_native::builder().map_err(|e| format!("native ISA: {}", e))?;
    let mut flag_builder = settings::builder();
    flag_builder.set("use_colocated_libcalls", "false").map_err(|e| e.to_string())?;
    flag_builder.set("is_pic", "true").map_err(|e| e.to_string())?;
    let isa = isa_builder.finish(settings::Flags::new(flag_builder)).map_err(|e| format!("isa finish: {}", e))?;

    let builder = ObjectBuilder::new(isa, "flux_module", default_libcall_names())
        .map_err(|e| format!("ObjectBuilder: {}", e))?;
    let mut module = ObjectModule::new(builder);

    // Pass 1: declare every function so Call expressions can resolve by name.
    let mut func_ids: HashMap<String, FuncId> = HashMap::with_capacity(unit.functions.len());
    let mut func_sigs: HashMap<String, Signature> = HashMap::with_capacity(unit.functions.len());
    for func in &unit.functions {
        let mut sig = module.make_signature();
        for p in &func.params { sig.params.push(AbiParam::new(cl_type(&p.ty))); }
        sig.returns.push(AbiParam::new(cl_type(&func.return_type)));
        let id = module.declare_function(&func.name, Linkage::Export, &sig)
            .map_err(|e| format!("declare {}: {}", func.name, e))?;
        func_ids.insert(func.name.clone(), id);
        func_sigs.insert(func.name.clone(), sig);
    }

    // Pass 1b: scan every body for Call expressions to unknown names. Declare them as
    // Linkage::Import with a default i64→i64 signature (guess by arity). The linker
    // resolves them at link time. Real signature inference from rlib metadata is TBD.
    let mut external_calls: HashMap<String, usize> = HashMap::new();
    for func in &unit.functions {
        collect_call_arities(&func.body, &mut external_calls);
    }
    for (name, arity) in external_calls {
        if func_ids.contains_key(&name) { continue; }
        let mut sig = module.make_signature();
        for _ in 0..arity { sig.params.push(AbiParam::new(types::I64)); }
        sig.returns.push(AbiParam::new(types::I64));
        let id = module.declare_function(&name, Linkage::Import, &sig)
            .map_err(|e| format!("declare import {}: {}", name, e))?;
        func_ids.insert(name, id);
    }

    // Pass 2: define each function body. Loopy functions take the MIR-direct path; the
    // rest take the Expr-based path (which is more compact for pure-expression bodies).
    for func in &unit.functions {
        let func_id = func_ids[&func.name];
        let sig = func_sigs.remove(&func.name).unwrap();

        let func_ir = if let Some(mir) = mir_overrides.get(&func.name) {
            compile_mir_into_function(mir, func_id, sig, &mut module, &func_ids)?
        } else {
            compile_expr_into_function(func, func_id, sig, &mut module, &func_ids)?
        };

        let mut ctx = Context::new();
        ctx.func = func_ir;
        module.define_function(func_id, &mut ctx)
            .map_err(|e| format!("define {}: {}", func.name, e))?;
    }

    let product = module.finish();
    let bytes = product.emit().map_err(|e| format!("emit: {}", e))?;
    std::fs::write(out_path, bytes).map_err(|e| format!("write {}: {}", out_path.display(), e))?;
    Ok(())
}

/// Build a Cranelift Function from an Expr-based FunctionDef. Used for non-loopy functions.
fn compile_expr_into_function(
    func: &FunctionDef,
    func_id: cranelift_module::FuncId,
    sig: Signature,
    module: &mut ObjectModule,
    func_ids: &std::collections::HashMap<String, cranelift_module::FuncId>,
) -> Result<Function, String> {
    let mut func_ir = Function::with_name_signature(UserFuncName::user(0, func_id.as_u32()), sig);
    let mut bctx = FunctionBuilderContext::new();
    let mut bc = FunctionBuilder::new(&mut func_ir, &mut bctx);
    let entry = bc.create_block();
    bc.switch_to_block(entry);
    bc.append_block_params_for_function_params(entry);
    let param_vals: Vec<Value> = bc.block_params(entry).to_vec();
    let val = compile_expr_with_calls(&mut bc, module, &func.body, &func.params, &param_vals, func_ids);
    bc.ins().return_(&[val]);
    bc.seal_all_blocks();
    bc.finalize();
    Ok(func_ir)
}

/// Build a Cranelift Function from a raw MIR function, using Cranelift Variables for
/// every MIR local. This handles loops, mutable updates, and arbitrary CFG shapes that
/// the Expr-based path can't express. Each MIR block becomes a Cranelift block.
fn compile_mir_into_function(
    mir: &MirFunction,
    func_id: cranelift_module::FuncId,
    sig: Signature,
    module: &mut ObjectModule,
    func_ids: &std::collections::HashMap<String, cranelift_module::FuncId>,
) -> Result<Function, String> {
    use std::collections::HashSet;

    let mut func_ir = Function::with_name_signature(UserFuncName::user(0, func_id.as_u32()), sig);
    let mut bctx = FunctionBuilderContext::new();
    let mut bc = FunctionBuilder::new(&mut func_ir, &mut bctx);

    // Collect every MIR local index, building a type map from the declared local types.
    // Tuples get their own treatment: each field becomes a separate Cranelift Variable
    // (scalar replacement of aggregates), so no memory operations are needed.
    let mut all_locals: HashSet<usize> = HashSet::new();
    let mut mir_type_strs: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    all_locals.insert(0);
    mir_type_strs.insert(0, mir.return_type.clone());
    for p in &mir.params {
        all_locals.insert(p.index);
        mir_type_strs.insert(p.index, p.ty.clone());
    }
    for loc in &mir.locals {
        all_locals.insert(loc.index);
        // Don't let debug-only entries (with empty ty) overwrite the real type
        // recorded from a `let` declaration.
        if !loc.ty.is_empty() {
            mir_type_strs.insert(loc.index, loc.ty.clone());
        }
    }
    for b in &mir.blocks {
        for s in &b.statements {
            if let MirStmt::Assign { dst, .. } = s {
                if let Some(idx) = parse_mir_local_idx(dst) { all_locals.insert(idx); }
            }
        }
        if let Some(MirTerminator::Call { dst, .. }) = &b.terminator {
            if let Some(idx) = parse_mir_local_idx(dst) { all_locals.insert(idx); }
        }
    }

    // Declare Variables. Single-valued locals → `vars`. Tuple locals → `tuple_vars` keyed
    // by (local_idx, field_idx), one Variable per field.
    let mut vars: std::collections::HashMap<usize, Variable> = std::collections::HashMap::new();
    let mut tuple_vars: std::collections::HashMap<(usize, usize), Variable> =
        std::collections::HashMap::new();
    for &idx in &all_locals {
        let ty_str = mir_type_strs.get(&idx).cloned().unwrap_or_default();
        if let Some(fields) = parse_tuple_type(&ty_str) {
            // Tuple — declare one Variable per field; IDs in a separate range to avoid
            // collisions with single-i64 locals.
            for (fi, field_ty) in fields.iter().enumerate() {
                let var_id = 1_000_000u32 + (idx as u32) * 32 + fi as u32;
                let v = Variable::from_u32(var_id);
                bc.declare_var(v, mir_type_to_cl(field_ty));
                tuple_vars.insert((idx, fi), v);
            }
        } else {
            let v = Variable::from_u32(idx as u32);
            bc.declare_var(v, mir_type_to_cl(&ty_str));
            vars.insert(idx, v);
        }
    }

    // Entry block FIRST (Cranelift treats the first-created block as the function entry).
    let entry = bc.create_block();
    // Pre-create a Cranelift block for each MIR block AFTER entry.
    let mut blocks: std::collections::HashMap<String, Block> = std::collections::HashMap::new();
    for b in &mir.blocks {
        blocks.insert(b.label.clone(), bc.create_block());
    }

    // Wire up entry: bind incoming params to their Variables, then jump to the first MIR block.
    bc.switch_to_block(entry);
    bc.append_block_params_for_function_params(entry);
    let entry_params: Vec<Value> = bc.block_params(entry).to_vec();
    for (i, p) in mir.params.iter().enumerate() {
        if let Some(&var) = vars.get(&p.index) {
            let val = entry_params[i];
            bc.def_var(var, val);
        }
    }
    let first_label = mir.blocks.first().map(|b| b.label.clone()).unwrap_or_default();
    if let Some(&first_bb) = blocks.get(&first_label) {
        bc.ins().jump(first_bb, &[]);
    } else {
        let zero = bc.ins().iconst(types::I64, 0);
        bc.ins().return_(&[zero]);
        bc.seal_all_blocks();
        bc.finalize();
        return Ok(func_ir);
    }
    bc.seal_block(entry);

    // Compile each MIR block into its corresponding Cranelift block.
    for mir_block in &mir.blocks {
        let cl_block = blocks[&mir_block.label];
        bc.switch_to_block(cl_block);

        for s in &mir_block.statements {
            if let MirStmt::Assign { dst, op, args } = s {
                if let Some(dst_idx) = parse_mir_local_idx(dst) {
                    // Tuple aggregate construction: parse_rhs emits op="" for `(a, b, ...)`.
                    if op.is_empty() && args.len() > 1 {
                        for (i, arg) in args.iter().enumerate() {
                            if let Some(&var) = tuple_vars.get(&(dst_idx, i)) {
                                let val = resolve_mir_operand_to_value(arg, &vars, &tuple_vars, &mut bc);
                                bc.def_var(var, val);
                            }
                        }
                        continue;
                    }
                    let val = compile_mir_op_to_value(op, args, &vars, &tuple_vars, &mut bc);
                    if let Some(&var) = vars.get(&dst_idx) {
                        bc.def_var(var, val);
                    } else if let Some(fields) = parse_tuple_type(&mir_type_strs.get(&dst_idx).cloned().unwrap_or_default()) {
                        // Single-arg op writing into a tuple: forward to field 0.
                        if !fields.is_empty() {
                            if let Some(&var) = tuple_vars.get(&(dst_idx, 0)) {
                                bc.def_var(var, val);
                            }
                        }
                    }
                }
            }
        }

        match &mir_block.terminator {
            Some(MirTerminator::Return) => {
                let ret_val = vars.get(&0)
                    .map(|&v| bc.use_var(v))
                    .or_else(|| tuple_vars.get(&(0, 0)).map(|&v| bc.use_var(v)))
                    .unwrap_or_else(|| bc.ins().iconst(types::I64, 0));
                bc.ins().return_(&[ret_val]);
            }
            Some(MirTerminator::Goto(target)) => {
                if let Some(&tgt) = blocks.get(target) { bc.ins().jump(tgt, &[]); }
                else { return Err(format!("Goto target '{}' not found", target)); }
            }
            Some(MirTerminator::Assert { target, .. }) => {
                if let Some(&tgt) = blocks.get(target) { bc.ins().jump(tgt, &[]); }
                else { return Err(format!("Assert target '{}' not found", target)); }
            }
            Some(MirTerminator::Call { func, args, dst, target }) => {
                let arg_vals: Vec<Value> = args.iter()
                    .map(|a| resolve_mir_operand_to_value(a, &vars, &tuple_vars, &mut bc))
                    .collect();
                let cleaned = func.rsplit("::").next().unwrap_or(func).to_string();
                if let Some(&fid) = func_ids.get(&cleaned) {
                    let func_ref = module.declare_func_in_func(fid, bc.func);
                    let inst = bc.ins().call(func_ref, &arg_vals);
                    let res = bc.inst_results(inst).first().copied()
                        .unwrap_or_else(|| bc.ins().iconst(types::I64, 0));
                    if let Some(dst_idx) = parse_mir_local_idx(dst) {
                        if let Some(&var) = vars.get(&dst_idx) { bc.def_var(var, res); }
                    }
                }
                if let Some(&tgt) = blocks.get(target) { bc.ins().jump(tgt, &[]); }
                else { return Err(format!("Call target '{}' not found", target)); }
            }
            Some(MirTerminator::SwitchInt { discr, targets, otherwise }) => {
                let default_bb = *blocks.get(otherwise)
                    .ok_or_else(|| format!("SwitchInt otherwise '{}' not found", otherwise))?;
                if targets.len() == 1 && targets[0].0 == "0" {
                    // Boolean if/else: switchInt(bool) -> [0: else, otherwise: then]
                    let discr_val = resolve_mir_operand_to_value(discr, &vars, &tuple_vars, &mut bc);
                    let else_bb = *blocks.get(&targets[0].1)
                        .ok_or_else(|| format!("SwitchInt else '{}' not found", targets[0].1))?;
                    bc.ins().brif(discr_val, default_bb, &[], else_bb, &[]);
                } else if targets.is_empty() {
                    bc.ins().jump(default_bb, &[]);
                } else {
                    // n-way match: chained icmp + brif, fall through to default.
                    for (val_str, target_label) in targets {
                        let value: i64 = val_str.parse().unwrap_or(0);
                        let discr_v = resolve_mir_operand_to_value(discr, &vars, &tuple_vars, &mut bc);
                        let const_v = bc.ins().iconst(types::I64, value);
                        let eq = bc.ins().icmp(IntCC::Equal, discr_v, const_v);
                        let target_bb = *blocks.get(target_label)
                            .ok_or_else(|| format!("SwitchInt target '{}' not found", target_label))?;
                        let next_check = bc.create_block();
                        bc.ins().brif(eq, target_bb, &[], next_check, &[]);
                        bc.switch_to_block(next_check);
                    }
                    bc.ins().jump(default_bb, &[]);
                }
            }
            None => return Err(format!("block {} has no terminator", mir_block.label)),
        }
    }

    bc.seal_all_blocks();
    bc.finalize();
    Ok(func_ir)
}

/// Parse a tuple type string like "(i64, i64)" into its field types. Returns None for
/// non-tuple types or malformed input.
fn parse_tuple_type(ty: &str) -> Option<Vec<String>> {
    let t = ty.trim();
    if !t.starts_with('(') || !t.ends_with(')') { return None; }
    let inner = &t[1..t.len()-1];
    if inner.is_empty() { return None; }
    // Quick depth-balanced split on top-level commas (no nested tuples for now).
    Some(inner.split(',').map(|s| s.trim().to_string()).collect())
}

fn mir_type_to_cl(t: &str) -> Type {
    match t.trim().trim_start_matches("-> ") {
        "i64" | "u64" => types::I64,
        "i32" | "u32" => types::I32,
        "i16" | "u16" => types::I16,
        "i8"  | "u8"  => types::I8,
        "bool" => types::I8,
        "f64" => types::F64,
        "f32" => types::F32,
        s if s.starts_with('(') => types::I64,
        _ => types::I64,
    }
}

fn parse_mir_local_idx(s: &str) -> Option<usize> {
    let s = s.trim();
    if !s.starts_with('_') { return None; }
    let n: String = s.chars().skip(1).take_while(|c| c.is_ascii_digit()).collect();
    n.parse().ok()
}

fn resolve_mir_operand_to_value(
    s: &str,
    vars: &std::collections::HashMap<usize, Variable>,
    tuple_vars: &std::collections::HashMap<(usize, usize), Variable>,
    bc: &mut FunctionBuilder,
) -> Value {
    let c = s.trim().trim_start_matches("copy ").trim_start_matches("move ").trim();
    let c = c.trim_start_matches('(').trim_end_matches(')');
    // Strip a trailing type annotation `: i64` etc.
    let c = if let Some(colon) = c.rfind(':') { c[..colon].trim() } else { c };

    // Field projection: `_3.0`
    if c.starts_with('_') && c.chars().nth(1).map_or(false, |ch| ch.is_ascii_digit()) {
        if let Some(dot) = c.find('.') {
            let local_n: String = c[1..dot].chars().take_while(|ch| ch.is_ascii_digit()).collect();
            let field_n: String = c[dot+1..].chars().take_while(|ch| ch.is_ascii_digit()).collect();
            if let (Ok(loc), Ok(field)) = (local_n.parse::<usize>(), field_n.parse::<usize>()) {
                if let Some(&var) = tuple_vars.get(&(loc, field)) {
                    return bc.use_var(var);
                }
                // Fallback for overflow-result tuples that we model as single i64:
                if let Some(&var) = vars.get(&loc) {
                    return bc.use_var(var);
                }
            }
        }
        // Plain `_N`
        if let Some(idx) = parse_mir_local_idx(c) {
            if let Some(&var) = vars.get(&idx) {
                return bc.use_var(var);
            }
            if let Some(&var) = tuple_vars.get(&(idx, 0)) {
                return bc.use_var(var);
            }
        }
    }

    if let Some(rest) = c.strip_prefix("const ") {
        let head = rest.split('_').next().unwrap_or("")
            .trim_end_matches(|c: char| c.is_ascii_alphabetic());
        if let Ok(i) = head.parse::<i64>() {
            return bc.ins().iconst(types::I64, i);
        }
        if rest == "true" { return bc.ins().iconst(types::I8, 1); }
        if rest == "false" { return bc.ins().iconst(types::I8, 0); }
    }

    if let Ok(i) = c.parse::<i64>() {
        return bc.ins().iconst(types::I64, i);
    }

    bc.ins().iconst(types::I64, 0)
}

fn compile_mir_op_to_value(
    op: &str,
    args: &[String],
    vars: &std::collections::HashMap<usize, Variable>,
    tuple_vars: &std::collections::HashMap<(usize, usize), Variable>,
    bc: &mut FunctionBuilder,
) -> Value {
    let two_args = |bc: &mut FunctionBuilder| -> (Value, Value) {
        let a = args.first().map(|a| resolve_mir_operand_to_value(a, vars, tuple_vars, bc))
            .unwrap_or_else(|| bc.ins().iconst(types::I64, 0));
        let b = args.get(1).map(|a| resolve_mir_operand_to_value(a, vars, tuple_vars, bc))
            .unwrap_or_else(|| bc.ins().iconst(types::I64, 0));
        (a, b)
    };
    match op {
        "AddWithOverflow" | "Add" => { let (a, b) = two_args(bc); bc.ins().iadd(a, b) }
        "SubWithOverflow" | "Sub" => { let (a, b) = two_args(bc); bc.ins().isub(a, b) }
        "MulWithOverflow" | "Mul" => { let (a, b) = two_args(bc); bc.ins().imul(a, b) }
        "Div"                     => { let (a, b) = two_args(bc); bc.ins().udiv(a, b) }
        "Eq"     => { let (a, b) = two_args(bc); bc.ins().icmp(IntCC::Equal, a, b) }
        "Ne"     => { let (a, b) = two_args(bc); bc.ins().icmp(IntCC::NotEqual, a, b) }
        "Lt"     => { let (a, b) = two_args(bc); bc.ins().icmp(IntCC::SignedLessThan, a, b) }
        "Gt"     => { let (a, b) = two_args(bc); bc.ins().icmp(IntCC::SignedGreaterThan, a, b) }
        "Le"     => { let (a, b) = two_args(bc); bc.ins().icmp(IntCC::SignedLessThanOrEqual, a, b) }
        "Ge"     => { let (a, b) = two_args(bc); bc.ins().icmp(IntCC::SignedGreaterThanOrEqual, a, b) }
        "BitAnd" => { let (a, b) = two_args(bc); bc.ins().band(a, b) }
        "BitOr"  => { let (a, b) = two_args(bc); bc.ins().bor(a, b) }
        "copy" | "move" | "Use" | "const" => {
            args.first().map(|a| resolve_mir_operand_to_value(a, vars, tuple_vars, bc))
                .unwrap_or_else(|| bc.ins().iconst(types::I64, 0))
        }
        _ => bc.ins().iconst(types::I64, 0),
    }
}

/// Walks an Expr collecting (function_name → max arg count) for every Call encountered.
/// Used to pre-declare external symbols as Linkage::Import before compiling function bodies.
fn collect_call_arities(expr: &Expr, out: &mut std::collections::HashMap<String, usize>) {
    match expr {
        Expr::Call { func, args } => {
            let arity = args.len();
            out.entry(func.clone()).and_modify(|n| if arity > *n { *n = arity }).or_insert(arity);
            for a in args { collect_call_arities(a, out); }
        }
        Expr::BinaryOp { left, right, .. } => {
            collect_call_arities(left, out);
            collect_call_arities(right, out);
        }
        Expr::Return(v) => collect_call_arities(v, out),
        Expr::Let { value, .. } => collect_call_arities(value, out),
        Expr::Block(stmts) => for s in stmts { collect_call_arities(s, out); },
        Expr::If { cond, then_branch, else_branch } => {
            collect_call_arities(cond, out);
            collect_call_arities(then_branch, out);
            if let Some(e) = else_branch { collect_call_arities(e, out); }
        }
        _ => {}
    }
}

/// Module-aware compile_expr: handles Expr::Call by resolving names through `func_ids`
/// (which includes both unit-local functions and pre-declared imports).
fn compile_expr_with_calls(
    bc: &mut FunctionBuilder,
    module: &mut ObjectModule,
    expr: &Expr,
    params: &[flux_frontend::Param],
    param_vals: &[Value],
    func_ids: &std::collections::HashMap<String, cranelift_module::FuncId>,
) -> Value {
    match expr {
        Expr::Literal(lit) => match lit {
            Literal::Int(i) => bc.ins().iconst(types::I64, *i),
            Literal::Float(f) => bc.ins().f64const(*f),
            Literal::Bool(b) => bc.ins().iconst(types::I8, if *b {1} else {0}),
            Literal::Str(_) => bc.ins().iconst(types::I64, 0),
        },
        Expr::Variable(name) => {
            if let Some(i) = params.iter().position(|p| &p.name == name) {
                // Use the captured entry-block values — they're reachable from any block.
                param_vals[i]
            } else { bc.ins().iconst(types::I64, 0) }
        }
        Expr::BinaryOp { op, left, right } => {
            let l = compile_expr_with_calls(bc, module, left, params, param_vals, func_ids);
            let r = compile_expr_with_calls(bc, module, right, params, param_vals, func_ids);
            match op {
                BinOp::Add => bc.ins().iadd(l, r), BinOp::Sub => bc.ins().isub(l, r),
                BinOp::Mul => bc.ins().imul(l, r), BinOp::Div => bc.ins().udiv(l, r),
                BinOp::Eq => bc.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::Equal, l, r),
                BinOp::Neq => bc.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::NotEqual, l, r),
                BinOp::Lt => bc.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedLessThan, l, r),
                BinOp::Gt => bc.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedGreaterThan, l, r),
                BinOp::Le => bc.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedLessThanOrEqual, l, r),
                BinOp::Ge => bc.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedGreaterThanOrEqual, l, r),
                BinOp::And => bc.ins().band(l, r), BinOp::Or => bc.ins().bor(l, r),
            }
        }
        Expr::Call { func, args } => {
            let arg_vals: Vec<Value> = args.iter()
                .map(|a| compile_expr_with_calls(bc, module, a, params, param_vals, func_ids))
                .collect();
            if let Some(&func_id) = func_ids.get(func) {
                let func_ref = module.declare_func_in_func(func_id, bc.func);
                let inst = bc.ins().call(func_ref, &arg_vals);
                bc.inst_results(inst).get(0).copied()
                    .unwrap_or_else(|| bc.ins().iconst(types::I64, 0))
            } else {
                bc.ins().iconst(types::I64, 0)
            }
        }
        Expr::If { cond, then_branch, else_branch } => {
            let cond_val = compile_expr_with_calls(bc, module, cond, params, param_vals, func_ids);
            let then_bb = bc.create_block();
            let else_bb = bc.create_block();
            let merge_bb = bc.create_block();
            bc.append_block_param(merge_bb, types::I64);

            bc.ins().brif(cond_val, then_bb, &[], else_bb, &[]);

            bc.switch_to_block(then_bb);
            let then_val = compile_expr_with_calls(bc, module, then_branch, params, param_vals, func_ids);
            bc.ins().jump(merge_bb, &[then_val]);
            bc.seal_block(then_bb);

            bc.switch_to_block(else_bb);
            let else_val = if let Some(eb) = else_branch {
                compile_expr_with_calls(bc, module, eb, params, param_vals, func_ids)
            } else {
                bc.ins().iconst(types::I64, 0)
            };
            bc.ins().jump(merge_bb, &[else_val]);
            bc.seal_block(else_bb);

            bc.switch_to_block(merge_bb);
            bc.seal_block(merge_bb);
            bc.block_params(merge_bb)[0]
        }
        Expr::Return(v) => compile_expr_with_calls(bc, module, v, params, param_vals, func_ids),
        Expr::Block(stmts) => {
            let mut last = bc.ins().iconst(types::I64, 0);
            for s in stmts { last = compile_expr_with_calls(bc, module, s, params, param_vals, func_ids); }
            last
        }
        _ => bc.ins().iconst(types::I64, 0),
    }
}

pub fn compile_to_clif(func: &FunctionDef) -> Result<String, String> {
    let mut flag_builder = settings::builder();
    flag_builder.set("use_colocated_libcalls", "false").unwrap();
    flag_builder.set("is_pic", "false").unwrap();
    let flags = settings::Flags::new(flag_builder);

    let mut sig = Signature::new(CallConv::SystemV);
    for p in &func.params { sig.params.push(AbiParam::new(cl_type(&p.ty))); }
    sig.returns.push(AbiParam::new(cl_type(&func.return_type)));

    let mut func_ir = Function::with_name_signature(UserFuncName::user(0, 0), sig);
    let mut ctx = FunctionBuilderContext::new();
    let mut bc = FunctionBuilder::new(&mut func_ir, &mut ctx);
    let entry = bc.create_block();
    bc.switch_to_block(entry);
    bc.append_block_params_for_function_params(entry);
    let val = compile_expr(&mut bc, &func.body, &func.params);
    bc.ins().return_(&[val]);
    bc.seal_all_blocks();
    bc.finalize();
    Ok(func_ir.display().to_string())
}

fn compile_expr(bc: &mut FunctionBuilder, expr: &Expr, params: &[flux_frontend::Param]) -> Value {
    match expr {
        Expr::Literal(lit) => match lit {
            Literal::Int(i) => bc.ins().iconst(types::I64, *i),
            Literal::Float(f) => bc.ins().f64const(*f),
            Literal::Bool(b) => bc.ins().iconst(types::I8, if *b {1} else {0}),
            Literal::Str(_) => bc.ins().iconst(types::I64, 0),
        },
        Expr::Variable(name) => {
            if let Some(i) = params.iter().position(|p| &p.name == name) {
                bc.block_params(bc.current_block().unwrap())[i]
            } else { bc.ins().iconst(types::I64, 0) }
        }
        Expr::BinaryOp { op, left, right } => {
            let l = compile_expr(bc, left, params);
            let r = compile_expr(bc, right, params);
            match op {
                BinOp::Add => bc.ins().iadd(l, r), BinOp::Sub => bc.ins().isub(l, r),
                BinOp::Mul => bc.ins().imul(l, r), BinOp::Div => bc.ins().udiv(l, r),
                BinOp::Eq => bc.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::Equal, l, r),
                BinOp::Neq => bc.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::NotEqual, l, r),
                BinOp::Lt => bc.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedLessThan, l, r),
                BinOp::Gt => bc.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedGreaterThan, l, r),
                BinOp::Le => bc.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedLessThanOrEqual, l, r),
                BinOp::Ge => bc.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedGreaterThanOrEqual, l, r),
                BinOp::And => bc.ins().band(l, r), BinOp::Or => bc.ins().bor(l, r),
            }
        }
        Expr::Return(v) => compile_expr(bc, v, params),
        Expr::Block(stmts) => { let mut last = bc.ins().iconst(types::I64, 0); for s in stmts { last = compile_expr(bc, s, params); } last }
        _ => bc.ins().iconst(types::I64, 0),
    }
}

fn cl_type(ty: &TypeRef) -> Type {
    match ty { TypeRef::I32 => types::I32, TypeRef::I64|TypeRef::U64 => types::I64, TypeRef::U32 => types::I32, TypeRef::Bool => types::I8, TypeRef::F32 => types::F32, TypeRef::F64 => types::F64, _ => types::I64 }
}

#[cfg(test)] mod tests { use super::*;
    #[test] fn test_add() { let u=flux_frontend::parse_source("fn add(a: i64, b: i64) -> i64 { return a + b }","t.rs").unwrap(); assert!(compile_to_clif(&u.functions[0]).unwrap().contains("function")); }
    #[test] fn test_mul() { let u=flux_frontend::parse_source("fn mul(a: i64, b: i64) -> i64 { return a * b }","t.rs").unwrap(); assert!(compile_to_clif(&u.functions[0]).is_ok()); }
}
