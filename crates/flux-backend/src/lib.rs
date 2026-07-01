// flux-backend — Phase 3d: Cranelift CLIF generation
// Takes flux-frontend IR, generates Cranelift IR text.
// Runs on any architecture. Native codegen via clif-util or future JIT.

use flux_frontend::{FunctionDef, TypeRef, Expr, Literal, BinOp, UnOp, TranslationUnit};
use flux_frontend::mir::{MirFunction, MirStmt, MirTerminator};
use cranelift_codegen::ir::{types, Type, AbiParam, Block, UserFuncName, Function, Signature, InstBuilder, Value};
use cranelift_codegen::ir::condcodes::{IntCC, FloatCC};
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
    // Required for 128-bit integers (i128/u128) in the calling convention — without it the x64 ABI
    // lowering panics when an I128 is a function param/return (ladder rung: 128-bit ints for SIGIL money math).
    flag_builder.set("enable_llvm_abi_extensions", "true").map_err(|e| e.to_string())?;
    let isa = isa_builder.finish(settings::Flags::new(flag_builder)).map_err(|e| format!("isa finish: {}", e))?;

    let builder = ObjectBuilder::new(isa, "flux_module", default_libcall_names())
        .map_err(|e| format!("ObjectBuilder: {}", e))?;
    let mut module = ObjectModule::new(builder);

    // Struct layout table (name -> field type-strings), from the source's struct defs. Lets
    // a named-struct local be scalar-replaced into per-field Variables, like a tuple.
    let mut struct_layout: HashMap<String, Vec<String>> = unit.structs.iter()
        .map(|s| (s.name.clone(), s.fields.iter().map(|f| typeref_str(&f.ty)).collect()))
        .collect();

    // Enum variant table ("EnumName::Variant" -> i64 discriminant), from the source's enum defs.
    // A C-like enum IS its discriminant: construction `_1 = Color::Green` becomes iconst(discriminant),
    // discriminant(_1) is a copy, and the enum type lowers to i64 (mir_type_to_cl's default).
    let enum_variants: HashMap<String, i64> = unit.enums.iter()
        .flat_map(|e| e.variants.iter().map(move |v| (format!("{}::{}", e.name, v.name), v.discriminant)))
        .collect();

    // Data-carrying enum variant field map: variant name → list of field type strings.
    // Used by the downcast-expansion path to compute correct sub-aggregate offsets.
    let mut variant_fields: HashMap<String, Vec<String>> = HashMap::new();
    for e in &unit.enums {
        for v in &e.variants {
            let fts: Vec<String> = v.fields.iter().map(typeref_str).collect();
            variant_fields.insert(v.name.clone(), fts);
        }
    }

    // Data-carrying enum layout (FIP-0001 ladder rung 4, part 2): a tagged union laid out as
    // [i64 tag, payload0, payload1, …], payload-slot count = the widest variant's arity. Only enums
    // with at least one payload-carrying variant get an entry; a C-like (all-fieldless) enum stays a
    // single i64 (its discriminant), unchanged. Merged INTO struct_layout so aggregate_fields scalar-
    // replaces an enum local into per-field Variables exactly like a struct/tuple — no new machinery.
    for e in &unit.enums {
        let widest = e.variants.iter().max_by_key(|v| v.fields.len());
        let max_arity = widest.map(|v| v.fields.len()).unwrap_or(0);
        if max_arity == 0 { continue; }
        let mut fields = Vec::with_capacity(max_arity + 1);
        fields.push("i64".to_string()); // field 0 = discriminant tag
        for i in 0..max_arity {
            fields.push(widest.and_then(|v| v.fields.get(i)).map(typeref_str).unwrap_or_else(|| "i64".to_string()));
        }
        struct_layout.insert(e.name.clone(), fields);
    }

    // Pass 1: declare every function so Call expressions can resolve by name.
    let mut func_ids: HashMap<String, FuncId> = HashMap::with_capacity(unit.functions.len());
    let mut func_sigs: HashMap<String, Signature> = HashMap::with_capacity(unit.functions.len());
    for func in &unit.functions {
        // Skip rustc's synthesized variant constructor `fn Enum::Variant(..)` — we lower construction
        // inline (`_d = Enum::Variant(args)`), and the `::` name would emit a bogus object symbol.
        if enum_variants.contains_key(&func.name) { continue; }
        let mut sig = module.make_signature();
        // Params: an aggregate (struct / tuple / data-carrying enum) passed BY VALUE becomes one
        // AbiParam PER scalar-replaced field — Flux's flattened ABI, symmetric with the call site and
        // the multi-value return. Real per-param type strings come from the MIR override (a TypeRef
        // can't spell "Opt" / "(i64,i64)"); the TypeRef fallback is for Expr-path funcs, which never
        // carry aggregate params (build_mir_overrides routes those to the MIR-direct path).
        match mir_overrides.get(&func.name) {
            Some(m) => for mp in &m.params {
                flatten_params(&mp.ty, &struct_layout, &mut sig.params);
            },
            None => for p in &func.params { sig.params.push(AbiParam::new(cl_type(&p.ty))); },
        }
        // Aggregate (tuple) by-value return: one AbiParam per field, taken from the MIR
        // override's type string (TypeRef can't express tuples). Else a single scalar return.
        match mir_overrides.get(&func.name).and_then(|m| aggregate_fields(&m.return_type, &struct_layout)) {
            Some(fields) => for f in &fields { sig.returns.push(AbiParam::new(mir_type_to_cl(f))); },
            // Prefer the MIR return-type STRING (knows "u128" -> I128); TypeRef collapses u128 -> Named -> I64.
            None => {
                let rt = mir_overrides.get(&func.name)
                    .map(|m| mir_type_to_cl(&m.return_type))
                    .unwrap_or_else(|| cl_type(&func.return_type));
                sig.returns.push(AbiParam::new(rt));
            }
        }
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
        if enum_variants.contains_key(&func.name) { continue; }
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
        if enum_variants.contains_key(&func.name) { continue; }
        let func_id = func_ids[&func.name];
        let sig = func_sigs.remove(&func.name).unwrap();

        let func_ir = if let Some(mir) = mir_overrides.get(&func.name) {
            compile_mir_into_function(mir, func_id, sig, &mut module, &func_ids, &struct_layout, &enum_variants, &variant_fields)?
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
    let rt = cl_type(&func.return_type);
    let val = coerce_int_width(&mut bc, val, rt);
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
    structs: &std::collections::HashMap<String, Vec<String>>,
    enum_variants: &std::collections::HashMap<String, i64>,
    variant_fields: &std::collections::HashMap<String, Vec<String>>,
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
        // Recursively flatten: each scalar leaf gets its own Variable, indexed by
        // a flat field number. E.g. Shape[i64, Point, Point] becomes 5 flat fields.
        let mut flat_fields: Vec<String> = Vec::new();
        collect_flat_fields(&ty_str, structs, &mut flat_fields);
        if flat_fields.len() > 1 || (flat_fields.len() == 1 && flat_fields[0] != ty_str) {
            for (fi, field_ty) in flat_fields.iter().enumerate() {
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
    // Bind incoming block-params to param Variables with a running cursor: an aggregate param consumes
    // one block-param per field (matching the flattened sig above), a scalar param consumes one.
    let mut pcursor = 0usize;
    for p in &mir.params {
        let ty = mir_type_strs.get(&p.index).cloned().unwrap_or_default();
        bind_param_flat(p.index, &ty, structs, &vars, &tuple_vars, &entry_params, &mut pcursor, &mut bc);
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
                    // Downcast expansion: `_N|Variant|K` — extract sub-aggregate at index K
                    // of variant V from the tagged-union local _N. The backend computes the
                    // correct field offset from the enum layout, handling nested aggregates.
                    if (op == "copy" || op == "move") && args.len() == 1 {
                        if let Some((src_local, variant, sub_idx)) = parse_downcast_operand(&args[0]) {
                            if let Some(vf) = variant_fields.get(&variant) {
                                if sub_idx < vf.len() {
                                    // Compute starting offset: 1 (skip tag) + sum of sizes
                                    // of sub-aggregates before index sub_idx.
                                    let mut offset: usize = 1;
                                    for i in 0..sub_idx {
                                        offset += aggregate_fields(&vf[i], structs)
                                            .map(|f| f.len()).unwrap_or(1);
                                    }

                                    let sub_type = &vf[sub_idx];
                                    let sub_fields = aggregate_fields(sub_type, structs)
                                        .unwrap_or_else(|| vec![sub_type.clone()]);
                                    // Copy each sub-field from _src.(offset+i) to _dst.(i)
                                    for (i, _) in sub_fields.iter().enumerate() {
                                        let src_field = format!("_{}.{}", src_local, offset + i);
                                        let val = resolve_mir_operand_to_value(
                                            &src_field, &vars, &tuple_vars, &mut bc);
                                        if let Some(&var) = tuple_vars.get(&(dst_idx, i)) {
                                            bc.def_var(var, val);
                                        } else if i == 0 {
                                            if let Some(&var) = vars.get(&dst_idx) {
                                                let want = mir_type_to_cl(
                                                    &mir_type_strs.get(&dst_idx).cloned()
                                                        .unwrap_or_default());
                                                let val = coerce_int_width(&mut bc, val, want);
                                                bc.def_var(var, val);
                                            }
                                        }
                                    }
                                    continue;
                                }
                            }
                        }
                    }
                    // Enum construction: parse_rhs emits op="EnumName::Variant" with the payload
                    // operands as args (C-like variants carry no args). A concrete-instantiated generic
                    // enum renders the turbofish in the path (`MyOpt::<i64>::Some`); normalize it away so
                    // it matches the bare variant key.
                    let op_norm = strip_turbofish(op);
                    if let Some(&disc) = enum_variants.get(op.as_str()).or_else(|| enum_variants.get(op_norm.as_str())) {
                        // Data-carrying enum: dst is an aggregate [tag, payload…]. Write disc into the
                        // tag field (0) and each construction arg into payload field (1+i); zero-fill any
                        // payload the variant doesn't supply (a unit variant of a data-carrying enum, so
                        // all the local's fields are defined on every path before the switchInt merge).
                        if tuple_vars.contains_key(&(dst_idx, 0)) {
                            let dst_ty = mir_type_strs.get(&dst_idx).cloned().unwrap_or_default();
                            let fields = aggregate_fields(&dst_ty, structs).unwrap_or_default();
                            let tag = bc.ins().iconst(types::I64, disc);
                            if let Some(&v0) = tuple_vars.get(&(dst_idx, 0)) { bc.def_var(v0, tag); }
                            for fi in 1..fields.len() {
                                let want = mir_type_to_cl(&fields[fi]);
                                let val = if let Some(arg) = args.get(fi - 1) {
                                    let raw = resolve_mir_operand_to_value(arg, &vars, &tuple_vars, &mut bc);
                                    if matches!(want, types::F64 | types::F32) { raw }
                                    else { coerce_int_width(&mut bc, raw, want) }
                                } else if want == types::F64 { bc.ins().f64const(0.0) }
                                  else if want == types::F32 { bc.ins().f32const(0.0f32) }
                                  else { bc.ins().iconst(want, 0) };
                                if let Some(&v) = tuple_vars.get(&(dst_idx, fi)) { bc.def_var(v, val); }
                            }
                            continue;
                        }
                        // C-like enum (single i64 var): the value IS the discriminant.
                        let val = bc.ins().iconst(types::I64, disc);
                        if let Some(&var) = vars.get(&dst_idx) { bc.def_var(var, val); }
                        continue;
                    }
                    let unsigned = operand_is_unsigned(args.first().map(|s| s.as_str()).unwrap_or(""), &mir_type_strs);
                    let val = compile_mir_op_to_value(op, args, &vars, &tuple_vars, unsigned, module, &mut bc);

                    if let Some(&var) = vars.get(&dst_idx) {
                        let want = mir_type_to_cl(&mir_type_strs.get(&dst_idx).cloned().unwrap_or_default());
                        let val = coerce_int_width(&mut bc, val, want);
                        bc.def_var(var, val);
                    } else if let Some(fields) = aggregate_fields(&mir_type_strs.get(&dst_idx).cloned().unwrap_or_default(), structs) {
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
                if let Some(fields) = aggregate_fields(&mir.return_type, structs) {
                    // Aggregate-by-value return (tuple OR named struct): return every
                    // scalar-replaced field of _0.
                    let vals: Vec<Value> = (0..fields.len())
                        .map(|i| tuple_vars.get(&(0, i)).map(|&v| bc.use_var(v))
                            .unwrap_or_else(|| bc.ins().iconst(mir_type_to_cl(&fields[i]), 0)))
                        .collect();
                    bc.ins().return_(&vals);
                } else {
                    let ret_val = vars.get(&0)
                        .map(|&v| bc.use_var(v))
                        .or_else(|| tuple_vars.get(&(0, 0)).map(|&v| bc.use_var(v)))
                        .unwrap_or_else(|| bc.ins().iconst(types::I64, 0));
                    let rt = mir_type_to_cl(&mir.return_type);
                    let ret_val = coerce_int_width(&mut bc, ret_val, rt);
                    bc.ins().return_(&[ret_val]);
                }
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
                // Aggregate-by-value arg: expand a bare aggregate local (`move _3` where _3: Opt) into
                // its scalar-replaced fields, matching the callee's flattened param ABI. Scalars,
                // projections (`(_4.0: i64)`), and consts resolve to a single value as before.
                let mut arg_vals: Vec<Value> = Vec::with_capacity(args.len());
                for a in args {
                    if let Some(idx) = bare_local(a) {
                        if let Some(fields) = aggregate_fields(&mir_type_strs.get(&idx).cloned().unwrap_or_default(), structs) {
                            for fi in 0..fields.len() {
                                let v = tuple_vars.get(&(idx, fi)).map(|&v| bc.use_var(v))
                                    .unwrap_or_else(|| bc.ins().iconst(types::I64, 0));
                                arg_vals.push(v);
                            }
                            continue;
                        }
                    }
                    arg_vals.push(resolve_mir_operand_to_value(a, &vars, &tuple_vars, &mut bc));
                }
                let cleaned = func.rsplit("::").next().unwrap_or(func).to_string();
                if let Some(&fid) = func_ids.get(&cleaned) {
                    let func_ref = module.declare_func_in_func(fid, bc.func);
                    let inst = bc.ins().call(func_ref, &arg_vals);
                    let results = bc.inst_results(inst).to_vec();
                    if let Some(dst_idx) = parse_mir_local_idx(dst) {
                        if results.len() > 1 {
                            // Aggregate (multi-value) return: distribute each result into the
                            // dst's scalar-replaced fields.
                            for (i, &r) in results.iter().enumerate() {
                                if let Some(&var) = tuple_vars.get(&(dst_idx, i)) { bc.def_var(var, r); }
                            }
                        } else {
                            let res = results.first().copied().unwrap_or_else(|| bc.ins().iconst(types::I64, 0));
                            if let Some(&var) = vars.get(&dst_idx) { bc.def_var(var, res); }
                            else if let Some(&var) = tuple_vars.get(&(dst_idx, 0)) { bc.def_var(var, res); }
                        }
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
                        // Match the comparison constant's int width to the discriminant
                        // (i8/i16/i32/i64) so the icmp is type-consistent for non-i64 scrutinees.
                        let dty = bc.func.dfg.value_type(discr_v);
                        let const_v = bc.ins().iconst(dty, value);
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
            Some(MirTerminator::Unreachable) => {
                // rustc's exhaustive-match `otherwise` arm. Genuinely unreachable, but the block still
                // needs a valid terminator. Return a zero of the function's return shape — never
                // observed at runtime, and version-agnostic (no dependency on Cranelift's TrapCode API).
                if let Some(fields) = aggregate_fields(&mir.return_type, structs) {
                    let vals: Vec<Value> = fields.iter().map(|f| {
                        let ty = mir_type_to_cl(f);
                        if ty == types::F64 { bc.ins().f64const(0.0) }
                        else if ty == types::F32 { bc.ins().f32const(0.0f32) }
                        else { bc.ins().iconst(ty, 0) }
                    }).collect();
                    bc.ins().return_(&vals);
                } else {
                    let ty = mir_type_to_cl(&mir.return_type);
                    let z = if ty == types::F64 { bc.ins().f64const(0.0) }
                        else if ty == types::F32 { bc.ins().f32const(0.0f32) }
                        else { bc.ins().iconst(ty, 0) };
                    bc.ins().return_(&[z]);
                }
            }
            None => return Err(format!("block {} has no terminator", mir_block.label)),
        }
    }

    bc.seal_all_blocks();
    bc.finalize();
    Ok(func_ir)
}

/// Recursively bind flat block params to scalar-replaced tuple_vars.
fn bind_param_flat(
    local_idx: usize, ty: &str,
    structs: &std::collections::HashMap<String, Vec<String>>,
    vars: &std::collections::HashMap<usize, Variable>,
    tuple_vars: &std::collections::HashMap<(usize, usize), Variable>,
    entry_params: &[Value],
    pcursor: &mut usize,
    bc: &mut FunctionBuilder,
) {
    let mut stack: Vec<(usize, Vec<usize>)> = Vec::new();
    if let Some(fields) = aggregate_fields(ty, structs) {
        for fi in (0..fields.len()).rev() { stack.push((local_idx, vec![fi])); }
    } else {
        if let (Some(&var), Some(&val)) = (vars.get(&local_idx), entry_params.get(*pcursor)) {
            bc.def_var(var, val);
        }
        *pcursor += 1;
        return;
    }
    // `tuple_vars` is keyed by (local, per-local FLAT-LEAF index) — the leaf index restarts at 0 for
    // each local (see collect_flat_fields + the declaration loop). `pcursor`, by contrast, runs GLOBALLY
    // across ALL params (one slot per incoming block-param). For the FIRST aggregate param the two
    // coincide, but for any LATER aggregate param pcursor is already > 0, so keying tuple_vars by pcursor
    // misses (e.g. looks up (2,2) when the var is (2,0)) and that field silently never binds. Track a
    // per-local leaf counter for the tuple_vars key; keep pcursor for the global entry_params slot.
    let mut leaf = 0usize;
    while let Some((base, path)) = stack.pop() {
        // Walk the path to get the current field type and check if it has sub-aggregates.
        let current_ty = walk_to_field(ty, &path, structs);
        if let Some(sub_fields) = aggregate_fields(&current_ty, structs) {
            // Still an aggregate — push children for depth-first walk
            for fi in (0..sub_fields.len()).rev() {
                let mut sub = path.clone();
                sub.push(fi);
                stack.push((base, sub));
            }
        } else {
            // Scalar leaf reached — bind to next block param
            if let (Some(&var), Some(&val)) = (tuple_vars.get(&(base, leaf)), entry_params.get(*pcursor)) {
                bc.def_var(var, val);
            }
            leaf += 1;
            *pcursor += 1;
        }
    }
}

/// Walk a field-path through nested aggregate types and return the type name
/// at the end of the path. Returns the aggregate type name if the path ends
/// inside an aggregate (so callers can check for further sub-fields).
fn walk_to_field(root_ty: &str, path: &[usize], structs: &std::collections::HashMap<String, Vec<String>>) -> String {
    let mut current_ty = root_ty.to_string();
    for &fi in path {
        if let Some(fields) = aggregate_fields(&current_ty, structs) {
            if fi < fields.len() {
                current_ty = fields[fi].clone();
            } else {
                return current_ty; // path out of bounds — return what we have
            }
        } else {
            return current_ty; // can't descend further
        }
    }
    current_ty
}

/// Recursively collect all scalar field types from an aggregate into a flat list.
fn collect_flat_fields(ty: &str, structs: &std::collections::HashMap<String, Vec<String>>, out: &mut Vec<String>) {
    if let Some(fields) = parse_tuple_type(ty).or_else(|| structs.get(ty).cloned()) {
        for f in &fields { collect_flat_fields(f, structs, out); }
    } else {
        out.push(ty.to_string());
    }
}

/// Recursively flatten an aggregate type into its scalar AbiParams, pushing onto `out`.
fn flatten_params(ty: &str, structs: &std::collections::HashMap<String, Vec<String>>, out: &mut Vec<AbiParam>) {
    if let Some(fields) = parse_tuple_type(ty).or_else(|| structs.get(ty).cloned()) {
        for f in &fields { flatten_params(f, structs, out); }
    } else if ty.starts_with('(') {
        // Unparseable tuple, fall back to I64
        out.push(AbiParam::new(types::I64));
    } else {
        out.push(AbiParam::new(mir_type_to_cl(ty)));
    }
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

/// Field type-strings of an aggregate local: a tuple `(i64,i64)` via parse_tuple_type, or a named
/// struct resolved through the layout table built from the source's struct definitions. Structs are
/// "named tuples" — same scalar-replacement, just resolved by name.
fn aggregate_fields(ty: &str, structs: &std::collections::HashMap<String, Vec<String>>) -> Option<Vec<String>> {
    let t = ty.trim();
    parse_tuple_type(t)
        .or_else(|| structs.get(t).cloned())
        // Generic instantiation (ladder rung 5, part 1): `MyOpt<i64>` / `Point<i64>` share the layout of
        // their base type. rustc renders the concrete payload type at each USE site (the def's type param
        // defaults to i64 in the layout table), so the base layout is correct for scalar instantiations.
        // A non-scalar or non-i64-width type param is a later slice (needs real substitution + coercion).
        .or_else(|| structs.get(t.split('<').next().unwrap_or(t).trim()).cloned())
}

/// TypeRef -> the type string mir_type_to_cl understands (for the struct layout table).
fn typeref_str(t: &TypeRef) -> String {
    match t {
        TypeRef::I32 => "i32".to_string(), TypeRef::I64 => "i64".to_string(),
        TypeRef::U32 => "u32".to_string(), TypeRef::U64 => "u64".to_string(),
        TypeRef::Bool => "bool".to_string(), TypeRef::F32 => "f32".to_string(),
        TypeRef::F64 => "f64".to_string(),
        TypeRef::Named(s) => s.clone(),
        _ => "i64".to_string(),
    }
}

fn mir_type_to_cl(t: &str) -> Type {
    match t.trim().trim_start_matches("-> ") {
        "i128" | "u128" => types::I128,
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

/// Parse a downcast operand `_N|Variant|K` into its components.
/// Returns (local_index, variant_name, sub_aggregate_index).
fn parse_downcast_operand(s: &str) -> Option<(usize, String, usize)> {
    let s = s.trim().trim_start_matches("copy ").trim_start_matches("move ");
    if !s.starts_with('_') { return None; }
    let rest = &s[1..];
    let local_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let local: usize = local_str.parse().ok()?;
    let after_local = &rest[local_str.len()..];
    if !after_local.starts_with('|') { return None; }
    let parts: Vec<&str> = after_local[1..].split('|').collect();
    if parts.len() != 2 { return None; }
    let variant = parts[0].to_string();
    let idx: usize = parts[1].parse().ok()?;
    Some((local, variant, idx))
}

/// The bare local index of an operand like `move _3` / `copy _3` / `_3`. Used to spot an
/// aggregate-by-value call argument that must be expanded into its fields. Returns None for a
/// projection (`(_4.0: i64)`), a const, or anything that isn't a plain `_N` — those resolve to a
/// single value the normal way.
fn bare_local(s: &str) -> Option<usize> {
    let c = s.trim().trim_start_matches("copy ").trim_start_matches("move ").trim();
    if !c.starts_with('_') || c.contains('.') || c.contains('(') || c.contains(':') { return None; }
    c[1..].parse::<usize>().ok()
}

/// Strip turbofish segments from a `::`-path (ladder rung 5, part 1). A generic enum's variant
/// construction is rendered with the instantiation in the path — `MyOpt::<i64>::Some` — but the enum
/// variant table is keyed on the bare `MyOpt::Some`. Dropping any `<…>` segment normalizes the two so
/// a concrete-instantiated generic enum constructs through the same path as a non-generic one.
fn strip_turbofish(name: &str) -> String {
    name.split("::").filter(|seg| !seg.starts_with('<')).collect::<Vec<_>>().join("::")
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
        let r = rest.trim();
        if r == "true"  { return bc.ins().iconst(types::I8, 1); }
        if r == "false" { return bc.ins().iconst(types::I8, 0); }
        // Floats: rustc renders them as `0f64` / `1.5f64` / `2.5e3f32` — the f-suffix is
        // attached DIRECTLY (no underscore, unlike ints `3_i64`). Strip the suffix, then
        // parse, or the literal falls back to iconst(I64) and mismatches its F64 Variable
        // (a `def_var` type-mismatch panic / verifier reject).
        if let Some(num) = r.strip_suffix("f64").or_else(|| r.strip_suffix("f32")) {
            if let Ok(f) = num.trim_end_matches('_').parse::<f64>() {
                return bc.ins().f64const(f);
            }
        }
        // Integers: `3_i64`, `-5_i32`, `500000000_u128`, or bare `42`.
        let head = r.split('_').next().unwrap_or("")
            .trim_end_matches(|c: char| c.is_ascii_alphabetic());
        // 128-bit literal: by `_u128`/`_i128` suffix, OR magnitude beyond i64. iconst can't hold it
        // (64-bit immediate) — assemble via iconcat. MUST be tried before the i64 parse.
        if r.contains("u128") || r.contains("i128") {
            if let Ok(v) = head.parse::<i128>() { return build_i128_const(v, bc); }
        }
        if let Ok(i) = head.parse::<i64>() {
            return bc.ins().iconst(types::I64, i);
        }
        if let Ok(v) = head.parse::<i128>() {
            return build_i128_const(v, bc); // > i64 range, unsuffixed
        }
    }

    if let Ok(i) = c.parse::<i64>() {
        return bc.ins().iconst(types::I64, i);
    }
    if let Ok(v) = c.parse::<i128>() {
        return build_i128_const(v, bc);
    }

    bc.ins().iconst(types::I64, 0)
}

/// True if a Cranelift Value carries a float type. Used to dispatch f-ops vs i-ops so the
/// float typing already present in cl_type / mir_type_to_cl is actually honored in codegen.
fn val_is_float(bc: &mut FunctionBuilder, v: Value) -> bool {
    matches!(bc.func.dfg.value_type(v), types::F64 | types::F32)
}

/// Coerce an integer Value to a target int width via sextend/ireduce. No-op for equal width
/// or non-int operands. Signed extension by default; unsigned widening is refined later.
fn coerce_int_width(bc: &mut FunctionBuilder, v: Value, want: Type) -> Value {
    let have = bc.func.dfg.value_type(v);
    if have == want || !have.is_int() || !want.is_int() { return v; }
    if want.bits() > have.bits() { bc.ins().sextend(want, v) } else { bc.ins().ireduce(want, v) }
}

/// Bring two int operands to a common width (the wider) so iadd/icmp/... typecheck. Leaves
/// floats / mixed operands untouched (the float dispatch handles those).
fn unify_int_width(bc: &mut FunctionBuilder, a: Value, b: Value) -> (Value, Value) {
    let (ta, tb) = (bc.func.dfg.value_type(a), bc.func.dfg.value_type(b));
    if ta == tb || !ta.is_int() || !tb.is_int() { return (a, b); }
    let want = if ta.bits() >= tb.bits() { ta } else { tb };
    (coerce_int_width(bc, a, want), coerce_int_width(bc, b, want))
}

/// Whether a MIR operand is an unsigned integer — from a `const N_u32` suffix or, for a
/// `_N` local, its declared type. Drives the udiv/urem/ushr/unsigned-compare choice (the
/// Cranelift Value type carries width but not sign, so we recover sign from the MIR type).
fn operand_is_unsigned(s: &str, types: &std::collections::HashMap<usize, String>) -> bool {
    let c = s.trim().trim_start_matches("copy ").trim_start_matches("move ").trim();
    if let Some(rest) = c.strip_prefix("const ") {
        return rest.contains("u8") || rest.contains("u16") || rest.contains("u32")
            || rest.contains("u64") || rest.contains("u128") || rest.contains("usize");
    }
    if let Some(idx) = parse_mir_local_idx(c) {
        if let Some(t) = types.get(&idx) {
            return matches!(t.trim(), "u8" | "u16" | "u32" | "u64" | "u128" | "usize");
        }
    }
    false
}

/// Expr-path signedness: an operand is unsigned if it's a u32/u64-typed parameter (the only
/// unsigned widths the IR TypeRef distinguishes). Recurses left for nested binary ops.
fn expr_is_unsigned(e: &Expr, params: &[flux_frontend::Param]) -> bool {
    match e {
        Expr::Variable(name) => params.iter().find(|p| &p.name == name)
            .map_or(false, |p| matches!(p.ty, TypeRef::U32 | TypeRef::U64)),
        Expr::BinaryOp { left, .. } => expr_is_unsigned(left, params),
        _ => false,
    }
}

/// Expr-path bool detection for `!` lowering: a bool gets logical-not (flip the low bit), an
/// integer gets bitwise-not (bnot). Comparison results, bool params, and bool literals are bool.
fn expr_is_bool(e: &Expr, params: &[flux_frontend::Param]) -> bool {
    match e {
        Expr::Variable(name) => params.iter().find(|p| &p.name == name)
            .map_or(false, |p| matches!(p.ty, TypeRef::Bool)),
        Expr::Literal(Literal::Bool(_)) => true,
        Expr::BinaryOp { op, .. } => matches!(op, BinOp::Eq | BinOp::Neq | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge),
        _ => false,
    }
}

/// Numeric cast (`as`): int<->int (width coerce), int<->float (fcvt), f32<->f64
/// (fpromote/fdemote). Signed conversions by default, matching this milestone's signed default.
fn cast_value(bc: &mut FunctionBuilder, v: Value, want: Type) -> Value {
    let have = bc.func.dfg.value_type(v);
    if have == want { return v; }
    match (have.is_int(), want.is_int()) {
        (true, true)   => coerce_int_width(bc, v, want),
        (true, false)  => bc.ins().fcvt_from_sint(want, v),
        (false, true)  => bc.ins().fcvt_to_sint_sat(want, v),
        (false, false) => if want.bits() > have.bits() { bc.ins().fpromote(want, v) } else { bc.ins().fdemote(want, v) },
    }
}

/// Cranelift's x64 backend (0.114) cannot lower i128 div/rem inline (it panics in machinst/lower) — it
/// expects the producer to emit the compiler-rt libcall. Declare + call __udivti3/__divti3/__umodti3/
/// __modti3 (two i128 args -> i128); they resolve against compiler_builtins/compiler-rt at link time.
fn emit_i128_libcall(name: &str, a: Value, b: Value, module: &mut ObjectModule, bc: &mut FunctionBuilder) -> Value {
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I128));
    sig.params.push(AbiParam::new(types::I128));
    sig.returns.push(AbiParam::new(types::I128));
    let fid = module.declare_function(name, Linkage::Import, &sig).expect("declare i128 libcall");
    let fref = module.declare_func_in_func(fid, bc.func);
    let call = bc.ins().call(fref, &[a, b]);
    bc.inst_results(call)[0]
}

/// Build an I128 constant. Cranelift's `iconst` immediate is 64-bit only, so a 128-bit value is
/// assembled from its low/high 64-bit halves via `iconcat` (ladder rung: 128-bit ints).
fn build_i128_const(v: i128, bc: &mut FunctionBuilder) -> Value {
    let u = v as u128;
    let lo = bc.ins().iconst(types::I64, (u as u64) as i64);
    let hi = bc.ins().iconst(types::I64, ((u >> 64) as u64) as i64);
    bc.ins().iconcat(lo, hi)
}

fn compile_mir_op_to_value(
    op: &str,
    args: &[String],
    vars: &std::collections::HashMap<usize, Variable>,
    tuple_vars: &std::collections::HashMap<(usize, usize), Variable>,
    unsigned: bool,
    module: &mut ObjectModule,
    bc: &mut FunctionBuilder,
) -> Value {
    let two_args = |bc: &mut FunctionBuilder| -> (Value, Value) {
        let a = args.first().map(|a| resolve_mir_operand_to_value(a, vars, tuple_vars, bc))
            .unwrap_or_else(|| bc.ins().iconst(types::I64, 0));
        let b = args.get(1).map(|a| resolve_mir_operand_to_value(a, vars, tuple_vars, bc))
            .unwrap_or_else(|| bc.ins().iconst(types::I64, 0));

        unify_int_width(bc, a, b)
    };
    match op {
        "AddWithOverflow" | "Add" => { let (a, b) = two_args(bc); if val_is_float(bc, a) { bc.ins().fadd(a, b) } else { bc.ins().iadd(a, b) } }
        "SubWithOverflow" | "Sub" => { let (a, b) = two_args(bc); if val_is_float(bc, a) { bc.ins().fsub(a, b) } else { bc.ins().isub(a, b) } }
        "MulWithOverflow" | "Mul" => { let (a, b) = two_args(bc); if val_is_float(bc, a) { bc.ins().fmul(a, b) } else { bc.ins().imul(a, b) } }
        "Div"                     => { let (a, b) = two_args(bc); if val_is_float(bc, a) { bc.ins().fdiv(a, b) } else if bc.func.dfg.value_type(a) == types::I128 { emit_i128_libcall(if unsigned { "__udivti3" } else { "__divti3" }, a, b, module, bc) } else if unsigned { bc.ins().udiv(a, b) } else { bc.ins().sdiv(a, b) } }
        "Eq"     => { let (a, b) = two_args(bc); if val_is_float(bc, a) { bc.ins().fcmp(FloatCC::Equal, a, b) } else { bc.ins().icmp(IntCC::Equal, a, b) } }
        "Ne"     => { let (a, b) = two_args(bc); if val_is_float(bc, a) { bc.ins().fcmp(FloatCC::NotEqual, a, b) } else { bc.ins().icmp(IntCC::NotEqual, a, b) } }
        "Lt"     => { let (a, b) = two_args(bc); if val_is_float(bc, a) { bc.ins().fcmp(FloatCC::LessThan, a, b) } else { bc.ins().icmp(if unsigned { IntCC::UnsignedLessThan } else { IntCC::SignedLessThan }, a, b) } }
        "Gt"     => { let (a, b) = two_args(bc); if val_is_float(bc, a) { bc.ins().fcmp(FloatCC::GreaterThan, a, b) } else { bc.ins().icmp(if unsigned { IntCC::UnsignedGreaterThan } else { IntCC::SignedGreaterThan }, a, b) } }
        "Le"     => { let (a, b) = two_args(bc); if val_is_float(bc, a) { bc.ins().fcmp(FloatCC::LessThanOrEqual, a, b) } else { bc.ins().icmp(if unsigned { IntCC::UnsignedLessThanOrEqual } else { IntCC::SignedLessThanOrEqual }, a, b) } }
        "Ge"     => { let (a, b) = two_args(bc); if val_is_float(bc, a) { bc.ins().fcmp(FloatCC::GreaterThanOrEqual, a, b) } else { bc.ins().icmp(if unsigned { IntCC::UnsignedGreaterThanOrEqual } else { IntCC::SignedGreaterThanOrEqual }, a, b) } }
        "BitAnd" => { let (a, b) = two_args(bc); bc.ins().band(a, b) }
        "BitOr"  => { let (a, b) = two_args(bc); bc.ins().bor(a, b) }
        "Rem"    => { let (a, b) = two_args(bc); if bc.func.dfg.value_type(a) == types::I128 { emit_i128_libcall(if unsigned { "__umodti3" } else { "__modti3" }, a, b, module, bc) } else if unsigned { bc.ins().urem(a, b) } else { bc.ins().srem(a, b) } }
        "BitXor" => { let (a, b) = two_args(bc); bc.ins().bxor(a, b) }
        "Shl" | "ShlUnchecked" => { let (a, b) = two_args(bc); bc.ins().ishl(a, b) }
        "Shr" | "ShrUnchecked" => { let (a, b) = two_args(bc); if unsigned { bc.ins().ushr(a, b) } else { bc.ins().sshr(a, b) } }
        "Neg" => { let a = args.first().map(|x| resolve_mir_operand_to_value(x, vars, tuple_vars, bc)).unwrap_or_else(|| bc.ins().iconst(types::I64, 0)); if val_is_float(bc, a) { bc.ins().fneg(a) } else { bc.ins().ineg(a) } }
        "Not" => { let a = args.first().map(|x| resolve_mir_operand_to_value(x, vars, tuple_vars, bc)).unwrap_or_else(|| bc.ins().iconst(types::I64, 0)); bc.ins().bnot(a) }
        "as" => { let a = args.first().map(|x| resolve_mir_operand_to_value(x, vars, tuple_vars, bc)).unwrap_or_else(|| bc.ins().iconst(types::I64, 0)); let want = mir_type_to_cl(args.get(1).map(|s| s.as_str()).unwrap_or("i64")); cast_value(bc, a, want) }
        // C-like enum: discriminant(_x) is the value itself (the enum is stored as its discriminant).
        "copy" | "move" | "Use" | "const" | "discriminant" => {
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
        Expr::Unary { operand, .. } => collect_call_arities(operand, out),
        Expr::Cast { value, .. } => collect_call_arities(value, out),
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
            let (l, r) = unify_int_width(bc, l, r);
            let f = val_is_float(bc, l);
            let u = expr_is_unsigned(left, params);
            match op {
                BinOp::Add => if f { bc.ins().fadd(l, r) } else { bc.ins().iadd(l, r) },
                BinOp::Sub => if f { bc.ins().fsub(l, r) } else { bc.ins().isub(l, r) },
                BinOp::Mul => if f { bc.ins().fmul(l, r) } else { bc.ins().imul(l, r) },
                BinOp::Div => if f { bc.ins().fdiv(l, r) } else if u { bc.ins().udiv(l, r) } else { bc.ins().sdiv(l, r) },
                BinOp::Eq => if f { bc.ins().fcmp(FloatCC::Equal, l, r) } else { bc.ins().icmp(IntCC::Equal, l, r) },
                BinOp::Neq => if f { bc.ins().fcmp(FloatCC::NotEqual, l, r) } else { bc.ins().icmp(IntCC::NotEqual, l, r) },
                BinOp::Lt => if f { bc.ins().fcmp(FloatCC::LessThan, l, r) } else { bc.ins().icmp(if u { IntCC::UnsignedLessThan } else { IntCC::SignedLessThan }, l, r) },
                BinOp::Gt => if f { bc.ins().fcmp(FloatCC::GreaterThan, l, r) } else { bc.ins().icmp(if u { IntCC::UnsignedGreaterThan } else { IntCC::SignedGreaterThan }, l, r) },
                BinOp::Le => if f { bc.ins().fcmp(FloatCC::LessThanOrEqual, l, r) } else { bc.ins().icmp(if u { IntCC::UnsignedLessThanOrEqual } else { IntCC::SignedLessThanOrEqual }, l, r) },
                BinOp::Ge => if f { bc.ins().fcmp(FloatCC::GreaterThanOrEqual, l, r) } else { bc.ins().icmp(if u { IntCC::UnsignedGreaterThanOrEqual } else { IntCC::SignedGreaterThanOrEqual }, l, r) },
                BinOp::And => bc.ins().band(l, r), BinOp::Or => bc.ins().bor(l, r),
                BinOp::Rem => if u { bc.ins().urem(l, r) } else { bc.ins().srem(l, r) },
                BinOp::BitXor => bc.ins().bxor(l, r),
                BinOp::Shl => bc.ins().ishl(l, r),
                BinOp::Shr => if u { bc.ins().ushr(l, r) } else { bc.ins().sshr(l, r) },
            }
        }
        Expr::Unary { op, operand } => {
            let v = compile_expr_with_calls(bc, module, operand, params, param_vals, func_ids);
            match op {
                UnOp::Neg => if val_is_float(bc, v) { bc.ins().fneg(v) } else { bc.ins().ineg(v) },
                UnOp::Not => if expr_is_bool(operand, params) { bc.ins().bxor_imm(v, 1) } else { bc.ins().bnot(v) },
            }
        }
        Expr::Cast { value, target } => {
            let v = compile_expr_with_calls(bc, module, value, params, param_vals, func_ids);
            cast_value(bc, v, cl_type(target))
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
    let rt = cl_type(&func.return_type);
    let val = coerce_int_width(&mut bc, val, rt);
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
            let (l, r) = unify_int_width(bc, l, r);
            let f = val_is_float(bc, l);
            let u = expr_is_unsigned(left, params);
            match op {
                BinOp::Add => if f { bc.ins().fadd(l, r) } else { bc.ins().iadd(l, r) },
                BinOp::Sub => if f { bc.ins().fsub(l, r) } else { bc.ins().isub(l, r) },
                BinOp::Mul => if f { bc.ins().fmul(l, r) } else { bc.ins().imul(l, r) },
                BinOp::Div => if f { bc.ins().fdiv(l, r) } else if u { bc.ins().udiv(l, r) } else { bc.ins().sdiv(l, r) },
                BinOp::Eq => if f { bc.ins().fcmp(FloatCC::Equal, l, r) } else { bc.ins().icmp(IntCC::Equal, l, r) },
                BinOp::Neq => if f { bc.ins().fcmp(FloatCC::NotEqual, l, r) } else { bc.ins().icmp(IntCC::NotEqual, l, r) },
                BinOp::Lt => if f { bc.ins().fcmp(FloatCC::LessThan, l, r) } else { bc.ins().icmp(if u { IntCC::UnsignedLessThan } else { IntCC::SignedLessThan }, l, r) },
                BinOp::Gt => if f { bc.ins().fcmp(FloatCC::GreaterThan, l, r) } else { bc.ins().icmp(if u { IntCC::UnsignedGreaterThan } else { IntCC::SignedGreaterThan }, l, r) },
                BinOp::Le => if f { bc.ins().fcmp(FloatCC::LessThanOrEqual, l, r) } else { bc.ins().icmp(if u { IntCC::UnsignedLessThanOrEqual } else { IntCC::SignedLessThanOrEqual }, l, r) },
                BinOp::Ge => if f { bc.ins().fcmp(FloatCC::GreaterThanOrEqual, l, r) } else { bc.ins().icmp(if u { IntCC::UnsignedGreaterThanOrEqual } else { IntCC::SignedGreaterThanOrEqual }, l, r) },
                BinOp::And => bc.ins().band(l, r), BinOp::Or => bc.ins().bor(l, r),
                BinOp::Rem => if u { bc.ins().urem(l, r) } else { bc.ins().srem(l, r) },
                BinOp::BitXor => bc.ins().bxor(l, r),
                BinOp::Shl => bc.ins().ishl(l, r),
                BinOp::Shr => if u { bc.ins().ushr(l, r) } else { bc.ins().sshr(l, r) },
            }
        }
        Expr::Unary { op, operand } => {
            let v = compile_expr(bc, operand, params);
            match op {
                UnOp::Neg => if val_is_float(bc, v) { bc.ins().fneg(v) } else { bc.ins().ineg(v) },
                UnOp::Not => if expr_is_bool(operand, params) { bc.ins().bxor_imm(v, 1) } else { bc.ins().bnot(v) },
            }
        }
        Expr::Cast { value, target } => {
            let v = compile_expr(bc, value, params);
            cast_value(bc, v, cl_type(target))
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
    #[test] fn test_bare_local() {
        // Drives aggregate-by-value call-arg expansion: only a plain `_N` (optionally copy/move) is a
        // bare local to expand; projections / consts must resolve as single values instead.
        assert_eq!(bare_local("move _3"), Some(3));
        assert_eq!(bare_local("copy _12"), Some(12));
        assert_eq!(bare_local("_7"), Some(7));
        assert_eq!(bare_local("move (_4.0: i64)"), None); // projection — single value
        assert_eq!(bare_local("const 7_i64"), None);
        assert_eq!(bare_local("(_1 as Some).0"), None);
    }
    #[test] fn test_strip_turbofish() {
        // Generic enum variant path normalization (ladder rung 5, part 1).
        assert_eq!(strip_turbofish("MyOpt::<i64>::Some"), "MyOpt::Some");
        assert_eq!(strip_turbofish("MyOpt::<i64>::None"), "MyOpt::None");
        assert_eq!(strip_turbofish("Opt::Some"), "Opt::Some"); // non-generic untouched
    }
    #[test] fn test_aggregate_fields_generic() {
        let mut s = std::collections::HashMap::new();
        s.insert("MyOpt".to_string(), vec!["i64".to_string(), "i64".to_string()]);
        // A concrete instantiation resolves to the base type's layout.
        assert_eq!(aggregate_fields("MyOpt<i64>", &s), Some(vec!["i64".into(), "i64".into()]));
        assert_eq!(aggregate_fields("MyOpt", &s), Some(vec!["i64".into(), "i64".into()]));
        assert_eq!(aggregate_fields("i64", &s), None);
    }
    #[test] fn test_add() { let u=flux_frontend::parse_source("fn add(a: i64, b: i64) -> i64 { return a + b }","t.rs").unwrap(); assert!(compile_to_clif(&u.functions[0]).unwrap().contains("function")); }
    #[test] fn test_mul() { let u=flux_frontend::parse_source("fn mul(a: i64, b: i64) -> i64 { return a * b }","t.rs").unwrap(); assert!(compile_to_clif(&u.functions[0]).is_ok()); }

    // ── float codegen (verifier-backed) ────────────────────────────────────────────
    // compile_unit_to_object runs module.define_function, which runs the Cranelift CLIF
    // verifier — it REJECTS iadd/imul/icmp on F64 operands. So a green object == the
    // float instruction was type-correctly selected. (compile_to_clif alone does NOT
    // verify, so we assert the object, not just the CLIF string shape.)
    fn tmp(name: &str) -> std::path::PathBuf { let mut p = std::env::temp_dir(); p.push(name); p }

    #[test] fn test_float_expr_object_verifies() {
        let u = flux_frontend::parse_source("fn fmul(a: f64, b: f64) -> f64 { return a * b }", "t.rs").unwrap();
        compile_unit_to_object(&u, &tmp("flux_fmul.o")).expect("f64 mul must verify — fmul selected, not imul");
        let clif = compile_to_clif(&u.functions[0]).unwrap();
        assert!(clif.contains("fmul"), "expected fmul, got:\n{}", clif);
        assert!(!clif.contains("imul"), "must NOT emit imul on f64");
    }

    #[test] fn test_float_compare_object_verifies() {
        let u = flux_frontend::parse_source("fn flt(a: f64, b: f64) -> bool { return a < b }", "t.rs").unwrap();
        compile_unit_to_object(&u, &tmp("flux_flt.o")).expect("f64 compare must verify — fcmp selected");
        assert!(compile_to_clif(&u.functions[0]).unwrap().contains("fcmp"));
    }

    // Control: the integer path must stay byte-identical AND this proves the
    // object-emit harness itself works on this host (disambiguates codegen vs platform).
    #[test] fn test_int_nonregression_object_verifies() {
        let u = flux_frontend::parse_source("fn imul2(a: i64, b: i64) -> i64 { return a * b }", "t.rs").unwrap();
        compile_unit_to_object(&u, &tmp("flux_imul.o")).expect("i64 mul must verify");
        let clif = compile_to_clif(&u.functions[0]).unwrap();
        assert!(clif.contains("imul") && !clif.contains("fmul"));
    }

    // The headline fix: float on the MIR-direct path (loops). Hand-build the MIR for
    //   fn faccum() -> f64 { let mut acc=0.0; let mut i=0; while i<3 { acc=acc+1.5; i=i+1 } acc }  // → 4.5
    // and compile via compile_unit_to_object_with_mir → compile_mir_into_function →
    // define_function (verifier). Without the fix, Add(_1, const 1.5_f64) resolves the
    // const to iconst(I64,0) and emits iadd on F64 → the verifier rejects the function.
    #[test] fn test_float_mir_loop_object_verifies() {
        use flux_frontend::mir::{MirFunction, MirLocal, MirBlock, MirStmt, MirTerminator};
        let asg = |dst: &str, op: &str, args: &[&str]| MirStmt::Assign {
            dst: dst.into(), op: op.into(), args: args.iter().map(|s| s.to_string()).collect(),
        };
        let loc = |i: usize, ty: &str| MirLocal { index: i, name: format!("_{}", i), ty: ty.into(), mutable: true };
        let mir = MirFunction {
            name: "faccum".into(), params: vec![], return_type: "f64".into(),
            locals: vec![loc(0, "f64"), loc(1, "f64"), loc(2, "i64"), loc(3, "bool")],
            blocks: vec![
                MirBlock { label: "bb0".into(), statements: vec![
                    asg("_1", "const", &["const 0f64"]),
                    asg("_2", "const", &["const 0_i64"]),
                ], terminator: Some(MirTerminator::Goto("bb1".into())) },
                MirBlock { label: "bb1".into(), statements: vec![
                    asg("_3", "Lt", &["_2", "const 3_i64"]),
                ], terminator: Some(MirTerminator::SwitchInt {
                    discr: "_3".into(), targets: vec![("0".into(), "bb3".into())], otherwise: "bb2".into() }) },
                MirBlock { label: "bb2".into(), statements: vec![
                    asg("_1", "Add", &["_1", "const 1.5f64"]),
                    asg("_2", "Add", &["_2", "const 1_i64"]),
                ], terminator: Some(MirTerminator::Goto("bb1".into())) },
                MirBlock { label: "bb3".into(), statements: vec![
                    asg("_0", "copy", &["_1"]),
                ], terminator: Some(MirTerminator::Return) },
            ],
        };
        let u = flux_frontend::parse_source("fn faccum() -> f64 { return 0.0 }", "t.rs").unwrap();
        let mut ov = std::collections::HashMap::new();
        ov.insert("faccum".to_string(), mir);
        compile_unit_to_object_with_mir(&u, &ov, &tmp("flux_faccum.o"))
            .expect("MIR-path f64 loop must verify — f64const + fadd selected through the loop");
    }

    // ── 0.29.0 new operators: %, ^, &, |, <<, >> + signed div ──────────────────────
    // Each compiles to an object (verifier-backed) AND asserts the right Cranelift op is
    // selected — instruction selection determines the runtime value, so the string check
    // is the value proof the verifier alone can't give. Unique fn name per test = unique
    // temp object file (cargo runs tests in parallel).
    fn clif_of(src: &str) -> String {
        let u = flux_frontend::parse_source(src, "t.rs").unwrap();
        let fname = format!("flux_op_{}.o", u.functions[0].name);
        compile_unit_to_object(&u, &tmp(&fname)).expect("must verify");
        compile_to_clif(&u.functions[0]).unwrap()
    }
    #[test] fn test_modulo_srem()    { assert!(clif_of("fn r(a: i64, b: i64) -> i64 { return a % b }").contains("srem")); }
    #[test] fn test_bitxor()         { assert!(clif_of("fn x(a: i64, b: i64) -> i64 { return a ^ b }").contains("bxor")); }
    #[test] fn test_bitand_fix()     { let c = clif_of("fn n(a: i64, b: i64) -> i64 { return a & b }"); assert!(c.contains("band") && !c.contains("iadd"), "& must be band not iadd:\n{}", c); }
    #[test] fn test_bitor()          { assert!(clif_of("fn o(a: i64, b: i64) -> i64 { return a | b }").contains("bor")); }
    #[test] fn test_shl()            { assert!(clif_of("fn sl(a: i64, b: i64) -> i64 { return a << b }").contains("ishl")); }
    #[test] fn test_shr()            { assert!(clif_of("fn sr(a: i64, b: i64) -> i64 { return a >> b }").contains("sshr")); }
    #[test] fn test_signed_div_fix() { let c = clif_of("fn d(a: i64, b: i64) -> i64 { return a / b }"); assert!(c.contains("sdiv") && !c.contains("udiv"), "/ must be sdiv not udiv:\n{}", c); }

    // ── 0.30.0 integer-width coercion: a non-i64 var mixed with an i64-default const must
    // unify widths (the verifier rejects iadd/icmp on mismatched widths — same class as floats). ──
    #[test] fn test_i32_const_width() { let u = flux_frontend::parse_source("fn f(a: i32) -> i32 { return a + 5 }", "t.rs").unwrap(); compile_unit_to_object(&u, &tmp("flux_i32c.o")).expect("i32 + const must verify (width-coerced)"); }
    #[test] fn test_i16_const_width() { let u = flux_frontend::parse_source("fn f(a: i16) -> i16 { return a - 1 }", "t.rs").unwrap(); compile_unit_to_object(&u, &tmp("flux_i16c.o")).expect("i16 - const must verify"); }
    #[test] fn test_i32_cmp_width()   { let u = flux_frontend::parse_source("fn f(a: i32) -> bool { return a < 5 }", "t.rs").unwrap(); compile_unit_to_object(&u, &tmp("flux_i32cmp.o")).expect("i32 compare with const must verify"); }
    #[test] fn test_i64_width_noop()  { let u = flux_frontend::parse_source("fn f(a: i64) -> i64 { return a + 5 }", "t.rs").unwrap(); let c = compile_to_clif(&u.functions[0]).unwrap(); assert!(!c.contains("sextend") && !c.contains("ireduce"), "i64 path must stay coercion-free:\n{}", c); }

    // ── 0.30.0 signedness: u32/u64 select unsigned ops; signed types stay signed ──
    #[test] fn test_u32_div_unsigned()    { let c = clif_of("fn du(a: u32, b: u32) -> u32 { return a / b }"); assert!(c.contains("udiv") && !c.contains("sdiv"), "u32 / must be udiv:\n{}", c); }
    #[test] fn test_u64_rem_unsigned()    { let c = clif_of("fn ru(a: u64, b: u64) -> u64 { return a % b }"); assert!(c.contains("urem") && !c.contains("srem"), "u64 % must be urem:\n{}", c); }
    #[test] fn test_u32_cmp_unsigned()    { let c = clif_of("fn cu(a: u32, b: u32) -> bool { return a < b }"); assert!(c.contains("ult"), "u32 < must be unsigned (icmp ult):\n{}", c); }
    #[test] fn test_i32_div_still_signed(){ let c = clif_of("fn ds(a: i32, b: i32) -> i32 { return a / b }"); assert!(c.contains("sdiv") && !c.contains("udiv"), "i32 / must stay sdiv:\n{}", c); }

    // ── 0.30.0 unary operators: -x (ineg/fneg), !x (bnot for int, low-bit flip for bool) ──
    #[test] fn test_neg_int()   { assert!(clif_of("fn ng(a: i64) -> i64 { return -a }").contains("ineg")); }
    #[test] fn test_neg_float() { assert!(clif_of("fn nf(a: f64) -> f64 { return -a }").contains("fneg")); }
    #[test] fn test_not_int()   { assert!(clif_of("fn ni(a: i64) -> i64 { return !a }").contains("bnot")); }
    #[test] fn test_not_bool()  { let c = clif_of("fn nb(a: bool) -> bool { return !a }"); assert!(c.contains("bxor"), "bool ! must flip the low bit (bxor_imm), not bnot:\n{}", c); }

    // ── 0.30.0 numeric casts (`as`): int widen/narrow, int<->float, f64->f32 ──
    #[test] fn test_cast_widen()   { assert!(clif_of("fn ci(a: i32) -> i64 { return a as i64 }").contains("sextend")); }
    #[test] fn test_cast_narrow()  { assert!(clif_of("fn cn(a: i64) -> i32 { return a as i32 }").contains("ireduce")); }
    #[test] fn test_cast_int_flt() { assert!(clif_of("fn cf(a: i64) -> f64 { return a as f64 }").contains("fcvt_from_sint")); }
    #[test] fn test_cast_flt_int() { assert!(clif_of("fn fi(a: f64) -> i64 { return a as i64 }").contains("fcvt_to_sint")); }
    #[test] fn test_cast_f64_f32() { assert!(clif_of("fn fd(a: f64) -> f32 { return a as f32 }").contains("fdemote")); }

    // Regression: a fractional float const `2.5f64` must lower to Float through the MIR (Expr)
    // path, not truncate at the decimal to Int(2) — which produced imul(i64,f64) and a verifier
    // reject. Replicates the exact fluxc-run path (parse_mir -> lower_mir_to_ir -> codegen).
    #[test] fn test_cast_of_float_arith_mir() {
        let mir = "fn main() -> i64 {\n    let mut _0: i64;\n    let mut _1: f64;\n    let mut _2: f64;\n    bb0: {\n        _2 = const 2.5f64;\n        _1 = Mul(move _2, const 4f64);\n        _0 = move _1 as i64 (FloatToInt);\n        return;\n    }\n}";
        let funcs = flux_frontend::mir::parse_mir(mir).unwrap();
        let fd = flux_frontend::mir::lower_mir_to_ir(&funcs[0]);
        let u = flux_frontend::TranslationUnit { file_path: "t".into(), functions: vec![fd], structs: vec![], enums: vec![], imports: vec![] };
        compile_unit_to_object(&u, &tmp("cast_mir.o")).expect("cast of float-mul (MIR path) must verify");
        let c = compile_to_clif(&u.functions[0]).unwrap();
        assert!(c.contains("fmul") && c.contains("fcvt_to_sint_sat") && !c.contains("imul"), "expected fmul+fcvt, got:\n{}", c);
    }

    // Regression: parenthesized expressions must lower (syn::Expr::Paren), not become Empty -> 0.
    #[test] fn test_paren_lowering() {
        let c = clif_of("fn p(a: i64) -> i64 { return (a + 1) * 2 }");
        assert!(c.contains("iadd") && c.contains("imul"), "parens dropped:\n{}", c);
    }

    // 0.31 foundation: the existing scalar-replacement scaffolding must already construct a flat
    // (i64,i64) tuple and read both fields. Hand-build the MIR for `let t=(10,20); t.0 + t.1`.
    #[test] fn test_tuple_construct_read_mir() {
        use flux_frontend::mir::{MirFunction, MirLocal, MirBlock, MirStmt, MirTerminator};
        let asg = |dst: &str, op: &str, args: &[&str]| MirStmt::Assign { dst: dst.into(), op: op.into(), args: args.iter().map(|s| s.to_string()).collect() };
        let mir = MirFunction {
            name: "tup".into(), params: vec![], return_type: "i64".into(),
            locals: vec![
                MirLocal{index:0,name:"_0".into(),ty:"i64".into(),mutable:true},
                MirLocal{index:1,name:"_1".into(),ty:"(i64, i64)".into(),mutable:true},
            ],
            blocks: vec![ MirBlock { label: "bb0".into(), statements: vec![
                asg("_1", "", &["const 10_i64", "const 20_i64"]),
                asg("_0", "Add", &["_1.0", "_1.1"]),
            ], terminator: Some(MirTerminator::Return) } ],
        };
        let u = flux_frontend::parse_source("fn tup() -> i64 { return 0 }", "t").unwrap();
        let mut ov = std::collections::HashMap::new();
        ov.insert("tup".to_string(), mir);
        compile_unit_to_object_with_mir(&u, &ov, &tmp("tup.o")).expect("flat tuple construct+read must verify");
    }

    // 0.31 step 2: aggregate-by-value RETURN — `fn mk() -> (i64,i64) { (10,20) }` returns a
    // 2-element signature; the verifier rejects a return-arity mismatch, so green == correct ABI.
    #[test] fn test_tuple_return_mir() {
        use flux_frontend::mir::{MirFunction, MirLocal, MirBlock, MirStmt, MirTerminator};
        let asg = |dst: &str, op: &str, args: &[&str]| MirStmt::Assign { dst: dst.into(), op: op.into(), args: args.iter().map(|s| s.to_string()).collect() };
        let mir = MirFunction {
            name: "mk".into(), params: vec![], return_type: "(i64, i64)".into(),
            locals: vec![ MirLocal{index:0,name:"_0".into(),ty:"(i64, i64)".into(),mutable:true} ],
            blocks: vec![ MirBlock { label: "bb0".into(), statements: vec![
                asg("_0", "", &["const 10_i64", "const 20_i64"]),
            ], terminator: Some(MirTerminator::Return) } ],
        };
        let u = flux_frontend::parse_source("fn mk() -> i64 { return 0 }", "t").unwrap();
        let mut ov = std::collections::HashMap::new();
        ov.insert("mk".to_string(), mir);
        compile_unit_to_object_with_mir(&u, &ov, &tmp("mk.o")).expect("tuple by-value return must verify (2-element return sig)");
    }

    // 0.31 step 3: STRUCTS as named tuples — construct `P { x, y }` (scalar-replaced via the layout
    // table from unit.structs), read both fields, sum. Before the layout table, `_1: P` declared a
    // single var and the construction silently produced 0.
    #[test] fn test_struct_construct_read_mir() {
        use flux_frontend::mir::{MirFunction, MirLocal, MirBlock, MirStmt, MirTerminator};
        let asg = |dst: &str, op: &str, args: &[&str]| MirStmt::Assign { dst: dst.into(), op: op.into(), args: args.iter().map(|s| s.to_string()).collect() };
        let mir = MirFunction {
            name: "main".into(), params: vec![], return_type: "i64".into(),
            locals: vec![
                MirLocal{index:0,name:"_0".into(),ty:"i64".into(),mutable:true},
                MirLocal{index:1,name:"_1".into(),ty:"P".into(),mutable:true},
            ],
            blocks: vec![ MirBlock { label: "bb0".into(), statements: vec![
                asg("_1", "", &["const 3_i64", "const 4_i64"]),
                asg("_0", "Add", &["_1.0", "_1.1"]),
            ], terminator: Some(MirTerminator::Return) } ],
        };
        let u = flux_frontend::parse_source("struct P { x: i64, y: i64 }\nfn main() -> i64 { return 0 }", "t").unwrap();
        assert_eq!(u.structs.len(), 1, "struct P must be parsed into the unit");
        let mut ov = std::collections::HashMap::new();
        ov.insert("main".to_string(), mir);
        compile_unit_to_object_with_mir(&u, &ov, &tmp("struct.o")).expect("struct construct+read must verify");
    }

    // 0.32: call-site multi-result destructuring — `let p = mk(); p.0 + p.1` where mk returns a
    // tuple. The callee's 2-element return sig + the caller distributing both results to p's fields.
    #[test] fn test_tuple_returning_call_mir() {
        use flux_frontend::mir::{MirFunction, MirLocal, MirBlock, MirStmt, MirTerminator};
        let asg = |dst: &str, op: &str, args: &[&str]| MirStmt::Assign { dst: dst.into(), op: op.into(), args: args.iter().map(|s| s.to_string()).collect() };
        let mk = MirFunction {
            name: "mk".into(), params: vec![], return_type: "(i64, i64)".into(),
            locals: vec![ MirLocal{index:0,name:"_0".into(),ty:"(i64, i64)".into(),mutable:true} ],
            blocks: vec![ MirBlock { label: "bb0".into(), statements: vec![ asg("_0","",&["const 3_i64","const 4_i64"]) ], terminator: Some(MirTerminator::Return) } ],
        };
        let main = MirFunction {
            name: "main".into(), params: vec![], return_type: "i64".into(),
            locals: vec![
                MirLocal{index:0,name:"_0".into(),ty:"i64".into(),mutable:true},
                MirLocal{index:1,name:"_1".into(),ty:"(i64, i64)".into(),mutable:true},
            ],
            blocks: vec![
                MirBlock { label: "bb0".into(), statements: vec![], terminator: Some(MirTerminator::Call{ func:"mk".into(), args: vec![], dst:"_1".into(), target:"bb1".into() }) },
                MirBlock { label: "bb1".into(), statements: vec![ asg("_0","Add",&["_1.0","_1.1"]) ], terminator: Some(MirTerminator::Return) },
            ],
        };
        let u = flux_frontend::parse_source("fn mk() -> i64 { return 0 }\nfn main() -> i64 { return 0 }", "t").unwrap();
        let mut ov = std::collections::HashMap::new();
        ov.insert("mk".to_string(), mk);
        ov.insert("main".to_string(), main);
        compile_unit_to_object_with_mir(&u, &ov, &tmp("call.o")).expect("tuple-returning call must verify (multi-result destructure)");
    }

    // Reproduces the EXACT fluxc-run path (parse_mir -> parse_rhs) for a tuple-returning call.
    // Regression: `_1 = mk()` parsed mk's empty parens as args=[""] -> main called the 0-param mk
    // with 1 arg -> verifier reject. parse_rhs must yield [] for `mk()`.
    #[test] fn test_tuple_returning_call_real_mir() {
        let mir = "fn mk() -> (i64, i64) {\n    let mut _0: (i64, i64);\n    bb0: {\n        _0 = (const 3_i64, const 4_i64);\n        return;\n    }\n}\n\nfn main() -> i64 {\n    let mut _0: i64;\n    let _1: (i64, i64);\n    let mut _2: i64;\n    let mut _3: i64;\n    let mut _4: (i64, bool);\n    bb0: {\n        _1 = mk() -> [return: bb1, unwind continue];\n    }\n    bb1: {\n        _2 = copy (_1.0: i64);\n        _3 = copy (_1.1: i64);\n        _4 = AddWithOverflow(copy _2, copy _3);\n        assert(!move (_4.1: bool), \"x\") -> [success: bb2, unwind continue];\n    }\n    bb2: {\n        _0 = move (_4.0: i64);\n        return;\n    }\n}";
        let funcs = flux_frontend::mir::parse_mir(mir).unwrap();
        let u = flux_frontend::parse_source("fn mk() -> i64 { return 0 }\nfn main() -> i64 { return 0 }", "t").unwrap();
        let mut ov = std::collections::HashMap::new();
        for f in &funcs { ov.insert(f.name.clone(), f.clone()); }
        compile_unit_to_object_with_mir(&u, &ov, &tmp("realcall.o")).expect("real tuple-returning-call MIR must verify (mk() => 0 args)");
    }

    // 0.33 ADVERSARIAL: struct-RETURNING call. `fn mk() -> P { P{x,y} } fn main(){ let p=mk(); p.x+p.y }`.
    // The fix (aggregate_fields at both sig + return sites) must declare mk with a 2-element return
    // and emit 2 return values + distribute both into p's fields. Pre-fix mk()->P collapses to 1.
    #[test] fn test_struct_returning_call_mir() {
        use flux_frontend::mir::{MirFunction, MirLocal, MirBlock, MirStmt, MirTerminator};
        let asg = |dst: &str, op: &str, args: &[&str]| MirStmt::Assign { dst: dst.into(), op: op.into(), args: args.iter().map(|s| s.to_string()).collect() };
        let mk = MirFunction {
            name: "mk".into(), params: vec![], return_type: "P".into(),
            locals: vec![ MirLocal{index:0,name:"_0".into(),ty:"P".into(),mutable:true} ],
            blocks: vec![ MirBlock { label: "bb0".into(), statements: vec![ asg("_0","",&["const 6_i64","const 7_i64"]) ], terminator: Some(MirTerminator::Return) } ],
        };
        let main = MirFunction {
            name: "main".into(), params: vec![], return_type: "i64".into(),
            locals: vec![
                MirLocal{index:0,name:"_0".into(),ty:"i64".into(),mutable:true},
                MirLocal{index:1,name:"_1".into(),ty:"P".into(),mutable:true},
            ],
            blocks: vec![
                MirBlock { label: "bb0".into(), statements: vec![], terminator: Some(MirTerminator::Call{ func:"mk".into(), args: vec![], dst:"_1".into(), target:"bb1".into() }) },
                MirBlock { label: "bb1".into(), statements: vec![ asg("_0","Add",&["_1.0","_1.1"]) ], terminator: Some(MirTerminator::Return) },
            ],
        };
        let u = flux_frontend::parse_source("struct P { x: i64, y: i64 }\nfn mk() -> i64 { return 0 }\nfn main() -> i64 { return 0 }", "t").unwrap();
        assert_eq!(u.structs.len(), 1, "struct P must be in the unit for struct_layout to resolve it");
        let mut ov = std::collections::HashMap::new();
        ov.insert("mk".to_string(), mk);
        ov.insert("main".to_string(), main);
        compile_unit_to_object_with_mir(&u, &ov, &tmp("struct_call.o")).expect("struct-returning call must verify (2-result sig + distribute)");
    }

    // Real fluxc parse_mir path: `fn mk() -> P` yields return_type "P" (bare name, bypasses
    // parse_tuple_type) — this is the exact bug. struct P must be declared in the source unit.
    #[test] fn test_struct_returning_call_real_mir() {
        let mir = "fn mk() -> P {\n    let mut _0: P;\n    bb0: {\n        _0 = P { x: const 6_i64, y: const 7_i64 };\n        return;\n    }\n}\n\nfn main() -> i64 {\n    let mut _0: i64;\n    let _1: P;\n    let mut _2: i64;\n    let mut _3: i64;\n    bb0: {\n        _1 = mk() -> [return: bb1, unwind continue];\n    }\n    bb1: {\n        _2 = copy (_1.0: i64);\n        _3 = copy (_1.1: i64);\n        _0 = Add(copy _2, copy _3);\n        return;\n    }\n}";
        let funcs = flux_frontend::mir::parse_mir(mir).unwrap();
        assert_eq!(funcs[0].return_type, "P", "mk's return_type must be the bare struct name 'P'");
        let u = flux_frontend::parse_source("struct P { x: i64, y: i64 }\nfn mk() -> i64 { return 0 }\nfn main() -> i64 { return 0 }", "t").unwrap();
        let mut ov = std::collections::HashMap::new();
        for f in &funcs { ov.insert(f.name.clone(), f.clone()); }
        compile_unit_to_object_with_mir(&u, &ov, &tmp("realstructcall.o")).expect("real struct-returning-call MIR must verify");
    }

    #[test] fn test_clike_enum_match_mir() {
        // Ladder rung 4: a C-like enum match. Exercises every new piece — discriminant(_1) (copy),
        // switchInt on it, the `unreachable;` otherwise arm (-> return-zero terminator), and the
        // `_1 = Color::Green` construction (-> iconst of the variant discriminant). Verification-only:
        // proves the IR verifies (arity/types); the VALUE pick(Green)=2 is confirmed by e2e on epsilon.
        let mir = "fn pick(_1: Color) -> i64 {\n    let mut _0: i64;\n    let mut _2: isize;\n    bb0: {\n        _2 = discriminant(_1);\n        switchInt(move _2) -> [0: bb4, 1: bb3, 2: bb2, otherwise: bb1];\n    }\n    bb1: {\n        unreachable;\n    }\n    bb2: {\n        _0 = const 3_i64;\n        goto -> bb5;\n    }\n    bb3: {\n        _0 = const 2_i64;\n        goto -> bb5;\n    }\n    bb4: {\n        _0 = const 1_i64;\n        goto -> bb5;\n    }\n    bb5: {\n        return;\n    }\n}\n\nfn main() -> i64 {\n    let mut _0: i64;\n    let mut _1: Color;\n    bb0: {\n        _1 = Color::Green;\n        _0 = pick(move _1) -> [return: bb1, unwind continue];\n    }\n    bb1: {\n        return;\n    }\n}";
        let funcs = flux_frontend::mir::parse_mir(mir).unwrap();
        // the `unreachable;` arm must now be captured, not dropped
        let pick = funcs.iter().find(|f| f.name == "pick").unwrap();
        assert!(pick.blocks.iter().any(|b| matches!(b.terminator, Some(flux_frontend::mir::MirTerminator::Unreachable))),
            "unreachable; must parse to MirTerminator::Unreachable");
        let u = flux_frontend::parse_source("enum Color { Red, Green, Blue }\nfn pick(c: i64) -> i64 { return 0 }\nfn main() -> i64 { return 0 }", "t").unwrap();
        assert_eq!(u.enums.len(), 1);
        let mut ov = std::collections::HashMap::new();
        for f in &funcs { ov.insert(f.name.clone(), f.clone()); }
        compile_unit_to_object_with_mir(&u, &ov, &tmp("enummatch.o")).expect("C-like enum match MIR must verify");
    }
}
