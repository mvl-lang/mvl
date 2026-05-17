// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Effect hierarchy — builds and queries the transitive subsumption graph (#853).
//!
//! `EffectHierarchy` is constructed from all `EffectDecl` nodes collected across
//! every parsed module (std + user). It validates that all parent names exist and
//! detects cycles. The type checker uses `subsumes_transitive` to replace the
//! hardcoded `VALID_EFFECT_NAMES` list.

use std::collections::{HashMap, HashSet};

use crate::mvl::checker::errors::CheckError;
use crate::mvl::parser::ast::EffectDecl;

/// Resolved effect subsumption hierarchy.
///
/// `effects` is the set of all declared effect names.
/// `parents` maps each effect to its direct parents (effects it subsumes).
#[derive(Debug, Default, Clone)]
pub struct EffectHierarchy {
    effects: HashSet<String>,
    parents: HashMap<String, Vec<String>>,
}

impl EffectHierarchy {
    /// Build the hierarchy from a flat list of `EffectDecl` nodes.
    ///
    /// Validates: all parent names must be declared. Detects cycles.
    /// Returns errors for unknown parents and cycles; the hierarchy is still
    /// partially populated so the caller can continue and report all errors.
    pub fn from_decls(decls: &[&EffectDecl]) -> (Self, Vec<CheckError>) {
        let mut hierarchy = EffectHierarchy::default();
        let mut errors = Vec::new();

        // Pass 1: register all effect names.
        for decl in decls {
            hierarchy.effects.insert(decl.name.clone());
        }

        // Pass 2: register parent edges, validate parents exist.
        for decl in decls {
            let mut valid_parents = Vec::new();
            for parent in &decl.subsumes {
                if hierarchy.effects.contains(parent) {
                    valid_parents.push(parent.clone());
                } else {
                    errors.push(CheckError::UnknownEffectParent {
                        effect: decl.name.clone(),
                        parent: parent.clone(),
                        span: decl.span,
                    });
                }
            }
            hierarchy.parents.insert(decl.name.clone(), valid_parents);
        }

        // Pass 3: detect cycles (DFS from each node).
        let all_names: Vec<String> = hierarchy.effects.iter().cloned().collect();
        for name in &all_names {
            if let Some(chain) = hierarchy.find_cycle(name) {
                // Only report cycles that start at `name` to avoid duplicates.
                // (The cycle is always reported at the first node alphabetically
                //  in the chain to make error messages deterministic.)
                // Deduplicate: only report the cycle when `name` is the
                // lexicographically smallest node in the cycle chain, so each
                // cycle is reported exactly once regardless of traversal order.
                let min = chain.iter().min().expect("cycle chain is always non-empty");
                if min == name {
                    // Use the span of the effect that starts the cycle.
                    let span = decls
                        .iter()
                        .find(|d| &d.name == name)
                        .map(|d| d.span)
                        .expect("cycle node must have a declaration");
                    errors.push(CheckError::EffectCycle { chain, span });
                }
            }
        }

        (hierarchy, errors)
    }

    /// Returns `true` if `declared` (transitively) subsumes `required`.
    ///
    /// `IO > Log > Clock` means `subsumes_transitive("IO", "Clock")` is true.
    pub fn subsumes_transitive(&self, declared: &str, required: &str) -> bool {
        if declared == required {
            return true;
        }
        let mut visited = HashSet::new();
        self.can_reach(declared, required, &mut visited)
    }

    /// Returns `true` if `declared` effect is known to the hierarchy.
    pub fn contains(&self, name: &str) -> bool {
        self.effects.contains(name)
    }

    /// Returns `true` if the hierarchy has at least one declared effect.
    /// Used to skip validation when no effects.mvl was loaded (e.g. unit tests).
    pub fn has_any(&self) -> bool {
        !self.effects.is_empty()
    }

