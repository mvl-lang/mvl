// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Naming convention and function-length rules — operate on the AST.

use crate::mvl::linter::{config::LintConfig, errors::LintDiag};
use crate::mvl::parser::ast::{Decl, Program, TypeBody};

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
            Decl::Fn(f) if !is_snake_case(&f.name) => {
                out.push(LintDiag::warning(
                    "naming-fn",
                    format!("function `{}` should be snake_case", f.name),
                    f.span.line,
                    f.span.col,
                ));
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
                    TypeBody::Struct { fields, .. } => {
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
            Decl::Const(c) if !is_screaming_snake_case(&c.name) => {
                out.push(LintDiag::warning(
                    "naming-const",
                    format!("constant `{}` should be SCREAMING_SNAKE_CASE", c.name),
                    c.span.line,
                    c.span.col,
                ));
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

#[cfg(test)]
mod tests {
    use super::*;

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
