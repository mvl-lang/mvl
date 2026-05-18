// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Type-declaration and type-expression parser.
//!
//! Handles:
//! - `type Name [[params]] = struct { … }` (Requirement 3)
//! - `type Name [[params]] = enum { … }`   (Requirement 3)
//! - `type Name = ExistingType`             (Requirement 3)
//! - `type Name = T where predicate`        (Requirement 3)
//! - All type expressions including security labels (Requirement 7)

use crate::mvl::parser::ast::{
    ArithOp, Capability, CmpOp, Effect, FieldDecl, GenericParam, LogicOp, RefExpr, SecurityLabel,
    SessionOp, TypeBody, TypeDecl, TypeExpr, Variant, VariantFields,
};
use crate::mvl::parser::lexer::{Span, TokenKind};
use crate::mvl::parser::{ParseError, Parser};

impl Parser {
    // ── Type declarations ─────────────────────────────────────────────────

    /// Parse `type Name [<params>] = type_body`.
    /// Pre-condition: current token is `type`.
    pub fn parse_type_decl(&mut self) -> Result<TypeDecl, ()> {
        let start = self.peek_span();
        self.advance(); // consume `type`

        let ident_result = self.expect_ident();
        let (name, _) = self.require(ident_result)?;

        let params = self.parse_type_params_decl();

        let eq = self.expect(&TokenKind::Eq);
        self.require(eq)?;

        let body = self.parse_type_body()?;

        let span = self.span_from(start);
        Ok(TypeDecl {
            visible: false, // set by parse_decl when `pub` prefix is present
            name,
            params,
            body,
            span,
        })
    }

    /// Parse `struct { … }`, `enum { … }`, or a type expression (alias).
    fn parse_type_body(&mut self) -> Result<TypeBody, ()> {
        match self.peek_kind() {
            // Fix #5: match reserved TokenKind::Struct / TokenKind::Enum instead of
            // string-guarded Ident, so these keywords are properly reserved.
            TokenKind::Struct => {
                self.advance();
                let fields = self.parse_struct_body()?;
                let invariant = if matches!(self.peek_kind(), TokenKind::With) {
                    self.advance(); // consume `with`
                    let inv = self.expect(&TokenKind::Invariant);
                    self.require(inv)?;
                    Some(self.parse_ref_expr()?)
                } else {
                    None
                };
                Ok(TypeBody::Struct { fields, invariant })
            }
            TokenKind::Enum => {
                self.advance();
                let variants = self.parse_enum_body()?;
                Ok(TypeBody::Enum(variants))
            }
            _ => {
                let ty = self.parse_alias_body()?;
                Ok(TypeBody::Alias(Box::new(ty)))
            }
        }
    }

    /// Parse `{ field* }` after `struct`.
    fn parse_struct_body(&mut self) -> Result<Vec<FieldDecl>, ()> {
        let brace = self.expect(&TokenKind::LBrace);
        self.require(brace)?;

        let mut fields = Vec::new();
        loop {
            if matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
                break;
            }
            if let Ok(f) = self.parse_field_decl() {
                fields.push(f);
            } else {
                break;
            }
            // Fields are comma-separated; trailing comma allowed
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }

