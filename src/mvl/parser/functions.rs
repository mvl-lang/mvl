//! Function-declaration parser (Requirement 4).
//!
//! Parses:
//! - `[total|partial] fn Name [<TypeParams>] (params) -> ReturnType [! Effects] [where Constraints] { body }`
//! - `test fn Name() -> Unit { body }` — unit test function
//! - Parameters with optional capability (`iso`/`val`/`ref`/`tag`), `mut`, type, and refinement
//! - Totality annotations, effect lists, and where-clause constraints

use crate::mvl::parser::ast::{
    Constraint, ExternDecl, ExternFnDecl, FnDecl, Param, Totality, UseDecl,
};
use crate::mvl::parser::lexer::TokenKind;
use crate::mvl::parser::{ParseError, Parser};

impl Parser {
    // ── Function declarations ─────────────────────────────────────────────

    /// Parse `[test] [total|partial] fn Name …`.
    /// Pre-condition: current token is `test`, `total`, `partial`, or `fn`.
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
            visible: false, // set by parse_decl when `pub` prefix is present
            is_test,
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

        // Optional `pub` visibility modifier
        let visible = self.eat(&TokenKind::Pub);

        match self.peek_kind() {
            TokenKind::Use => Ok(Decl::Use(self.parse_use_decl(visible)?)),
            TokenKind::Type => {
                let mut d = self.parse_type_decl()?;
                d.visible = visible;
                Ok(Decl::Type(d))
            }
            TokenKind::Fn | TokenKind::Total | TokenKind::Partial | TokenKind::Test => {
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

        // Parse module path: one or more `::` -separated identifiers
        let mut path = Vec::new();
        let ident_result = self.expect_ident();
        let (first, _) = self.require(ident_result)?;
        path.push(first);

        while self.eat(&TokenKind::ColonColon) {
            let ident_result = self.expect_ident();
            let (seg, _) = self.require(ident_result)?;
            path.push(seg);
        }

        let semi = self.expect(&TokenKind::Semicolon);
        self.require(semi)?;

        let span = self.span_from(start);
        Ok(UseDecl {
            reexport,
            path,
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
                    if let Ok(f) = self.parse_extern_fn_decl() {
                        fns.push(f);
                    }
                }
                _ => {
                    let err = ParseError {
                        message: format!(
                            "expected `fn` inside extern block, found `{}`",
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

    /// Parse a single `fn foo(params) -> RetType [! Effects]` inside an extern block.
    /// Note: no body (terminated by `;` or end of signature before next `fn`/`}`).
    fn parse_extern_fn_decl(&mut self) -> Result<ExternFnDecl, ()> {
        let start = self.peek_span();
        self.advance(); // consume `fn`

        let ident_result = self.expect_ident();
        let (name, _) = self.require(ident_result)?;

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

        // Optional effects: `! Console, Net`
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
    fn sha256(data: &String) -> String;
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
    fn http_get(url: String) -> Result<String, String> ! Net;
}"#,
        );
        assert_eq!(ed.fns.len(), 1);
        assert_eq!(ed.fns[0].name, "http_get");
        assert_eq!(ed.fns[0].effects, vec!["Net"]);
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
        assert_eq!(ed.fns[1].effects, vec!["DB"]);
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
}
