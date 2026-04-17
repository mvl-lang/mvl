//! Emit Rust expressions from MVL [`Expr`] nodes.

use crate::mvl::parser::ast::{BinaryOp, Expr, Literal, MatchArm, MatchBody, Pattern, UnaryOp};
use crate::mvl::transpiler::codegen::Codegen;
use crate::mvl::transpiler::emit_types::emit_type_expr;

/// Emit an expression into the code buffer (no trailing newline).
pub fn emit_expr(cg: &mut Codegen, expr: &Expr) {
    match expr {
        Expr::Literal(lit, _) => emit_literal(cg, lit),
        Expr::Ident(name, _) => cg.push(&map_ident(name)),
        Expr::FieldAccess { expr, field, .. } => {
            emit_expr(cg, expr);
            cg.push(".");
            cg.push(field);
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            // Methods that don't map directly to a Rust method of the same name.
            match method.as_str() {
                // xs.slice(start, end) — clamps negative indices to 0, OOB end to len,
                // inverted range (start > end) returns empty. Never panics.
                "slice" if args.len() == 2 => {
                    emit_safe_list_slice(cg, receiver, &args[0], &args[1]);
                }
                // s.substring(start, end) — char-based (UTF-8 safe), clamps negatives,
                // inverted range returns empty string. Never panics.
                "substring" if args.len() == 2 => {
                    emit_safe_substring(cg, receiver, &args[0], &args[1]);
                }
                // take(n)/skip(n) — List<T> slice methods that need iterator adapter in Rust
                "take" | "skip" => {
                    emit_expr(cg, receiver);
                    cg.push(".into_iter().");
                    cg.push(method);
                    cg.push("(");
                    emit_args(cg, args);
                    cg.push(").collect::<Vec<_>>()");
                }
                // take_while(f)/skip_while(f) — Rust iterator expects &T, MVL predicate takes T
                "take_while" | "skip_while" => {
                    emit_expr(cg, receiver);
                    cg.push(".into_iter().");
                    cg.push(method);
                    cg.push("(|__x| ");
                    if let Some(arg) = args.first() {
                        emit_expr(cg, arg);
                    }
                    cg.push("(__x.clone())).collect::<Vec<_>>()");
                }
                // windows(n)/chunks(n) — Rust returns &[T] slices, collect into Vec<Vec<T>>
                "windows" | "chunks" => {
                    emit_expr(cg, receiver);
                    cg.push(".");
                    cg.push(method);
                    cg.push("(");
                    emit_args(cg, args);
                    cg.push(").map(|w| w.to_vec()).collect::<Vec<_>>()");
                }
                // flatten() — List<List<T>> → List<T>
                "flatten" => {
                    emit_expr(cg, receiver);
                    cg.push(".into_iter().flatten().collect::<Vec<_>>()");
                }
                // partition(f) — Rust iterator expects &T predicate, MVL takes T
                // Use turbofish ::<Vec<_>, _> so Rust can infer the element type
                "partition" => {
                    emit_expr(cg, receiver);
                    cg.push(".into_iter().partition::<Vec<_>, _>(|__x| ");
                    if let Some(arg) = args.first() {
                        emit_expr(cg, arg);
                    }
                    cg.push("(__x.clone()))");
                }
                // group_by(f: fn(T) -> K) — no native Rust equivalent; fold into HashMap
                "group_by" => {
                    cg.push("{ let mut __m = std::collections::HashMap::new(); for __v in ");
                    emit_expr(cg, receiver);
                    cg.push(".into_iter() { __m.entry(");
                    if let Some(arg) = args.first() {
                        emit_expr(cg, arg);
                    }
                    cg.push("(__v.clone())).or_insert_with(Vec::new).push(__v); } __m }");
                }
                // chars() — String::chars() returns Chars<'a> in Rust, not Vec<String>
                "chars" => {
                    emit_expr(cg, receiver);
                    cg.push(".chars().map(|__c| __c.to_string()).collect::<Vec<_>>()");
                }
                // first()/last() — Vec::first/last return Option<&T>; MVL expects Option<T>
                "first" | "last" => {
                    emit_expr(cg, receiver);
                    cg.push(".");
                    cg.push(method);
                    cg.push("().cloned()");
                }
                // contains(x) — slice::contains takes &T, so borrow the argument
                "contains" => {
                    emit_expr(cg, receiver);
                    cg.push(".contains(&(");
                    emit_args(cg, args);
                    cg.push("))");
                }
                _ => {
                    emit_expr(cg, receiver);
                    cg.push(".");
                    cg.push(method);
                    cg.push("(");
                    emit_args(cg, args);
                    cg.push(")");
                }
            }
        }
        Expr::FnCall {
            name,
            type_args,
            args,
            ..
        } => {
            // println!/print! are Rust macros: first arg must be a bare string
            // literal, not a `.to_string()` expression.
            if matches!(name.as_str(), "println" | "print" | "format") {
                cg.push(&format!("{name}!"));
                cg.push("(");
                emit_args_for_macro(cg, args);
                cg.push(")");
            } else if try_emit_special_fn(cg, name, args) {
                // Handled by special-case emitter (e.g. range)
            } else {
                let is_extern = cg.extern_fns.contains(name.as_str());
                if is_extern {
                    cg.push("unsafe { ");
                }
                cg.push(&map_fn_name(name));
                if !type_args.is_empty() {
                    cg.push("::<");
                    let strs: Vec<String> = type_args.iter().map(emit_type_expr).collect();
                    cg.push(&strs.join(", "));
                    cg.push(">");
                }
                cg.push("(");
                emit_args(cg, args);
                cg.push(")");
                if is_extern {
                    cg.push(" }");
                }
            }
        }
        Expr::Unary { op, expr, .. } => match op {
            UnaryOp::Neg => {
                cg.push("-");
                emit_expr(cg, expr);
            }
            UnaryOp::Not => {
                cg.push("!");
                emit_expr(cg, expr);
            }
            UnaryOp::Deref => {
                cg.push("*(");
                emit_expr(cg, expr);
                cg.push(")");
            }
        },
        Expr::Binary {
            op, left, right, ..
        } => {
            cg.push("(");
            emit_expr(cg, left);
            cg.push(" ");
            cg.push(emit_binary_op(*op));
            cg.push(" ");
            emit_expr(cg, right);
            cg.push(")");
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            cg.push("if ");
            emit_expr(cg, cond);
            cg.push(" {");
            cg.nl();
            cg.push_indent();
            emit_block_as_value(cg, &then.stmts);
            cg.pop_indent();
            cg.indent();
            cg.push("}");
            if let Some(else_expr) = else_ {
                cg.push(" else ");
                emit_expr(cg, else_expr);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            let has_str_pattern = arms_have_str_pattern(arms);
            cg.push("match ");
            emit_expr(cg, scrutinee);
            if has_str_pattern {
                cg.push(".as_str()");
            }
            cg.push(" {");
            cg.nl();
            cg.push_indent();
            for arm in arms {
                emit_match_arm(cg, arm);
            }
            cg.pop_indent();
            cg.indent();
            cg.push("}");
        }
        Expr::Block(block) => {
            cg.push("{");
            cg.nl();
            cg.push_indent();
            emit_block_as_value(cg, &block.stmts);
            cg.pop_indent();
            cg.indent();
            cg.push("}");
        }
        Expr::Propagate { expr, .. } => {
            emit_expr(cg, expr);
            cg.push("?");
        }
        Expr::Construct { name, fields, .. } => {
            cg.push(name);
            cg.push(" { ");
            let parts: Vec<String> = fields
                .iter()
                .map(|(fname, fexpr)| {
                    let mut tmp = Codegen::new();
                    tmp.push(&format!("{fname}: "));
                    // Clone field values: placing a value into a struct field is a move
                    // in Rust. MVL value semantics require the source binding to remain
                    // valid. Spec 009 Req 2: clone ALL non-Copy arguments.
                    emit_expr_as_arg(&mut tmp, fexpr);
                    tmp.finish()
                })
                .collect();
            cg.push(&parts.join(", "));
            cg.push(" }");
        }
        Expr::List { elems, .. } => {
            cg.push("vec![");
            emit_args(cg, elems);
            cg.push("]");
        }
        Expr::Map { pairs, .. } => {
            cg.push("std::collections::HashMap::from([");
            let pair_strs: Vec<String> = pairs
                .iter()
                .map(|(k, v)| {
                    let mut tmp = Codegen::new();
                    tmp.push("(");
                    emit_expr(&mut tmp, k);
                    tmp.push(", ");
                    emit_expr(&mut tmp, v);
                    tmp.push(")");
                    tmp.finish()
                })
                .collect();
            cg.push(&pair_strs.join(", "));
            cg.push("])");
        }
        Expr::Set { elems, .. } => {
            cg.push("std::collections::HashSet::from([");
            emit_args(cg, elems);
            cg.push("])");
        }
        Expr::Move { expr, .. } => {
            // `move` in MVL means transfer ownership — Rust does this implicitly
            emit_expr(cg, expr);
        }
        Expr::Consume { expr, .. } => {
            // `consume` mirrors Pony's `consume` for iso; just emit the inner expr in Phase 1
            emit_expr(cg, expr);
        }
        Expr::Declassify { expr, .. } => {
            cg.push("declassify(");
            emit_expr(cg, expr);
            cg.push(")");
        }
        Expr::Sanitize { expr, .. } => {
            cg.push("sanitize(");
            emit_expr(cg, expr);
            cg.push(")");
        }
        Expr::Lambda {
            params,
            ret_type,
            body,
            ..
        } => {
            cg.push("|");
            let param_strs: Vec<String> = params
                .iter()
                .map(|p| {
                    let ty_str = emit_type_expr(&p.ty);
                    format!("{}: {ty_str}", p.name)
                })
                .collect();
            cg.push(&param_strs.join(", "));
            cg.push("|");
            if let Some(ret) = ret_type {
                cg.push(" -> ");
                cg.push(&emit_type_expr(ret));
            }
            cg.push(" ");
            emit_expr(cg, body);
        }
    }
}

// ── Literal ───────────────────────────────────────────────────────────────

/// Re-escape a decoded string value for insertion into a Rust string literal.
fn escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            other => out.push(other),
        }
    }
    out
}

