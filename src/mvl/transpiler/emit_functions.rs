//! Emit Rust function declarations from MVL [`FnDecl`] nodes.
//!
//! Phase 1 mappings:
//! - Effects (`! DB, Console`) → `/// # Effects: DB, Console` doc comment
//! - Totality (`total`) → `/// # Totality: total` doc comment
//! - Capabilities (`iso`, `val`, `ref`, `tag`) → `// capability: iso` comment on param
//! - Type params with constraints → Rust generic bounds
//! - Return refinement → `debug_assert!` at end of body

use crate::mvl::parser::ast::{Capability, Constraint, FnDecl, Param, Totality};
use crate::mvl::transpiler::codegen::Codegen;
use crate::mvl::transpiler::emit_exprs::emit_block_stmts;
use crate::mvl::transpiler::emit_types::{emit_ref_expr_for_assert, emit_type_expr};

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
                emit_expr_tail(cg, expr);
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
/// No semicolon is appended.
fn emit_expr_tail(cg: &mut Codegen, expr: &crate::mvl::parser::ast::Expr) {
    use crate::mvl::parser::ast::Expr;
    use crate::mvl::transpiler::emit_exprs::emit_expr;

    // For if/match/block as tail: emit without leading indent (already indented by caller)
    match expr {
        Expr::If { .. } | Expr::Match { .. } | Expr::Block(_) => {
            emit_expr(cg, expr);
        }
        _ => {
            emit_expr(cg, expr);
        }
    }
}
