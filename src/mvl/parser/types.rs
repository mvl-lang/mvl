//! Type-declaration and type-expression parser.
//!
//! Handles:
//! - `type Name [<params>] = struct { … }` (Requirement 3)
//! - `type Name [<params>] = enum { … }`   (Requirement 3)
//! - `type Name = ExistingType`             (Requirement 3)
//! - `type Name = T where predicate`        (Requirement 3)
//! - All type expressions including security labels (Requirement 7)

use crate::mvl::parser::ast::{
    ArithOp, Capability, CmpOp, FieldDecl, LogicOp, RefExpr, SecurityLabel, TypeBody, TypeDecl,
    TypeExpr, Variant, VariantFields,
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
                Ok(TypeBody::Struct(fields))
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

    /// Parse a single struct field: `[mut] name: type [where pred]`.
    fn parse_field_decl(&mut self) -> Result<FieldDecl, ()> {
        let start = self.peek_span();
        let mutable = self.eat(&TokenKind::Mut);
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
            mutable,
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

    /// Parse optional `<A, B, C>` on a type declaration.  Returns empty vec
    /// if the next token is not `<`.
    pub fn parse_type_params_decl(&mut self) -> Vec<String> {
        if !self.eat(&TokenKind::Lt) {
            return Vec::new();
        }
        let mut params = Vec::new();
        loop {
            if matches!(self.peek_kind(), TokenKind::Gt | TokenKind::Eof) {
                break;
            }
            match self.expect_ident() {
                Ok((name, _)) => params.push(name),
                Err(e) => {
                    self.push_error(e);
                    break;
                }
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        // Fix #13: report a diagnostic if the closing `>` is missing rather
        // than silently accepting `fn f<T(x: T) -> T` as valid syntax.
        if !self.eat(&TokenKind::Gt) {
            self.push_error(ParseError {
                message: "expected `>` to close type parameter list".into(),
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
            // &T  or  &mut T
            TokenKind::Amp => {
                self.advance();
                let mutable = self.eat(&TokenKind::Mut);
                let inner = self.parse_type_expr()?;
                let span = self.span_from(start);
                Ok(TypeExpr::Ref {
                    mutable,
                    inner: Box::new(inner),
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

            // Security-labeled types: Public<T>, Tainted<T>, Secret<T>, Clean<T>
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

            // Named types: Option<T>, Result<T, E>, or generic Foo<A, B>
            TokenKind::Ident(name) => {
                self.advance();
                self.parse_named_type(name, start)
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

    /// Parse `<T>` or `<T where pred>` for a security-labeled type.
    fn parse_labeled_inner(&mut self, start: Span) -> Result<(Box<TypeExpr>, Span), ()> {
        let lt = self.expect(&TokenKind::Lt);
        self.require(lt)?;
        let ty_start = self.peek_span();
        let inner = self.parse_type_expr()?;
        // Allow inline refinement: `Public<Int where self > 0>`
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
        let gt = self.expect(&TokenKind::Gt);
        self.require(gt)?;
        let span = self.span_from(start);
        Ok((Box::new(inner), span))
    }

    /// Parse `Option<T>`, `Result<T, E>`, or a generic `Foo<A, B>`.
    /// `name` has already been consumed; `start` is its span.
    fn parse_named_type(&mut self, name: String, start: Span) -> Result<TypeExpr, ()> {
        match name.as_str() {
            "Option" => {
                let lt = self.expect(&TokenKind::Lt);
                self.require(lt)?;
                let inner = self.parse_type_expr()?;
                let gt = self.expect(&TokenKind::Gt);
                self.require(gt)?;
                let span = self.span_from(start);
                Ok(TypeExpr::Option {
                    inner: Box::new(inner),
                    span,
                })
            }
            "Result" => {
                let lt = self.expect(&TokenKind::Lt);
                self.require(lt)?;
                let ok = self.parse_type_expr()?;
                let comma = self.expect(&TokenKind::Comma);
                self.require(comma)?;
                let err = self.parse_type_expr()?;
                let gt = self.expect(&TokenKind::Gt);
                self.require(gt)?;
                let span = self.span_from(start);
                Ok(TypeExpr::Result {
                    ok: Box::new(ok),
                    err: Box::new(err),
                    span,
                })
            }
            _ => {
                // Generic base type or plain base type
                let args = if self.eat(&TokenKind::Lt) {
                    let list = self.parse_type_list()?;
                    let gt = self.expect(&TokenKind::Gt);
                    self.require(gt)?;
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
    /// Does NOT consume the surrounding `<` or `>`.
    pub(crate) fn parse_type_list(&mut self) -> Result<Vec<TypeExpr>, ()> {
        let mut types = Vec::new();
        loop {
            if matches!(
                self.peek_kind(),
                TokenKind::Gt | TokenKind::RParen | TokenKind::Eof
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

    /// Parse `! Effect, Effect, …` if the next token is `!`.
    pub fn parse_optional_effects(&mut self) -> Vec<String> {
        if !self.eat(&TokenKind::Bang) {
            return Vec::new();
        }
        self.parse_effect_list()
    }

    /// Parse a non-empty comma-separated list of effect names.
    ///
    /// Effect names must be plain identifiers.  Accepting keyword tokens here
    /// caused `where` and other keywords to be silently consumed as effect names
    /// (Fix #6).
    pub fn parse_effect_list(&mut self) -> Vec<String> {
        let mut effects = Vec::new();
        // Fix #6: only plain Ident tokens are valid effect names.
        // Previously the fallback accepted any alphabetic token string,
        // which incorrectly consumed `where`, `fn`, `let`, etc.
        while let TokenKind::Ident(s) = self.peek_kind().clone() {
            self.advance();
            effects.push(s);
            if !self.eat(&TokenKind::Comma) {
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
            // len(ident)
            TokenKind::Ident(ref s) if s == "len" => {
                self.advance();
                let lp = self.expect(&TokenKind::LParen);
                self.require(lp)?;
                let ident_result = self.expect_ident();
                let (ident, _) = self.require(ident_result)?;
                let rp = self.expect(&TokenKind::RParen);
                self.require(rp)?;
                let span = self.span_from(start);
                Ok(RefExpr::Len { ident, span })
            }
            TokenKind::Ident(name) => {
                self.advance();
                let span = self.span_from(start);
                Ok(RefExpr::Ident { name, span })
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
        let TypeBody::Struct(fields) = d.body else {
            panic!("expected Struct body")
        };
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "x");
        assert_eq!(fields[1].name, "y");
    }

    #[test]
    fn parse_struct_with_mut_field() {
        let d = type_decl("type Counter = struct { mut count: Int }");
        let TypeBody::Struct(fields) = d.body else {
            panic!()
        };
        assert!(fields[0].mutable);
        assert_eq!(fields[0].name, "count");
    }

    #[test]
    fn parse_struct_with_refined_field() {
        let d = type_decl("type Positive = struct { value: Int where self > 0 }");
        let TypeBody::Struct(fields) = d.body else {
            panic!()
        };
        assert!(fields[0].refinement.is_some());
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
        // GIVEN: type Result<T, E> = enum { Ok(T), Err(E) }
        let d = type_decl("type Result<T, E> = enum { Ok(T), Err(E) }");
        assert_eq!(d.name, "Result");
        assert_eq!(d.params, vec!["T", "E"]);
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
        // GIVEN: Tainted<String>
        // THEN: LabeledType with label=Tainted, inner=String
        let ty = type_expr("Tainted<String>");
        assert!(
            matches!(
                &ty,
                TypeExpr::Labeled {
                    label: SecurityLabel::Tainted,
                    ..
                }
            ),
            "expected Tainted<String>, got {:?}",
            ty
        );
    }

    #[test]
    fn parse_all_security_labels() {
        for (src, expected) in [
            ("Public<Int>", SecurityLabel::Public),
            ("Tainted<String>", SecurityLabel::Tainted),
            ("Secret<Key>", SecurityLabel::Secret),
            ("Clean<String>", SecurityLabel::Clean),
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
        // GIVEN: Public<Option<Secret<Key>>>
        // THEN: LabeledType(Public) → OptionType → LabeledType(Secret)
        let ty = type_expr("Public<Option<Secret<Key>>>");
        let TypeExpr::Labeled {
            label: SecurityLabel::Public,
            inner: opt,
            ..
        } = ty
        else {
            panic!("outer must be Public<…>")
        };
        let TypeExpr::Option {
            inner: secret_key, ..
        } = *opt
        else {
            panic!("middle must be Option<…>")
        };
        assert!(
            matches!(
                *secret_key,
                TypeExpr::Labeled {
                    label: SecurityLabel::Secret,
                    ..
                }
            ),
            "inner must be Secret<Key>"
        );
    }

    // ── Additional type expression tests ─────────────────────────────────

    #[test]
    fn parse_result_type() {
        let ty = type_expr("Result<Session, AuthError>");
        assert!(matches!(ty, TypeExpr::Result { .. }));
    }

    #[test]
    fn parse_option_type() {
        let ty = type_expr("Option<User>");
        assert!(matches!(ty, TypeExpr::Option { .. }));
    }

    #[test]
    fn parse_ref_type() {
        let ty = type_expr("&DbConn");
        assert!(matches!(ty, TypeExpr::Ref { mutable: false, .. }));
    }

    #[test]
    fn parse_mut_ref_type() {
        let ty = type_expr("&mut Vec");
        assert!(matches!(ty, TypeExpr::Ref { mutable: true, .. }));
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
        let ty = type_expr("Map<Key, Value>");
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
}
