// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Integer and float literal lexing (decimal, hex, binary, underscores).
//!
//! Methods are `pub(super)` so the dispatcher in `mod.rs` can call into them.

use super::{LexError, Lexer, Span, TokenKind};

impl<'src> Lexer<'src> {
    pub(super) fn lex_number(
        &mut self,
        first: char,
        start_line: u32,
        start_col: u32,
        start_offset: usize,
    ) -> TokenKind {
        // Handle base prefixes: 0x (hex), 0b (binary), 0o (octal)
        if first == '0' {
            match self.peek_char() {
                Some('x') | Some('X') => {
                    self.advance(); // consume 'x'
                    return self.lex_integer_base(16, start_line, start_col, start_offset);
                }
                Some('b') | Some('B') => {
                    self.advance(); // consume 'b'
                    return self.lex_integer_base(2, start_line, start_col, start_offset);
                }
                Some('o') | Some('O') => {
                    self.advance(); // consume 'o'
                    return self.lex_integer_base(8, start_line, start_col, start_offset);
                }
                _ => {}
            }
        }

        let mut s = String::from(first);
        let mut is_float = false;

        loop {
            match self.peek_char() {
                Some(c) if c.is_ascii_digit() => {
                    s.push(c);
                    self.advance();
                }
                Some('.') if !is_float => {
                    // Only treat as float if the character after '.' is a digit
                    if self.peek_second().is_some_and(|c| c.is_ascii_digit()) {
                        is_float = true;
                        s.push('.');
                        self.advance(); // consume '.'
                    } else {
                        break;
                    }
                }
                // Scientific notation: 1.5e10, 2e-3, 1E+4
                Some('e') | Some('E') => {
                    is_float = true;
                    s.push('e');
                    self.advance(); // consume 'e'/'E'
                                    // Optional sign
                    if matches!(self.peek_char(), Some('+') | Some('-')) {
                        s.push(self.peek_char().unwrap());
                        self.advance();
                    }
                    // Exponent digits
                    while self.peek_char().is_some_and(|c| c.is_ascii_digit()) {
                        s.push(self.peek_char().unwrap());
                        self.advance();
                    }
                    break;
                }
                _ => break,
            }
        }

        if is_float {
            match s.parse::<f64>() {
                Ok(f) => TokenKind::Float(f),
                Err(_) => {
                    self.errors.push(LexError {
                        message: format!("invalid float literal `{s}`"),
                        span: Span::new(start_line, start_col, start_offset as u32, s.len() as u32),
                    });
                    TokenKind::Float(0.0)
                }
            }
        } else {
            // Fix #3: report overflow instead of silently producing 0
            match s.parse::<i64>() {
                Ok(n) => TokenKind::Integer(n),
                Err(_) => {
                    self.errors.push(LexError {
                        message: format!(
                            "integer literal `{}` overflows i64; value is too large",
                            s
                        ),
                        span: Span::new(start_line, start_col, start_offset as u32, s.len() as u32),
                    });
                    TokenKind::Integer(0)
                }
            }
        }
    }

    /// Scan digits for a non-decimal integer literal (hex/binary/octal).
    pub(super) fn lex_integer_base(
        &mut self,
        radix: u32,
        start_line: u32,
        start_col: u32,
        start_offset: usize,
    ) -> TokenKind {
        let valid_digit = |c: char| c.is_digit(radix);
        let mut s = String::new();
        while self.peek_char().is_some_and(|c| valid_digit(c) || c == '_') {
            let c = self.peek_char().unwrap();
            self.advance();
            if c != '_' {
                s.push(c);
            }
        }
        if s.is_empty() {
            self.errors.push(LexError {
                message: "empty integer literal (no digits after base prefix)".to_string(),
                span: Span::new(start_line, start_col, start_offset as u32, 2),
            });
            return TokenKind::Integer(0);
        }
        match i64::from_str_radix(&s, radix) {
            Ok(n) => TokenKind::Integer(n),
            Err(_) => {
                self.errors.push(LexError {
                    message: "integer literal overflows i64".to_string(),
                    span: Span::new(
                        start_line,
                        start_col,
                        start_offset as u32,
                        s.len() as u32 + 2,
                    ),
                });
                TokenKind::Integer(0)
            }
        }
    }
}
