//! Function-declaration parser (Requirement 4).
//!
//! Parses:
//! - `[total|partial] fn Name [<TypeParams>] (params) -> ReturnType [! Effects] [where Constraints] { body }`
//! - Parameters with optional capability (`iso`/`val`/`ref`/`tag`), `mut`, type, and refinement
//! - Totality annotations, effect lists, and where-clause constraints

use crate::mvl::parser::ast::{Constraint, FnDecl, Param, Totality};
use crate::mvl::parser::lexer::TokenKind;
use crate::mvl::parser::{ParseError, Parser};

impl Parser {
    // ── Function declarations ─────────────────────────────────────────────

    /// Parse `[total|partial] fn Name …`.
    /// Pre-condition: current token is `total`, `partial`, or `fn`.
    pub fn parse_fn_decl(&mut self) -> Result<FnDecl, ()> {
        let start = self.peek_span();

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

        // `fn` keyword
        let fn_kw = self.expect(&TokenKind::Fn);
        self.require(fn_kw)?;

        // Function name
        let ident_result = self.expect_ident();
        let (name, _) = self.require(ident_result)?;

        // Optional generic type parameters
        let type_params = self.parse_type_params_decl();

        // Parameter list
        let params = self.parse_param_list()?;

        // `-> return_type`
        let arrow = self.expect(&TokenKind::Arrow);
        self.require(arrow)?;
        let return_type = self.parse_type_expr()?;

        // Optional inline return refinement: `-> T where pred` (before effects)
        // We parse it only if `where` follows AND the next token after is NOT an ident `:`
        // (which would indicate function-level constraints, not type refinement).
        let return_refinement = self.try_parse_return_refinement();

        // Optional effect list: `! Effect, Effect`
        let effects = self.parse_optional_effects();

        // Optional where-clause constraints: `where T: Trait, U: Trait`
        let constraints = self.parse_where_constraints();

        // Body block
        let body = self.parse_block()?;

        let span = self.span_from(start);
        Ok(FnDecl {
            totality,
            name,
            type_params,
            params,
            return_type: Box::new(return_type),
            return_refinement,
            effects,
            constraints,
            body,
            span,
        })
    }

    // ── Parameter list ────────────────────────────────────────────────────

    fn parse_param_list(&mut self) -> Result<Vec<Param>, ()> {
        let paren = self.expect(&TokenKind::LParen);
        self.require(paren)?;

        let mut params = Vec::new();
        loop {
            if matches!(self.peek_kind(), TokenKind::RParen | TokenKind::Eof) {
                break;
            }
            match self.parse_param() {
                Ok(p) => params.push(p),
                Err(()) => break,
            }
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

        // Optional `mut`
        let mutable = self.eat(&TokenKind::Mut);

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
            mutable,
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
        match self.peek_kind() {
            TokenKind::Type => Ok(Decl::Type(self.parse_type_decl()?)),
            TokenKind::Fn | TokenKind::Total | TokenKind::Partial => {
                Ok(Decl::Fn(self.parse_fn_decl()?))
            }
            TokenKind::Const => Ok(Decl::Const(self.parse_const_decl()?)),
            TokenKind::Module => Ok(Decl::Module(self.parse_module_decl()?)),
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
            name,
            ty,
            value,
            span,
        })
    }

