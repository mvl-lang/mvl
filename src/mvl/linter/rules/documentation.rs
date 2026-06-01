// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Documentation quality rules — check comment style and doc-comment coverage.

use crate::mvl::linter::{config::LintConfig, errors::LintDiag};
use crate::mvl::parser::ast::{Decl, Program};

/// Flag block-comment syntax (`/*`).
///
/// Rule id: `consistent-comment-style`
///
/// MVL allows only `//` line comments (and `///` doc comments). Block comments
/// from other languages are not part of the grammar; this rule catches them in
/// raw source so the lexer does not need to be extended.
pub fn consistent_comment_style(src: &str, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.consistent_comment_style {
        return;
    }
    for (i, line) in src.lines().enumerate() {
        let line_no = (i + 1) as u32;
        // Scan for `/*` not inside a line comment. If `//` appears before `/*`
        // on the same line, the `/*` is inside a comment and must be ignored.
        // Known limitation: `/*` inside string literals is still flagged; the
        // parser rejects such source anyway, so false positives are rare.
        if let Some(pos) = line.find("/*") {
            let in_line_comment = line.find("//").is_some_and(|cc| cc < pos);
            if !in_line_comment {
                out.push(LintDiag::warning(
                    "consistent-comment-style",
                    "block comment `/*` not allowed; use `//` line comments",
                    line_no,
                    (pos + 1) as u32,
                ));
            }
        }
    }
}

/// Require `///` doc comments on every public function and type declaration.
///
/// Rule id: `missing-doc-comment`
///
/// Because the lexer discards comments, this rule correlates AST span line
/// numbers with raw source text: a declaration is considered documented if one
/// or more `///` lines appear immediately above it (blank lines between the
/// comment and the declaration are allowed; a non-comment, non-blank line
/// breaks the block).
pub fn doc_comments_required(prog: &Program, src: &str, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.require_doc_comments {
        return;
    }
    let src_lines: Vec<&str> = src.lines().collect();
    for decl in &prog.declarations {
        match decl {
            Decl::Fn(f)
                if f.visible && !has_doc_comment_before(f.span.line as usize, &src_lines) =>
            {
                out.push(LintDiag::warning(
                    "missing-doc-comment",
                    format!(
                        "public function `{}` is missing a doc comment (`///`)",
                        f.name
                    ),
                    f.span.line,
                    f.span.col,
                ));
            }
            Decl::Type(t)
                if t.visible && !has_doc_comment_before(t.span.line as usize, &src_lines) =>
            {
                out.push(LintDiag::warning(
                    "missing-doc-comment",
                    format!("public type `{}` is missing a doc comment (`///`)", t.name),
                    t.span.line,
                    t.span.col,
                ));
            }
            Decl::Const(c)
                if c.visible && !has_doc_comment_before(c.span.line as usize, &src_lines) =>
            {
                out.push(LintDiag::warning(
                    "missing-doc-comment",
                    format!("public const `{}` is missing a doc comment (`///`)", c.name),
                    c.span.line,
                    c.span.col,
                ));
            }
            _ => {}
        }
    }
}

/// Recommend an `Example:` section inside doc-comment blocks on public items.
///
/// Rule id: `doc-comment-example`
///
/// This rule is opt-in (`doc_comment_examples = false` by default). When
/// enabled it emits a warning for every public function or type whose doc
/// comment block does not contain an `Example:` or `# Example` line.
pub fn doc_comment_examples(prog: &Program, src: &str, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.doc_comment_examples {
        return;
    }
    let src_lines: Vec<&str> = src.lines().collect();
    for decl in &prog.declarations {
        // Only pub fn and pub type are checked; pub const is intentionally
        // excluded (example sections are less meaningful for constants).
        let Some((span, kind, name)) = (match decl {
            Decl::Fn(f) if f.visible => Some((f.span, "function", f.name.as_str())),
            Decl::Type(t) if t.visible => Some((t.span, "type", t.name.as_str())),
            _ => None,
        }) else {
            continue;
        };
        let doc_lines = collect_doc_lines_before(span.line as usize, &src_lines);
        if doc_lines.is_empty() {
            // missing-doc-comment will fire; skip duplicate noise here
            continue;
        }
        let has_example = doc_lines.iter().any(|l| {
            let lower = l.trim_start_matches('/').trim().to_ascii_lowercase();
            lower.starts_with("example") || lower.starts_with("# example")
        });
        if !has_example {
            out.push(LintDiag::warning(
                "doc-comment-example",
                format!(
                    "public {kind} `{name}` doc comment has no `Example:` section (recommended)"
                ),
                span.line,
                span.col,
            ));
        }
    }
}

/// Returns `true` if the source line immediately preceding `decl_line`
/// (1-based) belongs to a `///` doc-comment block.
///
/// Blank lines between the comment block and the declaration are skipped.
/// A regular `//` comment (not `///`) does **not** count as documentation.
fn has_doc_comment_before(decl_line: usize, src_lines: &[&str]) -> bool {
    !collect_doc_lines_before(decl_line, src_lines).is_empty()
}

/// Collect all `///` lines from the comment block immediately above
/// `decl_line` (1-based). Returns an empty vec if none are found.
fn collect_doc_lines_before<'a>(decl_line: usize, src_lines: &[&'a str]) -> Vec<&'a str> {
    if decl_line == 0 || decl_line > src_lines.len() {
        return vec![];
    }
    // Walk backwards from the line immediately above the declaration.
    // decl_line is 1-based, so the line above is at 0-based index decl_line - 2,
    // meaning we iterate over 0..decl_line-1 in reverse.
    let mut result: Vec<&'a str> = vec![];
    for i in (0..decl_line.saturating_sub(1)).rev() {
        let line = src_lines[i].trim();
        if line.starts_with("///") {
            result.push(src_lines[i]);
        } else if line.is_empty() {
            // blank lines between doc block and declaration are allowed
            continue;
        } else {
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::linter::config::LintConfig;
    use crate::mvl::parser::Parser;

    fn cfg() -> LintConfig {
        let mut c = LintConfig::default();
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

    // ── consistent_comment_style ──────────────────────────────────────────

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

    // ── doc_comments_required ─────────────────────────────────────────────

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

    // ── doc_comment_examples ──────────────────────────────────────────────

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
}
