// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Extern block and extern function declaration parser.

use crate::mvl::parser::ast::{ExternDecl, ExternFnDecl, ImplDecl, Totality};
use crate::mvl::parser::lexer::TokenKind;
use crate::mvl::parser::{ParseError, Parser};

impl Parser {
    pub fn parse_extern_decl(&mut self) -> Result<ExternDecl, ()> {
        let start = self.peek_span();
        self.advance(); // consume `extern`

        // ABI string: e.g. `"rust"`, `"c"`, or `"C"` (normalized to lowercase).
        let abi = match self.peek_kind() {
            TokenKind::Str(_) => {
                let tok = self.advance();
                match tok.kind {
                    TokenKind::Str(s) => s.to_ascii_lowercase(),
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

        // Optional `link("lib1", "lib2")` — library names to link against.
        let link_libs = if matches!(self.peek_kind(), TokenKind::Ident(s) if s == "link") {
            self.advance(); // consume `link`
            let lparen = self.expect(&TokenKind::LParen);
            self.require(lparen)?;
            let mut libs = Vec::new();
            while !matches!(self.peek_kind(), TokenKind::RParen | TokenKind::Eof) {
                if !libs.is_empty() {
                    let comma = self.expect(&TokenKind::Comma);
                    if self.require(comma).is_err() {
                        break;
                    }
                }
                if matches!(self.peek_kind(), TokenKind::RParen) {
                    break;
                }
                match self.peek_kind() {
                    TokenKind::Str(_) => {
                        let tok = self.advance();
                        if let TokenKind::Str(s) = tok.kind {
                            libs.push(s);
                        }
                    }
                    _ => {
                        let err = ParseError {
                            message: format!(
                                "expected library name string in `link(...)`, found `{}`",
                                self.peek_kind()
                            ),
                            span: self.peek_span(),
                        };
                        self.push_recover(err);
                        break;
                    }
                }
            }
            let rparen = self.expect(&TokenKind::RParen);
            self.require(rparen)?;
            libs
        } else {
            Vec::new()
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
        Ok(ExternDecl {
            abi,
            link_libs,
            fns,
            span,
        })
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
}
