// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Declaration parsers: use, const, label, relabel, effect.

use crate::mvl::parser::ast::{EffectDecl, LabelDecl, RelabelDecl, UseDecl};
use crate::mvl::parser::lexer::TokenKind;
use crate::mvl::parser::{ParseError, Parser};

impl Parser {
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

        // Optional `audit` contextual keyword (#896): `relabel trust: T -> _ audit`
        let audit = matches!(self.peek_kind(), TokenKind::Ident(kw) if kw == "audit");
        if audit {
            self.advance(); // consume `audit`
        }

        let span = self.span_from(start);
        Ok(RelabelDecl {
            visible,
            name,
            from,
            to,
            audit,
            span,
        })
    }

    /// Parse one side of a relabel declaration: `_` (bare) or an ident (label name).
    fn parse_relabel_side(&mut self) -> Result<Option<String>, ()> {
        match self.peek_kind() {
            TokenKind::Underscore => {
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
}
