// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Source-level style rules — operate on the raw source string.

use crate::mvl::linter::{config::LintConfig, errors::LintDiag};

/// Check trailing whitespace on every line.
///
/// Rule id: `trailing-whitespace`
pub fn trailing_whitespace(src: &str, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.trailing_ws {
        return;
    }
    for (i, line) in src.lines().enumerate() {
        let line_no = (i + 1) as u32;
        let trimmed = line.trim_end();
        if trimmed.len() < line.len() {
            let col = (trimmed.len() + 1) as u32;
            out.push(LintDiag::warning(
                "trailing-whitespace",
                "trailing whitespace",
                line_no,
                col,
            ));
        }
    }
}

/// Check that no line exceeds `cfg.line_length` characters.
///
/// Rule id: `line-length`
pub fn line_length(src: &str, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if cfg.line_length == 0 {
        return;
    }
    for (i, line) in src.lines().enumerate() {
        let line_no = (i + 1) as u32;
        let len = line.chars().count();
        if len > cfg.line_length {
            out.push(LintDiag::warning(
                "line-length",
                format!("line is {len} characters (max {})", cfg.line_length),
                line_no,
                (cfg.line_length + 1).min(u32::MAX as usize) as u32,
            ));
        }
    }
}

/// Check indentation style and width.
///
/// Rule id: `indentation`
///
/// Flags:
/// * Mixed tabs/spaces on an indented line.
/// * Use of the wrong character (tabs when `indent_style = spaces`, or vice versa).
/// * Indent not a multiple of `indent_size` when using spaces.
pub fn indentation(src: &str, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.indentation {
        return;
    }
    for (i, line) in src.lines().enumerate() {
        let line_no = (i + 1) as u32;
        if line.is_empty() {
            continue;
        }
        let leading: String = line
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        if leading.is_empty() {
            continue;
        }
        let has_spaces = leading.contains(' ');
        let has_tabs = leading.contains('\t');
        if has_spaces && has_tabs {
            out.push(LintDiag::warning(
                "indentation",
                "mixed tabs and spaces in indentation",
                line_no,
                1,
            ));
            continue;
        }
        if cfg.indent_spaces && has_tabs {
            out.push(LintDiag::warning(
                "indentation",
                "tab used for indentation (expected spaces)",
                line_no,
                1,
            ));
        } else if !cfg.indent_spaces && has_spaces {
            out.push(LintDiag::warning(
                "indentation",
                "spaces used for indentation (expected tabs)",
                line_no,
                1,
            ));
        } else if cfg.indent_spaces && cfg.indent_size > 0 {
            let n = leading.len();
            if !n.is_multiple_of(cfg.indent_size) {
                out.push(LintDiag::warning(
                    "indentation",
                    format!(
                        "indent of {n} spaces is not a multiple of {}",
                        cfg.indent_size
                    ),
                    line_no,
                    1,
                ));
            }
        }
    }
}

/// Check that the file ends with exactly one newline (no trailing blank lines).
///
/// Rule id: `final-newline`
pub fn final_newline(src: &str, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.final_newline {
        return;
    }
    if src.is_empty() {
        return;
    }
    if !src.ends_with('\n') {
        let line_no = src.lines().count() as u32;
        out.push(LintDiag::warning(
            "final-newline",
            "file must end with a newline",
            line_no,
            1,
        ));
    } else if src.ends_with("\n\n") {
        let line_no = src.lines().count() as u32 + 1;
        out.push(LintDiag::warning(
            "final-newline",
            "file has trailing blank lines",
            line_no,
            1,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::linter::config::LintConfig;

    fn cfg() -> LintConfig {
        let mut c = LintConfig::default();
        c.line_length = 120;
        c.trailing_ws = true;
        c.indentation = true;
        c.final_newline = true;
        c.consistent_comment_style = true;
        c
    }

    // -- trailing_whitespace --

    #[test]
    fn trailing_ws_detected() {
        let src = "fn foo() -> Int { 1 }   \nfn bar() -> Int { 2 }\n";
        let mut diags = vec![];
        trailing_whitespace(src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "trailing-whitespace");
        assert_eq!(diags[0].span.line, 1);
    }

    #[test]
    fn trailing_ws_clean() {
        let src = "fn foo() -> Int { 1 }\n";
        let mut diags = vec![];
        trailing_whitespace(src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn trailing_ws_disabled() {
        let src = "fn foo() -> Int { 1 }   \n";
        let mut diags = vec![];
        let mut c = cfg();
        c.trailing_ws = false;
        trailing_whitespace(src, &c, &mut diags);
        assert!(diags.is_empty());
    }

    // -- line_length --

    #[test]
    fn line_length_over_limit() {
        let long = "x".repeat(121);
        let src = format!("fn foo() -> Int {{\n    {long}\n}}\n");
        let mut diags = vec![];
        line_length(&src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "line-length");
        assert_eq!(diags[0].span.line, 2);
    }

    #[test]
    fn line_length_at_limit_is_ok() {
        let exactly = "x".repeat(120);
        let src = format!("{exactly}\n");
        let mut diags = vec![];
        line_length(&src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    // -- indentation --

    #[test]
    fn mixed_indent_detected() {
        let src = "fn foo() {\n\t    x\n}\n"; // tab + spaces
        let mut diags = vec![];
        indentation(src, &cfg(), &mut diags);
        assert!(diags.iter().any(|d| d.rule == "indentation"));
    }

    #[test]
    fn tab_indent_when_spaces_expected() {
        let src = "fn foo() {\n\tx\n}\n";
        let mut diags = vec![];
        indentation(src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("tab"));
    }

    #[test]
    fn non_multiple_indent() {
        let src = "fn foo() {\n   x\n}\n"; // 3 spaces, not multiple of 4
        let mut diags = vec![];
        indentation(src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("multiple"));
    }

    #[test]
    fn correct_indent_clean() {
        let src = "fn foo() {\n    x\n}\n"; // 4 spaces
        let mut diags = vec![];
        indentation(src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }
}