/// Re-escape a decoded char value for insertion into a Rust char literal.
fn escape_char(c: char) -> String {
    match c {
        '\\' => "\\\\".to_string(),
        '\'' => "\\'".to_string(),
        '\n' => "\\n".to_string(),
        '\t' => "\\t".to_string(),
        '\r' => "\\r".to_string(),
        '\0' => "\\0".to_string(),
        other => other.to_string(),
    }
}

fn emit_literal(cg: &mut Codegen, lit: &Literal) {
    match lit {
        Literal::Integer(n) => cg.push(&n.to_string()),
        Literal::Float(f) => {
            // Ensure float literals have a decimal point in Rust
            let s = format!("{f}");
            if s.contains('.') || s.contains('e') {
                cg.push(&s);
            } else {
                cg.push(&format!("{s}.0"));
            }
        }
        Literal::Str(s) => cg.push(&format!("\"{}\".to_string()", escape_str(s))),
        Literal::Char(c) => cg.push(&format!("'{}'", escape_char(*c))),
        Literal::Bool(b) => cg.push(if *b { "true" } else { "false" }),
        Literal::Unit => cg.push("()"),
    }
}

/// Returns true if any match arm uses a string literal pattern.
///
/// When true, the scrutinee must be coerced to `&str` via `.as_str()` so that
/// Rust's pattern matching works (both `String` and IFC-labeled strings expose
/// `.as_str()`).  Called from both `Expr::Match` and `Stmt::Match` codegen.
pub fn arms_have_str_pattern(arms: &[MatchArm]) -> bool {
    arms.iter()
        .any(|a| matches!(&a.pattern, Pattern::Literal(Literal::Str(_), _)))
}

