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
            emit_expr(cg, receiver);
            cg.push(".");
            cg.push(method);
            cg.push("(");
            emit_args(cg, args);
            cg.push(")");
        }
        Expr::FnCall {
            name,
            type_args,
            args,
            ..
        } => {
            // println!/print! are Rust macros: first arg must be a bare string
            // literal, not a `.to_string()` expression.
            if matches!(name.as_str(), "println" | "print") {
                cg.push(&format!("{name}!"));
                cg.push("(");
                emit_args_for_macro(cg, args);
                cg.push(")");
            } else {
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
            }
        }
        Expr::Unary { op, expr, .. } => {
            let op_str = match op {
                UnaryOp::Neg => "-",
                UnaryOp::Not => "!",
            };
            cg.push(op_str);
            emit_expr(cg, expr);
        }
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
            emit_block_stmts(cg, &then.stmts);
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
            cg.push("match ");
            emit_expr(cg, scrutinee);
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
            emit_block_stmts(cg, &block.stmts);
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
                    emit_expr(&mut tmp, fexpr);
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
        Literal::Str(s) => cg.push(&format!("\"{s}\".to_string()")),
        Literal::Char(c) => cg.push(&format!("'{c}'")),
        Literal::Bool(b) => cg.push(if *b { "true" } else { "false" }),
        Literal::Unit => cg.push("()"),
    }
}

// ── Arguments ─────────────────────────────────────────────────────────────

fn emit_args(cg: &mut Codegen, args: &[Expr]) {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            cg.push(", ");
        }
        emit_expr(cg, arg);
    }
}

/// Emit arguments for Rust macros like `println!` where the first argument
/// must be a bare string literal (not a `.to_string()` expression).
fn emit_args_for_macro(cg: &mut Codegen, args: &[Expr]) {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            cg.push(", ");
        }
        if i == 0 {
            // First arg: emit string literal bare, without `.to_string()`
            match arg {
                Expr::Literal(Literal::Str(s), _) => cg.push(&format!("\"{s}\"")),
                other => emit_expr(cg, other),
            }
        } else {
            emit_expr(cg, arg);
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
        Pattern::Literal(lit, _) => emit_literal(cg, lit),
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
        _ => name.to_string(),
    }
}
