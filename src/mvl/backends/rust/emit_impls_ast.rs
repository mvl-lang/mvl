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

use super::emitter::RustEmitter;
use crate::mvl::backends::rust::emit_types::emit_type_expr;
use crate::mvl::backends::rust::last_use::compute_last_uses_ast;
use crate::mvl::parser::ast::{ImplDecl, Stmt}; // Stmt used in match below

impl RustEmitter {
    /// Emit a trait implementation block.
    pub fn emit_impl_decl_ast(&mut self, id: &ImplDecl) {
        // Phase A: reset last_uses before each impl block so that stale spans from the
        // preceding function body cannot bleed into branches that do not call
        // compute_last_uses (the unsupported-trait fallthrough and the Display
        // None-method branch).  Each supported impl re-sets last_uses per method body
        // immediately before emitting that body.
        self.last_uses = Default::default();
        match id.trait_name.as_str() {
            "Display" => self.emit_display_impl_ast(id),
            "From" => self.emit_from_impl_ast(id),
            "Iterator" => self.emit_iterator_impl_ast(id),
            other => {
                self.line(&format!(
                    "// impl {other} for {} — unsupported trait (skipped)",
                    id.type_name
                ));
            }
        }
    }

    /// Emit `impl std::fmt::Display for TypeName`.
    fn emit_display_impl_ast(&mut self, id: &ImplDecl) {
        self.line(&format!("impl std::fmt::Display for {} {{", id.type_name));
        self.push_indent();

        // Find the `fmt` method.  Absence is rejected by the checker before transpilation (#990).
        let fmt_method = id.methods.iter().find(|m| m.name == "fmt");

        self.line("fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {");
        self.push_indent();

        match fmt_method {
            Some(fd) => {
                // Phase A: last-use analysis for clone elision within the fmt method body.
                self.last_uses = compute_last_uses_ast(&fd.body);
                let stmts = &fd.body.stmts;
                if stmts.is_empty() {
                    self.line("write!(f, \"\")");
                } else {
                    // Emit all but the last statement (let bindings, etc.)
                    let (head, tail) = stmts.split_at(stmts.len() - 1);
                    self.emit_block_stmts_ast(head);

                    // Last statement becomes the value passed to write!
                    let last = &tail[0];
                    self.indent();
                    self.push("write!(f, \"{}\", ");
                    match last {
                        Stmt::Expr { expr, .. } => self.emit_expr_ast(expr),
                        _non_expr => {
                            // A non-expression final statement (let, assign, etc.) cannot
                            // produce a value for write!.  MVL's block type checker ensures
                            // the tail is an Expr statement, so this arm is unreachable
                            // in well-typed programs.
                            unreachable!(
                                "Display::fmt body must end with an expression — enforced by checker"
                            );
                        }
                    }
                    self.push(")");
                    self.nl();
                }
            }
            None => {
                // Absence of `fmt` is rejected by the checker before transpilation (#990).
                unreachable!("impl Display missing `fmt` — blocked by checker (#990)");
            }
        }

        self.pop_indent();
        self.line("}");
        self.pop_indent();
        self.line("}");
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
    fn emit_iterator_impl_ast(&mut self, id: &ImplDecl) {
        let item_ty = match id.trait_type_args.first() {
            Some(ty) => emit_type_expr(ty),
            None => {
                self.line(&format!(
                    "// impl Iterator for {} — missing element type argument (skipped)",
                    id.type_name
                ));
                return;
            }
        };

        self.line(&format!("impl std::iter::Iterator for {} {{", id.type_name));
        self.push_indent();
        self.line(&format!("type Item = {item_ty};"));

        let next_method = id.methods.iter().find(|m| m.name == "next");

        self.line(&format!("fn next(&mut self) -> Option<{item_ty}> {{"));
        self.push_indent();

        match next_method {
            Some(fd) => match fd.body.stmts.split_last() {
                // Empty `next` body — caught as TypeMismatch by checker (#990).
                None => {
                    unreachable!("impl Iterator `next` has empty body — blocked by checker (#990)")
                }
                Some((last, head)) => {
                    // Phase A: last-use analysis for clone elision within the next method body.
                    self.last_uses = compute_last_uses_ast(&fd.body);
                    self.emit_block_stmts_ast(head);
                    match last {
                        Stmt::Expr { expr, .. } => {
                            self.indent();
                            self.emit_expr_ast(expr);
                            self.nl();
                        }
                        other => self.emit_block_stmts_ast(std::slice::from_ref(other)),
                    }
                }
            },
            // Absence of `next`: emit a todo!() stub so invalid code doesn't panic the transpiler.
            None => {
                self.line("todo!(\"Iterator::next not implemented\")");
            }
        }

        self.pop_indent();
        self.line("}");
        self.pop_indent();
        self.line("}");
    }

    /// Emit `impl std::convert::From<SourceType> for TargetType`.
    fn emit_from_impl_ast(&mut self, id: &ImplDecl) {
        let source_ty = match id.trait_type_args.first() {
            Some(ty) => emit_type_expr(ty),
            None => {
                self.line(&format!(
                    "// impl From for {} — missing source type argument (skipped)",
                    id.type_name
                ));
                return;
            }
        };

        self.line(&format!(
            "impl std::convert::From<{source_ty}> for {} {{",
            id.type_name
        ));
        self.push_indent();

        // Find the `from` method
        let from_method = id.methods.iter().find(|m| m.name == "from");

        // Use the actual MVL parameter name so the emitted body can reference it.
        let param_name = from_method
            .and_then(|fd| fd.params.first())
            .map(|p| p.name.as_str())
            .unwrap_or("value");
        self.line(&format!("fn from({param_name}: {source_ty}) -> Self {{"));
        self.push_indent();

        match from_method {
            Some(fd) => {
                // Phase A: last-use analysis for clone elision within the from method body.
                self.last_uses = compute_last_uses_ast(&fd.body);
                let stmts = &fd.body.stmts;
                if stmts.is_empty() {
                    // Empty body caught as TypeMismatch by checker (#990).
                    unreachable!("impl From `from` has empty body — blocked by checker (#990)");
                } else {
                    let (head, tail) = stmts.split_at(stmts.len() - 1);
                    self.emit_block_stmts_ast(head);
                    // Emit last statement as the return expression
                    let last = &tail[0];
                    match last {
                        Stmt::Expr { expr, .. } => {
                            self.indent();
                            self.emit_expr_ast(expr);
                            self.nl();
                        }
                        other => self.emit_block_stmts_ast(std::slice::from_ref(other)),
                    }
                }
            }
            None => {
                // Absence of `from`: emit a todo!() stub so invalid code doesn't panic the transpiler.
                self.line("todo!(\"From::from not implemented\")");
            }
        }

        self.pop_indent();
        self.line("}");
        self.pop_indent();
        self.line("}");
    }
}
