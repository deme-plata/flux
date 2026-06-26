// flux-frontend — Phase 3: Rust parser + AST → HIR lowering
//
// Uses `syn` (the same parser rustc and rust-analyzer use) to parse
// Rust source files. Extracts function definitions and produces a
// simplified High-Level IR that the backend can consume.
//
// This is the first real compiler frontend in Flux. Not a wrapper —
// actual parsing, analysis, and IR generation.

use serde::{Deserialize, Serialize};
pub mod mir;
use std::path::Path;

// ── FROZEN IR (FIP-0001 keep-A-open #1) ──
//
// The public IR types below (TranslationUnit, FunctionDef, Expr, TypeRef, BinOp, UnOp) are Flux's
// STABLE intermediate representation. Per FIP-0001 (Option B, accepted 2026-06-26), the rustc `--emit=mir`
// text dialect is known to exactly ONE place — `mir::parse_mir`. A future native frontend (Option A)
// emits this same IR directly, so the rest of the pipeline never learns whether MIR came from rustc or
// from Flux's own parser. Any breaking change to these types MUST bump IR_VERSION (and update the
// ir_version_frozen snapshot test), so a frontend swap stays a contained, intentional event.
pub const IR_VERSION: u32 = 2;

// ── High-Level IR ──

/// A compiled translation unit (one .rs file).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslationUnit {
    pub file_path: String,
    pub functions: Vec<FunctionDef>,
    pub structs: Vec<StructDef>,
    pub enums: Vec<EnumDef>,
    pub imports: Vec<String>,
}

/// A function definition in the IR.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    pub visibility: Visibility,
    pub params: Vec<Param>,
    pub return_type: TypeRef,
    pub body: Expr,
    pub is_async: bool,
}

/// A struct definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<FieldDef>,
    pub derives: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDef {
    pub name: String,
    pub ty: TypeRef,
    pub visibility: Visibility,
}

/// A C-like enum definition (FIP-0001 type-complexity ladder, rung 4). For now an enum is modelled
/// as its discriminant: each variant carries a name and a computed i64 discriminant (implicit
/// previous+1, overridable by an explicit `= N`). Data-carrying variants are a later rung.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<EnumVariant>,
    pub derives: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumVariant {
    pub name: String,
    pub discriminant: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub ty: TypeRef,
}

/// Simplified type representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TypeRef {
    Unit,
    I32, I64, U32, U64, Bool, F32, F64, String,
    Named(String),
    Ref(Box<TypeRef>),
    Option(Box<TypeRef>),
    Vec(Box<TypeRef>),
    Result(Box<TypeRef>, Box<TypeRef>),
    Unknown,
}

/// Expression in the IR.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Expr {
    Literal(Literal),
    Variable(String),
    BinaryOp { op: BinOp, left: Box<Expr>, right: Box<Expr> },
    Unary { op: UnOp, operand: Box<Expr> },
    Cast { value: Box<Expr>, target: TypeRef },
    Call { func: String, args: Vec<Expr> },
    Return(Box<Expr>),
    Let { name: String, value: Box<Expr> },
    Block(Vec<Expr>),
    If { cond: Box<Expr>, then_branch: Box<Expr>, else_branch: Option<Box<Expr>> },
    Empty,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Literal {
    Int(i64), Float(f64), Bool(bool), Str(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BinOp { Add, Sub, Mul, Div, Rem, Eq, Neq, Lt, Gt, Le, Ge, And, Or, BitXor, Shl, Shr }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UnOp { Neg, Not }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Visibility { Public, Private }

// ── Parser ──

/// Parse a Rust source file into a TranslationUnit.
pub fn parse_file(path: &Path) -> Result<TranslationUnit, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("Cannot read {}: {}", path.display(), e))?;
    parse_source(&source, &path.to_string_lossy())
}

