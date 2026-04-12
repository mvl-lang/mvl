//! Emit Rust trait implementations from MVL [`ImplDecl`] nodes.
//!
//! Phase 1 supports:
//! - `impl Display for T` → `impl std::fmt::Display for T`
//!
//! The MVL syntax for Display is:
//! ```text
//! impl Display for Point {
//!     fn fmt(self: Point) -> String {
//!         format("({}, {})", self.x, self.y)
//!     }
//! }
//! ```
//!
//! Which transpiles to:
//! ```text
//! impl std::fmt::Display for Point {
//!     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//!         write!(f, "{}", format!("({}, {})", self.x, self.y))
//!     }
//! }
//! ```

use crate::mvl::parser::ast::{ImplDecl, Stmt};
use crate::mvl::transpiler::codegen::Codegen;
use crate::mvl::transpiler::emit_exprs::{emit_block_stmts, emit_expr};

/// Emit a trait implementation block.
pub fn emit_impl_decl(cg: &mut Codegen, id: &ImplDecl) {
    match id.trait_name.as_str() {
        "Display" => emit_display_impl(cg, id),
        other => {
            cg.line(&format!(
                "// impl {other} for {} — unsupported trait in Phase 1 (skipped)",
                id.type_name
            ));
        }
    }
}

/// Emit `impl std::fmt::Display for TypeName`.
fn emit_display_impl(cg: &mut Codegen, id: &ImplDecl) {
    cg.line(&format!("impl std::fmt::Display for {} {{", id.type_name));
    cg.push_indent();

    // Find the `fmt` method; if not present, emit a todo!()
    let fmt_method = id.methods.iter().find(|m| m.name == "fmt");

    cg.line("fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {");
    cg.push_indent();

    match fmt_method {
        Some(fd) => {
            let stmts = &fd.body.stmts;
            if stmts.is_empty() {
                cg.line("write!(f, \"\")");
            } else {
                // Emit all but the last statement (let bindings, etc.)
                let (head, tail) = stmts.split_at(stmts.len() - 1);
                emit_block_stmts(cg, head);

                // Last statement becomes the value passed to write!
                let last = &tail[0];
                cg.indent();
                cg.push("write!(f, \"{}\", ");
                match last {
                    Stmt::Expr { expr, .. } => emit_expr(cg, expr),
                    other => {
                        // Non-expression statement: emit it, then use unit
                        cg.push("{\n");
                        emit_block_stmts(cg, std::slice::from_ref(other));
                        cg.indent();
                        cg.push("}");
                    }
                }
                cg.push(")");
                cg.nl();
            }
        }
        None => {
            cg.line("todo!(\"Display::fmt not implemented\")");
        }
    }

    cg.pop_indent();
    cg.line("}");
    cg.pop_indent();
    cg.line("}");
}
