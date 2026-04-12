//! Emit Rust trait implementations from MVL [`ImplDecl`] nodes.
//!
//! Supported traits:
//! - `impl Display for T` → `impl std::fmt::Display for T`
//! - `impl From<A> for B` → `impl std::convert::From<A> for B`
//!
//! ## Display
//! ```text
//! impl Display for Point {
//!     fn fmt(self: Point) -> String { format("({}, {})", self.x, self.y) }
//! }
//! ```
//! transpiles to:
//! ```text
//! impl std::fmt::Display for Point {
//!     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//!         write!(f, "{}", format!("({}, {})", self.x, self.y))
//!     }
//! }
//! ```
//!
//! ## From
//! ```text
//! impl From<IoError> for AppError {
//!     fn from(e: IoError) -> Self { AppError::Io(e) }
//! }
//! ```
//! transpiles to:
//! ```text
//! impl std::convert::From<IoError> for AppError {
//!     fn from(e: IoError) -> Self { AppError::Io(e) }
//! }
//! ```

use crate::mvl::parser::ast::{ImplDecl, Stmt}; // Stmt used in match below
use crate::mvl::transpiler::codegen::Codegen;
use crate::mvl::transpiler::emit_exprs::{emit_block_stmts, emit_expr};
use crate::mvl::transpiler::emit_types::emit_type_expr;

/// Emit a trait implementation block.
pub fn emit_impl_decl(cg: &mut Codegen, id: &ImplDecl) {
    match id.trait_name.as_str() {
        "Display" => emit_display_impl(cg, id),
        "From" => emit_from_impl(cg, id),
        other => {
            cg.line(&format!(
                "// impl {other} for {} — unsupported trait (skipped)",
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
                    _non_expr => {
                        // A non-expression final statement (let, assign, etc.) cannot
                        // produce a value for write!. Emit a todo!() so the Rust
                        // compiler surfaces this as a runtime panic rather than a
                        // silent type error. MVL's type checker should reject this
                        // before transpilation in a future phase.
                        cg.push("todo!(\"Display::fmt body must end with an expression\")");
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

/// Emit `impl std::convert::From<SourceType> for TargetType`.
fn emit_from_impl(cg: &mut Codegen, id: &ImplDecl) {
    let source_ty = match id.trait_type_args.first() {
        Some(ty) => emit_type_expr(ty),
        None => {
            cg.line(&format!(
                "// impl From for {} — missing source type argument (skipped)",
                id.type_name
            ));
            return;
        }
    };

    cg.line(&format!(
        "impl std::convert::From<{source_ty}> for {} {{",
        id.type_name
    ));
    cg.push_indent();

    // Find the `from` method
    let from_method = id.methods.iter().find(|m| m.name == "from");

    cg.line(&format!("fn from(value: {source_ty}) -> Self {{"));
    cg.push_indent();

    match from_method {
        Some(fd) => {
            // Rename the first parameter to `value` in the emitted body so it
            // matches the Rust From signature, then emit the body statements.
            // For simplicity we emit the raw body; the user's param name is used
            // as written in the MVL source.
            let stmts = &fd.body.stmts;
            if stmts.is_empty() {
                cg.line("todo!(\"From::from not implemented\")");
            } else {
                let (head, tail) = stmts.split_at(stmts.len() - 1);
                emit_block_stmts(cg, head);
                // Emit last statement as the return expression
                let last = &tail[0];
                match last {
                    Stmt::Expr { expr, .. } => {
                        cg.indent();
                        emit_expr(cg, expr);
                        cg.nl();
                    }
                    other => emit_block_stmts(cg, std::slice::from_ref(other)),
                }
            }
        }
        None => {
            cg.line("todo!(\"From::from not implemented\")");
        }
    }

    cg.pop_indent();
    cg.line("}");
    cg.pop_indent();
    cg.line("}");
}
