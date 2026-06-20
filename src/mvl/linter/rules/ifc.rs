// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! IFC hygiene rules — audit-trail quality checks for `relabel` expressions.

use crate::mvl::linter::{config::LintConfig, errors::LintDiag};
use crate::mvl::parser::ast::{Decl, Expr, Program, Stmt};
use crate::mvl::parser::visit::{walk_block, walk_expr, walk_stmt, Visit};
use std::collections::HashMap;

/// Flag reused or boilerplate audit tags on `relabel trust` / `relabel classify` expressions.
///
/// Rule id: `relabel-tag-hygiene`
///
/// The audit tag on a `relabel` call is meant to encode *why* this specific site is safe to
/// declassify. Tags that are generic placeholders (`"TODO"`, `""`, `"XXX"`, etc.) or that are
/// reused across 3+ call sites within the same file have no audit value.
///
/// Patterns detected:
/// 1. **Boilerplate tags** — empty, `"TODO"`, `"FIXME"`, `"XXX"`, single-character, or
///    all-whitespace strings.
/// 2. **Reused tags** — the same tag literal appears at 3 or more call sites in the file.
///
/// Per-site suppression: `// allow: relabel-tag-hygiene <reason>` on the preceding line.
pub fn relabel_tag_hygiene(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.relabel_tag_hygiene {
        return;
    }

    // Collect all relabel call sites: (tag, line, col).
    let mut sites: Vec<(String, u32, u32)> = Vec::new();
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            let mut v = CollectRelabels { sites: &mut sites };
            walk_block(&mut v, &f.body);
        }
    }

    // Pass 1: flag boilerplate tags immediately.
    for (tag, line, col) in &sites {
        if is_boilerplate_tag(tag) {
            out.push(LintDiag::warning(
                "relabel-tag-hygiene",
                format!(
                    "relabel audit tag {tag:?} has no audit value — use a descriptive, unique tag such as \"XSS-001\""
                ),
                *line,
                *col,
            ));
        }
    }

    // Pass 2: flag tags that appear at 3 or more distinct call sites.
    let mut tag_locations: HashMap<&str, Vec<(u32, u32)>> = HashMap::new();
    for (tag, line, col) in &sites {
        tag_locations
            .entry(tag.as_str())
            .or_default()
            .push((*line, *col));
    }
    for (tag, locations) in &tag_locations {
        if locations.len() >= 3 && !is_boilerplate_tag(tag) {
            // Emit a diagnostic at each reuse site.
            for (line, col) in locations {
                out.push(LintDiag::warning(
                    "relabel-tag-hygiene",
                    format!(
                        "relabel audit tag {tag:?} is reused at {} sites — each site needs a unique tag",
                        locations.len()
                    ),
                    *line,
                    *col,
                ));
            }
        }
    }
}

/// True if the tag string is a boilerplate placeholder with no audit value.
fn is_boilerplate_tag(tag: &str) -> bool {
    let trimmed = tag.trim();
    if trimmed.is_empty() {
        return true;
    }
    if trimmed.chars().count() <= 1 {
        return true;
    }
    matches!(
        trimmed.to_ascii_uppercase().as_str(),
        "TODO" | "FIXME" | "XXX" | "HACK" | "TBD" | "PLACEHOLDER" | "N/A"
    )
}

struct CollectRelabels<'a> {
    sites: &'a mut Vec<(String, u32, u32)>,
}

impl<'ast> Visit<'ast> for CollectRelabels<'ast> {
    fn visit_stmt(&mut self, s: &'ast Stmt) {
        walk_stmt(self, s);
    }
    fn visit_expr(&mut self, e: &'ast Expr) {
        if let Expr::Relabel { tag, span, .. } = e {
            self.sites.push((tag.clone(), span.line, span.col));
        }
        walk_expr(self, e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::linter::config::LintConfig;
    use crate::mvl::parser::Parser;

    fn parse(src: &str) -> Program {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        prog
    }

    fn cfg() -> LintConfig {
        LintConfig::default()
    }

    #[test]
    fn boilerplate_todo_tag_detected() {
        let src = concat!(
            "fn handle(input: Tainted[String]) -> String {\n",
            "    relabel trust(input, \"TODO\")\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        relabel_tag_hygiene(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "relabel-tag-hygiene"),
            "expected relabel-tag-hygiene for TODO tag; got: {diags:?}"
        );
    }

    #[test]
    fn boilerplate_empty_tag_detected() {
        let src = concat!(
            "fn handle(input: Tainted[String]) -> String {\n",
            "    relabel trust(input, \"\")\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        relabel_tag_hygiene(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "relabel-tag-hygiene"),
            "expected relabel-tag-hygiene for empty tag; got: {diags:?}"
        );
    }

    #[test]
    fn boilerplate_fixme_tag_detected() {
        let src = concat!(
            "fn handle(input: Tainted[String]) -> String {\n",
            "    relabel trust(input, \"FIXME\")\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        relabel_tag_hygiene(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "relabel-tag-hygiene"),
            "expected relabel-tag-hygiene for FIXME tag; got: {diags:?}"
        );
    }

    #[test]
    fn descriptive_unique_tag_clean() {
        let src = concat!(
            "fn handle(input: Tainted[String]) -> String {\n",
            "    relabel trust(input, \"XSS-001\")\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        relabel_tag_hygiene(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "relabel-tag-hygiene"),
            "unique descriptive tag must not warn; got: {diags:?}"
        );
    }

    #[test]
    fn reused_tag_three_times_detected() {
        // Same tag at 3 sites — should warn at all 3
        let src = concat!(
            "fn a(input: Tainted[String]) -> String { relabel trust(input, \"AUTH-001\") }\n",
            "fn b(input: Tainted[String]) -> String { relabel trust(input, \"AUTH-001\") }\n",
            "fn c(input: Tainted[String]) -> String { relabel trust(input, \"AUTH-001\") }\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        relabel_tag_hygiene(&prog, &cfg(), &mut diags);
        let reuse_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.rule == "relabel-tag-hygiene" && d.message.contains("reused"))
            .collect();
        assert_eq!(
            reuse_diags.len(),
            3,
            "expected 3 reuse diagnostics; got: {diags:?}"
        );
    }

    #[test]
    fn reused_tag_twice_clean() {
        // Same tag at 2 sites — below the threshold of 3
        let src = concat!(
            "fn a(input: Tainted[String]) -> String { relabel trust(input, \"AUTH-002\") }\n",
            "fn b(input: Tainted[String]) -> String { relabel trust(input, \"AUTH-002\") }\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        relabel_tag_hygiene(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "relabel-tag-hygiene"),
            "tag used twice must not warn; got: {diags:?}"
        );
    }

    #[test]
    fn relabel_tag_hygiene_disabled() {
        let src = concat!(
            "fn handle(input: Tainted[String]) -> String {\n",
            "    relabel trust(input, \"TODO\")\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        let cfg_off = LintConfig {
            relabel_tag_hygiene: false,
            ..LintConfig::default()
        };
        relabel_tag_hygiene(&prog, &cfg_off, &mut diags);
        assert!(diags.is_empty(), "rule must be silent when disabled");
    }
}
