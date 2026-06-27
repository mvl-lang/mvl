// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Cursor primitives: peek, advance, span construction.
//!
//! Methods are `pub(super)` so the dispatcher in `mod.rs` and the literal
//! helpers in sibling modules can call into them.

use super::{Lexer, Span};

impl<'src> Lexer<'src> {
    pub(super) fn peek_char(&self) -> Option<char> {
        self.current.map(|(_, c)| c)
    }

    pub(super) fn peek_offset(&self) -> usize {
        self.current.map_or(self.src.len(), |(i, _)| i)
    }

    /// Peek at the character *after* the current one without advancing.
    pub(super) fn peek_second(&self) -> Option<char> {
        self.current.and_then(|(i, _)| self.src[i..].chars().nth(1))
    }

    /// Consume the current character and advance.
    pub(super) fn advance(&mut self) -> Option<char> {
        let result = self.current.map(|(_, c)| c);
        if let Some((_, ch)) = self.current {
            if ch == '\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
        }
        self.current = self.chars.next();
        result
    }

    pub(super) fn skip_whitespace_and_comments(&mut self) {
        loop {
            match self.peek_char() {
                Some(' ') | Some('\t') | Some('\r') | Some('\n') => {
                    self.advance();
                }
                Some('/') if self.peek_second() == Some('/') => {
                    // Line comment — skip to end of line
                    while self.peek_char().is_some() && self.peek_char() != Some('\n') {
                        self.advance();
                    }
                }
                _ => break,
            }
        }
    }

    pub(super) fn make_span(&self, start_offset: usize, start_line: u32, start_col: u32) -> Span {
        let end = self.peek_offset();
        // Fix #3: guard against silent truncation on hypothetical very large files
        debug_assert!(
            start_offset <= u32::MAX as usize,
            "source offset {} exceeds u32::MAX",
            start_offset
        );
        Span::new(
            start_line,
            start_col,
            start_offset as u32,
            (end - start_offset) as u32,
        )
    }
}
