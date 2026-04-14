//! MVL linter — style and structural checks for source files.
//!
//! # Entry point
//!
//! ```ignore
//! use mvl::mvl::linter::{lint, config::LintConfig};
//!
//! let cfg = LintConfig::load(project_root);
//! let result = lint(&program, source, &cfg);
//! for diag in &result.diags {
//!     eprintln!("{}", diag.render(file_path));
//! }
//! ```

pub mod config;
pub mod errors;
pub mod rules;

use crate::mvl::parser::ast::Program;
use config::LintConfig;
use errors::LintDiag;

/// Aggregated result of linting one file.
pub struct LintResult {
    pub diags: Vec<LintDiag>,
}

impl LintResult {
    /// `true` if there are no error-severity diagnostics.
    pub fn is_ok(&self) -> bool {
        self.diags
            .iter()
            .all(|d| d.severity < errors::Severity::Error)
    }

    /// Total number of warnings.
    pub fn warning_count(&self) -> usize {
        self.diags
            .iter()
            .filter(|d| d.severity == errors::Severity::Warning)
            .count()
    }

    /// Total number of errors.
    pub fn error_count(&self) -> usize {
        self.diags
            .iter()
            .filter(|d| d.severity == errors::Severity::Error)
            .count()
    }
}

/// Run all enabled lint rules against a parsed program and its source.
///
/// Phase 1 — style rules: trailing whitespace, line length, indentation,
/// final newline, naming conventions, function body length.
///
/// Phase 2 — semantic rules: unreachable code, redundant match, unnecessary
/// type annotations, redundant effect declarations, redundant IFC labels.
pub fn lint(prog: &Program, src: &str, cfg: &LintConfig) -> LintResult {
    let mut diags: Vec<LintDiag> = Vec::new();

    // Phase 1: source rules
    rules::trailing_whitespace(src, cfg, &mut diags);
    rules::line_length(src, cfg, &mut diags);
    rules::indentation(src, cfg, &mut diags);
    rules::final_newline(src, cfg, &mut diags);

    // Phase 1: AST rules
    rules::naming(prog, cfg, &mut diags);
    rules::fn_length(prog, src, cfg, &mut diags);

    // Phase 2: semantic rules
    rules::unreachable_code(prog, cfg, &mut diags);
    rules::redundant_match(prog, cfg, &mut diags);
    rules::unnecessary_annotations(prog, cfg, &mut diags);
    rules::redundant_effects(prog, cfg, &mut diags);
    rules::redundant_ifc_labels(prog, cfg, &mut diags);

    // Sort by line then col for consistent output
    diags.sort_by_key(|d| (d.span.line, d.span.col));

    LintResult { diags }
}
