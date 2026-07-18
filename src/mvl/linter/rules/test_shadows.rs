// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Rule `test-shadow` — flag type redeclarations and fn shadows in `*_test.mvl` files.
//!
//! Pattern 006 (`.openspec/patterns/006-no-test-shadows.md`) prohibits:
//! * Any `type` declaration in a test file (always a shadow).
//! * Any `fn`/`total fn`/`partial fn` declaration in a test file whose name
//!   collides with a `pub` fn in a sibling production `.mvl` file.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::mvl::linter::{config::LintConfig, errors::LintDiag};
use crate::mvl::parser::ast::{Decl, Program};

/// Check for shadow declarations in test files (rule id: `test-shadow`).
pub fn test_shadow(prog: &Program, cfg: &LintConfig, path: &Path, out: &mut Vec<LintDiag>) {
    if !cfg.test_shadow {
        return;
    }
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if !filename.ends_with("_test.mvl") {
        return;
    }

    let sibling_pub_fns = collect_sibling_pub_fns(path);

    for decl in &prog.declarations {
        match decl {
            Decl::Type(t) => {
                out.push(LintDiag::warning(
                    "test-shadow",
                    format!(
                        "type `{}` declared in a test file — move to production code and import it",
                        t.name
                    ),
                    t.span.line,
                    t.span.col,
                ));
            }
            Decl::Fn(f) if !f.is_test && sibling_pub_fns.contains(&f.name) => {
                out.push(LintDiag::warning(
                    "test-shadow",
                    format!(
                        "fn `{}` shadows a `pub` fn in a sibling production file — \
                         import the production fn instead",
                        f.name
                    ),
                    f.span.line,
                    f.span.col,
                ));
            }
            _ => {}
        }
    }
}

/// Scan sibling production `.mvl` files and collect `pub fn` names.
///
/// Uses a lightweight line scan (no full parse) matching the same patterns
/// as `tools/audit_test_shadows.py`. Only `pub` items are collected — private
/// helpers colliding by name are not an API-drift risk.
fn collect_sibling_pub_fns(test_path: &Path) -> HashSet<String> {
    let mut names = HashSet::new();
    let parent = match test_path.parent() {
        Some(p) => p,
        None => return names,
    };
    let entries = match fs::read_dir(parent) {
        Ok(e) => e,
        Err(_) => return names,
    };
    for entry in entries.flatten() {
        let sibling = entry.path();
        let name = sibling
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name.ends_with("_test.mvl") || name.ends_with("_smoke.mvl") || !name.ends_with(".mvl")
        {
            continue;
        }
        if let Ok(src) = fs::read_to_string(&sibling) {
            for line in src.lines() {
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") {
                    continue;
                }
                if let Some(fn_name) = extract_pub_fn_name(trimmed) {
                    names.insert(fn_name);
                }
            }
        }
    }
    names
}

/// Extract the fn name from a `pub [total|partial] fn <name>` line.
///
/// Returns `None` for non-pub fns, `test fn`, comments, and anything else.
fn extract_pub_fn_name(line: &str) -> Option<String> {
    // Must start with `pub`
    let rest = line.strip_prefix("pub")?;
    let rest = rest.trim_start();
    // Skip `test fn` — test helpers are not production API
    if rest.starts_with("test") {
        return None;
    }
    // Consume optional `total` or `partial`
    let rest = if let Some(r) = rest.strip_prefix("total") {
        r.trim_start()
    } else if let Some(r) = rest.strip_prefix("partial") {
        r.trim_start()
    } else {
        rest
    };
    let rest = rest.strip_prefix("fn")?.trim_start();
    // Extract identifier (snake_case fn name)
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_pub_fn_plain() {
        assert_eq!(
            extract_pub_fn_name("pub fn do_thing("),
            Some("do_thing".into())
        );
    }

    #[test]
    fn extract_pub_fn_total() {
        assert_eq!(
            extract_pub_fn_name("pub total fn factorial("),
            Some("factorial".into())
        );
    }

    #[test]
    fn extract_pub_fn_partial() {
        assert_eq!(
            extract_pub_fn_name("pub partial fn accept_loop("),
            Some("accept_loop".into())
        );
    }

    #[test]
    fn extract_pub_fn_ignores_private() {
        assert_eq!(extract_pub_fn_name("fn helper("), None);
    }

    #[test]
    fn extract_pub_fn_ignores_test_fn() {
        assert_eq!(extract_pub_fn_name("pub test fn check_it("), None);
        assert_eq!(extract_pub_fn_name("test fn check_it("), None);
    }

    #[test]
    fn extract_pub_fn_ignores_comment() {
        assert_eq!(extract_pub_fn_name("// pub fn not_a_fn("), None);
    }

    #[test]
    fn extract_pub_fn_empty_name() {
        assert_eq!(extract_pub_fn_name("pub fn ("), None);
    }
}
