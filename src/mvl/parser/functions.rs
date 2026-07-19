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

use crate::mvl::parser::ast::{Constraint, FnDecl, Param, Totality};
use crate::mvl::parser::lexer::TokenKind;
use crate::mvl::parser::{ParseError, Parser};

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

    pub(crate) fn parse_param_list(&mut self) -> Result<Vec<Param>, ()> {
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
    pub(crate) fn parse_param_list_with_receiver(
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

    pub(crate) fn parse_param(&mut self) -> Result<Param, ()> {
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

    // ── Where-clause constraints (rejected) ───────────────────────────────

    /// The trailing `where T: Trait` clause on fn signatures is REJECTED —
    /// see ADR-0053.  MVL has no trait system; the grammar accepted these
    /// bounds silently and passed them through to the Rust emitter, which
    /// let rustc concepts (`Clone`, `Ord`, `Eq`) leak into MVL source.
    ///
    /// `where` in MVL means one thing: a solver-discharged refinement
    /// predicate on a param/return/field/alias.  Reserving the trailing
    /// slot for trait bounds violates the single-meaning rule and requires
    /// MVL to maintain a Rust-trait vocabulary it doesn't own.
    ///
    /// The function returns an empty vec unconditionally; a `where` token
    /// at this position emits a hard parse error pointing users at the
    /// refinement form or removal.
    pub(crate) fn parse_where_constraints(&mut self) -> Vec<Constraint> {
        if !matches!(self.peek_kind(), TokenKind::Where) {
            return Vec::new();
        }
        let where_span = self.peek_span();
        // Detect a `where <Ident>: <Ident>` shape so the diagnostic is precise.
        // Look at the two tokens after `where` — if they form `Name: Name`, it's
        // a former trait-bound clause.  Anything else at this position is also
        // wrong (refinement `where` only attaches to a type expression, not to
        // an fn signature) — same error.
        self.push_recover(ParseError {
            message: "trailing `where T: Trait` bound on fn signature is not \
                      valid MVL — see ADR-0053.  `where` in MVL is a solver \
                      predicate on a param/return type (`n: Int where self > 0`), \
                      not a trait bound.  Remove the clause; MVL has no trait \
                      system."
                .to_string(),
            span: where_span,
        });
        // Consume the malformed clause so the parser can continue and report
        // other errors in the same file.
        self.advance(); // eat `where`
        while !matches!(self.peek_kind(), TokenKind::LBrace | TokenKind::Eof) {
            self.advance();
        }
        Vec::new()
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

    #[test]
    fn parse_fn_with_bounded_forall_requires() {
        // #1915: fn with bounded universal quantifier in a requires clause.
        let d = fn_decl(
            "partial fn f() -> Int\n    requires forall i in [0..9]. i < 10\n{\n    0\n}",
        );
        assert_eq!(d.requires.len(), 1);
        assert!(matches!(d.requires[0], Expr::Quantifier(_, _)));
        let Expr::Quantifier(ref re, _) = d.requires[0] else {
            unreachable!()
        };
        assert!(
            matches!(**re, RefExpr::BoundedForall { .. }),
            "expected BoundedForall, got {:?}",
            re
        );
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
    fn where_trait_bound_on_fn_is_rejected() {
        // ADR-0053: trailing `where T: Trait` clause on fn signatures is not
        // valid MVL syntax.  Parser must produce a hard error diagnostic.
        let (mut p, _) = Parser::new("fn compare[T](a: T, b: T) -> Bool where T: Eq { }");
        let _ = p.parse_fn_decl();
        assert!(
            p.errors.iter().any(|e| e.message.contains("ADR-0053")),
            "expected ADR-0053 rejection diagnostic, got: {:?}",
            p.errors
        );
    }

    #[test]
    fn where_refinement_on_param_still_parses() {
        // Refinement predicates `n: Int where self > 0` remain valid — they
        // feed the Z3 solver.  Only trait bounds are rejected (ADR-0053).
        let d = fn_decl("fn foo(n: Int where self > 0) -> Int { n }");
        assert_eq!(d.name, "foo");
    }

    #[test]
    fn where_refinement_on_return_still_parses() {
        // Return-type refinement `-> Int where self > 0` remains valid.
        let d = fn_decl("fn foo() -> Int where self > 0 { 0 }");
        assert!(d.return_refinement.is_some());
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

    // ── Relabel declaration: wildcard support ──────────────────────────────

    #[test]
    fn parse_relabel_wildcard_destination() {
        // GIVEN: relabel unaudit_target: AuditTarget -> _
        // THEN: from == Some("AuditTarget"), to == None (wildcard)
        let src = "pub relabel unaudit_target: AuditTarget -> _";
        let (mut p, lex_errs) = Parser::new(src);
        assert!(lex_errs.is_empty(), "lex errors: {:?}", lex_errs);
        let prog = p.parse_program();
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        assert_eq!(prog.declarations.len(), 1);
        if let Decl::Relabel(rd) = &prog.declarations[0] {
            assert_eq!(rd.from, Some("AuditTarget".to_string()));
            assert_eq!(rd.to, None, "wildcard `_` should parse as None");
        } else {
            panic!("expected RelabelDecl");
        }
    }

    #[test]
    fn parse_relabel_wildcard_source() {
        // GIVEN: relabel audit_target: _ -> AuditTarget
        // THEN: from == None (wildcard), to == Some("AuditTarget")
        let src = "pub relabel audit_target: _ -> AuditTarget";
        let (mut p, lex_errs) = Parser::new(src);
        assert!(lex_errs.is_empty(), "lex errors: {:?}", lex_errs);
        let prog = p.parse_program();
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        assert_eq!(prog.declarations.len(), 1);
        if let Decl::Relabel(rd) = &prog.declarations[0] {
            assert_eq!(rd.from, None, "wildcard `_` source should parse as None");
            assert_eq!(rd.to, Some("AuditTarget".to_string()));
        } else {
            panic!("expected RelabelDecl");
        }
    }

    #[test]
    fn parse_relabel_both_wildcards() {
        // GIVEN: relabel symmetric: _ -> _
        // THEN: from == None, to == None (erase label both directions)
        let src = "pub relabel symmetric: _ -> _";
        let (mut p, lex_errs) = Parser::new(src);
        assert!(lex_errs.is_empty(), "lex errors: {:?}", lex_errs);
        let prog = p.parse_program();
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        if let Decl::Relabel(rd) = &prog.declarations[0] {
            assert_eq!(rd.from, None);
            assert_eq!(rd.to, None);
        } else {
            panic!("expected RelabelDecl");
        }
    }

    // ── #896: audit keyword on relabel declarations ────────────────────────

    #[test]
    fn parse_relabel_decl_audit_keyword() {
        // GIVEN: pub relabel release: Secret -> _ audit
        // THEN: audit == true
        let src = "pub relabel release: Secret -> _ audit";
        let (mut p, lex_errs) = Parser::new(src);
        assert!(lex_errs.is_empty(), "lex errors: {:?}", lex_errs);
        let prog = p.parse_program();
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        if let Decl::Relabel(rd) = &prog.declarations[0] {
            assert!(rd.audit, "expected audit=true");
            assert_eq!(rd.name, "release");
            assert_eq!(rd.from, Some("Secret".to_string()));
            assert_eq!(rd.to, None);
        } else {
            panic!("expected RelabelDecl");
        }
    }

    #[test]
    fn parse_relabel_decl_no_audit() {
        // GIVEN: pub relabel trust: Tainted -> _
        // THEN: audit == false
        let src = "pub relabel trust: Tainted -> _";
        let (mut p, lex_errs) = Parser::new(src);
        assert!(lex_errs.is_empty(), "lex errors: {:?}", lex_errs);
        let prog = p.parse_program();
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        if let Decl::Relabel(rd) = &prog.declarations[0] {
            assert!(!rd.audit, "expected audit=false");
        } else {
            panic!("expected RelabelDecl");
        }
    }
}
