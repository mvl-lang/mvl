// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Function-declaration parser (Requirement 4).
//!
//! Parses:
//! - `[total|partial] fn Name [[TypeParams]] (params) -> ReturnType [! Effects] [where Constraints] { body }`
//! - `builtin fn Name [[TypeParams]] (params) -> ReturnType [! Effects]` — runtime-provided, no body
//! - `test fn Name() -> Unit { body }` — unit test function
//! - Parameters with optional capability (`iso`/`val`/`ref`/`tag`), `mut`, type, and refinement
//! - Totality annotations, effect lists, and where-clause constraints

use crate::mvl::parser::ast::{
    ActorDecl, ActorMethod, BinaryOp, Constraint, EffectDecl, Expr, ExternDecl, ExternFnDecl,
    FnDecl, ImplDecl, LabelDecl, Literal, MailboxConfig, MailboxPolicy, Param, RefExpr,
    RelabelDecl, Totality, UnaryOp, UseDecl,
};
use crate::mvl::parser::lexer::TokenKind;
use crate::mvl::parser::{ParseError, Parser};

/// Convert a `RefExpr` back to an `Expr` for storage in the widened AST (#983).
/// Note: Currently unused; RefExpr quantifiers are wrapped directly as `Expr::Quantifier`.
/// Kept for potential future use in other contract refinements.
#[allow(dead_code)]
fn ref_expr_to_expr(re: &RefExpr) -> Expr {
    use crate::mvl::parser::ast::{ArithOp, CmpOp, LogicOp};
    match re {
        RefExpr::Ident { name, span } => Expr::Ident(name.clone(), *span),
        RefExpr::Integer { value, span } => Expr::Literal(Literal::Integer(*value), *span),
        RefExpr::Float { value, span } => Expr::Literal(Literal::Float(*value), *span),
        RefExpr::Compare {
            op,
            left,
            right,
            span,
        } => {
            let bop = match op {
                CmpOp::Lt => BinaryOp::Lt,
                CmpOp::Gt => BinaryOp::Gt,
                CmpOp::Le => BinaryOp::Le,
                CmpOp::Ge => BinaryOp::Ge,
                CmpOp::Eq => BinaryOp::Eq,
                CmpOp::Ne => BinaryOp::Ne,
            };
            Expr::Binary {
                op: bop,
                left: Box::new(ref_expr_to_expr(left)),
                right: Box::new(ref_expr_to_expr(right)),
                span: *span,
            }
        }
        RefExpr::LogicOp {
            op,
            left,
            right,
            span,
        } => {
            let bop = match op {
                LogicOp::And => BinaryOp::And,
                LogicOp::Or => BinaryOp::Or,
            };
            Expr::Binary {
                op: bop,
                left: Box::new(ref_expr_to_expr(left)),
                right: Box::new(ref_expr_to_expr(right)),
                span: *span,
            }
        }
        RefExpr::ArithOp {
            op,
            left,
            right,
            span,
        } => {
            let bop = match op {
                ArithOp::Add => BinaryOp::Add,
                ArithOp::Sub => BinaryOp::Sub,
                ArithOp::Mul => BinaryOp::Mul,
                ArithOp::Div => BinaryOp::Div,
                ArithOp::Rem => BinaryOp::Rem,
            };
            Expr::Binary {
                op: bop,
                left: Box::new(ref_expr_to_expr(left)),
                right: Box::new(ref_expr_to_expr(right)),
                span: *span,
            }
        }
        RefExpr::Not { inner, span } => Expr::Unary {
            op: UnaryOp::Not,
            expr: Box::new(ref_expr_to_expr(inner)),
            span: *span,
        },
        RefExpr::Grouped { inner, .. } => ref_expr_to_expr(inner),
        RefExpr::Len { ident, span } => {
            // len(x) → x.len() as a method call
            Expr::MethodCall {
                receiver: Box::new(Expr::Ident(ident.clone(), *span)),
                method: "len".to_string(),
                args: vec![],
                span: *span,
            }
        }
        RefExpr::FieldAccess {
            object,
            field,
            span,
        } => Expr::FieldAccess {
            expr: Box::new(ref_expr_to_expr(object)),
            field: field.clone(),
            span: *span,
        },
        // Old/Forall/Exists: preserve as-is by wrapping in a dummy.
        // The checker's expr_to_ref_expr_ext will handle these via fallback.
        RefExpr::Old { inner, .. } => ref_expr_to_expr(inner),
        RefExpr::Forall { .. } | RefExpr::Exists { .. } => {
            // These cannot be cleanly represented as Expr; use an Ident placeholder
            // that expr_to_ref_expr_ext won't match, triggering RuntimeCheck.
            let span = match re {
                RefExpr::Forall { span, .. } | RefExpr::Exists { span, .. } => *span,
                _ => unreachable!(),
            };
            Expr::Ident("__quantifier_placeholder".to_string(), span)
        }
    }
}

impl Parser {
    // ── Function declarations ─────────────────────────────────────────────