/// Parse Rust source code string.
pub fn parse_source(source: &str, file_path: &str) -> Result<TranslationUnit, String> {
    let ast = syn::parse_file(source)
        .map_err(|e| format!("Parse error in {}: {}", file_path, e))?;

    let mut functions = Vec::new();
    let mut structs = Vec::new();
    let mut enums = Vec::new();
    let mut imports = Vec::new();

    for item in ast.items {
        match item {
            syn::Item::Fn(f) => {
                if let Some(func) = lower_function(&f) {
                    functions.push(func);
                }
            }
            syn::Item::Struct(s) => {
                structs.push(lower_struct(&s));
            }
            syn::Item::Enum(e) => {
                enums.push(lower_enum(&e));
            }
            syn::Item::Use(u) => {
                imports.push(quote::quote!(#u).to_string().replace("use ", "").replace(";", ""));
            }
            _ => {}
        }
    }

    Ok(TranslationUnit {
        file_path: file_path.to_string(),
        functions,
        structs,
        enums,
        imports,
    })
}

// ── Lowering: syn AST → Flux IR ──

fn lower_function(f: &syn::ItemFn) -> Option<FunctionDef> {
    let params: Vec<Param> = f.sig.inputs.iter().filter_map(|arg| {
        if let syn::FnArg::Typed(pat) = arg {
            if let syn::Pat::Ident(ident) = &*pat.pat {
                return Some(Param {
                    name: ident.ident.to_string(),
                    ty: lower_type(&pat.ty),
                });
            }
        }
        None
    }).collect();

    let return_type = match &f.sig.output {
        syn::ReturnType::Default => TypeRef::Unit,
        syn::ReturnType::Type(_, ty) => lower_type(ty),
    };

    let body = lower_block(&f.block);

    Some(FunctionDef {
        name: f.sig.ident.to_string(),
        visibility: match f.vis { syn::Visibility::Public(_) => Visibility::Public, _ => Visibility::Private },
        params,
        return_type,
        body,
        is_async: f.sig.asyncness.is_some(),
    })
}

fn lower_type(ty: &syn::Type) -> TypeRef {
    match ty {
        syn::Type::Path(tp) => {
            let segments: Vec<String> = tp.path.segments.iter()
                .map(|s| s.ident.to_string()).collect();
            let name = segments.join("::");
            match name.as_str() {
                "i32" => TypeRef::I32,
                "i64" => TypeRef::I64,
                "u32" => TypeRef::U32,
                "u64" => TypeRef::U64,
                "bool" => TypeRef::Bool,
                "f32" => TypeRef::F32,
                "f64" => TypeRef::F64,
                "String" => TypeRef::String,
                "()" => TypeRef::Unit,
                _ => TypeRef::Named(name),
            }
        }
        syn::Type::Reference(tr) => {
            TypeRef::Ref(Box::new(lower_type(&tr.elem)))
        }
        _ => TypeRef::Unknown,
    }
}

fn lower_block(block: &syn::Block) -> Expr {
    let stmts: Vec<Expr> = block.stmts.iter().filter_map(lower_stmt).collect();
    if stmts.is_empty() {
        Expr::Empty
    } else {
        Expr::Block(stmts)
    }
}

fn lower_stmt(stmt: &syn::Stmt) -> Option<Expr> {
    match stmt {
        syn::Stmt::Local(local) => {
            if let syn::Pat::Ident(ident) = &local.pat {
                let init = if let Some(init) = &local.init {
                    lower_expr(&init.expr)
                } else {
                    Expr::Empty
                };
                Some(Expr::Let {
                    name: ident.ident.to_string(),
                    value: Box::new(init),
                })
            } else {
                None
            }
        }
        syn::Stmt::Expr(expr, _) => Some(lower_expr(expr)),
        syn::Stmt::Item(_) => None,
        _ => None,
    }
}

fn lower_expr(expr: &syn::Expr) -> Expr {
    match expr {
        syn::Expr::Lit(lit) => lower_literal(&lit.lit),
        syn::Expr::Path(path) => {
            let name = path.path.segments.iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>()
                .join("::");
            Expr::Variable(name)
        }
        syn::Expr::Binary(bin) => {
            Expr::BinaryOp {
                op: lower_binop(&bin.op),
                left: Box::new(lower_expr(&bin.left)),
                right: Box::new(lower_expr(&bin.right)),
            }
        }
        syn::Expr::Unary(un) => match &un.op {
            syn::UnOp::Neg(_) => Expr::Unary { op: UnOp::Neg, operand: Box::new(lower_expr(&un.expr)) },
            syn::UnOp::Not(_) => Expr::Unary { op: UnOp::Not, operand: Box::new(lower_expr(&un.expr)) },
            _ => Expr::Empty, // deref etc. unsupported
        },
        syn::Expr::Cast(c) => Expr::Cast {
            value: Box::new(lower_expr(&c.expr)),
            target: lower_type(&c.ty),
        },
        syn::Expr::Call(call) => {
            let func_name = if let syn::Expr::Path(p) = &*call.func {
                p.path.segments.iter().map(|s| s.ident.to_string()).collect::<Vec<_>>().join("::")
            } else {
                "unknown".to_string()
            };
            let args: Vec<Expr> = call.args.iter().map(|a| lower_expr(a)).collect();
            Expr::Call { func: func_name, args }
        }
        syn::Expr::Return(ret) => {
            if let Some(val) = &ret.expr {
                Expr::Return(Box::new(lower_expr(val)))
            } else {
                Expr::Return(Box::new(Expr::Empty))
            }
        }
        syn::Expr::If(if_expr) => {
            Expr::If {
                cond: Box::new(lower_expr(&if_expr.cond)),
                then_branch: Box::new(lower_block(&if_expr.then_branch)),
                else_branch: if_expr.else_branch.as_ref().map(|(_, b)| {
                    Box::new(lower_expr(b))
                }),
            }
        }
        syn::Expr::Block(block) => lower_block(&block.block),
        syn::Expr::Paren(p) => lower_expr(&p.expr),
        _ => Expr::Empty,
    }
}

fn lower_literal(lit: &syn::Lit) -> Expr {
    match lit {
        syn::Lit::Int(i) => Expr::Literal(Literal::Int(i.base10_parse().unwrap_or(0))),
        syn::Lit::Float(f) => Expr::Literal(Literal::Float(f.base10_parse().unwrap_or(0.0))),
        syn::Lit::Bool(b) => Expr::Literal(Literal::Bool(b.value)),
        syn::Lit::Str(s) => Expr::Literal(Literal::Str(s.value())),
        _ => Expr::Empty,
    }
}

fn lower_binop(op: &syn::BinOp) -> BinOp {
    match op {
        syn::BinOp::Add(_) => BinOp::Add,
        syn::BinOp::Sub(_) => BinOp::Sub,
        syn::BinOp::Mul(_) => BinOp::Mul,
        syn::BinOp::Div(_) => BinOp::Div,
        syn::BinOp::Rem(_) => BinOp::Rem,
        syn::BinOp::Eq(_) => BinOp::Eq,
        syn::BinOp::Ne(_) => BinOp::Neq,
        syn::BinOp::Lt(_) => BinOp::Lt,
        syn::BinOp::Gt(_) => BinOp::Gt,
        syn::BinOp::Le(_) => BinOp::Le,
        syn::BinOp::Ge(_) => BinOp::Ge,
        syn::BinOp::And(_) => BinOp::And,   // logical && on bools → band (0/1)
        syn::BinOp::Or(_) => BinOp::Or,     // logical || on bools → bor  (0/1)
        syn::BinOp::BitAnd(_) => BinOp::And, // FIX: was falling through to Add
        syn::BinOp::BitOr(_) => BinOp::Or,   // FIX: was falling through to Add
        syn::BinOp::BitXor(_) => BinOp::BitXor,
        syn::BinOp::Shl(_) => BinOp::Shl,
        syn::BinOp::Shr(_) => BinOp::Shr,
        _ => BinOp::Add,
    }
}

fn lower_struct(s: &syn::ItemStruct) -> StructDef {
    let fields: Vec<FieldDef> = s.fields.iter().map(|f| {
        FieldDef {
            name: f.ident.as_ref().map(|i| i.to_string()).unwrap_or_default(),
            ty: lower_type(&f.ty),
            visibility: match f.vis { syn::Visibility::Public(_) => Visibility::Public, _ => Visibility::Private },
        }
    }).collect();

    let derives: Vec<String> = s.attrs.iter()
        .filter(|a| a.path().is_ident("derive"))
        .flat_map(|a| {
            a.parse_args_with(
                syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated
            ).ok().map(|paths| paths.iter().map(|p| {
                p.segments.iter().map(|s| s.ident.to_string()).collect::<Vec<_>>().join("::")
            }).collect::<Vec<_>>()).unwrap_or_default()
        })
        .collect();

    StructDef { name: s.ident.to_string(), fields, derives }
}

/// Lower a C-like enum: assign each variant its i64 discriminant (running previous+1, overridable by an
/// explicit `= N`), mirroring lower_struct's derive extraction. Data-carrying variants are a later rung.
fn lower_enum(e: &syn::ItemEnum) -> EnumDef {
    let mut next: i64 = 0;
    let variants: Vec<EnumVariant> = e.variants.iter().map(|v| {
        if let Some((_, expr)) = &v.discriminant {
            if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(i), .. }) = expr {
                next = i.base10_parse().unwrap_or(next);
            }
        }
        let d = next;
        next += 1;
        EnumVariant { name: v.ident.to_string(), discriminant: d }
    }).collect();

    let derives: Vec<String> = e.attrs.iter()
        .filter(|a| a.path().is_ident("derive"))
        .flat_map(|a| {
            a.parse_args_with(
                syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated
            ).ok().map(|paths| paths.iter().map(|p| {
                p.segments.iter().map(|s| s.ident.to_string()).collect::<Vec<_>>().join("::")
            }).collect::<Vec<_>>()).unwrap_or_default()
        })
        .collect();

    EnumDef { name: e.ident.to_string(), variants, derives }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_function() {
        let source = r#"
fn add(a: i32, b: i32) -> i32 {
    a + b
}
"#;
        let unit = parse_source(source, "test.rs").unwrap();
        assert_eq!(unit.functions.len(), 1);
        assert_eq!(unit.functions[0].name, "add");
        assert_eq!(unit.functions[0].params.len(), 2);
    }

    #[test]
    fn test_parse_struct() {
        let source = r#"
#[derive(Debug, Clone)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}
"#;
        let unit = parse_source(source, "test.rs").unwrap();
        assert_eq!(unit.structs.len(), 1);
        assert_eq!(unit.structs[0].name, "Point");
        assert_eq!(unit.structs[0].derives.len(), 2);
    }

    #[test]
    fn test_parse_if_else() {
        let source = r#"
fn max(a: i32, b: i32) -> i32 {
    if a > b { a } else { b }
}
"#;
        let unit = parse_source(source, "test.rs").unwrap();
        assert_eq!(unit.functions.len(), 1);
    }

    #[test]
    fn test_parse_empty() {
        let source = "// just a comment\n";
        let unit = parse_source(source, "test.rs").unwrap();
        assert!(unit.functions.is_empty());
    }
}
