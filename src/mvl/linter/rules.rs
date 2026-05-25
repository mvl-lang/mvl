// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Lint rules — split into submodules by phase.
//!
//! * [`style`] — Phase 1 source-level checks (trailing whitespace, line length, indentation)
//! * [`ast_style`] — Phase 1 AST checks (naming conventions, function length)
//! * [`semantic`] — Phase 2 semantic analysis (unreachable code, redundant patterns, IFC, termination)
//! * [`reading_quality`] — Phase 3 reading quality (comment style, doc comments)
//! * [`complexity`] — Phase 4 complexity metrics (cyclomatic, match depth, effect width)

mod ast_style;
mod complexity;
mod reading_quality;
mod semantic;
mod style;

// Re-export all public rule functions so callers continue to use `rules::rule_name`.
pub use ast_style::{fn_length, naming};
#[cfg(test)]
use ast_style::{is_pascal_case, is_screaming_snake_case, is_snake_case};
pub use complexity::{
    complexity_cyclomatic, complexity_effect_width, complexity_extern_ratio,
    complexity_match_depth, complexity_module_fanout, complexity_trait_impl_count,
};
pub use reading_quality::{consistent_comment_style, doc_comment_examples, doc_comments_required};
pub use semantic::{
    for_iter_antipattern, missing_annotations, missing_totality, redundant_effects,
    redundant_ifc_labels, redundant_match, suggest_decreases, suggest_total_upgrade,
    unreachable_code, while_to_for_range,
};
pub use style::{final_newline, indentation, line_length, trailing_whitespace};

