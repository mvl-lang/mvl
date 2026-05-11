// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emit Rust trait implementations from MVL [`ImplDecl`] nodes.
//!
//! Supported traits:
//! - `impl Display for T` → `impl std::fmt::Display for T`
//! - `impl From<A> for B` → `impl std::convert::From<A> for B`
//! - `impl Iterator<T> for X` → `impl std::iter::Iterator for X`
//!
//! # Spec coverage
//!
//! - **001-type-system/Req 10** (Debug and Display Traits): `Display` impls are emitted
//!   here; `Debug` is handled via `#[derive(Debug)]` in `emit_types.rs`.
//! - **001-type-system/Req 5** (Error Visibility / `From` conversions): `impl From<A> for B`
//!   enables the `?` propagation that the type checker validates per Req 5.
//!
//! See ADR-0003 for the overall compilation strategy.
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

use crate::mvl::backends::rust::emit_exprs::{emit_block_stmts, emit_expr};
use crate::mvl::backends::rust::emit_types::emit_type_expr;
use crate::mvl::backends::rust::emitter::RustEmitter;
use crate::mvl::backends::rust::last_use::compute_last_uses;
use crate::mvl::parser::ast::{ImplDecl, Stmt}; // Stmt used in match below

/// Emit a trait implementation block.
pub fn emit_impl_decl(cg: &mut RustEmitter, id: &ImplDecl) {
    // Phase A: reset last_uses before each impl block so that stale spans from the
    // preceding function body cannot bleed into branches that do not call
    // compute_last_uses (the unsupported-trait fallthrough and the Display
    // None-method branch).  Each supported impl re-sets last_uses per method body
    // immediately before emitting that body.
    cg.last_uses = Default::default();
    match id.trait_name.as_str() {
        "Display" => emit_display_impl(cg, id),
        "From" => emit_from_impl(cg, id),
        "Iterator" => emit_iterator_impl(cg, id),
        other => {
            cg.line(&format!(
                "// impl {other} for {} — unsupported trait (skipped)",
                id.type_name
            ));
        }
    }
}

/// Emit `impl std::fmt::Display for TypeName`.
fn emit_display_impl(cg: &mut RustEmitter, id: &ImplDecl) {
    cg.line(&format!("impl std::fmt::Display for {} {{", id.type_name));
    cg.push_indent();

    // Find the `fmt` method; if not present, emit a todo!()
    let fmt_method = id.methods.iter().find(|m| m.name == "fmt");

    cg.line("fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {");
    cg.push_indent();

    match fmt_method {
        Some(fd) => {
            // Phase A: last-use analysis for clone elision within the fmt method body.
            cg.last_uses = compute_last_uses(&fd.body);
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

/// Emit `impl std::iter::Iterator for TypeName` (001-type-system Req 11).
///
/// ```text
/// impl Iterator<Int> for Counter {
///     fn next(mut self) -> Option<Int> { … }
/// }
/// ```
/// transpiles to:
/// ```text
/// impl std::iter::Iterator for Counter {
///     type Item = i64;
///     fn next(&mut self) -> Option<i64> { … }
/// }
/// ```
fn emit_iterator_impl(cg: &mut RustEmitter, id: &ImplDecl) {
    let item_ty = match id.trait_type_args.first() {
        Some(ty) => emit_type_expr(ty),
        None => {
            cg.line(&format!(
                "// impl Iterator for {} — missing element type argument (skipped)",
                id.type_name
            ));
            return;
        }
    };

    cg.line(&format!("impl std::iter::Iterator for {} {{", id.type_name));
    cg.push_indent();
    cg.line(&format!("type Item = {item_ty};"));

    let next_method = id.methods.iter().find(|m| m.name == "next");

    cg.line(&format!("fn next(&mut self) -> Option<{item_ty}> {{"));
    cg.push_indent();

    match next_method {
        Some(fd) => match fd.body.stmts.split_last() {
            None => cg.line("todo!(\"Iterator::next not implemented\")"),
            Some((last, head)) => {
                // Phase A: last-use analysis for clone elision within the next method body.
                cg.last_uses = compute_last_uses(&fd.body);
                emit_block_stmts(cg, head);
                match last {
                    Stmt::Expr { expr, .. } => {
                        cg.indent();
                        emit_expr(cg, expr);
                        cg.nl();
                    }
                    other => emit_block_stmts(cg, std::slice::from_ref(other)),
                }
            }
        },
        None => cg.line("todo!(\"Iterator::next not implemented\")"),
    }

    cg.pop_indent();
    cg.line("}");
    cg.pop_indent();
    cg.line("}");
}

/// Emit `impl std::convert::From<SourceType> for TargetType`.
fn emit_from_impl(cg: &mut RustEmitter, id: &ImplDecl) {
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

    // Use the actual MVL parameter name so the emitted body can reference it.
    let param_name = from_method
        .and_then(|fd| fd.params.first())
        .map(|p| p.name.as_str())
        .unwrap_or("value");
    cg.line(&format!("fn from({param_name}: {source_ty}) -> Self {{"));
    cg.push_indent();

    match from_method {
        Some(fd) => {
            // Phase A: last-use analysis for clone elision within the from method body.
            cg.last_uses = compute_last_uses(&fd.body);
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
