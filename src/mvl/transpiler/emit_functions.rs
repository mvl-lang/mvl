//! Emit Rust function declarations from MVL [`FnDecl`] nodes.
//!
//! Phase 1 mappings:
//! - Effects (`! DB, Console`) → `/// # Effects: DB, Console` doc comment
//! - Totality (`total`) → `/// # Totality: total` doc comment
//! - Capabilities (`iso`, `val`, `ref`, `tag`) → `// capability: iso` comment on param
//! - Type params with constraints → Rust generic bounds
//! - Return refinement → `debug_assert!` at end of body

use crate::mvl::parser::ast::{Capability, Constraint, Expr, FnDecl, Param, Totality, TypeExpr};
use crate::mvl::transpiler::codegen::Codegen;
use crate::mvl::transpiler::emit_exprs::{emit_block_stmts, emit_expr};
use crate::mvl::transpiler::emit_types::{emit_label, emit_ref_expr_for_assert, emit_type_expr};

pub fn emit_fn_decl(cg: &mut Codegen, fd: &FnDecl) {
    // Doc comments for MVL-specific annotations that Rust cannot express directly
    if let Some(Totality::Total) = &fd.totality {
        cg.line("/// # Totality");
        cg.line("/// This function is declared `total` in MVL: it must terminate for all inputs.");
    }
    if !fd.effects.is_empty() {
        cg.line(&format!("/// # Effects: {}", fd.effects.join(", ")));
        cg.line("/// MVL effect annotations — informational in Phase 1.");
    }

    // Function signature
    let generics = emit_generics(&fd.type_params, &fd.constraints);
    let params_str = emit_params(&fd.params);
    let ret_str = emit_type_expr(&fd.return_type);

    cg.line(&format!(
        "pub fn {}{generics}({params_str}) -> {ret_str} {{",
        fd.name
    ));
    cg.push_indent();

    // Emit body statements (all but last)
    let stmts = &fd.body.stmts;
    if stmts.is_empty() {
        cg.line("todo!(\"empty body\")");
    } else {
        // Emit all but the last statement normally
        let (head, tail) = stmts.split_at(stmts.len() - 1);
        emit_block_stmts(cg, head);

        // Last statement: if it's a bare Expr statement, emit without semicolon
        // so it becomes the implicit return value
        let last = &tail[0];
        use crate::mvl::parser::ast::Stmt;
        match last {
            Stmt::Expr { expr, .. } => {
                // Check if it's a return-like expression (if, match, block)
                // — emit as tail expression (no semicolon)
                cg.indent();
                emit_expr_tail_with_return_type(cg, expr, &fd.return_type, &fd.params);
                cg.nl();
            }
            other => emit_block_stmts(cg, std::slice::from_ref(other)),
        }
    }

    // Return refinement: emit debug_assert! before closing brace
    if let Some(pred) = &fd.return_refinement {
        let pred_str = emit_ref_expr_for_assert(pred, "_return_val");
        cg.line(&format!(
            "// return refinement: debug_assert!({pred_str}) — checked by MVL type checker"
        ));
    }

    cg.pop_indent();
    cg.line("}");
}

// ── Generics ─────────────────────────────────────────────────────────────

fn emit_generics(type_params: &[String], constraints: &[Constraint]) -> String {
    if type_params.is_empty() {
        return String::new();
    }
    // Build bounds map from constraints
    let mut bounds: std::collections::HashMap<&str, Vec<&str>> = std::collections::HashMap::new();
    for c in constraints {
        bounds.entry(&c.name).or_default().push(&c.bound);
    }

    let params: Vec<String> = type_params
        .iter()
        .map(|p| {
            let bs = bounds.get(p.as_str()).cloned().unwrap_or_default();
            if bs.is_empty() {
                p.clone()
            } else {
                format!("{p}: {}", bs.join(" + "))
            }
        })
        .collect();
    format!("<{}>", params.join(", "))
}

// ── Parameters ───────────────────────────────────────────────────────────

fn emit_params(params: &[Param]) -> String {
    params
        .iter()
        .map(|p| {
            let ty_str = emit_type_expr(&p.ty);
            // Capability annotation as a comment prefix: kept in name for now
            let cap_comment = match &p.capability {
                Some(Capability::Iso) => "/* iso */ ",
                Some(Capability::Val) => "/* val */ ",
                Some(Capability::Ref) => "/* ref */ ",
                Some(Capability::Tag) => "/* tag */ ",
                None => "",
            };
            let mut_prefix = if p.mutable { "mut " } else { "" };
            format!("{cap_comment}{mut_prefix}{}: {ty_str}", p.name)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// ── Tail expression emitter ───────────────────────────────────────────────

/// Emit an expression as the tail (implicit return) of a function body.
/// When the declared return type is a security label wrapper (e.g. `Secret<String>`),
/// literal expressions are wrapped with the appropriate constructor so that the
/// generated Rust code type-checks without manual coercions.
///
/// This handles common stub patterns like:
/// ```mvl
/// fn generate_token(id: UserId) -> Secret<String> { "token" }
/// ```
/// → `Secret("token".to_string())`
fn emit_expr_tail_with_return_type(
    cg: &mut Codegen,
    expr: &Expr,
    return_type: &TypeExpr,
    params: &[Param],
) {
    match return_type {
        TypeExpr::Labeled { label, .. } => {
            // Wrap only when the expression is a raw (unlabeled) value:
            // - literal → always raw
            // - ident that is a non-labeled parameter → raw
            if is_raw_value(expr, params) {
                let label_name = emit_label(*label);
                cg.push(&format!("{label_name}("));
                emit_expr(cg, expr);
                cg.push(")");
                return;
            }
        }
        TypeExpr::Result { ok, .. } => {
            // Ok(x) where x should be Labeled and x is a raw value: emit Ok(Label(x))
            if let TypeExpr::Labeled { label, .. } = ok.as_ref() {
                if let Expr::FnCall { name, args, .. } = expr {
                    if name == "Ok" && args.len() == 1 && is_raw_value(&args[0], params) {
                        let label_name = emit_label(*label);
                        cg.push("Ok(");
                        cg.push(&format!("{label_name}("));
                        emit_expr(cg, &args[0]);
                        cg.push("))");
                        return;
                    }
                }
            }
        }
        _ => {}
    }
    emit_expr(cg, expr);
}

/// Returns true when an expression produces a raw (non-labeled) value that needs
/// to be wrapped in a security label constructor.
///
/// - Literals are always raw.
/// - An identifier is raw when it refers to a function parameter whose declared
///   type has no security label (e.g. `f: Float` is raw; `v: Public<Float>` is not).
fn is_raw_value(expr: &Expr, params: &[Param]) -> bool {
    match expr {
        Expr::Literal(_, _) => true,
        Expr::Ident(name, _) => {
            // Check if this name is a function parameter with a non-labeled type
            params
                .iter()
                .any(|p| &p.name == name && !is_labeled_type(&p.ty))
        }
        _ => false,
    }
}

/// Returns true when the type is a direct security label wrapper (no wrapping needed).
fn is_labeled_type(ty: &TypeExpr) -> bool {
    matches!(ty, TypeExpr::Labeled { .. })
}