    /// Parse `[test] [total|partial|builtin] fn Name …`.
    /// Pre-condition: current token is `test`, `total`, `partial`, `builtin`, or `fn`.
    pub fn parse_fn_decl(&mut self) -> Result<FnDecl, ()> {
        let start = self.peek_span();

        // Optional `test` marker
        let is_test = if *self.peek_kind() == TokenKind::Test {
            self.advance();
            true
        } else {
            false
        };

        // Optional totality annotation
        let totality = match self.peek_kind() {
            TokenKind::Total => {
                self.advance();
                Some(Totality::Total)
            }
            TokenKind::Partial => {
                self.advance();
                Some(Totality::Partial)
            }
            _ => None,
        };

        // Optional `builtin` marker — mutually exclusive with totality and test
        let is_builtin = if *self.peek_kind() == TokenKind::Builtin {
            if totality.is_some() {
                let err = ParseError {
                    message: "`builtin` cannot be combined with `total` or `partial`".into(),
                    span: self.peek_span(),
                };
                self.push_recover(err);
                return Err(());
            }
            if is_test {
                let err = ParseError {
                    message: "`builtin` cannot be combined with `test`".into(),
                    span: self.peek_span(),
                };
                self.push_recover(err);
                return Err(());
            }
            self.advance();
            true
        } else {
            false
        };

        // `fn` keyword
        let fn_kw = self.expect(&TokenKind::Fn);
        self.require(fn_kw)?;

        // Function name — may be `TypeName[T]::method` for type-attached methods (#868).
        let ident_result = self.expect_ident();
        let (first_name, _) = self.require(ident_result)?;

        // #928: Receiver type args come between the type name and `::`, e.g.
        // `fn Option[T]::is_some(self)` or `fn List[List[T]]::flatten(self)`.
        // Speculatively consume `[T, ...]` as type expressions; if `::` follows,
        // they are receiver type args. Otherwise rewind (they are fn-level
        // generic type params parsed later).
        let receiver_type_args: Vec<crate::mvl::parser::ast::TypeExpr> =
            if *self.peek_kind() == TokenKind::LBracket {
                let saved_pos = self.pos;
                let saved_last_span = self.last_span;
                let saved_errors_len = self.errors.len();
                // Parse as type expressions to support nested types like List[T].
                self.advance(); // consume `[`
                let mut args = Vec::new();
                while !matches!(self.peek_kind(), TokenKind::RBracket | TokenKind::Eof) {
                    if let Ok(te) = self.parse_type_expr() {
                        args.push(te);
                    } else {
                        break;
                    }
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                let _ = self.eat(&TokenKind::RBracket);
                if *self.peek_kind() == TokenKind::ColonColon {
                    // Confirmed: these are receiver type args.
                    self.errors.truncate(saved_errors_len);
                    args
                } else {
                    // Not a receiver — rewind.
                    self.pos = saved_pos;
                    self.last_span = saved_last_span;
                    self.errors.truncate(saved_errors_len);
                    vec![]
                }
            } else {
                vec![]
            };

        let (receiver_type, name) = if self.eat(&TokenKind::ColonColon) {
            // `fn TypeName[T]::method_name(…)` — extract receiver type and method name.
            let method_result = self.expect_ident();
            let (method_name, _) = self.require(method_result)?;
            (Some(first_name), method_name)
        } else {
            (None, first_name)
        };

        // Optional generic type parameters (for the function itself, not the receiver).
        // Receiver type params (e.g. `T` from `List[T]::push`) are prepended so the
        // emitted Rust function has all generic params in scope.  Concrete type args
        // (e.g. `String` in `List[String]::join`) are NOT added.
        let fn_type_params = self.parse_type_params_decl();
        let type_params = {
            use crate::mvl::parser::ast::GenericParam;
            const CONCRETE_TYPES: &[&str] = &[
                "String", "Int", "Float", "Bool", "Byte", "UByte", "UInt", "Unit", "List", "Map",
                "Set", "Option", "Result",
            ];
            let mut all = Vec::new();
            // Recursively collect type variables from receiver type args.
            // e.g. `List[List[T]]::flatten` has receiver_type_args = [List[T]];
            // `List` is concrete (skipped), but `T` inside it is a type variable.
            fn collect_type_vars(
                te: &crate::mvl::parser::ast::TypeExpr,
                concrete: &[&str],
                fn_params: &[GenericParam],
                out: &mut Vec<GenericParam>,
            ) {
                if let crate::mvl::parser::ast::TypeExpr::Base { name, args, .. } = te {
                    if !concrete.contains(&name.as_str())
                        && !fn_params.iter().any(|gp| gp.name() == name)
                        && !out.iter().any(|gp| gp.name() == name)
                    {
                        out.push(GenericParam::Type(name.clone()));
                    }
                    // Recurse into nested type args (e.g. T inside List[T]).
                    for arg in args {
                        collect_type_vars(arg, concrete, fn_params, out);
                    }
                }
            }
            for rta in &receiver_type_args {
                collect_type_vars(rta, CONCRETE_TYPES, &fn_type_params, &mut all);
            }
            all.extend(fn_type_params);
            all
        };

        // Parameter list — passes receiver type (and its type args) so `self`
        // can be synthesised with the correct generic type.
        let params =
            self.parse_param_list_with_receiver(receiver_type.as_deref(), &receiver_type_args)?;

        // `-> return_type`
        let arrow = self.expect(&TokenKind::Arrow);
        self.require(arrow)?;
        let return_type = self.parse_type_expr()?;

        // Optional inline return refinement: `-> T where pred` (before effects)
        // We parse it only if `where` follows AND the next token after is NOT an ident `:`
        // (which would indicate function-level constraints, not type refinement).
        let return_refinement = self.try_parse_return_refinement();

        // Optional effect list: `! Effect + Effect`
        let effects = self.parse_optional_effects();

        // Optional where-clause constraints: `where T: Trait, U: Trait`
        let constraints = self.parse_where_constraints();

        // Optional contract clauses: `requires pred` / `ensures pred`
        // Uses parse_expr() instead of parse_ref_expr() (#983) so complex expressions
        // (method calls, etc.) produce a hard parse error instead of being silently dropped.
        // Falls back to parse_ref_expr() for quantifiers (forall/exists) which are
        // RefExpr-only constructs not supported by the main expression parser.
        let mut requires = Vec::new();
        let mut ensures = Vec::new();
        loop {
            match self.peek_kind() {
                TokenKind::Requires => {
                    self.advance();
                    requires.push(self.parse_contract_expr()?);
                }
                TokenKind::Ensures => {
                    self.advance();
                    ensures.push(self.parse_contract_expr()?);
                }
                _ => break,
            }
        }

        // Body block: required for normal functions, forbidden for builtin functions.
        let body = if is_builtin {
            if matches!(self.peek_kind(), TokenKind::LBrace) {
                let err = ParseError {
                    message: "`builtin` functions may not have a body".into(),
                    span: self.peek_span(),
                };
                self.push_recover(err);
                return Err(());
            }
            // Use an empty block to represent the absent body.
            crate::mvl::parser::ast::Block {
                stmts: vec![],
                span: self.peek_span(),
            }
        } else {
            self.parse_block()?
        };

        let span = self.span_from(start);
        Ok(FnDecl {
            visible: false, // set by parse_decl when `pub` prefix is present
            is_test,
            is_builtin,
            totality,
            receiver_type,
            name,
            type_params,
            params,
            return_type: Box::new(return_type),
            return_refinement,
            effects,
            constraints,
            requires,
            ensures,
            body,
            span,
        })
    }

    // ── Parameter list ────────────────────────────────────────────────────

    fn parse_param_list(&mut self) -> Result<Vec<Param>, ()> {
        self.parse_param_list_with_receiver(None, &[])
    }

    /// Parse a parameter list, optionally synthesising a `self` receiver param.
    ///
    /// When `receiver_type` is `Some("T")` and the first parameter is the bare
    /// identifier `self` (no capability, no `: type`), a `Param { name: "self",
    /// ty: T, … }` is synthesised automatically rather than requiring the author
    /// to write `self: T`.
    ///
    /// `receiver_type_args` carries the type arguments from the receiver (e.g.
    /// `[T]` in `fn Option[T]::is_some(self)`) so the synthesised `self` param
    /// has type `Option[T]` rather than bare `Option`.
    fn parse_param_list_with_receiver(
        &mut self,
        receiver_type: Option<&str>,
        receiver_type_args: &[crate::mvl::parser::ast::TypeExpr],
    ) -> Result<Vec<Param>, ()> {
        let paren = self.expect(&TokenKind::LParen);
        self.require(paren)?;

        let mut params = Vec::new();
        loop {
            if matches!(self.peek_kind(), TokenKind::RParen | TokenKind::Eof) {
                break;
            }
            // Synthesise the receiver `self` param when:
            //   - this is the first parameter
            //   - a receiver type is set
            //   - the current token is the identifier `self`
            //   - the *next* token is `,` or `)` (not `:` which would mean `self: SomeType`)
            let param = if params.is_empty() {
                if let Some(recv_ty_name) = receiver_type {
                    if let TokenKind::Ident(name) = self.peek_kind() {
                        if name == "self" {
                            let saved = self.pos;
                            let span = self.peek_span();
                            self.advance(); // consume `self`
                            if matches!(self.peek_kind(), TokenKind::Comma | TokenKind::RParen) {
                                // Bare `self` — synthesise param with receiver type (and type args).
                                let ty = crate::mvl::parser::ast::TypeExpr::Base {
                                    name: recv_ty_name.to_string(),
                                    args: receiver_type_args.to_vec(),
                                    span,
                                };
                                params.push(Param {
                                    capability: None,
                                    name: "self".to_string(),
                                    ty,
                                    refinement: None,
                                    span,
                                });
                                if !self.eat(&TokenKind::Comma) {
                                    break;
                                }
                                continue;
                            } else {
                                // Not bare `self` — roll back and parse normally.
                                self.pos = saved;
                            }
                        }
                    }
                }
                self.parse_param()?
            } else {
                self.parse_param()?
            };
            params.push(param);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }

        let paren = self.expect(&TokenKind::RParen);
        self.require(paren)?;
        Ok(params)
    }

    fn parse_param(&mut self) -> Result<Param, ()> {
        let start = self.peek_span();

        // Optional capability: iso / val / ref / tag
        let capability = self.try_parse_capability();

        // Parameter name
        let ident_result = self.expect_ident();
        let (name, _) = self.require(ident_result)?;

        // `: type`
        let colon = self.expect(&TokenKind::Colon);
        self.require(colon)?;

        let ty = self.parse_type_expr()?;

        // Optional inline refinement: `where pred`
        let refinement = if self.eat(&TokenKind::Where) {
            Some(self.parse_ref_expr()?)
        } else {
            None
        };

        let span = self.span_from(start);
        Ok(Param {
            capability,
            name,
            ty,
            refinement,
            span,
        })
    }

    // ── Return refinement ─────────────────────────────────────────────────

    /// Try to parse a return-type refinement: `where pred` that is NOT a
    /// function-level `where` clause (which looks like `where Ident :`).
    ///
    /// Heuristic: if `where` is next and the token after is `IDENT ":"`,
    /// it's a function constraint — skip. Otherwise treat it as a type
    /// refinement.
    ///
    /// Fix #2: position and error-list are restored atomically if refinement
    /// parsing fails, preventing the parser from getting stuck mid-stream.
    fn try_parse_return_refinement(&mut self) -> Option<crate::mvl::parser::ast::RefExpr> {
        if !matches!(self.peek_kind(), TokenKind::Where) {
            return None;
        }
        // Peek two ahead: is this `where IDENT ":"`? → function constraint
        let next_is_fn_constraint = matches!(self.peek_next_kind(), TokenKind::Ident(_)) && {
            // Need to look one more token ahead. Save position and peek.
            let saved = self.pos;
            self.advance(); // consume `where`
            self.advance(); // consume IDENT
            let is_colon = matches!(self.peek_kind(), TokenKind::Colon);
            self.pos = saved; // restore
            is_colon
        };

        if next_is_fn_constraint {
            return None;
        }

        // Save state before consuming `where` so we can fully roll back if
        // parse_ref_expr fails (e.g. the `where` turns out to belong to an
        // outer construct that we cannot see yet).
        let saved_pos = self.pos;
        let saved_err_len = self.errors.len();
        self.advance(); // consume `where`
        match self.parse_ref_expr() {
            Ok(expr) => Some(expr),
            Err(()) => {
                // Roll back position and any errors pushed during the failed attempt.
                self.pos = saved_pos;
                self.errors.truncate(saved_err_len);
                None
            }
        }
    }

    // ── Where-clause constraints ──────────────────────────────────────────

    /// Parse optional `where T: Trait, U: Trait` constraints.
    fn parse_where_constraints(&mut self) -> Vec<Constraint> {
        if !self.eat(&TokenKind::Where) {
            return Vec::new();
        }
        let mut constraints = Vec::new();
        loop {
            if matches!(self.peek_kind(), TokenKind::LBrace | TokenKind::Eof) {
                break;
            }
            match self.parse_constraint() {
                Ok(c) => constraints.push(c),
                Err(()) => break,
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        constraints
    }

    fn parse_constraint(&mut self) -> Result<Constraint, ()> {
        let ident_result = self.expect_ident();
        let (name, _) = self.require(ident_result)?;
        let colon = self.expect(&TokenKind::Colon);
        self.require(colon)?;
        let ident_result2 = self.expect_ident();
        let (bound, _) = self.require(ident_result2)?;
        Ok(Constraint { name, bound })
    }

    // ── Top-level declarations (dispatches to type/fn/const/module) ───────

    /// Parse a single top-level declaration.
    pub fn parse_decl(&mut self) -> Result<crate::mvl::parser::ast::Decl, ()> {
        use crate::mvl::parser::ast::Decl;

        // Reject forbidden constructs with clear diagnostics before consuming `pub`.
        if let TokenKind::Ident(kw) = self.peek_kind() {
            match kw.as_str() {
                "static" | "global" => {
                    let span = self.peek_span();
                    let err = ParseError {
                        message: "MVL does not allow global mutable state. Pass state explicitly via function parameters.".into(),
                        span,
                    };
                    self.push_recover(err);
                    return Err(());
                }
                _ => {}
            }
        }

        // Optional `pub` visibility modifier
        let visible = self.eat(&TokenKind::Pub);

        // Also reject forbidden constructs that appear after `pub` (e.g. `pub static`).
        if let TokenKind::Ident(kw) = self.peek_kind() {
            match kw.as_str() {
                "static" | "global" => {
                    let span = self.peek_span();
                    let err = ParseError {
                        message: "MVL does not allow global mutable state. Pass state explicitly via function parameters.".into(),
                        span,
                    };
                    self.push_recover(err);
                    return Err(());
                }
                _ => {}
            }
        }

        match self.peek_kind() {
            TokenKind::Use => Ok(Decl::Use(self.parse_use_decl(visible)?)),
            TokenKind::Type => {
                let mut d = self.parse_type_decl()?;
                d.visible = visible;
                Ok(Decl::Type(d))
            }
            TokenKind::Fn
            | TokenKind::Total
            | TokenKind::Partial
            | TokenKind::Test
            | TokenKind::Builtin => {
                let mut d = self.parse_fn_decl()?;
                d.visible = visible;
                if d.is_test && d.visible {
                    let err = ParseError {
                        message: "`pub` is not allowed on `test fn` declarations".into(),
                        span: d.span,
                    };
                    self.push_error(err);
                    d.visible = false;
                }
                Ok(Decl::Fn(d))
            }
            TokenKind::Const => {
                let mut d = self.parse_const_decl()?;
                d.visible = visible;
                Ok(Decl::Const(d))
            }
            TokenKind::Extern => Ok(Decl::Extern(self.parse_extern_decl()?)),
            TokenKind::Impl => Ok(Decl::Impl(self.parse_impl_decl()?)),
            TokenKind::Actor => {
                let mut d = self.parse_actor_decl()?;
                d.visible = visible;
                Ok(Decl::Actor(d))
            }
            TokenKind::Effect => Ok(Decl::EffectDecl(self.parse_effect_decl()?)),
            TokenKind::Label => {
                let d = self.parse_label_decl(visible)?;
                Ok(Decl::Label(d))
            }
            TokenKind::Relabel => {
                let d = self.parse_relabel_decl(visible)?;
                Ok(Decl::Relabel(d))
            }
            _ => {
                let err = ParseError {
                    message: format!("expected declaration, found `{}`", self.peek_kind()),
                    span: self.peek_span(),
                };
                self.push_recover(err);
                Err(())
            }
        }
    }

    /// Parse `use path::to::Item;` (visibility already consumed as `reexport`).
    pub fn parse_use_decl(&mut self, reexport: bool) -> Result<UseDecl, ()> {
        let start = self.peek_span();
        self.advance(); // consume `use`

        // Parse module path: segments separated by `.` or `::`
        let mut path = Vec::new();
        let ident_result = self.expect_ident();
        let (first, _) = self.require(ident_result)?;
        path.push(first);

        let mut brace_group = false;
        let mut items: Vec<String> = Vec::new();
        loop {
            // Accept both `.` and `::` as path separators.
            if !self.eat(&TokenKind::Dot) && !self.eat(&TokenKind::ColonColon) {
                break;
            }
            // Brace import: `use std.io.{ A, B, C }` — consume items, store module path.
            // The type-checker resolves stdlib items via hardcoded tables; individual items
            // are stored in UseDecl::items so the Rust backend can suppress type stubs.
            if matches!(self.peek_kind(), TokenKind::LBrace) {
                self.advance(); // consume `{`
                while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
                    if let Ok((name, _)) = self.expect_ident() {
                        items.push(name);
                    } else {
                        self.advance(); // skip commas, etc.
                    }
                }
                let rbrace = self.expect(&TokenKind::RBrace);
                self.require(rbrace)?;
                brace_group = true;
                break;
            }
            let ident_result = self.expect_ident();
            let (seg, _) = self.require(ident_result)?;
            path.push(seg);
        }

        // Semicolon is required for plain imports (`use std::io;`) but not after
        // a brace group (`use std.io.{ A, B }`).
        if !brace_group {
            let semi = self.expect(&TokenKind::Semicolon);
            self.require(semi)?;
        }

        let span = self.span_from(start);
        Ok(UseDecl {
            reexport,
            module_only: !brace_group && path.len() >= 2,
            path,
            items,
            span,
        })
    }

    // ── Const and module stubs ─────────────────────────────────────────────

    pub fn parse_const_decl(&mut self) -> Result<crate::mvl::parser::ast::ConstDecl, ()> {
        let start = self.peek_span();
        self.advance(); // consume `const`
        let ident_result = self.expect_ident();
        let (name, _) = self.require(ident_result)?;
        let colon = self.expect(&TokenKind::Colon);
        self.require(colon)?;
        let ty = self.parse_type_expr()?;
        let eq = self.expect(&TokenKind::Eq);
        self.require(eq)?;
        // Fix #4: wire up the real expression parser instead of skipping to `;`
        let value = self.parse_expr()?;
        let semi = self.expect(&TokenKind::Semicolon);
        self.require(semi)?;
        let span = self.span_from(start);
        Ok(crate::mvl::parser::ast::ConstDecl {
            visible: false, // set by parse_decl when `pub` prefix is present
            name,
            ty,
            value,
            span,
        })
    }

    // ── IFC label / relabel declarations (#894) ───────────────────────────

    /// Parse `label Name` — declares a user-defined IFC label.
    /// Pre-condition: current token is `label`.
    pub fn parse_label_decl(&mut self, visible: bool) -> Result<LabelDecl, ()> {
        let start = self.peek_span();
        self.advance(); // consume `label`
        let ident_result = self.expect_ident();
        let (name, _) = self.require(ident_result)?;
        let span = self.span_from(start);
        // Register in the parser's label set so subsequent type annotations work.
        self.known_labels.insert(name.clone());
        Ok(LabelDecl {
            visible,
            name,
            span,
        })
    }

    /// Parse `relabel name: From -> To` — declares an IFC relabel transition.
    ///
    /// `From` and `To` are either `_` (bare type) or a declared label name.
    /// Pre-condition: current token is `relabel`.
    pub fn parse_relabel_decl(&mut self, visible: bool) -> Result<RelabelDecl, ()> {
        let start = self.peek_span();
        self.advance(); // consume `relabel`
        let ident_result = self.expect_ident();
        let (name, _) = self.require(ident_result)?;

        let colon = self.expect(&TokenKind::Colon);
        self.require(colon)?;

        let from = self.parse_relabel_side()?;

        let arrow = self.expect(&TokenKind::Arrow);
        self.require(arrow)?;

        let to = self.parse_relabel_side()?;

        let span = self.span_from(start);
        Ok(RelabelDecl {
            visible,
            name,
            from,
            to,
            span,
        })
    }

    /// Parse one side of a relabel declaration: `_` (bare) or an ident (label name).
    fn parse_relabel_side(&mut self) -> Result<Option<String>, ()> {
        match self.peek_kind() {
            TokenKind::Ident(name) if name == "_" => {
                self.advance();
                Ok(None)
            }
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok(Some(name))
            }
            _ => {
                let err = ParseError {
                    message: format!(
                        "expected `_` or label name in relabel declaration, found `{}`",
                        self.peek_kind()
                    ),
                    span: self.peek_span(),
                };
                self.push_recover(err);
                Err(())
            }
        }
    }

    // ── Effect declaration ────────────────────────────────────────────────

    /// Parse `effect Name [> Parent [+ Parent]*]` (#852).
    /// Pre-condition: current token is `effect`.
    pub fn parse_effect_decl(&mut self) -> Result<EffectDecl, ()> {
        let start = self.peek_span();
        self.advance(); // consume `effect`
        let ident_result = self.expect_ident();
        let (name, _) = self.require(ident_result)?;

        let mut subsumes = Vec::new();
        if self.eat(&TokenKind::Gt) {
            // First parent (required after `>`)
            let first_result = self.expect_ident();
            let (first, _) = self.require(first_result)?;
            subsumes.push(first);
            // Additional parents separated by `+`
            while self.eat(&TokenKind::Plus) {
                let next_result = self.expect_ident();
                let (next, _) = self.require(next_result)?;
                subsumes.push(next);
            }
        }

        let span = self.span_from(start);
        Ok(EffectDecl {
            name,
            subsumes,
            span,
        })
    }

    // ── Extern block ──────────────────────────────────────────────────────

    /// Parse `extern "abi" { fn foo(…) -> T [! Effects]; … }`.
    /// Pre-condition: current token is `extern`.
    pub fn parse_extern_decl(&mut self) -> Result<ExternDecl, ()> {
        let start = self.peek_span();
        self.advance(); // consume `extern`

        // ABI string: e.g. `"rust"` or `"c"`
        let abi = match self.peek_kind() {
            TokenKind::Str(_) => {
                let tok = self.advance();
                match tok.kind {
                    TokenKind::Str(s) => s,
                    _ => unreachable!(),
                }
            }
            _ => {
                let err = ParseError {
                    message: format!(
                        "expected ABI string after `extern`, found `{}`",
                        self.peek_kind()
                    ),
                    span: self.peek_span(),
                };
                self.push_recover(err);
                return Err(());
            }
        };

        let lbrace = self.expect(&TokenKind::LBrace);
        self.require(lbrace)?;

        let mut fns = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            match self.peek_kind() {
                TokenKind::Fn => {
                    if let Ok(f) = self.parse_extern_fn_decl(None) {
                        fns.push(f);
                    }
                }
                TokenKind::Total => {
                    self.advance(); // consume `total`
                    if matches!(self.peek_kind(), TokenKind::Fn) {
                        if let Ok(f) = self.parse_extern_fn_decl(Some(Totality::Total)) {
                            fns.push(f);
                        }
                    } else {
                        let err = ParseError {
                            message: format!(
                                "expected `fn` after `total` inside extern block, found `{}`",
                                self.peek_kind()
                            ),
                            span: self.peek_span(),
                        };
                        self.push_recover(err);
                        self.advance();
                    }
                }
                _ => {
                    let err = ParseError {
                        message: format!(
                            "expected `fn` or `total fn` inside extern block, found `{}`",
                            self.peek_kind()
                        ),
                        span: self.peek_span(),
                    };
                    self.push_recover(err);
                    self.advance(); // skip unknown token
                }
            }
        }

        let rbrace = self.expect(&TokenKind::RBrace);
        self.require(rbrace)?;
        let span = self.span_from(start);
        Ok(ExternDecl { abi, fns, span })
    }

