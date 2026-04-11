//! Expression parser (Requirement 6).
//!
//! Parses all MVL expression forms:
//! - Literals: integers, floats, strings, chars, booleans
//! - Identifiers, field access, method calls, function calls
//! - Binary operators with Pratt precedence climbing
//! - Unary operators: `-`, `!`
//! - Postfix `?` propagation
//! - `if`/`match` expressions
//! - List literals `[e1, e2, …]`
//! - Struct construction `Name { field: expr, … }`
//! - Block expressions `{ stmts }`
//! - Security-flow: `move(e)`, `consume(e)`, `declassify(e)`, `sanitize(e)`

use crate::mvl::parser::ast::{BinaryOp, Expr, Literal, UnaryOp};
use crate::mvl::parser::lexer::TokenKind;
use crate::mvl::parser::{ParseError, Parser};

impl Parser {
    // ── Entry point ────────────────────────────────────────────────────────

    pub fn parse_expr(&mut self) -> Result<Expr, ()> {
        self.parse_expr_prec(0)
    }

    // ── Pratt precedence climbing ──────────────────────────────────────────

    fn parse_expr_prec(&mut self, min_prec: u8) -> Result<Expr, ()> {
        let start = self.peek_span();
        let mut left = self.parse_unary()?;

        loop {
            let (op, prec): (BinaryOp, u8) = match self.peek_kind() {
                TokenKind::PipePipe => (BinaryOp::Or, 2),
                TokenKind::AmpAmp => (BinaryOp::And, 3),
                TokenKind::EqEq => (BinaryOp::Eq, 4),
                TokenKind::BangEq => (BinaryOp::Ne, 4),
                TokenKind::Lt => (BinaryOp::Lt, 4),
                TokenKind::Gt => (BinaryOp::Gt, 4),
                TokenKind::LtEq => (BinaryOp::Le, 4),
                TokenKind::GtEq => (BinaryOp::Ge, 4),
                TokenKind::Plus => (BinaryOp::Add, 5),
                TokenKind::Minus => (BinaryOp::Sub, 5),
                TokenKind::Star => (BinaryOp::Mul, 6),
                TokenKind::Slash => (BinaryOp::Div, 6),
                TokenKind::Percent => (BinaryOp::Rem, 6),
                _ => break,
            };

            if prec < min_prec {
                break;
            }

            self.advance(); // consume operator
            let right = self.parse_expr_prec(prec + 1)?;
            let span = self.span_from(start);
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    // ── Unary ──────────────────────────────────────────────────────────────

    fn parse_unary(&mut self) -> Result<Expr, ()> {
        let start = self.peek_span();
        match self.peek_kind() {
            TokenKind::Minus => {
                self.advance();
                let expr = self.parse_unary()?;
                let span = self.span_from(start);
                Ok(Expr::Unary {
                    op: UnaryOp::Neg,
                    expr: Box::new(expr),
                    span,
                })
            }
            TokenKind::Bang => {
                self.advance();
                let expr = self.parse_unary()?;
                let span = self.span_from(start);
                Ok(Expr::Unary {
                    op: UnaryOp::Not,
                    expr: Box::new(expr),
                    span,
                })
            }
            _ => self.parse_postfix(),
        }
    }

    // ── Postfix operators ──────────────────────────────────────────────────

    fn parse_postfix(&mut self) -> Result<Expr, ()> {
        let start = self.peek_span();
        let mut expr = self.parse_atom()?;

        loop {
            match self.peek_kind() {
                TokenKind::Dot => {
                    self.advance();
                    let ir = self.expect_ident();
                    let (name, _) = self.require(ir)?;
                    if matches!(self.peek_kind(), TokenKind::LParen) {
                        self.advance();
                        let args = self.parse_call_args()?;
                        let span = self.span_from(start);
                        expr = Expr::MethodCall {
                            receiver: Box::new(expr),
                            method: name,
                            args,
                            span,
                        };
                    } else {
                        let span = self.span_from(start);
                        expr = Expr::FieldAccess {
                            expr: Box::new(expr),
                            field: name,
                            span,
                        };
                    }
                }
                TokenKind::Question => {
                    let span = self.advance().span;
                    expr = Expr::Propagate {
                        expr: Box::new(expr),
                        span,
                    };
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    // ── Atoms ──────────────────────────────────────────────────────────────

    fn parse_atom(&mut self) -> Result<Expr, ()> {
        let start = self.peek_span();
        match self.peek_kind().clone() {
            // ── Literals ────────────────────────────────────────────────────
            TokenKind::Integer(n) => {
                let span = self.advance().span;
                Ok(Expr::Literal(Literal::Integer(n), span))
            }
            TokenKind::Float(f) => {
                let span = self.advance().span;
                Ok(Expr::Literal(Literal::Float(f), span))
            }
            TokenKind::Str(s) => {
                let span = self.advance().span;
                Ok(Expr::Literal(Literal::Str(s), span))
            }
            TokenKind::Char(c) => {
                let span = self.advance().span;
                Ok(Expr::Literal(Literal::Char(c), span))
            }
            TokenKind::True => {
                let span = self.advance().span;
                Ok(Expr::Literal(Literal::Bool(true), span))
            }
            TokenKind::False => {
                let span = self.advance().span;
                Ok(Expr::Literal(Literal::Bool(false), span))
            }

            // ── Security-flow operations ─────────────────────────────────────
            TokenKind::Move => {
                self.advance();
                let lp = self.expect(&TokenKind::LParen);
                self.require(lp)?;
                let inner = self.parse_expr()?;
                let rp = self.expect(&TokenKind::RParen);
                self.require(rp)?;
                let span = self.span_from(start);
                Ok(Expr::Move {
                    expr: Box::new(inner),
                    span,
                })
            }
            TokenKind::Consume => {
                self.advance();
                let lp = self.expect(&TokenKind::LParen);
                self.require(lp)?;
                let inner = self.parse_expr()?;
                let rp = self.expect(&TokenKind::RParen);
                self.require(rp)?;
                let span = self.span_from(start);
                Ok(Expr::Consume {
                    expr: Box::new(inner),
                    span,
                })
            }
            TokenKind::Declassify => {
                self.advance();
                let lp = self.expect(&TokenKind::LParen);
                self.require(lp)?;
                let inner = self.parse_expr()?;
                let rp = self.expect(&TokenKind::RParen);
                self.require(rp)?;
                let span = self.span_from(start);
                Ok(Expr::Declassify {
                    expr: Box::new(inner),
                    span,
                })
            }
            TokenKind::Sanitize => {
                self.advance();
                let lp = self.expect(&TokenKind::LParen);
                self.require(lp)?;
                let inner = self.parse_expr()?;
                let rp = self.expect(&TokenKind::RParen);
                self.require(rp)?;
                let span = self.span_from(start);
                Ok(Expr::Sanitize {
                    expr: Box::new(inner),
                    span,
                })
            }

            // ── Composite expressions ────────────────────────────────────────
            TokenKind::If => self.parse_if_expr(),
            TokenKind::Match => self.parse_match_expr(),
            TokenKind::LBrace => {
                let block = self.parse_block()?;
                Ok(Expr::Block(block))
            }

            // ── Parenthesised expression ─────────────────────────────────────
            TokenKind::LParen => {
                self.advance();
                let inner = self.parse_expr()?;
                let rp = self.expect(&TokenKind::RParen);
                self.require(rp)?;
                Ok(inner)
            }

            // ── List literal ─────────────────────────────────────────────────
            TokenKind::LBracket => {
                self.advance();
                let mut elems = Vec::new();
                while !matches!(self.peek_kind(), TokenKind::RBracket | TokenKind::Eof) {
                    elems.push(self.parse_expr()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                let rb = self.expect(&TokenKind::RBracket);
                self.require(rb)?;
                let span = self.span_from(start);
                Ok(Expr::List { elems, span })
            }

            // ── Identifier, function call, or struct construction ────────────
            TokenKind::Ident(name) => {
                self.advance();
                if matches!(self.peek_kind(), TokenKind::LParen) {
                    // Function call: name(args)
                    self.advance();
                    let args = self.parse_call_args()?;
                    let span = self.span_from(start);
                    Ok(Expr::FnCall {
                        name,
                        type_args: vec![],
                        args,
                        span,
                    })
                } else if matches!(self.peek_kind(), TokenKind::LBrace)
                    && self.looks_like_struct_init()
                {
                    // Struct construction: Name { field: expr, ... }
                    self.advance(); // consume `{`
                    let mut fields = Vec::new();
                    while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
                        let ir = self.expect_ident();
                        let (fname, _) = self.require(ir)?;
                        let colon = self.expect(&TokenKind::Colon);
                        self.require(colon)?;
                        let fval = self.parse_expr()?;
                        fields.push((fname, fval));
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    let rb = self.expect(&TokenKind::RBrace);
                    self.require(rb)?;
                    let span = self.span_from(start);
                    Ok(Expr::Construct { name, fields, span })
                } else {
                    Ok(Expr::Ident(name, start))
                }
            }

            _ => {
                let err = ParseError {
                    message: format!("expected expression, found `{}`", self.peek_kind()),
                    span: self.peek_span(),
                };
                self.push_recover(err);
                Err(())
            }
        }
    }

    // ── If expression ──────────────────────────────────────────────────────

    fn parse_if_expr(&mut self) -> Result<Expr, ()> {
        let start = self.peek_span();
        self.advance(); // consume `if`

        let cond = self.parse_expr()?;
        let then = self.parse_block()?;

        let else_ = if self.eat(&TokenKind::Else) {
            if matches!(self.peek_kind(), TokenKind::If) {
                let nested = self.parse_if_expr()?;
                Some(Box::new(nested))
            } else {
                let block = self.parse_block()?;
                Some(Box::new(Expr::Block(block)))
            }
        } else {
            None
        };

        let span = self.span_from(start);
        Ok(Expr::If {
            cond: Box::new(cond),
            then,
            else_,
            span,
        })
    }

    // ── Match expression ───────────────────────────────────────────────────

    fn parse_match_expr(&mut self) -> Result<Expr, ()> {
        let start = self.peek_span();
        self.advance(); // consume `match`

        let scrutinee = self.parse_expr()?;

        let lb = self.expect(&TokenKind::LBrace);
        self.require(lb)?;

        let mut arms = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            match self.parse_match_arm() {
                Ok(arm) => arms.push(arm),
                Err(()) => break,
            }
        }

        let rb = self.expect(&TokenKind::RBrace);
        self.require(rb)?;
        let span = self.span_from(start);
        Ok(Expr::Match {
            scrutinee: Box::new(scrutinee),
            arms,
            span,
        })
    }

    // ── Helpers ────────────────────────────────────────────────────────────

    /// Parse comma-separated argument list; consumes closing `)`.
    fn parse_call_args(&mut self) -> Result<Vec<Expr>, ()> {
        let mut args = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RParen | TokenKind::Eof) {
            args.push(self.parse_expr()?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let rp = self.expect(&TokenKind::RParen);
        self.require(rp)?;
        Ok(args)
    }

    /// Returns `true` when the current token is `{` and the next two tokens
    /// are `IDENT :` — indicating a struct initializer rather than a block.
    fn looks_like_struct_init(&self) -> bool {
        // self.pos points at `{`
        let next = (self.pos + 1).min(self.tokens.len().saturating_sub(1));
        let next2 = (self.pos + 2).min(self.tokens.len().saturating_sub(1));
        matches!(&self.tokens[next].kind, TokenKind::Ident(_))
            && matches!(&self.tokens[next2].kind, TokenKind::Colon)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::mvl::parser::ast::*;
    use crate::mvl::parser::Parser;

    fn parse_expr(src: &str) -> Expr {
        let (mut p, lex_errs) = Parser::new(src);
        assert!(lex_errs.is_empty(), "lex errors: {:?}", lex_errs);
        let result = p.parse_expr();
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        result.expect("parse_expr failed")
    }

    // ── Literals ──────────────────────────────────────────────────────────

    #[test]
    fn parse_integer_literal() {
        let e = parse_expr("42");
        assert!(matches!(e, Expr::Literal(Literal::Integer(42), _)));
    }

    #[test]
    fn parse_bool_literals() {
        assert!(matches!(
            parse_expr("true"),
            Expr::Literal(Literal::Bool(true), _)
        ));
        assert!(matches!(
            parse_expr("false"),
            Expr::Literal(Literal::Bool(false), _)
        ));
    }

    #[test]
    fn parse_string_literal() {
        let e = parse_expr("\"hello\"");
        assert!(matches!(e, Expr::Literal(Literal::Str(_), _)));
    }

    // ── Identifiers ───────────────────────────────────────────────────────

    #[test]
    fn parse_identifier() {
        let e = parse_expr("foo");
        assert!(matches!(&e, Expr::Ident(name, _) if name == "foo"));
    }

    // ── Function calls ────────────────────────────────────────────────────

    #[test]
    fn parse_fn_call() {
        let e = parse_expr("greet(name)");
        assert!(matches!(&e, Expr::FnCall { name, .. } if name == "greet"));
    }

    #[test]
    fn parse_fn_call_no_args() {
        let e = parse_expr("tick()");
        match &e {
            Expr::FnCall { name, args, .. } => {
                assert_eq!(name, "tick");
                assert!(args.is_empty());
            }
            _ => panic!("got: {:?}", e),
        }
    }

    // ── Method calls ──────────────────────────────────────────────────────

    #[test]
    fn parse_method_call() {
        let e = parse_expr("xs.map(f)");
        assert!(matches!(&e, Expr::MethodCall { method, .. } if method == "map"));
    }

    #[test]
    fn parse_field_access() {
        let e = parse_expr("point.x");
        assert!(matches!(&e, Expr::FieldAccess { field, .. } if field == "x"));
    }

    // ── Binary operators ──────────────────────────────────────────────────

    #[test]
    fn parse_binary_add() {
        let e = parse_expr("a + b");
        assert!(matches!(
            &e,
            Expr::Binary {
                op: BinaryOp::Add,
                ..
            }
        ));
    }

    #[test]
    fn parse_binary_precedence() {
        // a + b * c  =>  a + (b * c)
        let e = parse_expr("a + b * c");
        match &e {
            Expr::Binary {
                op: BinaryOp::Add,
                right,
                ..
            } => {
                assert!(matches!(
                    right.as_ref(),
                    Expr::Binary {
                        op: BinaryOp::Mul,
                        ..
                    }
                ));
            }
            _ => panic!("expected Add at top, got: {:?}", e),
        }
    }

    #[test]
    fn parse_comparison() {
        let e = parse_expr("x > 0");
        assert!(matches!(
            &e,
            Expr::Binary {
                op: BinaryOp::Gt,
                ..
            }
        ));
    }

    // ── Unary ─────────────────────────────────────────────────────────────

    #[test]
    fn parse_unary_neg() {
        let e = parse_expr("-42");
        assert!(matches!(
            &e,
            Expr::Unary {
                op: UnaryOp::Neg,
                ..
            }
        ));
    }

    #[test]
    fn parse_unary_not() {
        let e = parse_expr("!flag");
        assert!(matches!(
            &e,
            Expr::Unary {
                op: UnaryOp::Not,
                ..
            }
        ));
    }

    // ── Requirement 6 / Scenario: Parse ? propagation ─────────────────────

    #[test]
    fn parse_propagation() {
        // GIVEN: parse_int(input)?
        // THEN: PropagateExpr wrapping a CallExpr
        let e = parse_expr("parse_int(input)?");
        assert!(
            matches!(&e, Expr::Propagate { expr, .. } if matches!(expr.as_ref(), Expr::FnCall { .. })),
            "got: {:?}",
            e
        );
    }

    // ── Requirement 6 / Scenario: Parse sanitize ──────────────────────────

    #[test]
    fn parse_sanitize() {
        // GIVEN: sanitize(user_input)
        // THEN: SanitizeExpr wrapping an identifier
        let e = parse_expr("sanitize(user_input)");
        assert!(
            matches!(&e, Expr::Sanitize { expr, .. } if matches!(expr.as_ref(), Expr::Ident(_, _))),
            "got: {:?}",
            e
        );
    }

    #[test]
    fn parse_declassify() {
        let e = parse_expr("declassify(secret)");
        assert!(matches!(&e, Expr::Declassify { .. }), "got: {:?}", e);
    }

    // ── Requirement 6 / Scenario: Parse if-expression ─────────────────────

    #[test]
    fn parse_if_expr() {
        // GIVEN: if valid { ok_value } else { err_value }
        // THEN: IfExpr with both branches
        let e = parse_expr("if valid { ok_value } else { err_value }");
        match &e {
            Expr::If { else_, .. } => {
                assert!(else_.is_some(), "expected else branch");
            }
            _ => panic!("expected if-expr, got: {:?}", e),
        }
    }

    // ── List literal ──────────────────────────────────────────────────────

    #[test]
    fn parse_list_literal() {
        let e = parse_expr("[1, 2, 3]");
        match &e {
            Expr::List { elems, .. } => assert_eq!(elems.len(), 3),
            _ => panic!("got: {:?}", e),
        }
    }

    // ── Match expression ──────────────────────────────────────────────────

    #[test]
    fn parse_match_expr() {
        let e = parse_expr("match x { 0 => true, _ => false }");
        assert!(matches!(&e, Expr::Match { .. }), "got: {:?}", e);
    }

    // ── Struct construction ───────────────────────────────────────────────

    #[test]
    fn parse_struct_construction() {
        let e = parse_expr("Point { x: 1, y: 2 }");
        match &e {
            Expr::Construct { name, fields, .. } => {
                assert_eq!(name, "Point");
                assert_eq!(fields.len(), 2);
            }
            _ => panic!("got: {:?}", e),
        }
    }
}