/// Emit a literal in pattern position.  String literals must be bare `"s"`
/// (not `"s".to_string()`) because Rust patterns cannot contain method calls.
fn emit_literal_in_pattern(cg: &mut Codegen, lit: &Literal) {
    match lit {
        Literal::Str(s) => cg.push(&format!("\"{}\"", escape_str(s))),
        other => emit_literal(cg, other),
    }
}

// ── Arguments ─────────────────────────────────────────────────────────────

fn emit_args(cg: &mut Codegen, args: &[Expr]) {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            cg.push(", ");
        }
        emit_expr_as_arg(cg, arg);
    }
}

/// Emit an expression in function-argument position.
///
/// MVL has value semantics: passing a value to a function is a copy, not a
/// move.  In Rust, non-Copy types (structs, enums, Vec, String) are moved by
/// default.  We insert `.clone()` for identifiers and field accesses so the
/// caller retains ownership after the call, matching MVL semantics.
/// `.clone()` on Copy types (i64, bool, char) is a no-op and is optimised
/// away by the compiler.
///
/// String literals get `.to_string().into()` for label type coercion.
fn emit_expr_as_arg(cg: &mut Codegen, expr: &Expr) {
    match expr {
        Expr::Literal(Literal::Str(s), _) => {
            cg.push(&format!("\"{}\".to_string().into()", escape_str(s)));
        }
        // Identifiers and field accesses may be non-Copy user types.
        // Clone so the caller retains ownership (MVL value semantics).
        Expr::Ident(_, _) | Expr::FieldAccess { .. } => {
            emit_expr(cg, expr);
            cg.push(".clone()");
        }
        _ => {
            // Temporaries (function call results, struct literals, block expressions)
            // are rvalues that Rust moves into the callee, so `.clone()` is technically
            // redundant here. However, we clone unconditionally per Spec 009 Req 2
            // "Phase 1: clone ALL non-Copy arguments" — LLVM removes redundant clones.
            emit_expr(cg, expr);
            cg.push(".clone()");
        }
    }
}

