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

    let return_type = return_part.trim_start_matches("-> ").trim().to_string();

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
        } else {
            lines.next();
        }
    }

    Ok(MirBlock { label, statements, terminator })
}

fn parse_rhs(rhs: &str) -> (String, Vec<String>) {
    if let Some(paren) = rhs.find('(') {
        let op = rhs[..paren].trim().to_string();
        let args_str = &rhs[paren+1..];
        if let Some(close) = args_str.rfind(')') {
            let args: Vec<String> = args_str[..close].split(',')
                .map(|a| a.trim().trim_start_matches("copy ").trim_start_matches("move ").to_string())
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
        "Eq"                                 if args.len() >= 2 => binop(BinOp::Eq, args),
        "Ne"                                 if args.len() >= 2 => binop(BinOp::Neq, args),
        "Lt"                                 if args.len() >= 2 => binop(BinOp::Lt, args),
        "Gt"                                 if args.len() >= 2 => binop(BinOp::Gt, args),
        "Le"                                 if args.len() >= 2 => binop(BinOp::Le, args),
        "Ge"                                 if args.len() >= 2 => binop(BinOp::Ge, args),
        "BitAnd"                             if args.len() >= 2 => binop(BinOp::And, args),
        "BitOr"                              if args.len() >= 2 => binop(BinOp::Or, args),
        "copy" | "move" | "Use" | "const"    if args.len() >= 1 => lower_operand(&args[0], pn),
        _ => Expr::Empty,
    }
}

fn lower_operand(op: &str, pn: &[String]) -> crate::Expr {
    let c = op.trim().trim_start_matches("copy ").trim_start_matches("move ").trim();
    // Tuple-projection like `(_3.0: i64)` → use _3 as the carrier; tuple unpacking proper TBD.
    let c = c.trim_start_matches('(').trim_end_matches(')');
    let c = if let Some(dot) = c.find('.') { &c[..dot] } else { c };
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
        // MIR literals: "const 2_i64", "const -5_i32", "const 1u8", "const 0.5_f64".
        let head = rest.split('_').next().unwrap_or("")
            .trim_end_matches(|c: char| c.is_ascii_alphabetic());
        if let Ok(i) = head.parse::<i64>() {
            crate::Expr::Literal(crate::Literal::Int(i))
        } else if let Ok(f) = head.parse::<f64>() {
            crate::Expr::Literal(crate::Literal::Float(f))
        } else if rest == "true" {
            crate::Expr::Literal(crate::Literal::Bool(true))
        } else if rest == "false" {
            crate::Expr::Literal(crate::Literal::Bool(false))
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
