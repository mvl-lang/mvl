// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Actor declaration parser.

use crate::mvl::parser::ast::{ActorDecl, ActorMethod, MailboxConfig, MailboxPolicy};
use crate::mvl::parser::lexer::TokenKind;
use crate::mvl::parser::{ParseError, Parser};

impl Parser {
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
}
