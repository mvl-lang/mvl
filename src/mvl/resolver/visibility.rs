// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Visibility checking — Spec 005 Requirement 2.
//!
//! Rules:
//! - All items are private by default.
//! - `pub` makes an item visible to importers.
//! - Struct fields are accessible when the type is in scope (no per-field gating).
//! - `pub use sub::Item` is only allowed if `Item` is already `pub` in `sub`.

use crate::mvl::parser::ast::{Decl, Program, UseDecl};
use std::collections::HashSet;

/// Result of a visibility check.
#[derive(Debug, Default)]
pub struct VisibilityResult {
    pub errors: Vec<VisibilityError>,
}

impl VisibilityResult {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Errors produced by visibility checking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisibilityError {
    /// `pub use` tries to re-export an item that is not `pub` in its source.
    ReexportOfPrivate { item: String, source_module: String },
}

impl std::fmt::Display for VisibilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VisibilityError::ReexportOfPrivate {
                item,
                source_module,
            } => {
                write!(
                    f,
                    "cannot re-export `{item}` from `{source_module}`: the item is not public"
                )
            }
        }
    }
}

/// Check visibility rules for a single program given the set of exported names
/// from each module it imports from.
///
/// `module_exports`: a map from module name to the set of exported (pub) item names.
pub fn check_visibility(
    prog: &Program,
    module_exports: &std::collections::HashMap<String, HashSet<String>>,
) -> VisibilityResult {
    let mut result = VisibilityResult::default();

    for decl in &prog.declarations {
        if let Decl::Use(ud) = decl {
            check_use_decl(ud, module_exports, &mut result);
        }
    }

    result
}

fn check_use_decl(
    ud: &UseDecl,
    module_exports: &std::collections::HashMap<String, HashSet<String>>,
    result: &mut VisibilityResult,
) {
    if !ud.reexport || ud.path.len() < 2 {
        return;
    }

    let item = ud.path.last().unwrap();
    let source_module = ud.path[..ud.path.len() - 1].join("::");

    if let Some(exports) = module_exports.get(&source_module) {
        if !exports.contains(item.as_str()) {
            result.errors.push(VisibilityError::ReexportOfPrivate {
                item: item.clone(),
                source_module,
            });
        }
    }
    // If the source module is unknown, the resolver will catch it as MissingModule.
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::Parser;
    use std::collections::HashMap;

    fn parse(src: &str) -> Program {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        prog
    }

    fn exports(pairs: &[(&str, &[&str])]) -> HashMap<String, HashSet<String>> {
        pairs
            .iter()
            .map(|(m, names)| (m.to_string(), names.iter().map(|n| n.to_string()).collect()))
            .collect()
    }

    #[test]
    fn reexport_of_public_ok() {
        // `pub use mod_a::Foo` where Foo is pub in mod_a — should pass
        // We can't easily parse `pub use` yet in the test, so we construct directly.
        use crate::mvl::parser::ast::UseDecl;
        use crate::mvl::parser::lexer::Span;

        let ud = UseDecl {
            reexport: true,
            module_only: false,
            path: vec!["mod_a".to_string(), "Foo".to_string()],
            items: vec![],
            span: Span::default(),
        };
        let prog = Program {
            declarations: vec![Decl::Use(ud)],
            span: Span::default(),
        };
        let module_exports = exports(&[("mod_a", &["Foo"])]);
        let result = check_visibility(&prog, &module_exports);
        assert!(
            result.is_ok(),
            "re-export of pub item should pass: {:?}",
            result.errors
        );
    }

    #[test]
    fn reexport_of_private_rejected() {
        use crate::mvl::parser::ast::UseDecl;
        use crate::mvl::parser::lexer::Span;

        let ud = UseDecl {
            reexport: true,
            module_only: false,
            path: vec!["mod_a".to_string(), "secret".to_string()],
            items: vec![],
            span: Span::default(),
        };
        let prog = Program {
            declarations: vec![Decl::Use(ud)],
            span: Span::default(),
        };
        // mod_a does NOT export `secret`
        let module_exports = exports(&[("mod_a", &["Foo"])]);
        let result = check_visibility(&prog, &module_exports);
        assert!(
            !result.is_ok(),
            "re-export of private item must be rejected"
        );
        assert!(matches!(
            &result.errors[0],
            VisibilityError::ReexportOfPrivate { item, .. } if item == "secret"
        ));
    }

    #[test]
    fn struct_fields_accessible_when_type_in_scope() {
        // Struct fields have no per-field gating; this is enforced at the
        // type level (if you have a value of a struct type, you can access its fields).
        // This is tested at the checker level, not the visibility level.
        // This test simply confirms the visibility checker doesn't reject normal decls.
        let prog = parse("pub type Point = struct { x: Int, y: Int }");
        let module_exports = exports(&[]);
        let result = check_visibility(&prog, &module_exports);
        assert!(result.is_ok());
    }
}