        let rbrace = self.expect(&TokenKind::RBrace);
        self.require(rbrace)?;
        Ok(fields)
    }

    /// Parse `{ Variant, Variant, … }` after `enum`.
    fn parse_enum_body(&mut self) -> Result<Vec<Variant>, ()> {
        let brace = self.expect(&TokenKind::LBrace);
        self.require(brace)?;

        let mut variants = Vec::new();
        loop {
            if matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
                break;
            }
            // Fix #7: break on variant parse failure (mirrors parse_struct_body)
            if let Ok(v) = self.parse_variant() {
                variants.push(v);
            } else {
                break;
            }
            // Variants are comma-separated; trailing comma allowed
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }

        let rbrace = self.expect(&TokenKind::RBrace);
        self.require(rbrace)?;
        Ok(variants)
    }

    /// Parse a single struct field: `name: type [where pred]`.
    pub(crate) fn parse_field_decl(&mut self) -> Result<FieldDecl, ()> {
        let start = self.peek_span();
        let ident_result = self.expect_ident();
        let (name, _) = self.require(ident_result)?;

        let colon = self.expect(&TokenKind::Colon);
        self.require(colon)?;

        let ty = self.parse_type_expr()?;

        let refinement = if self.eat(&TokenKind::Where) {
            Some(self.parse_ref_expr()?)
        } else {
            None
        };

        let span = self.span_from(start);
        Ok(FieldDecl {
            name,
            ty,
            refinement,
            span,
        })
    }

    /// Parse a single enum variant: `Name`, `Name(types)`, or `Name { fields }`.
    fn parse_variant(&mut self) -> Result<Variant, ()> {
        let start = self.peek_span();
        let ident_result = self.expect_ident();
        let (name, _) = self.require(ident_result)?;

        let fields = match self.peek_kind() {
            TokenKind::LParen => {
                self.advance();
                let mut tys = Vec::new();
                while !matches!(self.peek_kind(), TokenKind::RParen | TokenKind::Eof) {
                    match self.parse_type_expr() {
                        Ok(ty) => tys.push(ty),
                        Err(()) => break,
                    }
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                let paren = self.expect(&TokenKind::RParen);
                self.require(paren)?;
                VariantFields::Tuple(tys)
            }
            TokenKind::LBrace => {
                self.advance();
                let mut field_decls = Vec::new();
                while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
                    if let Ok(f) = self.parse_field_decl() {
                        field_decls.push(f);
                    }
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                let brace = self.expect(&TokenKind::RBrace);
                self.require(brace)?;
                VariantFields::Struct(field_decls)
            }
            _ => VariantFields::Unit,
        };

        let span = self.span_from(start);
        Ok(Variant { name, fields, span })
    }

    /// Parse a type alias body: a type expression with optional trailing
    /// `where pred` (producing `TypeExpr::Refined`).
    fn parse_alias_body(&mut self) -> Result<TypeExpr, ()> {
        let start = self.peek_span();
        let ty = self.parse_type_expr()?;
        if self.eat(&TokenKind::Where) {
            let pred = self.parse_ref_expr()?;
            let span = self.span_from(start);
            Ok(TypeExpr::Refined {
                inner: Box::new(ty),
                pred,
                span,
            })
        } else {
            Ok(ty)
        }
    }

    // ── Generic type-parameter declarations ───────────────────────────────

    /// Parse optional `[A, B, const N: Int]` on a type or function declaration.
    /// Returns empty vec if the next token is not `[`.
    pub fn parse_type_params_decl(&mut self) -> Vec<GenericParam> {
        if !self.eat(&TokenKind::LBracket) {
            return Vec::new();
        }
        let mut params = Vec::new();
        loop {
            if matches!(self.peek_kind(), TokenKind::RBracket | TokenKind::Eof) {
                break;
            }
            // Const generic: `const N: Int`
            if self.eat(&TokenKind::Const) {
                match self.expect_ident() {
                    Ok((name, _)) => {
                        if let Err(e) = self.expect(&TokenKind::Colon) {
                            self.push_error(e);
                            break;
                        }
                        match self.expect_ident() {
                            Ok((ty, _)) => params.push(GenericParam::Const(name, ty)),
                            Err(e) => {
                                self.push_error(e);
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        self.push_error(e);
                        break;
                    }
                }
            } else {
                match self.expect_ident() {
                    Ok((name, _)) => {
                        // Reject HKT syntax: `F[_]` is not supported.
                        if matches!(self.peek_kind(), TokenKind::LBracket) {
                            self.push_error(ParseError {
                                message: format!(
                                    "higher-kinded type parameter `{name}[_]` is not supported; type parameters must be simple identifiers"
                                ),
                                span: self.peek_span(),
                            });
                            break;
                        }
                        // Reject inline constraint syntax: `T: Trait`; use `where` instead.
                        if matches!(self.peek_kind(), TokenKind::Colon) {
                            self.push_error(ParseError {
                                message: format!(
                                    "inline constraint syntax `{name}: Trait` is not supported; use a `where` clause instead"
                                ),
                                span: self.peek_span(),
                            });
                            break;
                        }
                        params.push(GenericParam::Type(name));
                    }
                    Err(e) => {
                        self.push_error(e);
                        break;
                    }
                }
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        // Fix #13: report a diagnostic if the closing `]` is missing rather
        // than silently accepting `fn f[T(x: T) -> T` as valid syntax.
        if !self.eat(&TokenKind::RBracket) {
            self.push_error(ParseError {
                message: "expected `]` to close type parameter list".into(),
                span: self.peek_span(),
            });
        }
        params
    }

    // ── Type expressions ──────────────────────────────────────────────────

    /// Parse a type expression (without consuming a trailing `where`).
    ///
    /// Callers that allow refinement (field/param declarations, alias bodies)
    /// must check for `where` themselves.
    pub fn parse_type_expr(&mut self) -> Result<TypeExpr, ()> {
        let start = self.peek_span();
        let kind = self.peek_kind().clone();

        match kind {
            // val T (immutable reference — replaces &T)
            TokenKind::Val => {
                self.advance();
                let inner = self.parse_type_expr()?;
                let span = self.span_from(start);
                Ok(TypeExpr::Ref {
                    mutable: false,
                    inner: Box::new(inner),
                    span,
                })
            }

            // ref T (mutable reference — replaces &mut T)
            TokenKind::Ref => {
                self.advance();
                let inner = self.parse_type_expr()?;
                let span = self.span_from(start);
                Ok(TypeExpr::Ref {
                    mutable: true,
                    inner: Box::new(inner),
                    span,
                })
            }

            // Session type: `&{ l: S, ... }` — external choice (other side selects branch).
            // Falls through to the Rust-borrow error for `&T` (non-brace lookahead).
            TokenKind::Amp if matches!(self.peek_kind_at(1), TokenKind::LBrace) => {
                let op = self.parse_session_op()?;
                let span = self.span_from(start);
                Ok(TypeExpr::Session {
                    op: Box::new(op),
                    span,
                })
            }

            // Reject Rust-style borrow syntax with helpful error
            TokenKind::Amp => {
                self.advance();
                // Try to parse the inner type so we can give a better error
                let _ = self.parse_type_expr();
                let span = self.span_from(start);
                let err = ParseError {
                    message: "use `val T` or `ref T` instead of `&T`".to_string(),
                    span,
                };
                self.push_recover(err);
                Err(())
            }

            // Session type: `!T. S` — send T then continue as S.
            TokenKind::Bang => {
                let op = self.parse_session_op()?;
                let span = self.span_from(start);
                Ok(TypeExpr::Session {
                    op: Box::new(op),
                    span,
                })
            }

            // Session type: `?T. S` — receive T then continue as S.
            TokenKind::Question => {
                let op = self.parse_session_op()?;
                let span = self.span_from(start);
                Ok(TypeExpr::Session {
                    op: Box::new(op),
                    span,
                })
            }

            // Session type: `+{ l: S, ... }` — internal choice (this side selects branch).
            TokenKind::Plus if matches!(self.peek_kind_at(1), TokenKind::LBrace) => {
                let op = self.parse_session_op()?;
                let span = self.span_from(start);
                Ok(TypeExpr::Session {
                    op: Box::new(op),
                    span,
                })
            }

            // fn(A, B) -> C  [! Effects]
            TokenKind::Fn => {
                self.advance();
                let params = self.parse_fn_type_params()?;
                let arrow = self.expect(&TokenKind::Arrow);
                self.require(arrow)?;
                let ret = self.parse_type_expr()?;
                let effects = self.parse_optional_effects();
                let span = self.span_from(start);
                Ok(TypeExpr::Fn {
                    params,
                    ret: Box::new(ret),
                    effects,
                    span,
                })
            }

            // (A, B, C)  — tuple type
            TokenKind::LParen => {
                self.advance();
                let first = self.parse_type_expr()?;
                if !self.eat(&TokenKind::Comma) {
                    // single-element paren — just grouping, unwrap
                    let paren = self.expect(&TokenKind::RParen);
                    self.require(paren)?;
                    return Ok(first);
                }
                let mut elems = vec![first];
                while !matches!(self.peek_kind(), TokenKind::RParen | TokenKind::Eof) {
                    match self.parse_type_expr() {
                        Ok(ty) => elems.push(ty),
                        Err(()) => break,
                    }
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                let paren = self.expect(&TokenKind::RParen);
                self.require(paren)?;
                let span = self.span_from(start);
                Ok(TypeExpr::Tuple { elems, span })
            }

            // Security-labeled types: Public[T], Tainted[T], Secret[T], Clean[T]
            TokenKind::Public => {
                self.advance();
                let (inner, span) = self.parse_labeled_inner(start)?;
                Ok(TypeExpr::Labeled {
                    label: SecurityLabel::Public,
                    inner,
                    span,
                })
            }
            TokenKind::Tainted => {
                self.advance();
                let (inner, span) = self.parse_labeled_inner(start)?;
                Ok(TypeExpr::Labeled {
                    label: SecurityLabel::Tainted,
                    inner,
                    span,
                })
            }
            TokenKind::Secret => {
                self.advance();
                let (inner, span) = self.parse_labeled_inner(start)?;
                Ok(TypeExpr::Labeled {
                    label: SecurityLabel::Secret,
                    inner,
                    span,
                })
            }
            TokenKind::Clean => {
                self.advance();
                let (inner, span) = self.parse_labeled_inner(start)?;
                Ok(TypeExpr::Labeled {
                    label: SecurityLabel::Clean,
                    inner,
                    span,
                })
            }

            // Named types: Option[T], Result[T, E], or generic Foo[A, B]
            TokenKind::Ident(name) => {
                self.advance();
                self.parse_named_type(name, start)
            }

            // Integer const generic argument: `Array<T, 16>`
            TokenKind::Integer(n) => {
                self.advance();
                let span = self.span_from(start);
                Ok(TypeExpr::IntConst { value: n, span })
            }

            _ => {
                let err = ParseError {
                    message: format!("expected type, found `{}`", self.peek_kind()),
                    span: start,
                };
                self.push_recover(err);
                Err(())
            }
        }
    }

    /// Parse `[T]` or `[T where pred]` for a security-labeled type.
    fn parse_labeled_inner(&mut self, start: Span) -> Result<(Box<TypeExpr>, Span), ()> {
        let lb = self.expect(&TokenKind::LBracket);
        self.require(lb)?;
        let ty_start = self.peek_span();
        let inner = self.parse_type_expr()?;
        // Allow inline refinement: `Public[Int where self > 0]`
        let inner = if self.eat(&TokenKind::Where) {
            let pred = self.parse_ref_expr()?;
            let refined_span = self.span_from(ty_start);
            TypeExpr::Refined {
                inner: Box::new(inner),
                pred,
                span: refined_span,
            }
        } else {
            inner
        };
        let rb = self.expect(&TokenKind::RBracket);
        self.require(rb)?;
        let span = self.span_from(start);
        Ok((Box::new(inner), span))
    }

    /// Parse `Option[T]`, `Result[T, E]`, or a generic `Foo[A, B]`.
    /// `name` has already been consumed; `start` is its span.
    fn parse_named_type(&mut self, name: String, start: Span) -> Result<TypeExpr, ()> {
        match name.as_str() {
            "Option" => {
                let lb = self.expect(&TokenKind::LBracket);
                self.require(lb)?;
                let inner = self.parse_type_expr()?;
                let rb = self.expect(&TokenKind::RBracket);
                self.require(rb)?;
                let span = self.span_from(start);
                Ok(TypeExpr::Option {
                    inner: Box::new(inner),
                    span,
                })
            }
            "Result" => {
                let lb = self.expect(&TokenKind::LBracket);
                self.require(lb)?;
                let ok = self.parse_type_expr()?;
                let comma = self.expect(&TokenKind::Comma);
                self.require(comma)?;
                let err = self.parse_type_expr()?;
                let rb = self.expect(&TokenKind::RBracket);
                self.require(rb)?;
                let span = self.span_from(start);
                Ok(TypeExpr::Result {
                    ok: Box::new(ok),
                    err: Box::new(err),
                    span,
                })
            }
            _ => {
                // Generic base type or plain base type
                let args = if self.eat(&TokenKind::LBracket) {
                    let list = self.parse_type_list()?;
                    let rb = self.expect(&TokenKind::RBracket);
                    self.require(rb)?;
                    list
                } else {
                    Vec::new()
                };
                let span = self.span_from(start);
                Ok(TypeExpr::Base { name, args, span })
            }
        }
    }

    /// Parse a comma-separated list of type expressions.
    /// Does NOT consume the surrounding `[` or `]`.
    pub(super) fn parse_type_list(&mut self) -> Result<Vec<TypeExpr>, ()> {
        let mut types = Vec::new();
        loop {
            if matches!(
                self.peek_kind(),
                TokenKind::RBracket | TokenKind::RParen | TokenKind::Eof
            ) {
                break;
            }
            match self.parse_type_expr() {
                Ok(ty) => types.push(ty),
                Err(()) => break,
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        Ok(types)
    }

    /// Parse `(A, B, C)` for a function type — the param types.
    fn parse_fn_type_params(&mut self) -> Result<Vec<TypeExpr>, ()> {
        let paren = self.expect(&TokenKind::LParen);
        self.require(paren)?;
        let types = self.parse_type_list()?;
        let paren = self.expect(&TokenKind::RParen);
        self.require(paren)?;
        Ok(types)
    }

    /// Parse `! Effect + Effect + …` if the next token is `!`.
    pub fn parse_optional_effects(&mut self) -> Vec<Effect> {
        if !self.eat(&TokenKind::Bang) {
            return Vec::new();
        }
        self.parse_effect_list()
    }

    /// Parse a non-empty `+`-separated list of effect names with optional parameters.
    ///
    /// Grammar: `effect_list = effect ('+' effect)*`
    ///          `effect      = IDENT`
    ///
    /// `+` is used as the separator (not `,`) so the grammar stays LL(1): `,` is
    /// unambiguously a parameter/tuple separator everywhere else, so the parser
    /// can always determine the effect-list boundary with zero lookahead.
    ///
    /// Examples:
    /// - `FileRead`           — single effect
    /// - `FileRead + Console` — two effects
    ///
    /// Fix #6: only plain Ident tokens are valid effect names.  Previously the
    /// fallback accepted any alphabetic token string, which incorrectly consumed
    /// `where`, `fn`, `let`, etc.
    pub fn parse_effect_list(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        while let TokenKind::Ident(name) = self.peek_kind().clone() {
            let start = self.peek_span();
            self.advance();
            let span = self.span_from(start);
            effects.push(Effect::new(name, span));
            if !self.eat(&TokenKind::Plus) {
                break;
            }
        }
        effects
    }

    // ── Refinement predicates ─────────────────────────────────────────────

    /// Parse a refinement predicate expression.
    ///
    /// Precedence (lowest → highest):
    ///   1. `&&`, `||`
    ///   2. `==`, `!=`, `<`, `>`, `<=`, `>=`
    ///   3. `+`, `-`
    ///   4. `*`, `/`, `%`
    ///   5. `!expr`, atoms
    pub fn parse_ref_expr(&mut self) -> Result<RefExpr, ()> {
        self.parse_ref_logic()
    }

    fn parse_ref_logic(&mut self) -> Result<RefExpr, ()> {
        let start = self.peek_span();
        let mut left = self.parse_ref_compare()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::AmpAmp => LogicOp::And,
                TokenKind::PipePipe => LogicOp::Or,
                _ => break,
            };
            self.advance();
            let right = self.parse_ref_compare()?;
            let span = self.span_from(start);
            left = RefExpr::LogicOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    fn parse_ref_compare(&mut self) -> Result<RefExpr, ()> {
        let start = self.peek_span();
        let left = self.parse_ref_add()?;
        let op = match self.peek_kind() {
            TokenKind::EqEq => CmpOp::Eq,
            TokenKind::BangEq => CmpOp::Ne,
            TokenKind::Lt => CmpOp::Lt,
            TokenKind::Gt => CmpOp::Gt,
            TokenKind::LtEq => CmpOp::Le,
            TokenKind::GtEq => CmpOp::Ge,
            _ => return Ok(left),
        };
        self.advance();
        let right = self.parse_ref_add()?;
        let span = self.span_from(start);
        Ok(RefExpr::Compare {
            op,
            left: Box::new(left),
            right: Box::new(right),
            span,
        })
    }

    fn parse_ref_add(&mut self) -> Result<RefExpr, ()> {
        let start = self.peek_span();
        let mut left = self.parse_ref_mul()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Plus => ArithOp::Add,
                TokenKind::Minus => ArithOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_ref_mul()?;
            let span = self.span_from(start);
            left = RefExpr::ArithOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    fn parse_ref_mul(&mut self) -> Result<RefExpr, ()> {
        let start = self.peek_span();
        let mut left = self.parse_ref_unary()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Star => ArithOp::Mul,
                TokenKind::Slash => ArithOp::Div,
                TokenKind::Percent => ArithOp::Rem,
                _ => break,
            };
            self.advance();
            let right = self.parse_ref_unary()?;
            let span = self.span_from(start);
            left = RefExpr::ArithOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    fn parse_ref_unary(&mut self) -> Result<RefExpr, ()> {
        if matches!(self.peek_kind(), TokenKind::Bang) {
            let start = self.peek_span();
            self.advance();
            let inner = self.parse_ref_unary()?;
            let span = self.span_from(start);
            Ok(RefExpr::Not {
                inner: Box::new(inner),
                span,
            })
        } else {
            self.parse_ref_atom()
        }
    }

    fn parse_ref_atom(&mut self) -> Result<RefExpr, ()> {
        let start = self.peek_span();
        let kind = self.peek_kind().clone();
        match kind {
            // len(ident) or len(a.b.c) — field-access paths allowed (#726)
            TokenKind::Ident(ref s) if s == "len" => {
                self.advance();
                let lp = self.expect(&TokenKind::LParen);
                self.require(lp)?;
                let ident_result = self.expect_ident();
                let (mut ident, _) = self.require(ident_result)?;
                while matches!(self.peek_kind(), TokenKind::Dot) {
                    self.advance(); // consume '.'
                    let field_result = self.expect_ident();
                    let (field, _) = self.require(field_result)?;
                    ident = format!("{ident}.{field}");
                }
                let rp = self.expect(&TokenKind::RParen);
                self.require(rp)?;
                let span = self.span_from(start);
                Ok(RefExpr::Len { ident, span })
            }
            // forall x: T, pred — universal quantifier, ghost/contract context (Phase 5, #628)
            TokenKind::Forall => {
                self.advance(); // consume `forall`
                let ident_result = self.expect_ident();
                let (var, _) = self.require(ident_result)?;
                let colon = self.expect(&TokenKind::Colon);
                self.require(colon)?;
                let ty = self.parse_type_expr()?;
                let comma = self.expect(&TokenKind::Comma);
                self.require(comma)?;
                let body = self.parse_ref_expr()?;
                let span = self.span_from(start);
                Ok(RefExpr::Forall {
                    var,
                    ty: Box::new(ty),
                    body: Box::new(body),
                    span,
                })
            }
            // exists x: T, pred — existential quantifier, ghost/contract context (Phase 5, #628)
            TokenKind::Exists => {
                self.advance(); // consume `exists`
                let ident_result = self.expect_ident();
                let (var, _) = self.require(ident_result)?;
                let colon = self.expect(&TokenKind::Colon);
                self.require(colon)?;
                let ty = self.parse_type_expr()?;
                let comma = self.expect(&TokenKind::Comma);
                self.require(comma)?;
                let body = self.parse_ref_expr()?;
                let span = self.span_from(start);
                Ok(RefExpr::Exists {
                    var,
                    ty: Box::new(ty),
                    body: Box::new(body),
                    span,
                })
            }
            // old(expr) — entry-time value in ensures predicates (Phase 4, #627)
            TokenKind::Ident(ref s) if s == "old" => {
                self.advance();
                let lp = self.expect(&TokenKind::LParen);
                self.require(lp)?;
                let inner = self.parse_ref_expr()?;
                let rp = self.expect(&TokenKind::RParen);
                self.require(rp)?;
                let span = self.span_from(start);
                Ok(RefExpr::Old {
                    inner: Box::new(inner),
                    span,
                })
            }
            TokenKind::Ident(name) => {
                self.advance();
                let mut expr = RefExpr::Ident {
                    name,
                    span: self.span_from(start),
                };
                // Handle field access: `self.field`, `self.a.b`, etc.
                while matches!(self.peek_kind(), TokenKind::Dot) {
                    self.advance(); // consume '.'
                    let field_result = self.expect_ident();
                    let (field, _) = self.require(field_result)?;
                    let span = self.span_from(start);
                    expr = RefExpr::FieldAccess {
                        object: Box::new(expr),
                        field,
                        span,
                    };
                }
                Ok(expr)
            }
            TokenKind::Integer(n) => {
                self.advance();
                let span = self.span_from(start);
                Ok(RefExpr::Integer { value: n, span })
            }
            TokenKind::Float(f) => {
                self.advance();
                let span = self.span_from(start);
                Ok(RefExpr::Float { value: f, span })
            }
            TokenKind::LParen => {
                self.advance();
                let inner = self.parse_ref_expr()?;
                let rp = self.expect(&TokenKind::RParen);
                self.require(rp)?;
                let span = self.span_from(start);
                Ok(RefExpr::Grouped {
                    inner: Box::new(inner),
                    span,
                })
            }
            _ => {
                let err = ParseError {
                    message: format!("expected refinement, found `{}`", self.peek_kind()),
                    span: start,
                };
                self.push_recover(err);
                Err(())
            }
        }
    }

    // ── Capability parsing (used by function parameter parser) ────────────

    /// Try to parse a capability keyword (`iso`, `val`, `ref`, `tag`).
    pub fn try_parse_capability(&mut self) -> Option<Capability> {
        match self.peek_kind() {
            TokenKind::Iso => {
                self.advance();
                Some(Capability::Iso)
            }
            TokenKind::Val => {
                self.advance();
                Some(Capability::Val)
            }
            TokenKind::Ref => {
                self.advance();
                Some(Capability::Ref)
            }
            TokenKind::Tag => {
                self.advance();
                Some(Capability::Tag)
            }
            _ => None,
        }
    }

    // ── Session type parser (Honda 1993) ──────────────────────────────────

    /// Parse a session type operation.
    ///
    /// Grammar (right-recursive to encode left-to-right sequencing):
    /// ```text
    /// session_op =
    ///     '!' type_expr '.' session_op        -- send
    ///   | '?' type_expr '.' session_op        -- receive
    ///   | '+' '{' choice_branches '}'         -- internal choice
    ///   | '&' '{' choice_branches '}'         -- external choice
    ///   | 'end'                               -- protocol termination
    ///
    /// choice_branches = ident ':' session_op (',' ident ':' session_op)*
    /// ```
    pub(crate) fn parse_session_op(&mut self) -> Result<SessionOp, ()> {
        let start = self.peek_span();
        match self.peek_kind().clone() {
            // `!T. S` — send
            TokenKind::Bang => {
                self.advance(); // consume `!`
                let msg = self.parse_type_expr()?;
                let dot = self.expect(&TokenKind::Dot);
                self.require(dot)?;
                let cont = self.parse_session_op()?;
                let span = self.span_from(start);
                Ok(SessionOp::Send {
                    msg: Box::new(msg),
                    cont: Box::new(cont),
                    span,
                })
            }

            // `?T. S` — receive
            TokenKind::Question => {
                self.advance(); // consume `?`
                let msg = self.parse_type_expr()?;
                let dot = self.expect(&TokenKind::Dot);
                self.require(dot)?;
                let cont = self.parse_session_op()?;
                let span = self.span_from(start);
                Ok(SessionOp::Receive {
                    msg: Box::new(msg),
                    cont: Box::new(cont),
                    span,
                })
            }

            // `+{ l1: S1, l2: S2, ... }` — internal choice
            TokenKind::Plus => {
                self.advance(); // consume `+`
                let branches = self.parse_session_choice_branches()?;
                let span = self.span_from(start);
                Ok(SessionOp::InternalChoice { branches, span })
            }

            // `&{ l1: S1, l2: S2, ... }` — external choice
            TokenKind::Amp => {
                self.advance(); // consume `&`
                let branches = self.parse_session_choice_branches()?;
                let span = self.span_from(start);
                Ok(SessionOp::ExternalChoice { branches, span })
            }

            // `end` — protocol termination
            TokenKind::Ident(name) if name == "end" => {
                self.advance();
                let span = self.span_from(start);
                Ok(SessionOp::End { span })
            }

            _ => {
                let err = ParseError {
                    message: format!(
                        "expected session type operation (`!`, `?`, `+{{`, `&{{`, or `end`), found `{}`",
                        self.peek_kind()
                    ),
                    span: start,
                };
                self.push_recover(err);
                Err(())
            }
        }
    }

    /// Parse `{ l1: S1, l2: S2, ... }` for choice branches.
    fn parse_session_choice_branches(&mut self) -> Result<Vec<(String, SessionOp)>, ()> {
        let lb = self.expect(&TokenKind::LBrace);
        self.require(lb)?;

        let mut branches = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            let label_result = self.expect_ident();
            let (label, _) = self.require(label_result)?;
            let colon = self.expect(&TokenKind::Colon);
            self.require(colon)?;
            let op = self.parse_session_op()?;
            branches.push((label, op));

            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }

        let rb = self.expect(&TokenKind::RBrace);
        self.require(rb)?;

        if branches.is_empty() {
            let err = ParseError {
                message: "session type choice must have at least one branch".to_string(),
                span: self.peek_span(),
            };
            self.push_recover(err);
            return Err(());
        }

        Ok(branches)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::ast::*;

    fn type_decl(src: &str) -> TypeDecl {
        let (mut p, lex_errs) = Parser::new(src);
        assert!(lex_errs.is_empty(), "lex errors: {:?}", lex_errs);
        let result = p.parse_type_decl();
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        result.expect("parse_type_decl failed")
    }

    fn type_expr(src: &str) -> TypeExpr {
        let (mut p, lex_errs) = Parser::new(src);
        assert!(lex_errs.is_empty(), "lex errors: {:?}", lex_errs);
        let result = p.parse_type_expr();
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        result.expect("parse_type_expr failed")
    }

    // ── Requirement 3 / Scenario: Parse struct ────────────────────────────

    #[test]
    fn parse_struct_simple() {
        // GIVEN: type Point = struct { x: Float64, y: Float64 }
        let d = type_decl("type Point = struct { x: Float64, y: Float64 }");
        assert_eq!(d.name, "Point");
        assert!(d.params.is_empty());
        let TypeBody::Struct { fields, invariant } = d.body else {
            panic!("expected Struct body")
        };
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "x");
        assert_eq!(fields[1].name, "y");
        assert!(invariant.is_none());
    }

    #[test]
    fn parse_struct_with_ref_field() {
        // `ref Int` in the field type encodes mutability (replaces `mut count: Int`)
        let d = type_decl("type Counter = struct { count: ref Int }");
        let TypeBody::Struct { fields, .. } = d.body else {
            panic!()
        };
        assert!(matches!(fields[0].ty, TypeExpr::Ref { mutable: true, .. }));
        assert_eq!(fields[0].name, "count");
    }

    #[test]
    fn parse_struct_with_refined_field() {
        let d = type_decl("type Positive = struct { value: Int where self > 0 }");
        let TypeBody::Struct { fields, .. } = d.body else {
            panic!()
        };
        assert!(fields[0].refinement.is_some());
    }

    // ── Phase 6 / Scenario: Parse struct invariants (#654) ────────────────

    #[test]
    fn parse_struct_with_invariant() {
        // GIVEN: a struct with a `with invariant` cross-field predicate
        let d = type_decl(
            "type Stack = struct { size: Int where self >= 0, capacity: Int where self > 0, } with invariant self.size <= self.capacity"
        );
        let TypeBody::Struct { fields, invariant } = d.body else {
            panic!("expected Struct body")
        };
        assert_eq!(fields.len(), 2);
        assert!(invariant.is_some(), "expected invariant to be parsed");
        // The invariant should be a Compare: self.size <= self.capacity
        let inv = invariant.unwrap();
        assert!(
            matches!(inv, RefExpr::Compare { .. }),
            "expected Compare expression in invariant"
        );
    }

    #[test]
    fn parse_struct_without_invariant_gives_none() {
        let d = type_decl("type Point = struct { x: Int, y: Int }");
        let TypeBody::Struct { invariant, .. } = d.body else {
            panic!()
        };
        assert!(invariant.is_none());
    }

    #[test]
    fn parse_struct_invariant_field_access() {
        // GIVEN: invariant uses self.lo and self.hi
        let d = type_decl(
            "type Range = struct { lo: Int, hi: Int, } with invariant self.lo <= self.hi",
        );
        let TypeBody::Struct { invariant, .. } = d.body else {
            panic!()
        };
        let inv = invariant.expect("expected invariant");
        // Top-level: Compare { FieldAccess("lo") <= FieldAccess("hi") }
        let RefExpr::Compare { left, right, .. } = inv else {
            panic!("expected Compare")
        };
        assert!(matches!(*left, RefExpr::FieldAccess { .. }));
        assert!(matches!(*right, RefExpr::FieldAccess { .. }));
    }

    #[test]
    fn parse_struct_with_keyword_produces_error_without_invariant() {
        // GIVEN: `with` appears but `invariant` keyword is absent
        let (mut p, _) = Parser::new("type T = struct { x: Int } with");
        let _ = p.parse_type_decl();
        assert!(
            !p.errors.is_empty(),
            "expected a parse error when `invariant` keyword is absent after `with`"
        );
    }

    // ── Requirement 3 / Scenario: Parse enum ─────────────────────────────

    #[test]
    fn parse_enum_unit_variants() {
        // GIVEN: type Color = enum { Red, Green, Blue }
        let d = type_decl("type Color = enum { Red, Green, Blue }");
        assert_eq!(d.name, "Color");
        let TypeBody::Enum(variants) = d.body else {
            panic!("expected Enum body")
        };
        assert_eq!(variants.len(), 3);
        assert_eq!(variants[0].name, "Red");
        assert_eq!(variants[2].name, "Blue");
        assert!(matches!(variants[0].fields, VariantFields::Unit));
    }

    #[test]
    fn parse_enum_with_type_params() {
        // GIVEN: type Result[T, E] = enum { Ok(T), Err(E) }
        let d = type_decl("type Result[T, E] = enum { Ok(T), Err(E) }");
        assert_eq!(d.name, "Result");
        assert_eq!(
            d.params,
            vec![
                GenericParam::Type("T".to_string()),
                GenericParam::Type("E".to_string()),
            ]
        );
        let TypeBody::Enum(variants) = d.body else {
            panic!()
        };
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].name, "Ok");
        assert!(matches!(variants[0].fields, VariantFields::Tuple(_)));
        assert_eq!(variants[1].name, "Err");
    }

    #[test]
    fn parse_enum_struct_variant() {
        let d = type_decl(
            "type AuthError = enum { AccountLocked { attempts: Int where self >= 0 }, NotFound }",
        );
        let TypeBody::Enum(variants) = d.body else {
            panic!()
        };
        assert_eq!(variants.len(), 2);
        let VariantFields::Struct(fields) = &variants[0].fields else {
            panic!("expected struct variant fields")
        };
        assert_eq!(fields[0].name, "attempts");
        assert!(fields[0].refinement.is_some());
    }

    // ── Requirement 3 / Scenario: Parse alias / refinement ───────────────

    #[test]
    fn parse_type_alias() {
        // type UserId = Int
        let d = type_decl("type UserId = Int");
        assert_eq!(d.name, "UserId");
        let TypeBody::Alias(ty) = d.body else {
            panic!("expected Alias body")
        };
        assert!(matches!(*ty, TypeExpr::Base { ref name, .. } if name == "Int"));
    }

    #[test]
    fn parse_refined_alias() {
        // GIVEN: type PositiveInt = Int where self > 0
        let d = type_decl("type PositiveInt = Int where self > 0");
        let TypeBody::Alias(ty) = d.body else {
            panic!()
        };
        // THEN: AliasDecl with refinement predicate `self > 0`
        let TypeExpr::Refined { inner, pred, .. } = *ty else {
            panic!("expected Refined type, got {:?}", ty)
        };
        assert!(matches!(*inner, TypeExpr::Base { ref name, .. } if name == "Int"));
        assert!(
            matches!(pred, RefExpr::Compare { op: CmpOp::Gt, .. }),
            "expected self > 0 predicate"
        );
    }

    // ── Requirement 7 / Scenario: Parse labeled type ─────────────────────

    #[test]
    fn parse_labeled_type_tainted() {
        // GIVEN: Tainted[String]
        // THEN: LabeledType with label=Tainted, inner=String
        let ty = type_expr("Tainted[String]");
        assert!(
            matches!(
                &ty,
                TypeExpr::Labeled {
                    label: SecurityLabel::Tainted,
                    ..
                }
            ),
            "expected Tainted[String], got {:?}",
            ty
        );
    }

    #[test]
    fn parse_all_security_labels() {
        for (src, expected) in [
            ("Public[Int]", SecurityLabel::Public),
            ("Tainted[String]", SecurityLabel::Tainted),
            ("Secret[Key]", SecurityLabel::Secret),
            ("Clean[String]", SecurityLabel::Clean),
        ] {
            let ty = type_expr(src);
            let TypeExpr::Labeled { label, .. } = ty else {
                panic!("expected labeled type for {}", src)
            };
            assert_eq!(label, expected);
        }
    }

    // ── Requirement 7 / Scenario: Nested labels ───────────────────────────

    #[test]
    fn parse_nested_labeled_types() {
        // GIVEN: Public[Option[Secret[Key]]]
        // THEN: LabeledType(Public) → OptionType → LabeledType(Secret)
        let ty = type_expr("Public[Option[Secret[Key]]]");
        let TypeExpr::Labeled {
            label: SecurityLabel::Public,
            inner: opt,
            ..
        } = ty
        else {
            panic!("outer must be Public[…]")
        };
        let TypeExpr::Option {
            inner: secret_key, ..
        } = *opt
        else {
            panic!("middle must be Option[…]")
        };
        assert!(
            matches!(
                *secret_key,
                TypeExpr::Labeled {
                    label: SecurityLabel::Secret,
                    ..
                }
            ),
            "inner must be Secret[Key]"
        );
    }

    // ── Additional type expression tests ─────────────────────────────────

    #[test]
    fn parse_result_type() {
        let ty = type_expr("Result[Session, AuthError]");
        assert!(matches!(ty, TypeExpr::Result { .. }));
    }

    #[test]
    fn parse_option_type() {
        let ty = type_expr("Option[User]");
        assert!(matches!(ty, TypeExpr::Option { .. }));
    }

    #[test]
    fn parse_val_type() {
        let ty = type_expr("val DbConn");
        assert!(matches!(ty, TypeExpr::Ref { mutable: false, .. }));
    }

    #[test]
    fn parse_ref_cap_type() {
        let ty = type_expr("ref Vec");
        assert!(matches!(ty, TypeExpr::Ref { mutable: true, .. }));
    }

    #[test]
    fn parse_ampersand_type_rejected() {
        let (mut p, _) = crate::mvl::parser::Parser::new("&DbConn");
        let result = p.parse_type_expr();
        assert!(result.is_err());
        assert!(
            p.errors().iter().any(|e| e.message.contains("use `val T`")),
            "expected helpful error about val T"
        );
    }

    #[test]
    fn parse_ampersand_mut_type_rejected() {
        let (mut p, _) = crate::mvl::parser::Parser::new("&Vec");
        let result = p.parse_type_expr();
        assert!(result.is_err());
        assert!(
            p.errors().iter().any(|e| e.message.contains("val T")),
            "expected helpful error about val T / ref T"
        );
    }

    #[test]
    fn parse_fn_type() {
        let ty = type_expr("fn(Int, String) -> Bool");
        assert!(matches!(ty, TypeExpr::Fn { .. }));
    }

    #[test]
    fn parse_tuple_type() {
        let ty = type_expr("(Int, String)");
        let TypeExpr::Tuple { elems, .. } = ty else {
            panic!()
        };
        assert_eq!(elems.len(), 2);
    }

    #[test]
    fn parse_generic_base_type() {
        let ty = type_expr("Map[Key, Value]");
        let TypeExpr::Base { name, args, .. } = ty else {
            panic!()
        };
        assert_eq!(name, "Map");
        assert_eq!(args.len(), 2);
    }

    // ── Refinement predicate tests ────────────────────────────────────────

    #[test]
    fn parse_refinement_gt() {
        let d = type_decl("type X = Int where self > 0");
        let TypeBody::Alias(ty) = d.body else {
            panic!()
        };
        assert!(matches!(*ty, TypeExpr::Refined { .. }));
    }

    #[test]
    fn parse_refinement_len() {
        let d = type_decl("type Name = String where len(self) < 256");
        let TypeBody::Alias(ty) = d.body else {
            panic!()
        };
        let TypeExpr::Refined { pred, .. } = *ty else {
            panic!()
        };
        assert!(matches!(pred, RefExpr::Compare { op: CmpOp::Lt, .. }));
    }

    #[test]
    fn parse_refinement_len_field_access() {
        // len(ps.tokens) — dotted path inside len() (#726)
        let d = type_decl("type T = String where len(ps.tokens) < 256");
        let TypeBody::Alias(ty) = d.body else {
            panic!()
        };
        let TypeExpr::Refined { pred, .. } = *ty else {
            panic!()
        };
        let RefExpr::Compare { left, .. } = pred else {
            panic!("expected Compare")
        };
        assert!(
            matches!(*left, RefExpr::Len { ref ident, .. } if ident == "ps.tokens"),
            "expected Len with ident 'ps.tokens'"
        );
    }

    #[test]
    fn parse_refinement_compound() {
        let d = type_decl("type Score = Int where self >= 0 && self <= 100");
        let TypeBody::Alias(ty) = d.body else {
            panic!()
        };
        let TypeExpr::Refined { pred, .. } = *ty else {
            panic!()
        };
        assert!(matches!(
            pred,
            RefExpr::LogicOp {
                op: LogicOp::And,
                ..
            }
        ));
    }

    // ── Const generics / Array<T, N> (Issue #68) ──────────────────────────

    #[test]
    fn parse_array_type_expr() {
        // Array[Int, 16] should parse as Base { name: "Array", args: [Int, IntConst(16)] }
        let ty = type_expr("Array[Int, 16]");
        let TypeExpr::Base { name, args, .. } = ty else {
            panic!("expected Base, got {ty:?}");
        };
        assert_eq!(name, "Array");
        assert_eq!(args.len(), 2);
        assert!(matches!(args[0], TypeExpr::Base { ref name, .. } if name == "Int"));
        assert!(matches!(args[1], TypeExpr::IntConst { value: 16, .. }));
    }

    #[test]
    fn parse_const_generic_param_in_type_decl() {
        // type FixedBuf[T, const N: Int] = struct { … }
        let d = type_decl("type FixedBuf[T, const N: Int] = struct { len: Int }");
        assert_eq!(
            d.params,
            vec![
                GenericParam::Type("T".to_string()),
                GenericParam::Const("N".to_string(), "Int".to_string()),
            ]
        );
    }

    #[test]
    fn parse_array_type_as_function_param() {
        // fn f(buf: Array[Byte, 32]) -> Int
        let ty = type_expr("Array[Byte, 32]");
        let TypeExpr::Base { name, args, .. } = ty else {
            panic!("expected Base");
        };
        assert_eq!(name, "Array");
        assert!(matches!(args[1], TypeExpr::IntConst { value: 32, .. }));
    }

    // ── Regression: angle-bracket generics must be rejected (ADR-0005) ────

    #[test]
    fn reject_angle_bracket_option() {
        let (mut p, _) = Parser::new("Option<String>");
        let _ = p.parse_type_expr();
        assert!(
            !p.errors.is_empty(),
            "Option<String> should produce a parse error (ADR-0005: use Option[String])"
        );
    }

    #[test]
    fn reject_angle_bracket_result() {
        let (mut p, _) = Parser::new("Result<Int, String>");
        let _ = p.parse_type_expr();
        assert!(
            !p.errors.is_empty(),
            "Result<Int, String> should produce a parse error (ADR-0005: use Result[Int, String])"
        );
    }

    #[test]
    fn reject_angle_bracket_list() {
        // List<Int> in a function signature should produce a parse error because
        // `<` is not a valid token after a type expression (ADR-0005: use List[Int])
        let (mut p, _) = Parser::new("fn f(x: List<Int>) -> Unit { }");
        let _ = p.parse_fn_decl();
        assert!(
            !p.errors.is_empty(),
            "List<Int> in function params should produce a parse error (ADR-0005: use List[Int])"
        );
    }

    #[test]
    fn reject_angle_bracket_fn_type_params() {
        let (mut p, _) = Parser::new("fn foo<T>(x: T) -> T { }");
        let _ = p.parse_fn_decl();
        assert!(
            !p.errors.is_empty(),
            "fn foo<T> should produce a parse error (ADR-0005: use fn foo[T])"
        );
    }

    #[test]
    fn reject_angle_bracket_type_decl() {
        let (mut p, _) = Parser::new("type Foo<T> = struct { x: T }");
        let _ = p.parse_type_decl();
        assert!(
            !p.errors.is_empty(),
            "type Foo<T> should produce a parse error (ADR-0005: use type Foo[T])"
        );
    }

    // ── Session type parsing ──────────────────────────────────────────────

    fn parse_session_type(src: &str) -> TypeExpr {
        let full = format!("type P = {src}");
        let (mut p, lex_errs) = Parser::new(&full);
        assert!(lex_errs.is_empty(), "lex errors: {lex_errs:?}");
        let td = p.parse_type_decl().expect("parse_type_decl failed");
        assert!(p.errors.is_empty(), "parse errors: {:?}", p.errors);
        match td.body {
            TypeBody::Alias(te) => *te,
            other => panic!("expected alias, got {other:?}"),
        }
    }

    #[test]
    fn session_send_int_end() {
        let te = parse_session_type("!Int. end");
        assert!(matches!(te, TypeExpr::Session { .. }));
    }

    #[test]
    fn session_receive_bool_end() {
        let te = parse_session_type("?Bool. end");
        assert!(matches!(te, TypeExpr::Session { .. }));
    }

    #[test]
    fn session_send_receive_sequence() {
        let te = parse_session_type("!Int. ?Bool. end");
        assert!(matches!(te, TypeExpr::Session { .. }));
    }

    #[test]
    fn session_internal_choice() {
        let te = parse_session_type("+{ accept: !Int. end, reject: end }");
        assert!(matches!(te, TypeExpr::Session { .. }));
        if let TypeExpr::Session { op, .. } = te {
            assert!(matches!(*op, SessionOp::InternalChoice { .. }));
        }
    }

    #[test]
    fn session_external_choice() {
        let te = parse_session_type("&{ ok: ?String. end, err: end }");
        assert!(matches!(te, TypeExpr::Session { .. }));
        if let TypeExpr::Session { op, .. } = te {
            assert!(matches!(*op, SessionOp::ExternalChoice { .. }));
        }
    }

    #[test]
    fn session_nested_choices() {
        // !Request. ?Quote. +{ accept: !Payment. ?Receipt. end, reject: end }
        let te = parse_session_type("!Int. ?Bool. +{ yes: !String. end, no: end }");
        assert!(matches!(te, TypeExpr::Session { .. }));
    }

    #[test]
    fn session_type_in_alias_declaration() {
        let td = type_decl("type BuyProtocol = !Int. ?Bool. end");
        let is_session = matches!(&td.body, TypeBody::Alias(te) if matches!(te.as_ref(), TypeExpr::Session { .. }));
        assert!(is_session, "expected session type alias, got {:?}", td.body);
    }
}
