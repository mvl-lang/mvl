pub mod ast;
pub mod diagnostics;
pub mod expressions;
pub mod functions;
pub mod lexer;
pub mod statements;
pub mod types;

use crate::mvl::parser::lexer::{LexError, Lexer, Span, Token, TokenKind};
use std::fmt;

// ── Parse error ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error at {}: {}", self.span, self.message)
    }
}

// ── Parser ─────────────────────────────────────────────────────────────────

/// Hand-written recursive descent LL(1) parser.
///
/// One function per grammar production. Each function returns `Ok(node)` or
/// pushes a [`ParseError`] and returns `Err(())` after recovering to the next
/// synchronization point.
pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Span of the most recently consumed token (used for span_from).
    last_span: Span,
    /// Fix #15: pub(crate) so external callers use the `errors()` accessor
    /// method rather than being able to mutate the error list directly.
    pub(crate) errors: Vec<ParseError>,
}

impl Parser {
    /// Create a parser from raw source text.  Returns the parser and any
    /// lexer-level errors (which are non-fatal — the token stream is always
    /// complete).
    pub fn new(src: &str) -> (Self, Vec<LexError>) {
        let (tokens, lex_errors) = Lexer::new(src).tokenize();
        let first_span = tokens.first().map(|t| t.span).unwrap_or_default();
        (
            Parser {
                tokens,
                pos: 0,
                last_span: first_span,
                errors: Vec::new(),
            },
            lex_errors,
        )
    }

    pub fn errors(&self) -> &[ParseError] {
        &self.errors
    }

    // ── Token inspection ─────────────────────────────────────────────────

    /// Kind of the current (not yet consumed) token.
    pub fn peek_kind(&self) -> &TokenKind {
        let idx = self.pos.min(self.tokens.len() - 1);
        &self.tokens[idx].kind
    }

    /// Span of the current token.
    pub fn peek_span(&self) -> Span {
        let idx = self.pos.min(self.tokens.len() - 1);
        self.tokens[idx].span
    }

    /// Kind of the token *after* the current one.
    pub fn peek_next_kind(&self) -> &TokenKind {
        let idx = (self.pos + 1).min(self.tokens.len() - 1);
        &self.tokens[idx].kind
    }

    pub fn at_eof(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Eof)
    }

    // ── Token consumption ─────────────────────────────────────────────────

    /// Consume and return the current token.
    pub fn advance(&mut self) -> Token {
        let idx = self.pos.min(self.tokens.len() - 1);
        let tok = self.tokens[idx].clone();
        self.last_span = tok.span;
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        tok
    }

    /// Span covering from `start` through the last consumed token.
    pub fn span_from(&self, start: Span) -> Span {
        let end = self.last_span.offset + self.last_span.len;
        Span::new(
            start.line,
            start.col,
            start.offset,
            end.saturating_sub(start.offset),
        )
    }

    /// Consume the current token and return its span, or return a `ParseError`
    /// if the discriminant does not match `expected`.
    pub fn expect(&mut self, expected: &TokenKind) -> Result<Span, ParseError> {
        if std::mem::discriminant(self.peek_kind()) == std::mem::discriminant(expected) {
            Ok(self.advance().span)
        } else {
            Err(ParseError {
                message: format!("expected `{}`, found `{}`", expected, self.peek_kind()),
                span: self.peek_span(),
            })
        }
    }

    /// Consume the current token if its discriminant matches `expected`.
    /// Returns `true` if consumed.
    pub fn eat(&mut self, expected: &TokenKind) -> bool {
        if std::mem::discriminant(self.peek_kind()) == std::mem::discriminant(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Consume an identifier and return its name + span, or a `ParseError`.
    pub fn expect_ident(&mut self) -> Result<(String, Span), ParseError> {
        let kind = self.peek_kind().clone();
        match kind {
            TokenKind::Ident(s) => {
                let span = self.advance().span;
                Ok((s, span))
            }
            _ => Err(ParseError {
                message: format!("expected identifier, found `{}`", self.peek_kind()),
                span: self.peek_span(),
            }),
        }
    }

    // ── Error handling ────────────────────────────────────────────────────

    pub fn push_error(&mut self, err: ParseError) {
        self.errors.push(err);
    }

    /// Push a `ParseError` and recover to the next sync point.
    pub fn push_recover(&mut self, err: ParseError) {
        self.errors.push(err);
        self.recover();
    }

    /// Convenience: convert `Result<T, ParseError>` → `Result<T, ()>`,
    /// pushing and recovering on error.
    pub fn require<T>(&mut self, result: Result<T, ParseError>) -> Result<T, ()> {
        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                self.push_recover(e);
                Err(())
            }
        }
    }

    /// Skip tokens until a synchronization point: `;`, `}`, `fn`, `type`,
    /// `const`, `module`, or EOF.
    pub fn recover(&mut self) {
        loop {
            match self.peek_kind() {
                TokenKind::Eof => break,
                TokenKind::Semicolon | TokenKind::RBrace => {
                    self.advance();
                    break;
                }
                TokenKind::Fn
                | TokenKind::Type
                | TokenKind::Const
                | TokenKind::Total
                | TokenKind::Partial
                | TokenKind::Pub
                | TokenKind::Use => break,
                _ => {
                    self.advance();
                }
            }
        }
    }
}
