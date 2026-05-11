// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Diagnostic rendering for parser errors (Requirement 8).
//!
//! Formats [`ParseError`] values into human-readable messages with:
//! - Source file location (line and column)
//! - The offending source line
//! - A caret (`^`) pointing to the exact token
//!
//! # Example output
//!
//! ```text
//! error at 3:5: expected `}`, found `fn`
//!    3 | fn bad( {
//!            ^
//! ```

use crate::mvl::parser::ParseError;

// ── Diagnostic type ─────────────────────────────────────────────────────────

/// A formatted diagnostic message with source context.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// Short human-readable message ("expected `}`, found `fn`").
    pub message: String,
    /// 1-based line number.
    pub line: u32,
    /// 1-based column number.
    pub col: u32,
    /// The source line text (for display).
    pub source_line: String,
    /// Length in characters of the offending token (for the caret span).
    pub len: usize,
}

impl Diagnostic {
    /// Format the diagnostic as a multi-line string.
    pub fn render(&self) -> String {
        let label = format!("error at {}:{}: {}", self.line, self.col, self.message);
        let line_prefix = format!("{:>4} | ", self.line);
        let caret_indent = " ".repeat(line_prefix.len() + self.col as usize - 1);
        let carets = "^".repeat(self.len.max(1));
        format!(
            "{}\n{}{}\n{}{}",
            label, line_prefix, self.source_line, caret_indent, carets
        )
    }
}

// ── Conversion from ParseError ───────────────────────────────────────────────

/// Build a [`Diagnostic`] from a [`ParseError`] and the original source text.
pub fn to_diagnostic(src: &str, error: &ParseError) -> Diagnostic {
    let line_text = src
        .lines()
        .nth(error.span.line.saturating_sub(1) as usize)
        .unwrap_or("")
        .to_string();

    Diagnostic {
        message: error.message.clone(),
        line: error.span.line,
        col: error.span.col,
        source_line: line_text,
        len: error.span.len as usize,
    }
}

/// Format all errors from a parser run as a single diagnostic string.
pub fn render_errors(src: &str, errors: &[ParseError]) -> String {
    errors
        .iter()
        .map(|e| to_diagnostic(src, e).render())
        .collect::<Vec<_>>()
        .join("\n\n")
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::lexer::Span;

    fn make_error(msg: &str, line: u32, col: u32, len: u32) -> ParseError {
        ParseError {
            message: msg.to_string(),
            span: Span::new(line, col, 0, len),
        }
    }

    // ── Requirement 8 / Scenario: Error with source location ──────────────

    #[test]
    fn diagnostic_contains_line_and_col() {
        // GIVEN: a missing `}` error at line 3, col 5
        // THEN: the diagnostic message includes "3:5"
        let src = "fn foo() -> Int {\n    let x = 1;\n    x\n";
        let err = make_error("expected `}`, found EOF", 4, 1, 0);
        let diag = to_diagnostic(src, &err);
        assert_eq!(diag.line, 4);
        assert_eq!(diag.col, 1);
        let rendered = diag.render();
        assert!(
            rendered.contains("4:1"),
            "rendered should contain 4:1, got:\n{}",
            rendered
        );
        assert!(
            rendered.contains("expected `}`, found EOF"),
            "rendered should include message"
        );
    }

    #[test]
    fn diagnostic_shows_source_line() {
        let src = "fn bad_fn(  -> Int { }";
        let err = make_error("expected `)`, found `->`", 1, 13, 2);
        let diag = to_diagnostic(src, &err);
        assert_eq!(diag.source_line, "fn bad_fn(  -> Int { }");
        let rendered = diag.render();
        assert!(rendered.contains("fn bad_fn(  -> Int { }"));
    }

    #[test]
    fn diagnostic_caret_position() {
        let src = "fn f(x Int) -> Int { x }";
        let err = make_error("expected `:`, found `Int`", 1, 7, 3);
        let diag = to_diagnostic(src, &err);
        let rendered = diag.render();
        // The rendered output should have at least one `^`
        assert!(rendered.contains('^'), "caret missing:\n{}", rendered);
    }

    // ── Requirement 8 / Scenario: Multiple errors reported ────────────────

    #[test]
    fn render_multiple_errors() {
        let src = "fn broken1( -> Int { }\nfn broken2 -> Int { }";
        let errors = vec![
            make_error("expected `)`, found `->`", 1, 13, 2),
            make_error("expected `(`, found `->`", 2, 12, 2),
        ];
        let rendered = render_errors(src, &errors);
        // Both errors should appear in the output
        assert!(
            rendered.contains("1:13"),
            "first error missing from output:\n{}",
            rendered
        );
        assert!(
            rendered.contains("2:12"),
            "second error missing from output:\n{}",
            rendered
        );
    }
}
