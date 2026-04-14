//! Phase-1 lint rules.
//!
//! Two families:
//!
//! * **Source rules** — operate on the raw source string line-by-line.
//!   They have no access to the AST.
//!
//! * **AST rules** — traverse the parsed [`Program`] to find naming and
//!   structural violations.

use crate::mvl::linter::{config::LintConfig, errors::LintDiag};
use crate::mvl::parser::ast::{Decl, Program, TypeBody};

// ── Source rules ───────────────────────────────────────────────────────────

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
    for (i, line) in src.lines().enumerate() {
        let line_no = (i + 1) as u32;
        let len = line.chars().count();
        if len > cfg.line_length {
            out.push(LintDiag::warning(
                "line-length",
                format!("line is {len} characters (max {})", cfg.line_length),
                line_no,
                (cfg.line_length + 1) as u32,
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
pub fn final_newline(src: &str, _cfg: &LintConfig, out: &mut Vec<LintDiag>) {
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

// ── AST rules ──────────────────────────────────────────────────────────────

/// Check naming conventions across all declarations.
///
/// Rules:
/// * Functions → `snake_case`  (rule id: `naming-fn`)
/// * Types     → `PascalCase`  (rule id: `naming-type`)
/// * Fields    → `snake_case`  (rule id: `naming-field`)
/// * Constants → `SCREAMING_SNAKE_CASE` (rule id: `naming-const`)
pub fn naming(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.naming {
        return;
    }
    for decl in &prog.declarations {
        match decl {
            Decl::Fn(f) => {
                if !is_snake_case(&f.name) {
                    out.push(LintDiag::warning(
                        "naming-fn",
                        format!("function `{}` should be snake_case", f.name),
                        f.span.line,
                        f.span.col,
                    ));
                }
            }
            Decl::Type(t) => {
                if !is_pascal_case(&t.name) {
                    out.push(LintDiag::warning(
                        "naming-type",
                        format!("type `{}` should be PascalCase", t.name),
                        t.span.line,
                        t.span.col,
                    ));
                }
                // Check field names inside structs and enum struct-variants
                match &t.body {
                    TypeBody::Struct(fields) => {
                        for field in fields {
                            if !is_snake_case(&field.name) {
                                out.push(LintDiag::warning(
                                    "naming-field",
                                    format!("field `{}` should be snake_case", field.name),
                                    field.span.line,
                                    field.span.col,
                                ));
                            }
                        }
                    }
                    TypeBody::Enum(variants) => {
                        for variant in variants {
                            if !is_pascal_case(&variant.name) {
                                out.push(LintDiag::warning(
                                    "naming-variant",
                                    format!("enum variant `{}` should be PascalCase", variant.name),
                                    variant.span.line,
                                    variant.span.col,
                                ));
                            }
                            // Struct-variant fields
                            if let crate::mvl::parser::ast::VariantFields::Struct(fields) =
                                &variant.fields
                            {
                                for field in fields {
                                    if !is_snake_case(&field.name) {
                                        out.push(LintDiag::warning(
                                            "naming-field",
                                            format!("field `{}` should be snake_case", field.name),
                                            field.span.line,
                                            field.span.col,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    TypeBody::Alias(_) => {}
                }
            }
            Decl::Const(c) => {
                if !is_screaming_snake_case(&c.name) {
                    out.push(LintDiag::warning(
                        "naming-const",
                        format!("constant `{}` should be SCREAMING_SNAKE_CASE", c.name),
                        c.span.line,
                        c.span.col,
                    ));
                }
            }
            _ => {}
        }
    }
}

/// Check function body length.
///
/// Rule id: `fn-length`
///
/// A function body that spans more than `cfg.max_fn_length` source lines is
/// flagged as a warning — long functions are harder for humans and LLMs alike.
pub fn fn_length(prog: &Program, src: &str, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if cfg.max_fn_length == 0 {
        return;
    }
    let lines: Vec<&str> = src.lines().collect();
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            let body = &f.body;
            // Body span covers the opening `{` to the closing `}`.
            let start_line = body.span.line as usize; // 1-based
                                                      // End line: walk from start, count braces.
            let end_line = body_end_line(src, &lines, start_line);
            let length = end_line.saturating_sub(start_line) + 1;
            if length > cfg.max_fn_length {
                out.push(LintDiag::warning(
                    "fn-length",
                    format!(
                        "function `{}` body is {length} lines (max {})",
                        f.name, cfg.max_fn_length
                    ),
                    f.span.line,
                    f.span.col,
                ));
            }
        }
    }
}

/// Estimate the closing-brace line of a block starting at `start_line` (1-based).
fn body_end_line(src: &str, lines: &[&str], start_line: usize) -> usize {
    let _ = src;
    let mut depth: i32 = 0;
    for (i, line) in lines.iter().enumerate().skip(start_line - 1) {
        for ch in line.chars() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return i + 1; // 1-based
                    }
                }
                _ => {}
            }
        }
    }
    lines.len() // fallback
}

// ── Naming helpers ─────────────────────────────────────────────────────────

/// `foo`, `foo_bar`, `_foo`, `foo_bar_baz42` — all lowercase, underscores ok.
fn is_snake_case(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    // Allow leading underscore (unused-binding convention: `_foo`)
    let check = s.trim_start_matches('_');
    check
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// `Foo`, `FooBar`, `Foo42` — starts with uppercase, no underscores.
fn is_pascal_case(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_uppercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric())
}

/// `FOO`, `FOO_BAR`, `FOO_BAR_42` — all uppercase, underscores ok.
fn is_screaming_snake_case(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    s.chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

// ── Unit tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::linter::config::LintConfig;

    fn cfg() -> LintConfig {
        LintConfig::default()
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

    // -- naming --

    #[test]
    fn naming_snake_case_fn_ok() {
        assert!(is_snake_case("foo_bar"));
        assert!(is_snake_case("_unused"));
        assert!(is_snake_case("foo42"));
    }

    #[test]
    fn naming_snake_case_fn_bad() {
        assert!(!is_snake_case("FooBar"));
        assert!(!is_snake_case("fooBar"));
    }

    #[test]
    fn naming_pascal_case_ok() {
        assert!(is_pascal_case("FooBar"));
        assert!(is_pascal_case("Foo42"));
    }

    #[test]
    fn naming_pascal_case_bad() {
        assert!(!is_pascal_case("foo_bar"));
        assert!(!is_pascal_case("fooBar"));
        assert!(!is_pascal_case("Foo_Bar"));
    }

    #[test]
    fn naming_screaming_snake_ok() {
        assert!(is_screaming_snake_case("FOO_BAR"));
        assert!(is_screaming_snake_case("MAX_LEN"));
    }

    #[test]
    fn naming_screaming_snake_bad() {
        assert!(!is_screaming_snake_case("foo_bar"));
        assert!(!is_screaming_snake_case("FooBar"));
    }
}