    /// `visited` memoises already-expanded nodes (avoids re-traversal on DAGs)
    /// and doubles as a cycle-break guard for hierarchies built via `Default`
    /// rather than `from_decls` (which would normally reject cycles).
    fn can_reach(&self, current: &str, target: &str, visited: &mut HashSet<String>) -> bool {
        if !visited.insert(current.to_string()) {
            return false; // already expanded or cycle guard
        }
        if let Some(parents) = self.parents.get(current) {
            for parent in parents {
                if parent == target {
                    return true;
                }
                if self.can_reach(parent, target, visited) {
                    return true;
                }
            }
        }
        false
    }

    /// DFS cycle detection. Returns the cycle chain if a cycle is found from `start`.
    fn find_cycle(&self, start: &str) -> Option<Vec<String>> {
        let mut path = Vec::new();
        let mut on_stack = HashSet::new();
        if self.dfs_cycle(start, &mut path, &mut on_stack) {
            Some(path)
        } else {
            None
        }
    }

    fn dfs_cycle(
        &self,
        node: &str,
        path: &mut Vec<String>,
        on_stack: &mut HashSet<String>,
    ) -> bool {
        path.push(node.to_string());
        on_stack.insert(node.to_string());

        if let Some(parents) = self.parents.get(node) {
            for parent in parents {
                if on_stack.contains(parent.as_str()) {
                    // Cycle detected — trim path to start at the cycle entry
                    // point so the chain contains only cycle members.
                    if let Some(idx) = path.iter().position(|n| n == parent) {
                        path.drain(..idx);
                    }
                    path.push(parent.clone());
                    return true;
                }
                if self.dfs_cycle(parent, path, on_stack) {
                    return true;
                }
            }
        }

        path.pop();
        on_stack.remove(node);
        false
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::lexer::Span;

    fn span() -> Span {
        Span::default() // zero-filled span for tests
    }

    fn decl(name: &str, subsumes: &[&str]) -> EffectDecl {
        EffectDecl {
            name: name.to_string(),
            subsumes: subsumes.iter().map(|s| s.to_string()).collect(),
            span: span(),
        }
    }

    #[test]
    fn base_effects_no_errors() {
        let decls = vec![decl("Clock", &[]), decl("Console", &[])];
        let refs: Vec<&EffectDecl> = decls.iter().collect();
        let (h, errors) = EffectHierarchy::from_decls(&refs);
        assert!(errors.is_empty());
        assert!(h.contains("Clock"));
        assert!(h.contains("Console"));
    }

    #[test]
    fn single_subsumption() {
        // Log > Clock: Log subsumes Clock
        let decls = vec![decl("Clock", &[]), decl("Log", &["Clock"])];
        let refs: Vec<&EffectDecl> = decls.iter().collect();
        let (h, errors) = EffectHierarchy::from_decls(&refs);
        assert!(errors.is_empty());
        assert!(h.subsumes_transitive("Log", "Clock"));
        assert!(!h.subsumes_transitive("Clock", "Log"));
    }

    #[test]
    fn transitive_subsumption() {
        // IO > Log > Clock: IO transitively subsumes Clock
        let decls = vec![
            decl("Clock", &[]),
            decl("Log", &["Clock"]),
            decl("IO", &["Log"]),
        ];
        let refs: Vec<&EffectDecl> = decls.iter().collect();
        let (h, errors) = EffectHierarchy::from_decls(&refs);
        assert!(errors.is_empty());
        assert!(h.subsumes_transitive("IO", "Clock"));
        assert!(h.subsumes_transitive("IO", "Log"));
        assert!(!h.subsumes_transitive("Clock", "IO"));
    }

    #[test]
    fn multiple_parents() {
        // IO > Console + FileRead: IO subsumes both
        let decls = vec![
            decl("Console", &[]),
            decl("FileRead", &[]),
            decl("IO", &["Console", "FileRead"]),
        ];
        let refs: Vec<&EffectDecl> = decls.iter().collect();
        let (h, errors) = EffectHierarchy::from_decls(&refs);
        assert!(errors.is_empty());
        assert!(h.subsumes_transitive("IO", "Console"));
        assert!(h.subsumes_transitive("IO", "FileRead"));
    }

    #[test]
    fn unknown_parent_error() {
        let decls = vec![decl("Billing", &["DB"])]; // DB not declared
        let refs: Vec<&EffectDecl> = decls.iter().collect();
        let (_, errors) = EffectHierarchy::from_decls(&refs);
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            CheckError::UnknownEffectParent { effect, parent, .. }
            if effect == "Billing" && parent == "DB"
        ));
    }

    #[test]
    fn cycle_detection() {
        // A > B, B > A — cycle
        let decls = vec![decl("A", &["B"]), decl("B", &["A"])];
        let refs: Vec<&EffectDecl> = decls.iter().collect();
        let (_, errors) = EffectHierarchy::from_decls(&refs);
        assert!(!errors.is_empty());
        assert!(errors
            .iter()
            .any(|e| matches!(e, CheckError::EffectCycle { .. })));
    }

    #[test]
    fn three_node_cycle_detected() {
        // A > B, B > C, C > A — three-node cycle
        let decls = vec![decl("A", &["B"]), decl("B", &["C"]), decl("C", &["A"])];
        let refs: Vec<&EffectDecl> = decls.iter().collect();
        let (_, errors) = EffectHierarchy::from_decls(&refs);
        assert!(!errors.is_empty(), "three-node cycle must be detected");
        let cycle_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, CheckError::EffectCycle { .. }))
            .collect();
        assert_eq!(
            cycle_errors.len(),
            1,
            "cycle should be reported exactly once"
        );
        if let CheckError::EffectCycle { chain, .. } = &cycle_errors[0] {
            assert!(
                chain.len() >= 3,
                "chain should contain the full cycle: {chain:?}"
            );
        }
    }

    #[test]
    fn duplicate_parent_names_accepted() {
        // effect A > B + B — duplicate parent is harmless (HashMap deduplicates names)
        let decls = vec![decl("B", &[]), decl("A", &["B", "B"])];
        let refs: Vec<&EffectDecl> = decls.iter().collect();
        let (h, errors) = EffectHierarchy::from_decls(&refs);
        assert!(
            errors.is_empty(),
            "duplicate parent should not error: {errors:?}"
        );
        assert!(h.subsumes_transitive("A", "B"));
    }

    #[test]
    fn duplicate_effect_declaration_last_wins() {
        // Two EffectDecl nodes with the same name — last definition's parents win.
        let decls = vec![
            decl("Parent", &[]),
            decl("Other", &[]),
            decl("IO", &["Parent"]),
            decl("IO", &["Other"]), // overwrites first IO definition
        ];
        let refs: Vec<&EffectDecl> = decls.iter().collect();
        let (h, errors) = EffectHierarchy::from_decls(&refs);
        assert!(
            errors.is_empty(),
            "duplicate decl should not error: {errors:?}"
        );
        // Last definition wins: IO > Other, not IO > Parent.
        assert!(h.subsumes_transitive("IO", "Other"));
        assert!(
            !h.subsumes_transitive("IO", "Parent"),
            "first decl was overwritten"
        );
    }

    #[test]
    fn forward_reference_ok() {
        // Billing declared before DB and Log — both should resolve
        let decls = vec![
            decl("Billing", &["DB", "Log"]),
            decl("DB", &[]),
            decl("Log", &[]),
        ];
        let refs: Vec<&EffectDecl> = decls.iter().collect();
        let (h, errors) = EffectHierarchy::from_decls(&refs);
        assert!(errors.is_empty(), "forward ref should be ok: {errors:?}");
        assert!(h.subsumes_transitive("Billing", "DB"));
        assert!(h.subsumes_transitive("Billing", "Log"));
    }
}
