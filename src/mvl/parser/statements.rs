// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Statement parser (Requirement 5).
//!
//! Parses all MVL statement forms within a block:
//! - `let [mut] pat [: Type] = expr;`
//! - `x = expr;` (assignment)
//! - `return [expr];`
//! - `if expr { } [else if … | else { }]`
//! - `match expr { pat => body, … }`
//! - `for pat in expr { }`
//! - `while expr { }`
//! - Expression statements (trailing `;` optional for block-final expressions)

use crate::mvl::parser::ast::{
    Block, ElseBranch, Expr, LValue, Literal, MatchArm, MatchBody, Pattern, Stmt,
};
use crate::mvl::parser::lexer::TokenKind;
use crate::mvl::parser::{ParseError, Parser};

impl Parser {
    // ── Block (real implementation — replaces stub in functions.rs) ────────

    /// Parse `{ stmts }`.
    pub fn parse_block(&mut self) -> Result<Block, ()> {
        let start = self.peek_span();
        let brace = self.expect(&TokenKind::LBrace);
        self.require(brace)?;

        let mut stmts = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            let pos_before = self.pos;
            match self.parse_stmt() {
                Ok(s) => stmts.push(s),
                Err(()) => {
                    if matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
                        break;
                    }
                    // Fix: if recovery stalled at a keyword (e.g. `fn`, `total`) without
                    // consuming any tokens, force-advance to prevent an infinite loop.
                    if self.pos == pos_before {
                        self.advance();
                    }
                }
            }
        }

        let rbrace = self.expect(&TokenKind::RBrace);
        self.require(rbrace)?;
        let span = self.span_from(start);
        Ok(Block { stmts, span })
    }

    // ── Statement dispatcher ───────────────────────────────────────────────

    pub fn parse_stmt(&mut self) -> Result<Stmt, ()> {
        // Reject exception-handling constructs with a clear diagnostic.
        if let TokenKind::Ident(kw) = self.peek_kind() {
            if matches!(kw.as_str(), "throw" | "try" | "catch") {
                let span = self.peek_span();
                let err = ParseError {
                    message: "MVL uses Result<T, E> for error handling, not exceptions.".into(),
                    span,
                };
                self.push_recover(err);
                return Err(());
            }
        }
        match self.peek_kind() {
            TokenKind::Let => self.parse_let_stmt(),
            TokenKind::Ghost => self.parse_ghost_let_stmt(),
            TokenKind::Return => self.parse_return_stmt(),
            TokenKind::If => self.parse_if_stmt(),
            TokenKind::Match => self.parse_match_stmt(),
            TokenKind::For => self.parse_for_stmt(),
            TokenKind::While => self.parse_while_stmt(),
            _ => self.parse_assign_or_expr_stmt(),
        }
    }

    // ── Let statement ──────────────────────────────────────────────────────

    fn parse_let_stmt(&mut self) -> Result<Stmt, ()> {
        let start = self.peek_span();
        self.advance(); // consume `let`

        let pattern = self.parse_pattern()?;

        let colon = self.expect(&TokenKind::Colon);
        self.require(colon)?;
        let ty = self.parse_type_expr()?;

        let eq = self.expect(&TokenKind::Eq);
        self.require(eq)?;
        let init = self.parse_expr()?;

        let semi = self.expect(&TokenKind::Semicolon);
        self.require(semi)?;

        let span = self.span_from(start);
        Ok(Stmt::Let {
            kind: crate::mvl::parser::ast::LetKind::Regular,
            pattern,
            ty,
            init,
            span,
        })
    }

    // ── Ghost let statement ────────────────────────────────────────────────

    /// Parse `ghost let name: T = expr;` — a specification-only binding (Phase 4, #627).
    /// Ghost bindings are type-checked normally but erased before transpilation/codegen.
    fn parse_ghost_let_stmt(&mut self) -> Result<Stmt, ()> {
        let start = self.peek_span();
        self.advance(); // consume `ghost`

        let let_kw = self.expect(&TokenKind::Let);
        self.require(let_kw)?;

        let pattern = self.parse_pattern()?;

        let colon = self.expect(&TokenKind::Colon);
        self.require(colon)?;
        let ty = self.parse_type_expr()?;

        let eq = self.expect(&TokenKind::Eq);
        self.require(eq)?;
        let init = self.parse_expr()?;

        let semi = self.expect(&TokenKind::Semicolon);
        self.require(semi)?;

        let span = self.span_from(start);
        Ok(Stmt::Let {
            kind: crate::mvl::parser::ast::LetKind::Ghost,
            pattern,
            ty,
            init,
            span,
        })
    }

    // ── Return statement ───────────────────────────────────────────────────

    fn parse_return_stmt(&mut self) -> Result<Stmt, ()> {
        let start = self.peek_span();
        self.advance(); // consume `return`

        let value = if matches!(
            self.peek_kind(),
            TokenKind::Semicolon | TokenKind::RBrace | TokenKind::Eof
        ) {
            None
        } else {
            Some(self.parse_expr()?)
        };

        self.eat(&TokenKind::Semicolon);
        let span = self.span_from(start);
        Ok(Stmt::Return { value, span })
    }

    // ── If statement ───────────────────────────────────────────────────────

    fn parse_if_stmt(&mut self) -> Result<Stmt, ()> {
        let start = self.peek_span();
        self.advance(); // consume `if`

        // `if let Pat = expr { body }` — desugar to exhaustive match
        if self.eat(&TokenKind::Let) {
            return self.parse_if_let_stmt(start);
        }

        let cond = self.parse_expr()?;
        let then = self.parse_block()?;

        let else_ = if self.eat(&TokenKind::Else) {
            if matches!(self.peek_kind(), TokenKind::If) {
                let inner = self.parse_if_stmt()?;
                Some(ElseBranch::If(Box::new(inner)))
            } else {
                Some(ElseBranch::Block(self.parse_block()?))
            }
        } else {
            None
        };

        let span = self.span_from(start);
        Ok(Stmt::If {
            cond,
            then,
            else_,
            span,
        })
    }

    /// Parse `if let Pat = expr { body } [else { alt }]` (already consumed `if` and `let`).
    ///
    /// Desugars to an exhaustive match so no new AST node, checker, or backend
    /// code is required:
    ///
    /// ```text
    /// match expr {
    ///     Pat => { body },
    ///     _   => { alt },   // or () if no else
    /// }
    /// ```
    fn parse_if_let_stmt(&mut self, start: crate::mvl::parser::lexer::Span) -> Result<Stmt, ()> {
        let pattern = self.parse_pattern()?;

        let eq = self.expect(&TokenKind::Eq);
        self.require(eq)?;

        let scrutinee = self.parse_expr()?;
        let body = self.parse_block()?;

        let else_body = if self.eat(&TokenKind::Else) {
            let alt = self.parse_block()?;
            MatchBody::Block(alt)
        } else {
            let span = self.span_from(start);
            MatchBody::Expr(Expr::Literal(Literal::Unit, span))
        };

        let span = self.span_from(start);

        let arms = vec![
            MatchArm {
                pattern,
                guard: None,
                body: MatchBody::Block(body),
                span,
            },
            MatchArm {
                pattern: Pattern::Wildcard(span),
                guard: None,
                body: else_body,
                span,
            },
        ];

        Ok(Stmt::Match {
            scrutinee,
            arms,
            span,
        })
    }

    // ── Match statement ────────────────────────────────────────────────────

    fn parse_match_stmt(&mut self) -> Result<Stmt, ()> {
        let start = self.peek_span();
        self.advance(); // consume `match`

        let scrutinee = self.parse_expr()?;

        let lbrace = self.expect(&TokenKind::LBrace);
        self.require(lbrace)?;

        let mut arms = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            match self.parse_match_arm() {
                Ok(arm) => arms.push(arm),
                Err(()) => break,
            }
        }

        let rbrace = self.expect(&TokenKind::RBrace);
        self.require(rbrace)?;

        let span = self.span_from(start);
        Ok(Stmt::Match {
            scrutinee,
            arms,
            span,
        })
    }

    // ── For statement ──────────────────────────────────────────────────────

    fn parse_for_stmt(&mut self) -> Result<Stmt, ()> {
        let start = self.peek_span();
        self.advance(); // consume `for`

        let pattern = self.parse_pattern()?;

        let in_kw = self.expect(&TokenKind::In);
        self.require(in_kw)?;

        let iter = self.parse_expr()?;

        // Optional invariant clauses: `invariant pred`* (Phase 3, #621)
        let mut invariants = Vec::new();
        while matches!(self.peek_kind(), TokenKind::Invariant) {
            self.advance(); // consume `invariant`
            invariants.push(self.parse_contract_expr()?);
        }

        let body = self.parse_block()?;

        let span = self.span_from(start);
        Ok(Stmt::For {
            pattern,
            iter,
            invariants,
            body,
            span,
        })
    }

    // ── While statement ────────────────────────────────────────────────────

    fn parse_while_stmt(&mut self) -> Result<Stmt, ()> {
        let start = self.peek_span();
        self.advance(); // consume `while`

        let cond = self.parse_expr()?;

        // Optional invariant clauses: `invariant pred`* (Phase 3, #621)
        let mut invariants = Vec::new();
        while matches!(self.peek_kind(), TokenKind::Invariant) {
            self.advance(); // consume `invariant`
            invariants.push(self.parse_contract_expr()?);
        }

        // Optional termination measure: `decreases expr` (Phase 5, #628)
        let decreases = if matches!(self.peek_kind(), TokenKind::Decreases) {
            self.advance(); // consume `decreases`
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };

        let body = self.parse_block()?;

        let span = self.span_from(start);
        Ok(Stmt::While {
            cond,
            invariants,
            decreases,
            body,
            span,
        })
    }

    // ── Assignment or expression statement ────────────────────────────────

    fn parse_assign_or_expr_stmt(&mut self) -> Result<Stmt, ()> {
        let start = self.peek_span();
        let expr = self.parse_expr()?;

        if self.eat(&TokenKind::Eq) {
            match expr_to_lvalue(expr) {
                Some(target) => {
                    let value = self.parse_expr()?;
                    self.eat(&TokenKind::Semicolon);
                    let span = self.span_from(start);
                    return Ok(Stmt::Assign {
                        target,
                        value,
                        span,
                    });
                }
                None => {
                    let err = ParseError {
                        message: "invalid assignment target".into(),
                        span: start,
                    };
                    self.push_recover(err);
                    return Err(());
                }
            }
        }

        self.eat(&TokenKind::Semicolon);
        let span = self.span_from(start);
        Ok(Stmt::Expr { expr, span })
    }

    // ── Match arms ─────────────────────────────────────────────────────────

    /// Parse a single match arm: `pat => body [,]`.
    /// Used by both statement-match and expression-match.
    pub fn parse_match_arm(&mut self) -> Result<MatchArm, ()> {
        let start = self.peek_span();
        let pattern = self.parse_pattern()?;

        // Optional guard: `pattern if expr => body` (#938)
        let guard = if self.eat(&TokenKind::If) {
            Some(self.parse_ref_expr()?)
        } else {
            None
        };
        let arrow = self.expect(&TokenKind::FatArrow);
        self.require(arrow)?;

        let body = if matches!(self.peek_kind(), TokenKind::LBrace) {
            MatchBody::Block(self.parse_block()?)
        } else {
            MatchBody::Expr(self.parse_expr()?)
        };

        self.eat(&TokenKind::Comma);

        let span = self.span_from(start);
        Ok(MatchArm {
            pattern,
            guard,
            body,
            span,
        })
    }

    // ── Patterns ───────────────────────────────────────────────────────────

    /// Parse a pattern (used in `let`, `match`, and `for` contexts).
    pub fn parse_pattern(&mut self) -> Result<Pattern, ()> {
        let start = self.peek_span();
        match self.peek_kind().clone() {
            TokenKind::Underscore => {
                let span = self.advance().span;
                Ok(Pattern::Wildcard(span))
            }
            TokenKind::Integer(n) => {
                let span = self.advance().span;
                Ok(Pattern::Literal(Literal::Integer(n), span))
            }
            TokenKind::Float(f) => {
                let span = self.advance().span;
                Ok(Pattern::Literal(Literal::Float(f), span))
            }
            // Fix #8: support negative integer literal patterns like `match n { -1 => … }`
            TokenKind::Minus => {
                self.advance(); // consume `-`
                match self.peek_kind().clone() {
                    TokenKind::Integer(n) => {
                        let span = self.advance().span;
                        Ok(Pattern::Literal(Literal::Integer(-n), span))
                    }
                    TokenKind::Float(f) => {
                        let span = self.advance().span;
                        Ok(Pattern::Literal(Literal::Float(-f), span))
                    }
                    _ => {
                        let err = ParseError {
                            message: "expected integer or float literal after `-` in pattern"
                                .into(),
                            span: self.peek_span(),
                        };
                        self.push_recover(err);
                        Err(())
                    }
                }
            }
            TokenKind::Str(s) => {
                let span = self.advance().span;
                Ok(Pattern::Literal(Literal::Str(s), span))
            }
            TokenKind::Char(c) => {
                let span = self.advance().span;
                Ok(Pattern::Literal(Literal::Char(c), span))
            }
            TokenKind::True => {
                let span = self.advance().span;
                Ok(Pattern::Literal(Literal::Bool(true), span))
            }
            TokenKind::False => {
                let span = self.advance().span;
                Ok(Pattern::Literal(Literal::Bool(false), span))
            }
            TokenKind::Ident(ref name) if name == "None" => {
                let span = self.advance().span;
                Ok(Pattern::None(span))
            }
            TokenKind::Ident(ref name) if name == "Some" => {
                self.advance();
                let lp = self.expect(&TokenKind::LParen);
                self.require(lp)?;
                let inner = self.parse_pattern()?;
                let rp = self.expect(&TokenKind::RParen);
                self.require(rp)?;
                let span = self.span_from(start);
                Ok(Pattern::Some {
                    inner: Box::new(inner),
                    span,
                })
            }
            TokenKind::Ident(ref name) if name == "Ok" => {
                self.advance();
                let lp = self.expect(&TokenKind::LParen);
                self.require(lp)?;
                let inner = self.parse_pattern()?;
                let rp = self.expect(&TokenKind::RParen);
                self.require(rp)?;
                let span = self.span_from(start);
                Ok(Pattern::Ok {
                    inner: Box::new(inner),
                    span,
                })
            }
            TokenKind::Ident(ref name) if name == "Err" => {
                self.advance();
                let lp = self.expect(&TokenKind::LParen);
                self.require(lp)?;
                let inner = self.parse_pattern()?;
                let rp = self.expect(&TokenKind::RParen);
                self.require(rp)?;
                let span = self.span_from(start);
                Ok(Pattern::Err {
                    inner: Box::new(inner),
                    span,
                })
            }
            TokenKind::Ident(name) => {
                self.advance();
                // Handle path patterns: Enum::Variant, Enum::Variant(fields), Enum::Variant { fields }
                let name = if matches!(self.peek_kind(), TokenKind::ColonColon) {
                    self.advance(); // consume `::`
                    match self.peek_kind().clone() {
                        TokenKind::Ident(variant) => {
                            self.advance();
                            format!("{name}::{variant}")
                        }
                        _ => name,
                    }
                } else {
                    name
                };
                if matches!(self.peek_kind(), TokenKind::LParen) {
                    // TupleStruct: Name(p1, p2, ...)
                    self.advance();
                    let mut fields = Vec::new();
                    while !matches!(self.peek_kind(), TokenKind::RParen | TokenKind::Eof) {
                        fields.push(self.parse_pattern()?);
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    let rp = self.expect(&TokenKind::RParen);
                    self.require(rp)?;
                    let span = self.span_from(start);
                    Ok(Pattern::TupleStruct { name, fields, span })
                } else if matches!(self.peek_kind(), TokenKind::LBrace) {
                    // Struct: Name { field: pat, ... }
                    self.advance();
                    let mut fields = Vec::new();
                    while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
                        let ir = self.expect_ident();
                        let (fname, _) = self.require(ir)?;
                        let colon = self.expect(&TokenKind::Colon);
                        self.require(colon)?;
                        let fp = self.parse_pattern()?;
                        fields.push((fname, fp));
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    let rb = self.expect(&TokenKind::RBrace);
                    self.require(rb)?;
                    let span = self.span_from(start);
                    Ok(Pattern::Struct { name, fields, span })
                } else {
                    Ok(Pattern::Ident(name, start))
                }
            }
            TokenKind::LParen => {
                // Tuple pattern: (p1, p2, ...)
                self.advance();
                let mut elems = Vec::new();
                while !matches!(self.peek_kind(), TokenKind::RParen | TokenKind::Eof) {
                    elems.push(self.parse_pattern()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                let rp = self.expect(&TokenKind::RParen);
                self.require(rp)?;
                let span = self.span_from(start);
                Ok(Pattern::Tuple { elems, span })
            }
            _ => {
                let err = ParseError {
                    message: format!("expected pattern, found `{}`", self.peek_kind()),
                    span: self.peek_span(),
                };
                self.push_recover(err);
                Err(())
            }
        }
    }
}

// ── LValue conversion ───────────────────────────────────────────────────────

fn expr_to_lvalue(expr: Expr) -> Option<LValue> {
    match expr {
        Expr::Ident(name, span) => Some(LValue::Ident(name, span)),
        Expr::FieldAccess { expr, field, span } => {
            let base = expr_to_lvalue(*expr)?;
            Some(LValue::Field {
                base: Box::new(base),
                field,
                span,
            })
        }
        _ => None,
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::mvl::parser::ast::*;
    use crate::mvl::parser::Parser;

    fn parse_stmts(src: &str) -> Vec<Stmt> {
        let (mut p, lex_errs) = Parser::new(src);
        assert!(lex_errs.is_empty(), "lex errors: {:?}", lex_errs);
        let block = p.parse_block();
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        block.expect("parse_block failed").stmts
    }

    fn one_stmt(src: &str) -> Stmt {
        let mut stmts = parse_stmts(src);
        assert_eq!(stmts.len(), 1, "expected exactly 1 statement");
        stmts.remove(0)
    }

    // ── Requirement 5 / Scenario: Parse let with type annotation ──────────

    #[test]
    fn parse_let_with_type() {
        // GIVEN: let x: Int = 42;
        // THEN: LetStmt with Regular kind, pattern=Ident("x"), type=Int, value=Literal(42)
        let s = one_stmt("{ let x: Int = 42; }");
        assert!(
            matches!(
                &s,
                Stmt::Let {
                    kind: LetKind::Regular,
                    pattern: Pattern::Ident(name, _),
                    ty: TypeExpr::Base { name: ty_name, .. },
                    init: Expr::Literal(Literal::Integer(42), _),
                    ..
                } if name == "x" && ty_name == "Int"
            ),
            "got: {:?}",
            s
        );
    }

    // ── Requirement 5 / Scenario: Parse mutable let (ref type) ───────────

    #[test]
    fn parse_let_mutable_ref() {
        // GIVEN: let count: ref Int = 0;  (ref in type encodes mutability)
        // THEN: LetStmt with Regular kind and Ref type
        let s = one_stmt("{ let count: ref Int = 0; }");
        assert!(
            matches!(
                &s,
                Stmt::Let {
                    kind: LetKind::Regular,
                    ty: TypeExpr::Ref { mutable: true, .. },
                    ..
                }
            ),
            "expected ref-typed let, got: {:?}",
            s
        );
    }

    #[test]
    fn parse_let_without_type() {
        // MVL forbids implicit types (#408) — the parser must reject `let y = 99;`
        let (mut p, lex_errs) = Parser::new("{ let y = 99; }");
        assert!(lex_errs.is_empty());
        let _block = p.parse_block();
        assert!(
            !p.errors.is_empty(),
            "expected parse error for let without type annotation"
        );
    }

    // ── Requirement 5 / Scenario: Parse exhaustive match ─────────────────

    #[test]
    fn parse_match_some_none() {
        // GIVEN: match option { Some(v) => v, None => 0 }
        // THEN: MatchStmt with two arms covering Some and None
        let s = one_stmt("{ match option { Some(v) => v, None => 0 } }");
        match &s {
            Stmt::Match { arms, .. } => {
                assert_eq!(arms.len(), 2);
                assert!(matches!(arms[0].pattern, Pattern::Some { .. }));
                assert!(matches!(arms[1].pattern, Pattern::None(_)));
            }
            _ => panic!("expected match stmt, got: {:?}", s),
        }
    }

    #[test]
    fn parse_match_ok_err() {
        let s = one_stmt("{ match result { Ok(v) => v, Err(e) => 0 } }");
        match &s {
            Stmt::Match { arms, .. } => {
                assert_eq!(arms.len(), 2);
                assert!(matches!(arms[0].pattern, Pattern::Ok { .. }));
                assert!(matches!(arms[1].pattern, Pattern::Err { .. }));
            }
            _ => panic!("expected match stmt, got: {:?}", s),
        }
    }

    #[test]
    fn parse_match_wildcard() {
        let s = one_stmt("{ match x { 0 => 1, _ => 2 } }");
        match &s {
            Stmt::Match { arms, .. } => {
                assert_eq!(arms.len(), 2);
                assert!(matches!(
                    arms[0].pattern,
                    Pattern::Literal(Literal::Integer(0), _)
                ));
                assert!(matches!(arms[1].pattern, Pattern::Wildcard(_)));
            }
            _ => panic!("expected match stmt, got: {:?}", s),
        }
    }

    // ── Guard patterns (#938) ─────────────────────────────────────────────

    #[test]
    fn parse_match_guard_simple() {
        let s = one_stmt("{ match n { x if x > 0 => 1, _ => 0 } }");
        match &s {
            Stmt::Match { arms, .. } => {
                assert_eq!(arms.len(), 2);
                assert!(matches!(arms[0].pattern, Pattern::Ident(..)));
                assert!(
                    arms[0].guard.is_some(),
                    "first arm should have a guard, got: {:?}",
                    arms[0]
                );
                assert!(arms[1].guard.is_none(), "wildcard arm has no guard");
            }
            _ => panic!("expected match stmt, got: {:?}", s),
        }
    }

    #[test]
    fn parse_match_guard_compound() {
        let s = one_stmt("{ match n { x if x > 0 && x < 100 => 1, _ => 0 } }");
        match &s {
            Stmt::Match { arms, .. } => {
                assert_eq!(arms.len(), 2);
                assert!(matches!(arms[0].guard, Some(RefExpr::LogicOp { .. })));
            }
            _ => panic!("expected match stmt, got: {:?}", s),
        }
    }

    #[test]
    fn parse_match_guard_with_block_body() {
        let s = one_stmt("{ match n { x if x > 0 => { x }, _ => { 0 } } }");
        match &s {
            Stmt::Match { arms, .. } => {
                assert_eq!(arms.len(), 2);
                assert!(arms[0].guard.is_some());
                assert!(matches!(arms[0].body, MatchBody::Block(..)));
            }
            _ => panic!("expected match stmt, got: {:?}", s),
        }
    }

    // ── Return ────────────────────────────────────────────────────────────

    #[test]
    fn parse_return_value() {
        let s = one_stmt("{ return 42; }");
        assert!(
            matches!(&s, Stmt::Return { value: Some(_), .. }),
            "got: {:?}",
            s
        );
    }

    #[test]
    fn parse_return_unit() {
        let s = one_stmt("{ return; }");
        assert!(
            matches!(&s, Stmt::Return { value: None, .. }),
            "got: {:?}",
            s
        );
    }

    // ── If / else ─────────────────────────────────────────────────────────

    #[test]
    fn parse_if_else() {
        let s = one_stmt("{ if x { return 1; } else { return 0; } }");
        assert!(
            matches!(&s, Stmt::If { else_: Some(_), .. }),
            "got: {:?}",
            s
        );
    }

    #[test]
    fn parse_if_else_if() {
        let s = one_stmt("{ if a { return 1; } else if b { return 2; } else { return 3; } }");
        match &s {
            Stmt::If {
                else_: Some(ElseBranch::If(_)),
                ..
            } => {}
            _ => panic!("expected else-if chain, got: {:?}", s),
        }
    }

    // ── For ───────────────────────────────────────────────────────────────

    #[test]
    fn parse_for_loop() {
        let s = one_stmt("{ for item in items { use_item(item); } }");
        match &s {
            Stmt::For { pattern, .. } => {
                assert!(matches!(pattern, Pattern::Ident(name, _) if name == "item"));
            }
            _ => panic!("expected for stmt, got: {:?}", s),
        }
    }

    // ── While ─────────────────────────────────────────────────────────────

    #[test]
    fn parse_while_loop() {
        let s = one_stmt("{ while running { tick(); } }");
        assert!(matches!(&s, Stmt::While { .. }), "got: {:?}", s);
    }

    // ── Assignment ────────────────────────────────────────────────────────

    #[test]
    fn parse_assignment() {
        let s = one_stmt("{ counter = 42; }");
        match &s {
            Stmt::Assign {
                target: LValue::Ident(name, _),
                ..
            } => {
                assert_eq!(name, "counter");
            }
            _ => panic!("expected assignment, got: {:?}", s),
        }
    }

    // ── Expression statement ──────────────────────────────────────────────

    #[test]
    fn parse_expr_stmt() {
        let s = one_stmt("{ greet(name); }");
        assert!(matches!(&s, Stmt::Expr { .. }), "got: {:?}", s);
    }

    // ── Multiple statements in block ──────────────────────────────────────

    #[test]
    fn parse_block_multiple_stmts() {
        let stmts = parse_stmts("{ let x: Int = 1; let y: Int = 2; return x; }");
        assert_eq!(stmts.len(), 3);
        assert!(matches!(stmts[0], Stmt::Let { .. }));
        assert!(matches!(stmts[1], Stmt::Let { .. }));
        assert!(matches!(stmts[2], Stmt::Return { .. }));
    }

    // ── Pattern variants ──────────────────────────────────────────────────

    #[test]
    fn parse_tuple_struct_pattern() {
        let s = one_stmt("{ match x { Point(a, b) => 0 } }");
        match &s {
            Stmt::Match { arms, .. } => {
                assert!(
                    matches!(&arms[0].pattern, Pattern::TupleStruct { name, .. } if name == "Point")
                );
            }
            _ => panic!("got: {:?}", s),
        }
    }

    #[test]
    fn parse_tuple_pattern() {
        let s = one_stmt("{ match x { (a, b) => 0 } }");
        match &s {
            Stmt::Match { arms, .. } => {
                assert!(
                    matches!(&arms[0].pattern, Pattern::Tuple { elems, .. } if elems.len() == 2)
                );
            }
            _ => panic!("got: {:?}", s),
        }
    }

    // ── Fix #8: negative integer literal patterns ──────────────────────────

    #[test]
    fn parse_negative_integer_pattern() {
        // GIVEN: match n { -1 => "neg", 0 => "zero", _ => "pos" }
        // THEN: first arm has Pattern::Literal(Integer(-1))
        let s = one_stmt("{ match n { -1 => x, 0 => y, _ => z } }");
        match &s {
            Stmt::Match { arms, .. } => {
                assert_eq!(arms.len(), 3);
                assert!(
                    matches!(arms[0].pattern, Pattern::Literal(Literal::Integer(-1), _)),
                    "expected Integer(-1) pattern, got {:?}",
                    arms[0].pattern
                );
                assert!(matches!(
                    arms[1].pattern,
                    Pattern::Literal(Literal::Integer(0), _)
                ));
                assert!(matches!(arms[2].pattern, Pattern::Wildcard(_)));
            }
            _ => panic!("expected match stmt, got: {:?}", s),
        }
    }

    // ── Req 8: No Exceptions ──────────────────────────────────────────────

    fn parse_stmts_with_errors(src: &str) -> Vec<crate::mvl::parser::ParseError> {
        let (mut p, _) = Parser::new(src);
        p.parse_block().ok();
        p.errors
    }

    #[test]
    fn throw_is_rejected() {
        // Spec 001 Req 8: MVL MUST NOT allow exception-based error handling
        let errs = parse_stmts_with_errors("{ throw SomeError; }");
        assert!(!errs.is_empty(), "expected parse error for throw");
        assert!(
            errs[0].message.contains("Result<T, E>"),
            "expected helpful error message, got: {}",
            errs[0].message
        );
    }

    #[test]
    fn try_is_rejected() {
        let errs = parse_stmts_with_errors("{ try { foo(); } }");
        assert!(!errs.is_empty(), "expected parse error for try");
        assert!(errs[0].message.contains("Result<T, E>"));
    }

    #[test]
    fn catch_is_rejected() {
        let errs = parse_stmts_with_errors("{ catch e { handle(e); } }");
        assert!(!errs.is_empty(), "expected parse error for catch");
        assert!(errs[0].message.contains("Result<T, E>"));
    }

    // ── Invariant with method call (#983) ────────────────────────────────

    #[test]
    fn while_invariant_method_call_not_dropped() {
        // GIVEN: while loop with invariant containing a method call
        // THEN: invariant clause is NOT silently dropped (#983)
        let s = one_stmt("{ while i < n invariant items.len() > 0 { i = i + 1; } }");
        assert!(
            matches!(&s, Stmt::While { invariants, .. } if invariants.len() == 1),
            "while invariant with method call must not be silently dropped (#983)"
        );
    }
}