    pub fn parse_module_decl(&mut self) -> Result<crate::mvl::parser::ast::ModuleDecl, ()> {
        let start = self.peek_span();
        self.advance(); // consume `module`
        let ident_result = self.expect_ident();
        let (name, _) = self.require(ident_result)?;
        let brace = self.expect(&TokenKind::LBrace);
        self.require(brace)?;

        let mut declarations = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            let pos_before = self.pos;
            if let Ok(d) = self.parse_decl() {
                declarations.push(d);
            }
            // Fix: if recovery stalled without consuming tokens, force-advance.
            if !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof)
                && self.pos == pos_before
            {
                self.advance();
            }
        }
        let rbrace = self.expect(&TokenKind::RBrace);
        self.require(rbrace)?;
        let span = self.span_from(start);
        Ok(crate::mvl::parser::ast::ModuleDecl {
            name,
            declarations,
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
        // GIVEN: total fn read(path: Path) -> Result<String, IOError> ! FileRead { }
        // THEN: FnDecl with totality=Total, effects=[FileRead], return=Result<String, IOError>
        let d = fn_decl("total fn read(path: Path) -> Result<String, IOError> ! FileRead { }");
        assert_eq!(d.totality, Some(Totality::Total));
        assert_eq!(d.name, "read");
        assert_eq!(d.effects, vec!["FileRead"]);
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
        // GIVEN: fn process(iso db: &DbConn) -> Result<Data, Error> ! DB { }
        // THEN: parameter has capability=Iso, type=Ref(DbConn)
        let d = fn_decl("fn process(iso db: &DbConn) -> Result<Data, Error> ! DB { }");
        assert_eq!(d.params[0].capability, Some(Capability::Iso));
        assert_eq!(d.params[0].name, "db");
        assert!(matches!(
            d.params[0].ty,
            TypeExpr::Ref { mutable: false, .. }
        ));
        assert_eq!(d.effects, vec!["DB"]);
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
        // GIVEN: fn handle(input: Tainted<String>, key: Secret<ApiKey>) -> Public<Response>
        // THEN: params have correct security labels, return has Public label
        let d = fn_decl(
            "fn handle(input: Tainted<String>, key: Secret<ApiKey>) -> Public<Response> { }",
        );
        assert_eq!(d.params.len(), 2);
        assert!(matches!(
            d.params[0].ty,
            TypeExpr::Labeled {
                label: SecurityLabel::Tainted,
                ..
            }
        ));
        assert!(matches!(
            d.params[1].ty,
            TypeExpr::Labeled {
                label: SecurityLabel::Secret,
                ..
            }
        ));
        assert!(matches!(
            *d.return_type,
            TypeExpr::Labeled {
                label: SecurityLabel::Public,
                ..
            }
        ));
    }

    #[test]
    fn parse_fn_multiple_effects() {
        let d = fn_decl("fn log(msg: String) -> Unit ! DB, Console { }");
        assert_eq!(d.effects, vec!["DB", "Console"]);
    }

    #[test]
    fn parse_fn_with_mut_param() {
        let d = fn_decl("fn inc(mut count: Int) -> Int { }");
        assert!(d.params[0].mutable);
        assert_eq!(d.params[0].name, "count");
    }

    #[test]
    fn parse_fn_no_params() {
        let d = fn_decl("fn unit() -> Unit { }");
        assert!(d.params.is_empty());
    }

    #[test]
    fn parse_fn_with_type_params() {
        let d = fn_decl("fn identity<T>(x: T) -> T { }");
        assert_eq!(d.type_params, vec!["T"]);
        assert_eq!(d.params[0].name, "x");
    }

    #[test]
    fn parse_fn_param_with_refinement() {
        let d = fn_decl("fn positive(x: Int where self > 0) -> Int { }");
        assert!(d.params[0].refinement.is_some());
    }

    #[test]
    fn parse_fn_where_constraints() {
        let d = fn_decl("fn compare<T>(a: T, b: T) -> Bool where T: Eq { }");
        assert_eq!(d.constraints.len(), 1);
        assert_eq!(d.constraints[0].name, "T");
        assert_eq!(d.constraints[0].bound, "Eq");
    }

    #[test]
    fn parse_authenticate_from_corpus() {
        // From tests/corpus/09_full_programs/auth_handler.mvl
        let src = r#"total fn authenticate(
    iso db: &DbConn,
    input_password: Tainted<String>,
    user_id: Public<UserId>
) -> Result<Session, AuthError> ! DB, Console { }"#;
        let d = fn_decl(src);
        assert_eq!(d.totality, Some(Totality::Total));
        assert_eq!(d.name, "authenticate");
        assert_eq!(d.params.len(), 3);
        assert_eq!(d.params[0].capability, Some(Capability::Iso));
        assert!(matches!(
            d.params[1].ty,
            TypeExpr::Labeled {
                label: SecurityLabel::Tainted,
                ..
            }
        ));
        assert!(matches!(
            d.params[2].ty,
            TypeExpr::Labeled {
                label: SecurityLabel::Public,
                ..
            }
        ));
        assert!(matches!(*d.return_type, TypeExpr::Result { .. }));
        assert_eq!(d.effects, vec!["DB", "Console"]);
    }
}
