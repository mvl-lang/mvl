//! Lint diagnostic variants.
//!
//! Each [`LintDiag`] has a [`Severity`] (warning or error), a human-readable
//! message, and a source location ([`LintSpan`]).

/// Coarse source location used by lint rules.
///
/// Unlike the parser's `Span` (byte-offset based), lint rules frequently
/// work line-by-line on the raw source, so we keep line + col here.
#[derive(Debug, Clone, PartialEq)]
pub struct LintSpan {
    pub line: u32, // 1-based
    pub col: u32,  // 1-based
}

/// How serious a lint finding is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warning,
    Error,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Warning => write!(f, "warning"),
            Severity::Error => write!(f, "error"),
        }
    }
}

/// A single lint finding.
#[derive(Debug, Clone, PartialEq)]
pub struct LintDiag {
    pub severity: Severity,
    pub rule: &'static str,
    pub message: String,
    pub span: LintSpan,
}

impl LintDiag {
    pub fn warning(rule: &'static str, message: impl Into<String>, line: u32, col: u32) -> Self {
        Self {
            severity: Severity::Warning,
            rule,
            message: message.into(),
            span: LintSpan { line, col },
        }
    }

    pub fn error(rule: &'static str, message: impl Into<String>, line: u32, col: u32) -> Self {
        Self {
            severity: Severity::Error,
            rule,
            message: message.into(),
            span: LintSpan { line, col },
        }
    }

    /// Render as a single human-readable line.
    pub fn render(&self, file: &str) -> String {
        format!(
            "{file}:{}:{}: {}: [{}] {}",
            self.span.line, self.span.col, self.severity, self.rule, self.message
        )
    }
}
