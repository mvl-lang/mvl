// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! String, char, and multi-line literal lexing.
//!
//! Methods are `pub(super)` so the dispatcher in `mod.rs` can call into them.

use super::{LexError, Lexer, Span, TokenKind};

impl<'src> Lexer<'src> {
    /// Parse `{NNNN}` after a `\u` escape has been consumed.
    ///
    /// Accepts 1–6 hex digits. Returns the corresponding `char`, or U+FFFD
    /// on any syntax error (missing braces, invalid codepoint, too many digits).
    pub(super) fn lex_unicode_escape(&mut self) -> char {
        if self.peek_char() != Some('{') {
            self.errors.push(LexError {
                message: r"expected `{` after `\u` in string escape".into(),
                span: Span::new(self.line, self.col, self.peek_offset() as u32, 1),
            });
            return '\u{FFFD}';
        }
        self.advance(); // consume '{'
        let mut hex = String::new();
        loop {
            match self.peek_char() {
                Some('}') => break,
                Some(c) if c.is_ascii_hexdigit() && hex.len() < 6 => {
                    hex.push(c);
                    self.advance();
                }
                Some(c) => {
                    self.errors.push(LexError {
                        message: format!(
                            "invalid character `{c}` in `\\u{{...}}` escape; expected hex digit or `}}`"
                        ),
                        span: Span::new(self.line, self.col, self.peek_offset() as u32, 1),
                    });
                    return '\u{FFFD}';
                }
                None => {
                    self.errors.push(LexError {
                        message: "unterminated `\\u{...}` escape: expected `}`".into(),
                        span: Span::new(self.line, self.col, self.peek_offset() as u32, 1),
                    });
                    return '\u{FFFD}';
                }
            }
        }
        self.advance(); // consume '}'
        if hex.is_empty() {
            self.errors.push(LexError {
                message: "`\\u{}` escape requires at least one hex digit".into(),
                span: Span::new(self.line, self.col, self.peek_offset() as u32, 1),
            });
            return '\u{FFFD}';
        }
        let codepoint = u32::from_str_radix(&hex, 16).unwrap_or(0xFFFF_FFFF);
        char::from_u32(codepoint).unwrap_or_else(|| {
            self.errors.push(LexError {
                message: format!(
                    "`\\u{{{hex}}}` is not a valid Unicode scalar (U+{codepoint:04X})"
                ),
                span: Span::new(self.line, self.col, self.peek_offset() as u32, 1),
            });
            '\u{FFFD}'
        })
    }

    pub(super) fn lex_string(
        &mut self,
        start_line: u32,
        start_col: u32,
        start_offset: usize,
    ) -> TokenKind {
        let mut s = String::new();
        loop {
            match self.advance() {
                None => {
                    self.errors.push(LexError {
                        message: "unterminated string literal".into(),
                        span: Span::new(start_line, start_col, start_offset as u32, 1),
                    });
                    break;
                }
                Some('"') => break,
                Some('\\') => match self.advance() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('r') => s.push('\r'),
                    Some('\\') => s.push('\\'),
                    Some('"') => s.push('"'),
                    Some('0') => s.push('\0'),
                    Some('u') => s.push(self.lex_unicode_escape()),
                    Some(c) => s.push(c),
                    None => break,
                },
                Some(c) => s.push(c),
            }
        }
        TokenKind::Str(s)
    }

    /// Lex `"""…"""` multiline string (escape sequences processed, literal newlines preserved).
    pub(super) fn lex_multiline_string(
        &mut self,
        start_line: u32,
        start_col: u32,
        start_offset: usize,
    ) -> TokenKind {
        let mut s = String::new();
        loop {
            match self.advance() {
                None => {
                    self.errors.push(LexError {
                        message: "unterminated multiline string literal".into(),
                        span: Span::new(start_line, start_col, start_offset as u32, 3),
                    });
                    break;
                }
                Some('"') => {
                    if self.peek_char() == Some('"') {
                        self.advance();
                        if self.peek_char() == Some('"') {
                            self.advance(); // consume closing third `"`
                            break;
                        } else {
                            s.push('"');
                            s.push('"');
                        }
                    } else {
                        s.push('"');
                    }
                }
                Some('\\') => match self.advance() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('r') => s.push('\r'),
                    Some('\\') => s.push('\\'),
                    Some('"') => s.push('"'),
                    Some('0') => s.push('\0'),
                    Some('u') => s.push(self.lex_unicode_escape()),
                    Some(c) => s.push(c),
                    None => break,
                },
                Some(c) => s.push(c),
            }
        }
        TokenKind::MultilineStr(s)
    }

    /// Lex `r"…"` raw single-line string (no escape processing).
    pub(super) fn lex_raw_string(
        &mut self,
        start_line: u32,
        start_col: u32,
        start_offset: usize,
    ) -> TokenKind {
        let mut s = String::new();
        loop {
            match self.advance() {
                None => {
                    self.errors.push(LexError {
                        message: "unterminated raw string literal".into(),
                        span: Span::new(start_line, start_col, start_offset as u32, 2),
                    });
                    break;
                }
                Some('"') => break,
                Some(c) => s.push(c),
            }
        }
        TokenKind::RawStr(s)
    }

    /// Lex `r"""…"""` raw multiline string (no escape processing).
    pub(super) fn lex_raw_multiline_string(
        &mut self,
        start_line: u32,
        start_col: u32,
        start_offset: usize,
    ) -> TokenKind {
        let mut s = String::new();
        loop {
            match self.advance() {
                None => {
                    self.errors.push(LexError {
                        message: "unterminated raw multiline string literal".into(),
                        span: Span::new(start_line, start_col, start_offset as u32, 4),
                    });
                    break;
                }
                Some('"') => {
                    if self.peek_char() == Some('"') {
                        self.advance();
                        if self.peek_char() == Some('"') {
                            self.advance(); // consume closing third `"`
                            break;
                        } else {
                            s.push('"');
                            s.push('"');
                        }
                    } else {
                        s.push('"');
                    }
                }
                Some(c) => s.push(c),
            }
        }
        TokenKind::RawMultilineStr(s)
    }

    pub(super) fn lex_char(
        &mut self,
        start_line: u32,
        start_col: u32,
        start_offset: usize,
    ) -> TokenKind {
        let c = match self.advance() {
            None => '\0',
            Some('\\') => match self.advance() {
                Some('n') => '\n',
                Some('t') => '\t',
                Some('r') => '\r',
                Some('\\') => '\\',
                Some('\'') => '\'',
                Some('0') => '\0',
                Some('u') => self.lex_unicode_escape(),
                Some(c) => c,
                None => '\0',
            },
            Some(c) => c,
        };
        if self.peek_char() == Some('\'') {
            self.advance();
        } else {
            self.errors.push(LexError {
                message: "unterminated character literal".into(),
                span: Span::new(start_line, start_col, start_offset as u32, 1),
            });
        }
        TokenKind::Char(c)
    }
}