/// Emit arguments for Rust macros like `println!` where the first argument
/// must be a bare string literal (not a `.to_string()` expression).
fn emit_args_for_macro(cg: &mut Codegen, args: &[Expr]) {
    if args.is_empty() {
        return;
    }
    match &args[0] {
        Expr::Literal(Literal::Str(s), _) => {
            // First arg is a string literal: emit bare, then remaining args as values.
            cg.push(&format!("\"{}\"", escape_str(s)));
            for arg in &args[1..] {
                cg.push(", ");
                emit_expr_as_arg(cg, arg);
            }
        }
        _ => {
            // First arg is not a string literal: generate one "{}" placeholder per
            // argument so the format string matches the argument count (#198).
            let placeholders = vec!["{}"; args.len()].join(" ");
            cg.push(&format!("\"{placeholders}\""));
            for arg in args {
                cg.push(", ");
                emit_expr_as_arg(cg, arg);
            }
        }
    }
}

// ── Binary operators ──────────────────────────────────────────────────────

fn emit_binary_op(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::Gt => ">",
        BinaryOp::Le => "<=",
        BinaryOp::Ge => ">=",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
    }
}

// ── Match arms ────────────────────────────────────────────────────────────

fn emit_match_arm(cg: &mut Codegen, arm: &MatchArm) {
    cg.indent();
    emit_pattern(cg, &arm.pattern);
    if let Some(guard) = &arm.guard {
        cg.push(" if ");
        // Reuse ref_expr emitter — guard uses the same predicate language
        use crate::mvl::transpiler::emit_types::emit_ref_expr_for_assert;
        cg.push(&emit_ref_expr_for_assert(guard, "_"));
    }
    cg.push(" => ");
    match &arm.body {
        MatchBody::Expr(e) => {
            emit_expr(cg, e);
            cg.push(",");
            cg.nl();
        }
        MatchBody::Block(block) => {
            cg.push("{");
            cg.nl();
            cg.push_indent();
            emit_block_stmts(cg, &block.stmts);
            cg.pop_indent();
            cg.indent();
            cg.push("},");
            cg.nl();
        }
    }
}

// ── Patterns ─────────────────────────────────────────────────────────────

pub fn emit_pattern(cg: &mut Codegen, pat: &Pattern) {
    match pat {
        Pattern::Wildcard(_) => cg.push("_"),
        Pattern::Ident(name, _) => cg.push(&map_ident(name)),
        Pattern::Literal(lit, _) => emit_literal_in_pattern(cg, lit),
        Pattern::Tuple { elems, .. } => {
            cg.push("(");
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    cg.push(", ");
                }
                emit_pattern(cg, e);
            }
            cg.push(")");
        }
        Pattern::TupleStruct { name, fields, .. } => {
            cg.push(name);
            cg.push("(");
            for (i, f) in fields.iter().enumerate() {
                if i > 0 {
                    cg.push(", ");
                }
                emit_pattern(cg, f);
            }
            cg.push(")");
        }
        Pattern::Struct { name, fields, .. } => {
            cg.push(name);
            cg.push(" { ");
            for (i, (fname, fpat)) in fields.iter().enumerate() {
                if i > 0 {
                    cg.push(", ");
                }
                cg.push(fname);
                cg.push(": ");
                emit_pattern(cg, fpat);
            }
            cg.push(" }");
        }
        Pattern::Some { inner, .. } => {
            cg.push("Some(");
            emit_pattern(cg, inner);
            cg.push(")");
        }
        Pattern::None(_) => cg.push("None"),
        Pattern::Ok { inner, .. } => {
            cg.push("Ok(");
            emit_pattern(cg, inner);
            cg.push(")");
        }
        Pattern::Err { inner, .. } => {
            cg.push("Err(");
            emit_pattern(cg, inner);
            cg.push(")");
        }
    }
}

// ── Block statements (used in if/match body/function body) ────────────────

