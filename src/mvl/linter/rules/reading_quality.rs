// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Phase 3 reading quality rules — comment style and documentation.

use crate::mvl::linter::{config::LintConfig, errors::LintDiag};
use crate::mvl::parser::ast::{Decl, Program};

// ── Phase 3: LLM corpus quality rules ──────────────────────────────────────

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

// ── Phase 3 helpers ─────────────────────────────────────────────────────────

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
