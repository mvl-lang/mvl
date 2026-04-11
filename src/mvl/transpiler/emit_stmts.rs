//! Emit Rust statements from MVL [`Stmt`] nodes.

use crate::mvl::parser::ast::{ElseBranch, LValue, Stmt};
use crate::mvl::transpiler::codegen::Codegen;
use crate::mvl::transpiler::emit_exprs::{emit_block_stmts, emit_expr, emit_pattern};
use crate::mvl::transpiler::emit_types::emit_type_expr;

/// Emit a single statement (with indentation and trailing newline).
pub fn emit_stmt(cg: &mut Codegen, stmt: &Stmt) {
    match stmt {
        Stmt::Let {
            mutable,
            pattern,
            ty,
            init,
            ..
        } => {
            cg.indent();
            if *mutable {
                cg.push("let mut ");
            } else {
                cg.push("let ");
            }
            emit_pattern(cg, pattern);
            if let Some(t) = ty {
                cg.push(": ");
                cg.push(&emit_type_expr(t));
            }
            cg.push(" = ");
            emit_expr(cg, init);
            cg.push(";");
            cg.nl();
        }

        Stmt::Assign { target, value, .. } => {
            cg.indent();
            emit_lvalue(cg, target);
            cg.push(" = ");
            emit_expr(cg, value);
            cg.push(";");
            cg.nl();
        }

        Stmt::Return { value, .. } => {
            cg.indent();
            if let Some(v) = value {
                cg.push("return ");
                emit_expr(cg, v);
                cg.push(";");
            } else {
                cg.push("return;");
            }
            cg.nl();
        }

        Stmt::If {
            cond, then, else_, ..
        } => {
            cg.indent();
            cg.push("if ");
            emit_expr(cg, cond);
            cg.push(" {");
            cg.nl();
            cg.push_indent();
            emit_block_stmts(cg, &then.stmts);
            cg.pop_indent();
            cg.indent();
            cg.push("}");
            if let Some(else_branch) = else_ {
                cg.push(" else ");
                emit_else_branch(cg, else_branch);
            }
            cg.nl();
        }

        Stmt::Match {
            scrutinee, arms, ..
        } => {
            cg.indent();
            cg.push("match ");
            emit_expr(cg, scrutinee);
            cg.push(" {");
            cg.nl();
            cg.push_indent();
            for arm in arms {
                use crate::mvl::parser::ast::MatchBody;
                use crate::mvl::transpiler::emit_types::emit_ref_expr_for_assert;
                cg.indent();
                emit_pattern(cg, &arm.pattern);
                if let Some(guard) = &arm.guard {
                    cg.push(" if ");
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
                        cg.push("}");
                        cg.nl();
                    }
                }
            }
            cg.pop_indent();
            cg.indent();
            cg.push("}");
            cg.nl();
        }

        Stmt::For {
            pattern,
            iter,
            body,
            ..
        } => {
            cg.indent();
            cg.push("for ");
            emit_pattern(cg, pattern);
            cg.push(" in ");
            emit_expr(cg, iter);
            cg.push(" {");
            cg.nl();
            cg.push_indent();
            emit_block_stmts(cg, &body.stmts);
            cg.pop_indent();
            cg.indent();
            cg.push("}");
            cg.nl();
        }

        Stmt::While { cond, body, .. } => {
            cg.indent();
            cg.push("while ");
            emit_expr(cg, cond);
            cg.push(" {");
            cg.nl();
            cg.push_indent();
            emit_block_stmts(cg, &body.stmts);
            cg.pop_indent();
            cg.indent();
            cg.push("}");
            cg.nl();
        }

        Stmt::Expr { expr, .. } => {
            cg.indent();
            emit_expr(cg, expr);
            // Determine if this needs a semicolon: add one for non-block expressions
            // that are used as statements (not the implicit tail expression).
            // Phase 1: always add semicolon for safety; tail expressions in Rust
            // blocks without semicolons are handled by emit_fn_decl's body emitter.
            cg.push(";");
            cg.nl();
        }
    }
}

fn emit_lvalue(cg: &mut Codegen, lv: &LValue) {
    match lv {
        LValue::Ident(name, _) => cg.push(name),
        LValue::Field { base, field, .. } => {
            emit_lvalue(cg, base);
            cg.push(".");
            cg.push(field);
        }
    }
}

fn emit_else_branch(cg: &mut Codegen, branch: &ElseBranch) {
    match branch {
        ElseBranch::Block(block) => {
            cg.push("{");
            cg.nl();
            cg.push_indent();
            emit_block_stmts(cg, &block.stmts);
            cg.pop_indent();
            cg.indent();
            cg.push("}");
        }
        ElseBranch::If(stmt) => {
            emit_stmt(cg, stmt);
        }
    }
}