pub fn emit_block_stmts(cg: &mut Codegen, stmts: &[crate::mvl::parser::ast::Stmt]) {
    use crate::mvl::transpiler::emit_stmts::emit_stmt;
    for stmt in stmts {
        emit_stmt(cg, stmt);
    }
}

/// Emit block statements where the final `Stmt::Expr` is a tail expression
/// (no semicolon), so it becomes the implicit return value of the block.
pub fn emit_block_as_value(cg: &mut Codegen, stmts: &[crate::mvl::parser::ast::Stmt]) {
    use crate::mvl::parser::ast::Stmt;
    use crate::mvl::transpiler::emit_stmts::emit_stmt;
    if stmts.is_empty() {
        return;
    }
    let (head, tail) = stmts.split_at(stmts.len() - 1);
    for stmt in head {
        emit_stmt(cg, stmt);
    }
    match &tail[0] {
        Stmt::Expr { expr, .. } => {
            cg.indent();
            emit_expr(cg, expr);
            cg.nl();
        }
        other => emit_stmt(cg, other),
    }
}

// ── Name mappings ─────────────────────────────────────────────────────────

fn map_ident(name: &str) -> String {
    // MVL `self` inside refinements → Rust parameter name is substituted upstream;
    // as an expression ident, pass through as-is
    name.to_string()
}

fn map_fn_name(name: &str) -> String {
    // Built-in MVL functions mapped to Rust / stdlib equivalents
    match name {
        "println" => "println!".to_string(),
        "assert" => "assert!".to_string(),
        "assert_eq" => "assert_eq!".to_string(),
        "assert_ne" => "assert_ne!".to_string(),
        _ => name.to_string(),
    }
}

/// Emit `xs.slice(start, end)` as a safe Rust block expression.
///
/// Semantics:
/// - Negative indices are clamped to 0.
/// - Out-of-bounds `start` or `end` are clamped to `xs.len()`.
/// - Inverted range (`start > end` after clamping) returns an empty slice.
/// - Never panics.
///
/// Emits: `{let _mvl_r=&(xs);
///          let _mvl_s=((start).max(0)as usize).min(_mvl_r.len());
///          let _mvl_e=((end).max(0)as usize).min(_mvl_r.len()).max(_mvl_s);
///          _mvl_r[_mvl_s.._mvl_e].to_vec()}`
fn emit_safe_list_slice(cg: &mut Codegen, receiver: &Expr, start: &Expr, end: &Expr) {
    cg.push("{let _mvl_r=&(");
    emit_expr(cg, receiver);
    cg.push(");let _mvl_s=((");
    emit_expr(cg, start);
    cg.push(").max(0)as usize).min(_mvl_r.len());let _mvl_e=((");
    emit_expr(cg, end);
    cg.push(").max(0)as usize).min(_mvl_r.len()).max(_mvl_s);_mvl_r[_mvl_s.._mvl_e].to_vec()}");
}

/// Emit `s.substring(start, end)` as a safe Rust block expression.
///
/// Semantics:
/// - Character-based (not byte-based) — safe for multi-byte UTF-8 strings.
/// - Negative indices are clamped to 0.
/// - Out-of-bounds `end` is handled naturally by `.take()` (iterator exhaustion).
/// - Inverted range (`end < start` after clamping) returns an empty string via
///   `saturating_sub`.
/// - Never panics.
///
/// Emits: `{let _mvl_s=&*(s);let _mvl_a=(start).max(0)as usize;
///          let _mvl_b=(end).max(0)as usize;
///          _mvl_s.chars().skip(_mvl_a).take(_mvl_b.saturating_sub(_mvl_a)).collect::<String>()}`
fn emit_safe_substring(cg: &mut Codegen, receiver: &Expr, start: &Expr, end: &Expr) {
    cg.push("{let _mvl_s=&*(");
    emit_expr(cg, receiver);
    cg.push(");let _mvl_a=(");
    emit_expr(cg, start);
    cg.push(").max(0)as usize;let _mvl_b=(");
    emit_expr(cg, end);
    cg.push(").max(0)as usize;_mvl_s.chars().skip(_mvl_a).take(_mvl_b.saturating_sub(_mvl_a)).collect::<String>()}");
}

/// Emit a free function call, handling special built-ins that require custom Rust output.
/// Returns true if the call was handled specially (caller should not emit further).
fn try_emit_special_fn(_cg: &mut Codegen, _name: &str, _args: &[Expr]) -> bool {
    false
}
