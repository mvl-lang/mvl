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

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_display_warning() {
        assert_eq!(format!("{}", Severity::Warning), "warning");
    }

    #[test]
    fn severity_display_error() {
        assert_eq!(format!("{}", Severity::Error), "error");
    }

    #[test]
    fn severity_ordering() {
        assert!(Severity::Warning < Severity::Error);
    }

    #[test]
    fn lint_diag_warning_constructor() {
        let d = LintDiag::warning("my-rule", "something is off", 5, 3);
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(d.rule, "my-rule");
        assert_eq!(d.message, "something is off");
        assert_eq!(d.span.line, 5);
        assert_eq!(d.span.col, 3);
    }

    #[test]
    fn lint_diag_error_constructor() {
        let d = LintDiag::error("bad-rule", "critical issue", 10, 1);
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.rule, "bad-rule");
        assert_eq!(d.message, "critical issue");
    }

    #[test]
    fn lint_diag_render_format() {
        let d = LintDiag::warning("line-length", "line too long", 42, 121);
        let rendered = d.render("src/main.mvl");
        assert_eq!(
            rendered,
            "src/main.mvl:42:121: warning: [line-length] line too long"
        );
    }

    #[test]
    fn lint_diag_render_error_format() {
        let d = LintDiag::error("naming", "function name must be snake_case", 1, 4);
        let rendered = d.render("lib.mvl");
        assert_eq!(
            rendered,
            "lib.mvl:1:4: error: [naming] function name must be snake_case"
        );
    }
}