    /// Parse a single `[total] fn foo(params) -> RetType [! Effects]` inside an extern block.
    /// `totality` is passed in by the caller after consuming an optional `total` token.
    /// Note: no body (terminated by `;` or end of signature before next `fn`/`}`).
    fn parse_extern_fn_decl(&mut self, totality: Option<Totality>) -> Result<ExternFnDecl, ()> {
        let start = self.peek_span();
        self.advance(); // consume `fn`

        let ident_result = self.expect_ident();
        let (name, _) = self.require(ident_result)?;

        // Optional generic type parameters: `fn foo<T>(…)` — parsed but not stored
        // since extern fns delegate generics entirely to the Rust implementation.
        let _ = self.parse_type_params_decl();

        // Parameter list
        let lparen = self.expect(&TokenKind::LParen);
        self.require(lparen)?;
        let mut params = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RParen | TokenKind::Eof) {
            if !params.is_empty() {
                let comma = self.expect(&TokenKind::Comma);
                if self.require(comma).is_err() {
                    break;
                }
            }
            if matches!(self.peek_kind(), TokenKind::RParen) {
                break;
            }
            match self.parse_param() {
                Ok(p) => params.push(p),
                Err(()) => break,
            }
        }
        let rparen = self.expect(&TokenKind::RParen);
        self.require(rparen)?;