// ── Unit tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::linter::config::LintConfig;
    use crate::mvl::parser::Parser;

    fn cfg() -> LintConfig {
        let mut c = LintConfig::default();
        // Enable all style rules so rule-level tests can verify they fire correctly.
        c.line_length = 120;
        c.trailing_ws = true;
        c.indentation = true;
        c.final_newline = true;
        c.consistent_comment_style = true;
        c
    }

    fn parse(src: &str) -> crate::mvl::parser::ast::Program {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        prog
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

    // ── Phase 2 tests ─────────────────────────────────────────────────────

    // -- unreachable_code --

    #[test]
    fn unreachable_after_return_detected() {
        let src = "fn f() -> Int { return 1; let x: Int = 2; x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        unreachable_code(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1, "expected 1 unreachable diagnostic");
        assert_eq!(diags[0].rule, "unreachable-code");
    }

    #[test]
    fn unreachable_code_disabled() {
        let src = "fn f() -> Int { return 1; let x: Int = 2; x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.unreachable_code = false;
        unreachable_code(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn reachable_code_clean() {
        let src = "fn f() -> Int { let x: Int = 1; return x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        unreachable_code(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    // -- redundant_match --

    #[test]
    fn single_arm_wildcard_detected() {
        let src = "fn f(x: Int) -> Int { match x { _ => x } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_match(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "redundant-match");
    }

    #[test]
    fn single_arm_binding_detected() {
        let src = "fn f(x: Int) -> Int { match x { v => v } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_match(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "redundant-match");
    }

    #[test]
    fn multi_arm_match_clean() {
        let src = "type Color = enum { Red, Green, Blue }\nfn f(c: Color) -> Int { match c { Color::Red => 1 Color::Green => 2 Color::Blue => 3 } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_match(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    // -- redundant_effects --

    #[test]
    fn effects_on_call_free_fn_detected() {
        let src = "fn f() -> Int ! Console { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_effects(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "redundant-effects");
        assert!(diags[0].message.contains("Console"));
    }

    #[test]
    fn effects_on_fn_with_call_clean() {
        let src = "fn f() -> Unit ! Console {\n    println(\"hi\")\n}\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_effects(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn no_effects_declared_clean() {
        let src = "fn f() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_effects(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    // -- redundant_ifc_labels --

    #[test]
    fn public_label_on_param_no_longer_detected(/* #894: Public is not a label anymore */) {
        // Post-#894: `Public` is a plain identifier, not a label keyword.
        // `Public[Int]` parses as a generic base type, not TypeExpr::Labeled.
        // The redundant-ifc-label rule no longer fires for it.
        let src = "fn f(x: Public[Int]) -> Int { x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty(), "Public is no longer a label (#894)");
    }

    #[test]
    fn secret_label_on_param_clean() {
        let src = "fn f(x: Secret[Int]) -> Int { x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn public_label_on_return_type_no_longer_detected() {
        // Post-#894: `Public[String]` is just a generic type — no lint fires.
        let src = "fn f() -> Public[String] { \"hi\" }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn redundant_ifc_disabled() {
        let src = "fn f(x: Tainted[Int]) -> Int { relabel trust(x, \"V-01\") }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.redundant_ifc_labels = false;
        redundant_ifc_labels(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn public_label_on_struct_field_no_longer_detected() {
        // Post-#894: `Public[Int]` in a struct is a generic type, not a labeled type.
        let src = "type Wrapper = struct { data: Public[Int] }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn public_label_in_type_alias_no_longer_detected() {
        // Post-#894: `Public[Int]` as type alias — not a labeled type.
        let src = "type MyInt = Public[Int]\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    // -- redundant_match: missing config-disable test --

    #[test]
    fn redundant_match_disabled() {
        let src = "fn f(x: Int) -> Int { match x { _ => x } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.redundant_match = false;
        redundant_match(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    // -- redundant_effects: missing config-disable test --

    #[test]
    fn redundant_effects_disabled() {
        let src = "fn f() -> Int ! Console { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.redundant_effects = false;
        redundant_effects(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    // ── Phase 3: consistent_comment_style ──────────────────────────────

    #[test]
    fn block_comment_detected() {
        let src = "/* this is illegal */\nfn f() -> Int { 42 }\n";
        let mut diags = vec![];
        consistent_comment_style(src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "consistent-comment-style");
        assert_eq!(diags[0].span.line, 1);
    }

    #[test]
    fn block_comment_mid_line_detected() {
        let src = "fn f() -> Int { 42 } /* whoops */\n";
        let mut diags = vec![];
        consistent_comment_style(src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "consistent-comment-style");
        assert_eq!(diags[0].span.col, 22);
    }

    #[test]
    fn line_comment_clean() {
        let src = "// ok\n/// doc\nfn f() -> Int { 42 }\n";
        let mut diags = vec![];
        consistent_comment_style(src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn consistent_comment_style_disabled() {
        let src = "/* block */\nfn f() -> Int { 42 }\n";
        let mut diags = vec![];
        let mut c = cfg();
        c.consistent_comment_style = false;
        consistent_comment_style(src, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn block_comment_after_line_comment_not_flagged() {
        // `/*` appearing after `//` on the same line is inside a line comment.
        let src = "// this is fine /* not a block comment */\nfn f() -> Int { 42 }\n";
        let mut diags = vec![];
        consistent_comment_style(src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn block_comment_multiple_on_same_line_single_diag() {
        // find() stops at the first match; only one diag per line is emitted.
        let src = "/* a */ /* b */\nfn f() -> Int { 42 }\n";
        let mut diags = vec![];
        consistent_comment_style(src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].span.col, 1); // only first occurrence reported
    }

    // ── Phase 3: doc_comments_required ─────────────────────────────────

    #[test]
    fn pub_fn_missing_doc_comment_detected() {
        let src = "pub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "missing-doc-comment");
        assert!(diags[0].message.contains("foo"));
    }

    #[test]
    fn pub_fn_with_doc_comment_ok() {
        let src = "/// Does something.\npub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn private_fn_no_doc_comment_ok() {
        let src = "fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn pub_type_missing_doc_comment_detected() {
        let src = "pub type Foo = struct { x: Int }\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "missing-doc-comment");
        assert!(diags[0].message.contains("Foo"));
    }

    #[test]
    fn pub_type_with_doc_comment_ok() {
        let src = "/// A wrapper type.\npub type Foo = struct { x: Int }\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn doc_comment_blank_line_between_ok() {
        // A blank line between doc comment and declaration is allowed.
        let src = "/// Docs here.\n\npub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn regular_comment_not_doc_comment() {
        // `//` is not `///`; should still flag.
        let src = "// not a doc comment\npub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "missing-doc-comment");
    }

    #[test]
    fn require_doc_comments_disabled() {
        let src = "pub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.require_doc_comments = false;
        doc_comments_required(&prog, src, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn pub_const_missing_doc_comment_detected() {
        let src = "pub const MAX: Int = 100;\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "missing-doc-comment");
        assert!(diags[0].message.contains("MAX"));
    }

    #[test]
    fn pub_const_with_doc_comment_ok() {
        let src = "/// The maximum value.\npub const MAX: Int = 100;\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn private_const_no_doc_comment_ok() {
        let src = "const MAX: Int = 100;\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    // ── Phase 3: doc_comment_examples ──────────────────────────────────

    #[test]
    fn pub_fn_doc_without_example_flagged() {
        let src = "/// Does something.\npub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.doc_comment_examples = true;
        doc_comment_examples(&prog, src, &c, &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "doc-comment-example");
        assert!(diags[0].message.contains("foo"));
    }

    #[test]
    fn pub_fn_doc_with_example_ok() {
        let src = "/// Does something.\n/// Example: foo() == 42\npub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.doc_comment_examples = true;
        doc_comment_examples(&prog, src, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn doc_comment_examples_disabled_by_default() {
        // default config has doc_comment_examples = false
        let src = "/// No example.\npub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comment_examples(&prog, src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn pub_fn_no_doc_skipped_for_examples() {
        // If there's no doc comment at all, missing-doc-comment fires but
        // doc-comment-example should stay silent to avoid duplicate noise.
        let src = "pub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.doc_comment_examples = true;
        doc_comment_examples(&prog, src, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn pub_type_doc_without_example_flagged() {
        let src = "/// A wrapper type.\npub type Foo = struct { x: Int }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.doc_comment_examples = true;
        doc_comment_examples(&prog, src, &c, &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "doc-comment-example");
        assert!(diags[0].message.contains("Foo"));
        assert!(diags[0].message.contains("type"));
    }

    #[test]
    fn doc_comment_example_case_insensitive_ok() {
        // "# Example" (capital E) and "Examples:" (plural) both accepted.
        let src =
            "/// Does something.\n/// # Example\n/// foo() == 42\npub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.doc_comment_examples = true;
        doc_comment_examples(&prog, src, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn doc_comment_examples_plural_ok() {
        let src = "/// Does something.\n/// Examples: foo() == 42\npub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.doc_comment_examples = true;
        doc_comment_examples(&prog, src, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn pub_const_no_example_not_flagged() {
        // doc_comment_examples intentionally excludes pub const; pin this design decision.
        let src = "/// The maximum value.\npub const MAX: Int = 100;\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.doc_comment_examples = true;
        doc_comment_examples(&prog, src, &c, &mut diags);
        assert!(diags.is_empty());
    }

    // ── Phase 4: complexity rules ──────────────────────────────────────

    // -- complexity_cyclomatic --

    #[test]
    fn cyclomatic_simple_fn_clean() {
        // CC = 1 (no branches)
        let src = "fn f(x: Int) -> Int { x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        complexity_cyclomatic(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn cyclomatic_if_increments() {
        // CC = 1 + 1 (if) = 2, well within default 10
        let src = "fn f(x: Int) -> Int { if x > 0 { x } else { 0 } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        complexity_cyclomatic(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn cyclomatic_exceeds_threshold() {
        // Build a function with CC > 10 via nested ifs
        let src = r#"fn complex(x: Int) -> Int {
    if x > 1 {
        if x > 2 {
            if x > 3 {
                if x > 4 {
                    if x > 5 {
                        if x > 6 {
                            if x > 7 {
                                if x > 8 {
                                    if x > 9 {
                                        if x > 10 {
                                            x
                                        } else { 0 }
                                    } else { 0 }
                                } else { 0 }
                            } else { 0 }
                        } else { 0 }
                    } else { 0 }
                } else { 0 }
            } else { 0 }
        } else { 0 }
    } else { 0 }
}
"#;
        let prog = parse(src);
        let mut diags = vec![];
        complexity_cyclomatic(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "complexity-cyclomatic"),
            "expected cyclomatic-complexity warning, got: {diags:?}"
        );
    }

    #[test]
    fn cyclomatic_disabled() {
        let src = r#"fn f(x: Int) -> Int {
    if x > 1 { if x > 2 { if x > 3 { if x > 4 { if x > 5 {
    if x > 6 { if x > 7 { if x > 8 { if x > 9 { if x > 10 {
        x } else { 0 } } else { 0 } } else { 0 } } else { 0 } } else { 0 }
    } else { 0 } } else { 0 } } else { 0 } } else { 0 } } else { 0 }
}
"#;
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.max_cyclomatic_complexity = 0;
        complexity_cyclomatic(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn cyclomatic_match_arms_contribute() {
        // 5-arm match → CC = 1 + 4 = 5
        let src = "type D = enum { A, B, C, D, E }\nfn f(d: D) -> Int { match d { D::A => 1 D::B => 2 D::C => 3 D::D => 4 D::E => 5 } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.max_cyclomatic_complexity = 3; // lower threshold to trigger
        complexity_cyclomatic(&prog, &c, &mut diags);
        assert!(diags.iter().any(|d| d.rule == "complexity-cyclomatic"));
    }

    // -- complexity_match_depth --

    #[test]
    fn match_depth_single_match_clean() {
        // depth = 1, default max = 3
        let src = "type C = enum { X, Y }\nfn f(c: C) -> Int { match c { C::X => 1 C::Y => 2 } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        complexity_match_depth(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn match_depth_exceeds_threshold() {
        // depth = 4 > max 3
        let src = "type C = enum { X, Y }\nfn f(c: C) -> Int {\n    match c {\n        C::X => match c { C::X => match c { C::X => match c { C::X => 1 C::Y => 2 } C::Y => 3 } C::Y => 4 }\n        C::Y => 0\n    }\n}\n";
        let prog = parse(src);
        let mut diags = vec![];
        complexity_match_depth(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "complexity-match-depth"),
            "expected match-depth warning, got: {diags:?}"
        );
    }

    #[test]
    fn match_depth_disabled() {
        let src = "type C = enum { X, Y }\nfn f(c: C) -> Int {\n    match c { C::X => match c { C::X => match c { C::X => match c { C::X => 1 C::Y => 2 } C::Y => 3 } C::Y => 4 } C::Y => 0 }\n}\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.max_nested_match_depth = 0;
        complexity_match_depth(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    // -- complexity_effect_width --

    #[test]
    fn effect_width_within_limit_clean() {
        let src = "fn f() -> Unit ! Console { f() }\n";
        let prog = parse(src);
        let mut diags = vec![];
        complexity_effect_width(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn effect_width_exceeds_threshold() {
        // 4 effects > default max 3
        let src = "fn f() -> Unit ! Console + DB + Network + FileSystem { f() }\n";
        let prog = parse(src);
        let mut diags = vec![];
        complexity_effect_width(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "complexity-effect-width"),
            "expected effect-width warning, got: {diags:?}"
        );
    }

    #[test]
    fn effect_width_disabled() {
        let src = "fn f() -> Unit ! Console + DB + Network + FileSystem { f() }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.max_effect_signature_width = 0;
        complexity_effect_width(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    // -- complexity_trait_impl_count --

    #[test]
    fn trait_impl_count_within_limit_clean() {
        let src = "type Foo = struct { x: Int }\nimpl Display for Foo { fn fmt(t: Foo) -> String { \"foo\" } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        complexity_trait_impl_count(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn trait_impl_count_exceeds_threshold() {
        let src = concat!(
            "type T = struct { x: Int }\n",
            "impl A for T { fn a(t: T) -> Int { 1 } }\n",
            "impl B for T { fn b(t: T) -> Int { 2 } }\n",
            "impl C for T { fn c(t: T) -> Int { 3 } }\n",
            "impl D for T { fn d(t: T) -> Int { 4 } }\n",
            "impl E for T { fn e(t: T) -> Int { 5 } }\n",
            "impl F for T { fn f(t: T) -> Int { 6 } }\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        complexity_trait_impl_count(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "complexity-trait-impl-count"),
            "expected trait-impl-count warning, got: {diags:?}"
        );
    }

    #[test]
    fn trait_impl_count_disabled() {
        let src = concat!(
            "type T = struct { x: Int }\n",
            "impl A for T { fn a(t: T) -> Int { 1 } }\n",
            "impl B for T { fn b(t: T) -> Int { 2 } }\n",
            "impl C for T { fn c(t: T) -> Int { 3 } }\n",
            "impl D for T { fn d(t: T) -> Int { 4 } }\n",
            "impl E for T { fn e(t: T) -> Int { 5 } }\n",
            "impl F for T { fn f(t: T) -> Int { 6 } }\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.max_trait_impl_count = 0;
        complexity_trait_impl_count(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    // -- complexity_module_fanout --

    #[test]
    fn module_fanout_within_limit_clean() {
        // Both imports from "std" → fanout = 1, well within default 15
        let src = "use std.io.{File, Read}\nfn f() -> Unit { f() }\n";
        let prog = parse(src);
        let mut diags = vec![];
        complexity_module_fanout(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn module_fanout_exceeds_threshold() {
        // 3 distinct root modules (a, b, c), threshold 2
        let src = "use a.{Foo}\nuse b.{Bar}\nuse c.{Baz}\nfn f() -> Unit { f() }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.max_module_fanout = 2;
        complexity_module_fanout(&prog, &c, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "complexity-module-fanout"),
            "expected module-fanout warning, got: {diags:?}"
        );
    }

    #[test]
    fn module_fanout_disabled() {
        let src = "use a.{Foo}\nuse b.{Bar}\nuse c.{Baz}\nfn f() -> Unit { f() }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.max_module_fanout = 0;
        complexity_module_fanout(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    // -- complexity_extern_ratio --

    #[test]
    fn extern_ratio_clean() {
        // 1 extern fn, 4 total → 25% ≤ 20%? No, so let's use 1 extern / 10 total
        let src = concat!(
            "extern \"rust\" { fn ext() -> Int }\n",
            "fn a() -> Int { 1 }\nfn b() -> Int { 2 }\nfn c() -> Int { 3 }\n",
            "fn d() -> Int { 4 }\nfn e() -> Int { 5 }\nfn g() -> Int { 6 }\n",
            "fn h() -> Int { 7 }\nfn i() -> Int { 8 }\nfn j() -> Int { 9 }\n",
        );
        // 1 extern / 10 total = 10% <= 20%
        let prog = parse(src);
        let mut diags = vec![];
        complexity_extern_ratio(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn extern_ratio_exceeds_threshold() {
        // 3 extern fns / 12 total = 25% > 20%; min_fns_for_extern_ratio = 0 to isolate ratio logic
        let src = concat!(
            "extern \"rust\" { fn e1() -> Int fn e2() -> Int fn e3() -> Int }\n",
            "fn n1() -> Int { 1 }\nfn n2() -> Int { 1 }\nfn n3() -> Int { 1 }\n",
            "fn n4() -> Int { 1 }\nfn n5() -> Int { 1 }\nfn n6() -> Int { 1 }\n",
            "fn n7() -> Int { 1 }\nfn n8() -> Int { 1 }\nfn n9() -> Int { 1 }\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        complexity_extern_ratio(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "complexity-extern-ratio"),
            "expected extern-ratio warning, got: {diags:?}"
        );
    }

    #[test]
    fn extern_ratio_disabled() {
        let src = concat!(
            "extern \"rust\" { fn e1() -> Int fn e2() -> Int }\n",
            "fn native() -> Int { 1 }\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.max_extern_ratio = 0.0;
        complexity_extern_ratio(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn extern_ratio_no_fns_at_all_clean() {
        // A program with zero functions must not panic (division-by-zero guard).
        let src = "type Foo = struct { x: Int }\n";
        let prog = parse(src);
        let mut diags = vec![];
        complexity_extern_ratio(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty(), "no fns → ratio undefined → no diagnostic");
    }

    // -- complexity_cyclomatic: &&/|| and impl methods --

    #[test]
    fn cyclomatic_and_or_contribute() {
        // CC = 1 (base) + 1 (if) + 1 (&&) + 1 (||) = 4; threshold=3 → must fire
        let src =
            "fn f(a: Int, b: Int, c: Int) -> Int { if a > 0 && b > 0 || c > 0 { 1 } else { 0 } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.max_cyclomatic_complexity = 3;
        complexity_cyclomatic(&prog, &c, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "complexity-cyclomatic"),
            "&&/|| must each contribute +1 to CC; got: {diags:?}"
        );
    }

    #[test]
    fn cyclomatic_impl_method_exceeds_threshold() {
        // impl method with CC > 10 must be flagged with trait/type context
        let src = concat!(
            "type Foo = struct { x: Int }\n",
            "impl Bar for Foo {\n",
            "    fn m(f: Foo) -> Int {\n",
            "        if f.x > 1 { if f.x > 2 { if f.x > 3 { if f.x > 4 {\n",
            "            if f.x > 5 { if f.x > 6 { if f.x > 7 { if f.x > 8 {\n",
            "                if f.x > 9 { if f.x > 10 { f.x } else { 0 } }\n",
            "                else { 0 } } else { 0 } } else { 0 } } else { 0 }\n",
            "        } else { 0 } } else { 0 } } else { 0 } } else { 0 }\n",
            "    }\n",
            "}\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        complexity_cyclomatic(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().any(
                |d| d.rule == "complexity-cyclomatic" && d.message.contains("impl Bar for Foo")
            ),
            "impl method CC must be flagged with trait/type context; got: {diags:?}"
        );
    }

    // -- complexity_match_depth: impl methods --

    #[test]
    fn match_depth_impl_method_exceeds_threshold() {
        let src = concat!(
            "type C = enum { X, Y }\n",
            "type Foo = struct { c: C }\n",
            "impl Bar for Foo {\n",
            "    fn m(f: Foo) -> Int {\n",
            "        match f.c {\n",
            "            C::X => match f.c { C::X => match f.c {\n",
            "                C::X => match f.c { C::X => 1 C::Y => 2 } C::Y => 3\n",
            "            } C::Y => 4 }\n",
            "            C::Y => 0\n",
            "        }\n",
            "    }\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        complexity_match_depth(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "complexity-match-depth"
                    && d.message.contains("impl Bar for Foo")),
            "impl method match depth must be flagged with trait/type context; got: {diags:?}"
        );
    }

    // -- complexity_effect_width: impl methods --

    #[test]
    fn effect_width_impl_method_exceeds_threshold() {
        // impl method with 4 effects (> default max 3) must fire
        let src = concat!(
            "type Foo = struct { x: Int }\n",
            "impl Bar for Foo {\n",
            "    fn m(f: Foo) -> Unit ! Console + DB + Network + FileSystem { f.x }\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        complexity_effect_width(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "complexity-effect-width"
                    && d.message.contains("impl Bar for Foo")),
            "impl method effect width must be flagged; got: {diags:?}"
        );
    }

    // -- missing_annotations --

    fn cfg_missing_annotations_on() -> LintConfig {
        LintConfig {
            missing_annotations: true,
            ..LintConfig::default()
        }
    }

    #[test]
    fn missing_annotations_fires_on_call_without_effects() {
        // fn has a call but no declared effects — must warn when rule is enabled
        let src = "fn foo() -> Unit {\n    bar()\n}\nfn bar() -> Unit { 1 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_annotations(&prog, &cfg_missing_annotations_on(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "missing-annotation" && d.message.contains("foo")),
            "expected missing-annotation for `foo`; got: {diags:?}"
        );
    }

    #[test]
    fn missing_annotations_off_by_default() {
        // default config has missing_annotations = false — rule must be silent
        let src = "fn foo() -> Unit {\n    bar()\n}\nfn bar() -> Unit { 1 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_annotations(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-annotation"),
            "missing-annotation must not fire with default config; got: {diags:?}"
        );
    }

    #[test]
    fn missing_annotations_no_fire_on_effects_declared() {
        // fn has calls AND declared effects — must not warn
        let src = "fn foo() -> Unit ! Console {\n    bar()\n}\nfn bar() -> Unit { 1 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_annotations(&prog, &cfg_missing_annotations_on(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-annotation"),
            "must not warn when effects are declared; got: {diags:?}"
        );
    }

    #[test]
    fn missing_annotations_no_fire_on_callless_fn() {
        // pure arithmetic fn — no calls, no effects — must not warn
        let src = "fn add(x: Int, y: Int) -> Int {\n    x + y\n}\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_annotations(&prog, &cfg_missing_annotations_on(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-annotation"),
            "must not warn on call-free function; got: {diags:?}"
        );
    }

    #[test]
    fn missing_annotations_no_fire_on_test_fn() {
        // test fn is excluded from the rule
        let src = "test fn check_add() -> Unit {\n    bar()\n}\nfn bar() -> Unit { 1 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_annotations(&prog, &cfg_missing_annotations_on(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-annotation"),
            "test fn must be excluded; got: {diags:?}"
        );
    }

    // -- missing_totality --

    #[test]
    fn missing_totality_fires_on_unannotated_pub_fn() {
        // unannotated pub fn must warn with default config (rule is ON by default)
        let src = "pub fn add(x: Int, y: Int) -> Int { x + y }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "missing-totality" && d.message.contains("add")),
            "expected missing-totality for `add`; got: {diags:?}"
        );
    }

    #[test]
    fn missing_totality_silent_on_private_fn() {
        // private fn must not warn even when unannotated
        let src = "fn helper(x: Int) -> Int { x + 1 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-totality"),
            "private fn must not trigger missing-totality; got: {diags:?}"
        );
    }

    #[test]
    fn missing_totality_silent_when_annotated() {
        // explicit `total` suppresses the warning
        let src = "pub total fn add(x: Int, y: Int) -> Int { x + y }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-totality"),
            "annotated pub fn must not warn; got: {diags:?}"
        );
    }

    #[test]
    fn missing_totality_suggests_total_for_simple_fn() {
        // no while, no recursion → suggest `total`
        let src = "pub fn double(x: Int) -> Int { x * 2 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "missing-totality" && d.message.contains("`total`")),
            "should suggest `total` for simple fn; got: {diags:?}"
        );
    }

    #[test]
    fn missing_totality_suggests_partial_for_while_no_decreases() {
        // while without decreases → suggest `partial`
        let src = concat!(
            "pub fn count(n: Int) -> Int {\n",
            "    let i: ref Int = 0;\n",
            "    while i < n {\n",
            "        i = i + 1\n",
            "    }\n",
            "    i\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "missing-totality" && d.message.contains("`partial`")),
            "should suggest `partial` for while without decreases; got: {diags:?}"
        );
    }

    #[test]
    fn missing_totality_suggests_partial_for_recursive_fn() {
        // direct recursion → suggest `partial`
        let src = concat!(
            "pub fn factorial(n: Int) -> Int {\n",
            "    if n <= 1 { 1 } else { n * factorial(n - 1) }\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "missing-totality" && d.message.contains("`partial`")),
            "should suggest `partial` for recursive fn; got: {diags:?}"
        );
    }

    // -- while_to_for_range --

    #[test]
    fn while_to_for_range_fires_on_counter_loop() {
        // classic counter pattern must warn with default config
        let src = concat!(
            "fn f(n: Int) -> Int {\n",
            "    let i: ref Int = 0;\n",
            "    let s: ref Int = 0;\n",
            "    while i < n {\n",
            "        s = s + i;\n",
            "        i = i + 1\n",
            "    }\n",
            "    s\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        while_to_for_range(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "while-to-for-range" && d.message.contains("range(0, n)")),
            "expected while-to-for-range for counter loop; got: {diags:?}"
        );
    }

    #[test]
    fn while_to_for_range_silent_with_decreases() {
        // while with decreases is already total — must not warn
        let src = concat!(
            "fn f(n: Int) -> Int {\n",
            "    let i: ref Int = 0;\n",
            "    while i < n decreases n - i {\n",
            "        i = i + 1\n",
            "    }\n",
            "    i\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        while_to_for_range(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "while-to-for-range"),
            "while with decreases must not warn; got: {diags:?}"
        );
    }

    #[test]
    fn while_to_for_range_silent_without_increment() {
        // while with no VAR=VAR+N increment in last position — not the pattern
        let src = concat!(
            "fn f(n: Int) -> Int {\n",
            "    let i: ref Int = 0;\n",
            "    while i < n {\n",
            "        i = i + 1;\n",
            "        let x: Int = 0;\n",
            "        x\n",
            "    }\n",
            "    i\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        while_to_for_range(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "while-to-for-range"),
            "increment not in last position must not warn; got: {diags:?}"
        );
    }

    #[test]
    fn while_to_for_range_suggestion_shows_start() {
        // start value from let binding must appear in suggestion
        let src = concat!(
            "fn f(n: Int) -> Int {\n",
            "    let i: ref Int = 3;\n",
            "    while i < n {\n",
            "        i = i + 1\n",
            "    }\n",
            "    i\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        while_to_for_range(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "while-to-for-range" && d.message.contains("range(3, n)")),
            "suggestion must include start value; got: {diags:?}"
        );
    }

    #[test]
    fn while_to_for_range_off_when_disabled() {
        // rule can be opted out via config
        let cfg_off = LintConfig {
            while_to_for_range: false,
            ..LintConfig::default()
        };
        let src = concat!(
            "fn f(n: Int) -> Int {\n",
            "    let i: ref Int = 0;\n",
            "    while i < n {\n",
            "        i = i + 1\n",
            "    }\n",
            "    i\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        while_to_for_range(&prog, &cfg_off, &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "while-to-for-range"),
            "rule must be silent when disabled; got: {diags:?}"
        );
    }

    #[test]
    fn missing_totality_off_when_disabled() {
        // rule can be opted out via config
        let cfg_off = LintConfig {
            require_explicit_totality: false,
            ..LintConfig::default()
        };
        let src = "pub fn add(x: Int, y: Int) -> Int { x + y }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg_off, &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-totality"),
            "rule must be silent when disabled; got: {diags:?}"
        );
    }

    // ── suggest-decreases (#1037) ────────────────────────────────────────

    #[test]
    fn suggest_decreases_fires_on_decrement_loop() {
        let src = concat!(
            "fn countdown(n: Int) -> Unit {\n",
            "    let i: ref Int = n;\n",
            "    while i > 0 {\n",
            "        i = i - 1\n",
            "    }\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        suggest_decreases(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "suggest-decreases" && d.message.contains("decreases i")),
            "expected suggest-decreases hint for i = i - 1 loop; got: {diags:?}"
        );
    }

    #[test]
    fn suggest_decreases_silent_when_decreases_present() {
        let src = concat!(
            "fn countdown(n: Int) -> Unit {\n",
            "    let i: ref Int = n;\n",
            "    while i > 0 decreases i {\n",
            "        i = i - 1\n",
            "    }\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        suggest_decreases(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "suggest-decreases"),
            "while with decreases must not hint; got: {diags:?}"
        );
    }

    #[test]
    fn suggest_decreases_silent_in_partial_fn() {
        let src = concat!(
            "partial fn server() -> Unit {\n",
            "    let running: ref Int = 1;\n",
            "    while running > 0 {\n",
            "        running = running - 1\n",
            "    }\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        suggest_decreases(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "suggest-decreases"),
            "partial fn must not get decreases suggestion; got: {diags:?}"
        );
    }

    #[test]
    fn suggest_decreases_off_when_disabled() {
        let cfg_off = LintConfig {
            suggest_decreases: false,
            ..LintConfig::default()
        };
        let src = concat!(
            "fn f(n: Int) -> Unit {\n",
            "    let i: ref Int = n;\n",
            "    while i > 0 {\n",
            "        i = i - 1\n",
            "    }\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        suggest_decreases(&prog, &cfg_off, &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "suggest-decreases"),
            "rule must be silent when disabled; got: {diags:?}"
        );
    }

    // ── suggest-total-upgrade (#1038) ────────────────────────────────────

    #[test]
    fn suggest_total_upgrade_fires_on_bounded_partial() {
        let src = concat!(
            "partial fn sum(items: List[Int]) -> Int {\n",
            "    let acc: ref Int = 0;\n",
            "    for x in items {\n",
            "        acc = acc + x\n",
            "    }\n",
            "    acc\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        suggest_total_upgrade(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().any(
                |d| d.rule == "suggest-total-upgrade" && d.message.contains("`partial fn sum`")
            ),
            "expected suggest-total-upgrade for bounded partial fn; got: {diags:?}"
        );
    }

    #[test]
    fn suggest_total_upgrade_silent_when_unbounded() {
        let src = concat!(
            "partial fn server() -> Unit {\n",
            "    while true { }\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        suggest_total_upgrade(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "suggest-total-upgrade"),
            "partial fn with unbounded while must not suggest total; got: {diags:?}"
        );
    }

    #[test]
    fn suggest_total_upgrade_silent_with_recursion() {
        let src = concat!(
            "partial fn factorial(n: Int) -> Int {\n",
            "    if n <= 1 { 1 } else { n * factorial(n - 1) }\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        suggest_total_upgrade(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "suggest-total-upgrade"),
            "partial fn with self-recursion must not suggest total; got: {diags:?}"
        );
    }

    #[test]
    fn suggest_total_upgrade_silent_on_total_fn() {
        let src = "total fn add(a: Int, b: Int) -> Int { a + b }\n";
        let prog = parse(src);
        let mut diags = vec![];
        suggest_total_upgrade(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "suggest-total-upgrade"),
            "total fn must not get upgrade suggestion; got: {diags:?}"
        );
    }

    #[test]
    fn suggest_total_upgrade_off_when_disabled() {
        let cfg_off = LintConfig {
            suggest_total_upgrade: false,
            ..LintConfig::default()
        };
        let src = concat!(
            "partial fn f(items: List[Int]) -> Int {\n",
            "    let acc: ref Int = 0;\n",
            "    for x in items { acc = acc + x }\n",
            "    acc\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        suggest_total_upgrade(&prog, &cfg_off, &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "suggest-total-upgrade"),
            "rule must be silent when disabled; got: {diags:?}"
        );
    }
}