        // Return type: `-> T`
        let arrow = self.expect(&TokenKind::Arrow);
        self.require(arrow)?;
        let return_type = Box::new(self.parse_type_expr()?);

        // Optional effects: `! Console + Net`
        let effects = self.parse_optional_effects();

        // Optional trailing semicolon
        if matches!(self.peek_kind(), TokenKind::Semicolon) {
            self.advance();
        }

        let span = self.span_from(start);
        Ok(ExternFnDecl {
            name,
            params,
            return_type,
            effects,
            totality,
            span,
        })
    }

    // ── Impl declaration ──────────────────────────────────────────────────

    /// Parse `impl TraitName for TypeName { fn … }`.
    /// Pre-condition: current token is `impl`.
    pub(crate) fn parse_impl_decl(&mut self) -> Result<ImplDecl, ()> {
        let start = self.peek_span();
        self.advance(); // consume `impl`

        // Trait name (e.g. `Display` or `From`)
        let ident_result = self.expect_ident();
        let (trait_name, _) = self.require(ident_result)?;

        // Optional generic type args on the trait, e.g. `[IoError]` in `From[IoError]`
        let trait_type_args = if self.eat(&TokenKind::LBracket) {
            let args = self.parse_type_list()?;
            let rb = self.expect(&TokenKind::RBracket);
            self.require(rb)?;
            args
        } else {
            Vec::new()
        };

        // `for`
        let for_kw = self.expect(&TokenKind::For);
        self.require(for_kw)?;

        // Type name (e.g. `Point`)
        let ident_result = self.expect_ident();
        let (type_name, _) = self.require(ident_result)?;

        // `{`
        let lbrace = self.expect(&TokenKind::LBrace);
        self.require(lbrace)?;

        // Methods
        let mut methods = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            let pos_before = self.pos;
            match self.parse_fn_decl() {
                Ok(f) => methods.push(f),
                Err(()) => {
                    if matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
                        break;
                    }
                }
            }
            // Guarantee forward progress to prevent infinite loop on unrecoverable tokens.
            if self.pos == pos_before
                && !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof)
            {
                self.advance();
            }
        }

        // `}`
        let rbrace = self.expect(&TokenKind::RBrace);
        self.require(rbrace)?;

        let span = self.span_from(start);
        Ok(ImplDecl {
            trait_name,
            trait_type_args,
            type_name,
            methods,
            span,
        })
    }

    // ── Actor declarations (Phase 8, #63) ─────────────────────────────────

    /// Parse `actor TypeName { fields* methods* }`.
    ///
    /// Inside an actor body:
    /// - `pub fn name(params) { … }` — public async behavior (message handler)
    /// - `fn name(params) -> T { … }` — private synchronous helper
    ///
    /// Fields come first (unambiguous: they have no `fn`/`pub` prefix), then methods.
    pub fn parse_actor_decl(&mut self) -> Result<ActorDecl, ()> {
        let start = self.peek_span();
        self.advance(); // consume `actor`

        let ident_result = self.expect_ident();
        let (name, _) = self.require(ident_result)?;

        let type_params = self.parse_type_params_decl();

        // Optional `traps_exit` modifier (Phase 9, #1177).
        let traps_exit = matches!(self.peek_kind(), TokenKind::Ident(s) if s == "traps_exit");
        if traps_exit {
            self.advance(); // consume `traps_exit`
        }

        let lbrace = self.expect(&TokenKind::LBrace);
        self.require(lbrace)?;

        let mut fields = Vec::new();
        let mut methods = Vec::new();

        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            match self.peek_kind() {
                // `pub fn` — public async behavior
                TokenKind::Pub => {
                    self.advance(); // consume `pub`
                    let fn_tok = self.expect(&TokenKind::Fn);
                    self.require(fn_tok)?;
                    match self.parse_actor_method(true) {
                        Ok(m) => methods.push(m),
                        Err(()) => break,
                    }
                }
                // `fn` — private synchronous helper
                TokenKind::Fn => {
                    self.advance(); // consume `fn`
                    match self.parse_actor_method(false) {
                        Ok(m) => methods.push(m),
                        Err(()) => break,
                    }
                }
                // anything else: try to parse a field declaration
                _ => match self.parse_field_decl() {
                    Ok(f) => {
                        fields.push(f);
                        self.eat(&TokenKind::Comma);
                    }
                    Err(()) => break,
                },
            }
        }

        let rbrace = self.expect(&TokenKind::RBrace);
        self.require(rbrace)?;

        // Optional `with mailbox(...)` configuration (#1127).
        let mailbox = if matches!(self.peek_kind(), TokenKind::With) {
            self.advance(); // consume `with`
            Some(self.parse_mailbox_config()?)
        } else {
            None
        };

        let span = self.span_from(start);
        Ok(ActorDecl {
            visible: false, // set by parse_decl when `pub` prefix is present
            name,
            type_params,
            fields,
            methods,
            mailbox,
            traps_exit,
            span,
        })
    }

    /// Parse `mailbox(...)` after `with` has been consumed.
    ///
    /// Syntax:
    /// - `mailbox(unbounded)`
    /// - `mailbox(N)`
    /// - `mailbox(N, block)`
    /// - `mailbox(N, drop_newest)`
    fn parse_mailbox_config(&mut self) -> Result<MailboxConfig, ()> {
        // Expect `mailbox` identifier.
        let ident_result = self.expect_ident();
        let (kw, kw_span) = self.require(ident_result)?;
        if kw != "mailbox" {
            self.push_recover(ParseError {
                message: format!("expected `mailbox` after `with`, found `{kw}`"),
                span: kw_span,
            });
            return Err(());
        }

        let lparen = self.expect(&TokenKind::LParen);
        self.require(lparen)?;

        // First argument: `unbounded` keyword or a positive integer capacity.
        let config = match self.peek_kind().clone() {
            TokenKind::Ident(s) if s == "unbounded" => {
                self.advance(); // consume `unbounded`
                let rparen = self.expect(&TokenKind::RParen);
                self.require(rparen)?;
                MailboxConfig::Unbounded
            }
            TokenKind::Integer(n) => {
                let span = self.advance().span;
                if n <= 0 {
                    self.push_recover(ParseError {
                        message: "mailbox capacity must be greater than zero".to_string(),
                        span,
                    });
                    return Err(());
                }
                let capacity = n as u64;

                // Optional policy: `, block` or `, drop_newest`.
                let policy = if matches!(self.peek_kind(), TokenKind::Comma) {
                    self.advance(); // consume `,`
                    let policy_result = self.expect_ident();
                    let (policy_str, policy_span) = self.require(policy_result)?;
                    match policy_str.as_str() {
                        "block" => MailboxPolicy::Block,
                        "drop_newest" => MailboxPolicy::DropNewest,
                        other => {
                            self.push_recover(ParseError {
                                message: format!(
                                    "unknown mailbox policy `{other}`; expected `block` or `drop_newest`"
                                ),
                                span: policy_span,
                            });
                            return Err(());
                        }
                    }
                } else {
                    MailboxPolicy::DropNewest
                };

                let rparen = self.expect(&TokenKind::RParen);
                self.require(rparen)?;
                MailboxConfig::Bounded { capacity, policy }
            }
            _ => {
                let span = self.peek_span();
                self.push_recover(ParseError {
                    message: format!(
                        "expected a capacity (integer) or `unbounded` in `mailbox(...)`, found `{}`",
                        self.peek_kind()
                    ),
                    span,
                });
                return Err(());
            }
        };

        Ok(config)
    }

    /// Parse an actor method after `fn` (or `pub fn`) has been consumed:
    /// `name(params) [-> ReturnType] [! Effects] { body }`.
    ///
    /// Return type defaults to `Unit` when the `->` arrow is absent.
    fn parse_actor_method(&mut self, is_public: bool) -> Result<ActorMethod, ()> {
        let start = self.peek_span();

        let ident_result = self.expect_ident();
        let (name, _) = self.require(ident_result)?;

        let params = self.parse_param_list()?;

        // Optional return type; default to Unit when absent.
        let return_type = if matches!(self.peek_kind(), TokenKind::Arrow) {
            self.advance(); // consume `->`
            self.parse_type_expr()?
        } else {
            crate::mvl::parser::ast::TypeExpr::Base {
                name: "Unit".to_string(),
                args: vec![],
                span: self.peek_span(),
            }
        };

        let effects = self.parse_optional_effects();

        let body = self.parse_block()?;

        let span = self.span_from(start);
        Ok(ActorMethod {
            is_public,
            name,
            params,
            return_type: Box::new(return_type),
            effects,
            body,
            span,
        })
    }

    /// Parse a complete program (sequence of declarations).
    pub fn parse_program(&mut self) -> crate::mvl::parser::ast::Program {
        let start = self.peek_span();
        let mut declarations = Vec::new();
        while !self.at_eof() {
            let pos_before = self.pos;
            if let Ok(d) = self.parse_decl() {
                declarations.push(d);
            }
            // Fix #10: if parse_decl returned Err without consuming any tokens
            // (e.g. recovery stopped at a sync point that is itself the bad token),
            // force-advance to prevent an infinite loop.
            if !self.at_eof() && self.pos == pos_before {
                self.advance();
            }
        }
        let span = self.span_from(start);
        crate::mvl::parser::ast::Program { declarations, span }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::ast::*;

    fn fn_decl(src: &str) -> FnDecl {
        let (mut p, lex_errs) = Parser::new(src);
        assert!(lex_errs.is_empty(), "lex errors: {:?}", lex_errs);
        let result = p.parse_fn_decl();
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        result.expect("parse_fn_decl failed")
    }

    // ── Requirement 4 / Scenario: Parse total function with effects ───────

    #[test]
    fn parse_total_fn_with_effects() {
        // GIVEN: total fn read(path: Path) -> Result[String, IOError] ! FileRead { }
        // THEN: FnDecl with totality=Total, effects=[FileRead], return=Result[String, IOError]
        let d = fn_decl("total fn read(path: Path) -> Result[String, IOError] ! FileRead { }");
        assert_eq!(d.totality, Some(Totality::Total));
        assert_eq!(d.name, "read");
        assert_eq!(
            d.effects
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["FileRead"]
        );
        assert!(matches!(*d.return_type, TypeExpr::Result { .. }));
    }

    #[test]
    fn parse_partial_fn() {
        let d = fn_decl("partial fn loop_until(n: Int) -> Int { }");
        assert_eq!(d.totality, Some(Totality::Partial));
    }

    #[test]
    fn parse_fn_no_totality() {
        let d = fn_decl("fn greet(name: String) -> String { }");
        assert_eq!(d.totality, None);
        assert_eq!(d.name, "greet");
    }

    // ── Requirement 4 / Scenario: Parse function with capability parameter ─

    #[test]
    fn parse_fn_with_iso_param() {
        // GIVEN: fn process(iso db: val DbConn) -> Result[Data, Error] ! DB { }
        // THEN: parameter has capability=Iso, type=Ref(DbConn)
        let d = fn_decl("fn process(iso db: val DbConn) -> Result[Data, Error] ! DB { }");
        assert_eq!(d.params[0].capability, Some(Capability::Iso));
        assert_eq!(d.params[0].name, "db");
        assert!(matches!(
            d.params[0].ty,
            TypeExpr::Ref { mutable: false, .. }
        ));
        assert_eq!(
            d.effects
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["DB"]
        );
    }

    #[test]
    fn parse_fn_with_all_capabilities() {
        let d = fn_decl("fn f(iso a: Int, val b: Int, ref c: Int, tag d: Int) -> Int { }");
        assert_eq!(d.params[0].capability, Some(Capability::Iso));
        assert_eq!(d.params[1].capability, Some(Capability::Val));
        assert_eq!(d.params[2].capability, Some(Capability::Ref));
        assert_eq!(d.params[3].capability, Some(Capability::Tag));
    }

    // ── Requirement 4 / Scenario: Parse function with security-labeled params

    #[test]
    fn parse_fn_with_security_labels() {
        // GIVEN: fn handle(input: Tainted[String], key: Secret[ApiKey]) -> Response
        // THEN: params have correct security labels; return is bare (unlabeled)
        let d = fn_decl("fn handle(input: Tainted[String], key: Secret[ApiKey]) -> Response { }");
        assert_eq!(d.params.len(), 2);
        assert!(matches!(
            d.params[0].ty,
            TypeExpr::Labeled { ref label, .. } if label == "Tainted"
        ));
        assert!(matches!(
            d.params[1].ty,
            TypeExpr::Labeled { ref label, .. } if label == "Secret"
        ));
        assert!(matches!(
            *d.return_type,
            TypeExpr::Base { ref name, .. } if name == "Response"
        ));
    }

    #[test]
    fn parse_fn_multiple_effects() {
        let d = fn_decl("fn log(msg: String) -> Unit ! DB + Console { }");
        assert_eq!(
            d.effects
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["DB", "Console"]
        );
    }

    #[test]
    fn parse_fn_with_ref_capability_param() {
        // ref capability replaces the old `mut` keyword on params
        let d = fn_decl("fn inc(ref count: Int) -> Int { }");
        assert_eq!(d.params[0].capability, Some(Capability::Ref));
        assert_eq!(d.params[0].name, "count");
    }

    #[test]
    fn parse_fn_no_params() {
        let d = fn_decl("fn unit() -> Unit { }");
        assert!(d.params.is_empty());
    }

    #[test]
    fn parse_fn_with_type_params() {
        let d = fn_decl("fn identity[T](x: T) -> T { }");
        assert_eq!(d.type_params, vec![GenericParam::Type("T".to_string())]);
        assert_eq!(d.params[0].name, "x");
    }

    #[test]
    fn parse_fn_with_const_generic() {
        let d = fn_decl("fn fill[T, const N: Int](item: T) -> Int { 0 }");
        assert_eq!(
            d.type_params,
            vec![
                GenericParam::Type("T".to_string()),
                GenericParam::Const("N".to_string(), "Int".to_string()),
            ]
        );
    }

    #[test]
    fn parse_fn_param_with_refinement() {
        let d = fn_decl("fn positive(x: Int where self > 0) -> Int { }");
        assert!(d.params[0].refinement.is_some());
    }

    #[test]
    fn parse_fn_where_constraints() {
        let d = fn_decl("fn compare[T](a: T, b: T) -> Bool where T: Eq { }");
        assert_eq!(d.constraints.len(), 1);
        assert_eq!(d.constraints[0].name, "T");
        assert_eq!(d.constraints[0].bound, "Eq");
    }

    #[test]
    fn parse_authenticate_from_corpus() {
        // From tests/corpus/11_programs/auth_handler.mvl (updated for #894)
        let src = r#"total fn authenticate(
    iso db: val DbConn,
    input_password: Tainted[String],
    user_id: UserId
) -> Result[Session, AuthError] ! DB + Console { }"#;
        let d = fn_decl(src);
        assert_eq!(d.totality, Some(Totality::Total));
        assert_eq!(d.name, "authenticate");
        assert_eq!(d.params.len(), 3);
        assert_eq!(d.params[0].capability, Some(Capability::Iso));
        assert!(matches!(
            d.params[1].ty,
            TypeExpr::Labeled { ref label, .. } if label == "Tainted"
        ));
        assert!(matches!(
            d.params[2].ty,
            TypeExpr::Base { ref name, .. } if name == "UserId"
        ));
        assert!(matches!(*d.return_type, TypeExpr::Result { .. }));
        assert_eq!(
            d.effects
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["DB", "Console"]
        );
    }

    // ── Extern block parsing ──────────────────────────────────────────────

    fn extern_decl(src: &str) -> ExternDecl {
        let (mut p, lex_errs) = Parser::new(src);
        assert!(lex_errs.is_empty(), "lex errors: {:?}", lex_errs);
        let result = p.parse_extern_decl();
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        result.expect("expected Ok(ExternDecl)")
    }

    #[test]
    fn parse_extern_rust_empty_block() {
        let ed = extern_decl(r#"extern "rust" {}"#);
        assert_eq!(ed.abi, "rust");
        assert!(ed.fns.is_empty());
    }

    #[test]
    fn parse_extern_rust_single_fn() {
        let ed = extern_decl(
            r#"extern "rust" {
    fn sha256(data: val String) -> String;
}"#,
        );
        assert_eq!(ed.abi, "rust");
        assert_eq!(ed.fns.len(), 1);
        assert_eq!(ed.fns[0].name, "sha256");
        assert_eq!(ed.fns[0].params.len(), 1);
        assert_eq!(ed.fns[0].params[0].name, "data");
        assert!(ed.fns[0].effects.is_empty());
    }

    #[test]
    fn parse_extern_rust_with_effects() {
        let ed = extern_decl(
            r#"extern "rust" {
    fn http_get(url: String) -> Result[String, String] ! Net;
}"#,
        );
        assert_eq!(ed.fns.len(), 1);
        assert_eq!(ed.fns[0].name, "http_get");
        assert_eq!(
            ed.fns[0]
                .effects
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Net"]
        );
        assert!(matches!(*ed.fns[0].return_type, TypeExpr::Result { .. }));
    }

    #[test]
    fn parse_extern_rust_multiple_fns() {
        let ed = extern_decl(
            r#"extern "rust" {
    fn connect(url: String) -> String;
    fn query(conn: String, sql: String) -> String ! DB;
    fn disconnect(conn: String) -> String;
}"#,
        );
        assert_eq!(ed.abi, "rust");
        assert_eq!(ed.fns.len(), 3);
        assert_eq!(ed.fns[0].name, "connect");
        assert_eq!(ed.fns[1].name, "query");
        assert_eq!(
            ed.fns[1]
                .effects
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["DB"]
        );
        assert_eq!(ed.fns[2].name, "disconnect");
    }

    #[test]
    fn parse_extern_as_top_level_decl() {
        let src = r#"extern "rust" {
    fn greet(name: String) -> String;
}
fn main() -> String { greet(String::new()) }"#;
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        assert_eq!(prog.declarations.len(), 2);
        assert!(matches!(prog.declarations[0], Decl::Extern(_)));
        assert!(matches!(prog.declarations[1], Decl::Fn(_)));
    }

    // ── Requirement 4 / Scenario: Parse test function declaration ─────────

    #[test]
    fn parse_test_fn() {
        // GIVEN: test fn check_add() -> Unit { }
        // THEN: FnDecl with is_test=true, name="check_add"
        let d = fn_decl("test fn check_add() -> Unit { }");
        assert!(d.is_test);
        assert_eq!(d.name, "check_add");
        assert_eq!(d.totality, None);
    }

    #[test]
    fn parse_test_fn_not_marked_for_normal_fn() {
        let d = fn_decl("fn add(a: Int, b: Int) -> Int { }");
        assert!(!d.is_test);
    }

    #[test]
    fn parse_test_fn_as_top_level_decl() {
        let src = "fn add(a: Int, b: Int) -> Int { }\ntest fn check_add() -> Unit { }";
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        assert_eq!(prog.declarations.len(), 2);
        if let Decl::Fn(fd) = &prog.declarations[1] {
            assert!(fd.is_test);
            assert_eq!(fd.name, "check_add");
        } else {
            panic!("expected Fn decl");
        }
    }

    #[test]
    fn pub_test_fn_is_rejected() {
        // Spec 004 Req 1: test fns MUST NOT be pub
        let src = "pub test fn my_test() -> Unit { }";
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(!p.errors.is_empty(), "expected parse error for pub test fn");
        // The decl is still produced with visible=false (error recovery)
        if let Decl::Fn(fd) = &prog.declarations[0] {
            assert!(!fd.visible, "pub should be cleared after error");
        }
    }

    // ── Req 8: No Global Mutable State ────────────────────────────────────

    #[test]
    fn static_mut_is_rejected() {
        // Spec 001 Req 8: MVL MUST NOT allow global mutable state
        let src = "static mut COUNTER: Int = 0;";
        let (mut p, _) = Parser::new(src);
        p.parse_program();
        assert!(!p.errors.is_empty(), "expected parse error for static mut");
        assert!(
            p.errors[0].message.contains("global mutable state"),
            "expected helpful error message, got: {}",
            p.errors[0].message
        );
    }

    #[test]
    fn static_decl_is_rejected() {
        // `static` without `mut` is also not allowed in MVL
        let src = "static X: Int = 42;";
        let (mut p, _) = Parser::new(src);
        p.parse_program();
        assert!(!p.errors.is_empty(), "expected parse error for static decl");
        assert!(p.errors[0].message.contains("global mutable state"));
    }

    #[test]
    fn global_keyword_is_rejected() {
        let src = "global X: Int = 0;";
        let (mut p, _) = Parser::new(src);
        p.parse_program();
        assert!(!p.errors.is_empty(), "expected parse error for global");
        assert!(p.errors[0].message.contains("global mutable state"));
    }

    #[test]
    fn pub_static_is_rejected_with_helpful_message() {
        // Spec 001 Req 8: `pub static` must also produce the MVL-specific diagnostic
        let src = "pub static X: Int = 42;";
        let (mut p, _) = Parser::new(src);
        p.parse_program();
        assert!(!p.errors.is_empty(), "expected parse error for pub static");
        assert!(
            p.errors[0].message.contains("global mutable state"),
            "expected helpful error message, got: {}",
            p.errors[0].message
        );
    }

    // ── builtin keyword tests ─────────────────────────────────────────────

    #[test]
    fn parse_builtin_fn_simple() {
        // GIVEN: builtin fn len(s: String) -> Int
        // THEN: FnDecl with is_builtin=true, no body stmts
        let d = fn_decl("builtin fn len(s: String) -> Int");
        assert!(d.is_builtin);
        assert!(!d.is_test);
        assert_eq!(d.totality, None);
        assert_eq!(d.name, "len");
        assert_eq!(d.params.len(), 1);
        assert!(d.body.stmts.is_empty());
    }

    #[test]
    fn parse_pub_builtin_fn() {
        // GIVEN: pub builtin fn len(s: String) -> Int
        // THEN: FnDecl with visible=true, is_builtin=true
        let src = "pub builtin fn len(s: String) -> Int";
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        assert_eq!(prog.declarations.len(), 1);
        if let Decl::Fn(fd) = &prog.declarations[0] {
            assert!(fd.visible);
            assert!(fd.is_builtin);
            assert_eq!(fd.name, "len");
        } else {
            panic!("expected FnDecl");
        }
    }

    #[test]
    fn parse_builtin_fn_with_effects() {
        // GIVEN: builtin fn read(path: Path) -> Result[String, IOError] ! FileRead
        // THEN: is_builtin=true, effects=[FileRead]
        let d = fn_decl("builtin fn read(path: Path) -> Result[String, IOError] ! FileRead");
        assert!(d.is_builtin);
        assert_eq!(d.effects.len(), 1);
        assert_eq!(d.effects[0].name, "FileRead");
    }

    #[test]
    fn builtin_fn_with_body_is_rejected() {
        // GIVEN: builtin fn len(s: String) -> Int { 0 }
        // THEN: parse error — builtin functions may not have a body
        let src = "builtin fn len(s: String) -> Int { 0 }";
        let (mut p, _) = Parser::new(src);
        p.parse_program();
        assert!(
            !p.errors.is_empty(),
            "expected parse error for builtin with body"
        );
        assert!(
            p.errors[0].message.contains("builtin"),
            "expected builtin error, got: {}",
            p.errors[0].message
        );
    }

    #[test]
    fn builtin_and_total_combined_is_rejected() {
        // GIVEN: total builtin fn len(s: String) -> Int
        // THEN: parse error — builtin cannot combine with total
        let src = "total builtin fn len(s: String) -> Int";
        let (mut p, _) = Parser::new(src);
        p.parse_program();
        assert!(
            !p.errors.is_empty(),
            "expected parse error for total builtin"
        );
        assert!(
            p.errors[0].message.contains("builtin"),
            "expected builtin error, got: {}",
            p.errors[0].message
        );
    }

    #[test]
    fn normal_fn_is_not_builtin() {
        // GIVEN: fn greet(name: String) -> String { }
        // THEN: is_builtin=false
        let d = fn_decl("fn greet(name: String) -> String { }");
        assert!(!d.is_builtin);
    }

    // ── Contract clause tests (#688) ───────────────────────────────────────

    #[test]
    fn parse_fn_with_requires() {
        // GIVEN: fn divide(a: Int, b: Int) -> Int requires b != 0 { }
        // THEN: FnDecl with one requires clause; ensures is empty
        let d = fn_decl("fn divide(a: Int, b: Int) -> Int\n  requires b != 0\n{ }");
        assert_eq!(d.requires.len(), 1, "expected one requires clause");
        assert!(d.ensures.is_empty(), "no ensures expected");
        assert!(matches!(d.requires[0], Expr::Binary { .. }));
    }

    #[test]
    fn parse_fn_with_ensures() {
        // GIVEN: fn nonneg(n: Int) -> Int ensures result >= 0 { }
        // THEN: FnDecl with one ensures clause; requires is empty
        let d = fn_decl("fn nonneg(n: Int) -> Int\n  ensures result >= 0\n{ }");
        assert_eq!(d.ensures.len(), 1, "expected one ensures clause");
        assert!(d.requires.is_empty(), "no requires expected");
        assert!(matches!(d.ensures[0], Expr::Binary { .. }));
    }

    #[test]
    fn parse_fn_with_multiple_contracts() {
        // GIVEN: fn factorial(n: Int) -> Int requires n >= 0 ensures result >= 1 { }
        // THEN: FnDecl has both requires and ensures, one each
        let d =
            fn_decl("fn factorial(n: Int) -> Int\n  requires n >= 0\n  ensures result >= 1\n{ }");
        assert_eq!(d.requires.len(), 1, "expected one requires clause");
        assert_eq!(d.ensures.len(), 1, "expected one ensures clause");
        assert!(matches!(d.requires[0], Expr::Binary { .. }));
        assert!(matches!(d.ensures[0], Expr::Binary { .. }));
    }

    #[test]
    fn parse_ghost_let_in_fn() {
        // GIVEN: fn abs with ensures and a ghost let binding in body
        // THEN: ensures clause present; body contains a Ghost LetKind statement
        let src = "fn abs(n: Int) -> Int\n  ensures result >= 0\n{\n  ghost let spec_n: Int = n;\n  if n >= 0 { n } else { 0 - n }\n}";
        let (mut p, lex_errs) = Parser::new(src);
        assert!(lex_errs.is_empty(), "lex errors: {lex_errs:?}");
        let d = p.parse_fn_decl().expect("parse_fn_decl failed");
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        assert_eq!(d.ensures.len(), 1, "expected one ensures clause");
        let has_ghost = d.body.stmts.iter().any(|s| {
            matches!(
                s,
                Stmt::Let {
                    kind: LetKind::Ghost,
                    ..
                }
            )
        });
        assert!(has_ghost, "expected a ghost let binding in body");
    }

    // ── Contract clause with method call (#983) ────────────────────────────

    #[test]
    fn parse_fn_requires_method_call_not_dropped() {
        // GIVEN: requires clause with method call expression (e.g. items.len() > 0)
        // THEN: clause is parsed and stored — NOT silently dropped (#983)
        let d = fn_decl(
            "fn process(items: List[Int]) -> List[Int]\n  requires items.len() > 0\n{ items }",
        );
        assert_eq!(
            d.requires.len(),
            1,
            "requires clause must not be silently dropped (#983)"
        );
        assert!(matches!(d.requires[0], Expr::Binary { .. }));
    }

    #[test]
    fn parse_fn_ensures_method_call_not_dropped() {
        // GIVEN: ensures clause with method call expression
        // THEN: clause is parsed and stored — NOT silently dropped (#983)
        let d = fn_decl(
            "fn process(items: List[Int]) -> List[Int]\n  ensures result.len() > 0\n{ items }",
        );
        assert_eq!(
            d.ensures.len(),
            1,
            "ensures clause must not be silently dropped (#983)"
        );
        assert!(matches!(d.ensures[0], Expr::Binary { .. }));
    }

    // ── Effect declaration tests (#852) ────────────────────────────────────

    fn effect_decl(src: &str) -> EffectDecl {
        let (mut p, lex_errs) = Parser::new(src);
        assert!(lex_errs.is_empty(), "lex errors: {:?}", lex_errs);
        let result = p.parse_effect_decl();
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        result.expect("parse_effect_decl failed")
    }

    #[test]
    fn parse_base_effect() {
        // GIVEN: `effect Clock`
        // THEN: EffectDecl { name: "Clock", subsumes: [] }
        let d = effect_decl("effect Clock");
        assert_eq!(d.name, "Clock");
        assert!(d.subsumes.is_empty());
    }

    #[test]
    fn parse_effect_single_parent() {
        // GIVEN: `effect Log > Clock`
        // THEN: EffectDecl { name: "Log", subsumes: ["Clock"] }
        let d = effect_decl("effect Log > Clock");
        assert_eq!(d.name, "Log");
        assert_eq!(d.subsumes, vec!["Clock"]);
    }

    #[test]
    fn parse_effect_multiple_parents() {
        // GIVEN: `effect Billing > DB + Log + Clock`
        // THEN: EffectDecl { name: "Billing", subsumes: ["DB", "Log", "Clock"] }
        let d = effect_decl("effect Billing > DB + Log + Clock");
        assert_eq!(d.name, "Billing");
        assert_eq!(d.subsumes, vec!["DB", "Log", "Clock"]);
    }

    #[test]
    fn parse_effect_decl_as_top_level() {
        // GIVEN: a program with an effect declaration
        // THEN: Decl::EffectDecl node in program
        let (mut p, lex_errs) = Parser::new("effect IO > Console + FileRead + Net");
        assert!(lex_errs.is_empty());
        let prog = p.parse_program();
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        assert_eq!(prog.declarations.len(), 1);
        let Decl::EffectDecl(ed) = &prog.declarations[0] else {
            panic!("expected EffectDecl, got {:?}", prog.declarations[0]);
        };
        assert_eq!(ed.name, "IO");
        assert_eq!(ed.subsumes, vec!["Console", "FileRead", "Net"]);
    }

    // ── Type-attached method tests (#868) ─────────────────────────────────────

    #[test]
    fn parse_type_attached_method_basic() {
        // GIVEN: fn Logger::info(self, msg: String) -> Unit { }
        // THEN: FnDecl with receiver_type="Logger", name="info", first param is self:Logger
        let d = fn_decl("fn Logger::info(self, msg: String) -> Unit { }");
        assert_eq!(d.receiver_type, Some("Logger".to_string()));
        assert_eq!(d.name, "info");
        assert_eq!(d.params.len(), 2);
        assert_eq!(d.params[0].name, "self");
        assert!(
            matches!(&d.params[0].ty, TypeExpr::Base { name, .. } if name == "Logger"),
            "expected self param type to be Logger"
        );
        assert_eq!(d.params[1].name, "msg");
    }

    #[test]
    fn parse_type_attached_method_self_only() {
        // GIVEN: fn Counter::reset(self) -> Unit { }
        // THEN: FnDecl with receiver_type="Counter", single self param
        let d = fn_decl("fn Counter::reset(self) -> Unit { }");
        assert_eq!(d.receiver_type, Some("Counter".to_string()));
        assert_eq!(d.name, "reset");
        assert_eq!(d.params.len(), 1);
        assert_eq!(d.params[0].name, "self");
    }

    #[test]
    fn parse_type_attached_method_with_effects() {
        // GIVEN: fn Logger::debug(self, msg: String) -> Unit ! Console { }
        // THEN: FnDecl with effects=[Console]
        let d = fn_decl("fn Logger::debug(self, msg: String) -> Unit ! Console { }");
        assert_eq!(d.receiver_type, Some("Logger".to_string()));
        assert_eq!(d.effects.len(), 1);
        assert_eq!(d.effects[0].name, "Console");
    }

    #[test]
    fn parse_ordinary_fn_has_no_receiver_type() {
        // GIVEN: fn add(a: Int, b: Int) -> Int { }
        // THEN: receiver_type is None
        let d = fn_decl("fn add(a: Int, b: Int) -> Int { }");
        assert_eq!(d.receiver_type, None);
    }

    #[test]
    fn parse_type_attached_method_as_top_level_decl() {
        // GIVEN: a program with a type-attached method
        // THEN: Decl::Fn with receiver_type set
        let src = "fn Point::scale(self, factor: Int) -> Point { self }";
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        assert_eq!(prog.declarations.len(), 1);
        if let Decl::Fn(fd) = &prog.declarations[0] {
            assert_eq!(fd.receiver_type, Some("Point".to_string()));
            assert_eq!(fd.name, "scale");
        } else {
            panic!("expected Fn decl");
        }
    }

    #[test]
    fn parse_type_attached_method_explicit_self_type_is_parsed_as_param() {
        // GIVEN: fn Counter::get(self: Counter) — explicit `self: T` (non-bare form)
        // THEN:  parsed normally; self param has type Counter, no rollback needed
        let d = fn_decl("fn Counter::get(self: Counter) -> Int { 0 }");
        assert_eq!(d.receiver_type, Some("Counter".to_string()));
        assert_eq!(d.name, "get");
        assert_eq!(d.params.len(), 1);
        assert_eq!(d.params[0].name, "self");
        assert!(
            matches!(&d.params[0].ty, TypeExpr::Base { name, .. } if name == "Counter"),
            "explicit self: Counter param should have Counter type"
        );
    }

    #[test]
    fn parse_type_attached_method_with_multiple_methods() {
        // GIVEN: two methods on the same type
        // THEN:  both parse without errors
        let src = r#"
fn Counter::get(self) -> Int { 0 }
fn Counter::set(self, n: Int) -> Unit { }
"#;
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        assert_eq!(prog.declarations.len(), 2);
        let names: Vec<_> = prog
            .declarations
            .iter()
            .filter_map(|d| {
                if let Decl::Fn(fd) = d {
                    Some(fd.name.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(names.contains(&"get") && names.contains(&"set"));
    }
}
