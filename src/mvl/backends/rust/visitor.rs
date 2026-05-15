// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Visitor-based emission — decorator pattern for instrumentation (spec 018).
//!
//! [`EmitVisitor`] is the injection-point interface: one method per AST node
//! that needs instrumentation (if/match branches, binary ops for mutation, etc.).
//!
//! [`BaseEmitter`] is the instrumentation-free base implementation.
//!
//! [`CoverageVisitor`] is a decorator that wraps any [`EmitVisitor`] and injects
//! `__mvl_cov::hit(N)` calls into branch bodies.
//!
//! # Composition
//!
//! ```rust
//! use mvl::mvl::backends::rust::visitor::{BaseEmitter, CoverageVisitor, EmitVisitor};
//!
//! let mut v = CoverageVisitor::new(BaseEmitter, 0);
//! let code = v.visit_if("x > 0", "{ return 1 }", Some("{ return 0 }"));
//! assert!(code.contains("__mvl_cov::hit(0)"));
//! assert_eq!(v.next_counter_id(), 2);
//! ```
//!
//! # Migration plan
//!
//! The `emit_stmts.rs` and `emit_exprs.rs` inline instrumentation calls
//! (`cg.alloc_branch`, `cg.emit_cov_hit`, `cg.alloc_mcdc_decision`) will be
//! replaced phase-by-phase with `EmitVisitor` dispatch as described in spec 018.

/// Visitor interface for the emission injection points.
///
/// Each method receives pre-emitted string fragments (code already generated
/// for sub-nodes) and returns the assembled Rust source for the full node.
/// This matches the current push-based emitter architecture: sub-node code is
/// generated first, then assembled by the parent node handler.
///
/// Decorators implement this trait by forwarding to an inner `V: EmitVisitor`
/// and wrapping the fragments with instrumentation calls before delegating.
pub trait EmitVisitor {
    /// Emit an `if` expression/statement.
    ///
    /// - `cond`  — already-emitted condition code
    /// - `then`  — already-emitted then-branch body (without braces)
    /// - `else_` — already-emitted else-branch body, if present (without braces)
    fn visit_if(&mut self, cond: &str, then: &str, else_: Option<&str>) -> String;

    /// Emit a single `match` arm body.
    ///
    /// - `pattern`  — already-emitted pattern code
    /// - `body`     — already-emitted arm body
    /// - `arm_idx`  — zero-based arm index (used for coverage counter allocation)
    fn visit_match_arm(&mut self, pattern: &str, body: &str, arm_idx: usize) -> String;

    /// Emit a `for` loop body.
    ///
    /// - `binding`  — loop variable name
    /// - `iter`     — already-emitted iterator expression
    /// - `body`     — already-emitted loop body (without braces)
    fn visit_for(&mut self, binding: &str, iter: &str, body: &str) -> String;

    /// Emit a `while` loop body.
    ///
    /// - `cond` — already-emitted condition code
    /// - `body` — already-emitted loop body (without braces)
    fn visit_while(&mut self, cond: &str, body: &str) -> String;
}

// ── BaseEmitter ───────────────────────────────────────────────────────────────

/// Instrumentation-free base emitter.
///
/// Produces clean Rust source with no coverage/MC/DC/mutation calls injected.
/// Used as the innermost layer when no instrumentation is needed, and as the
/// inner `V` for decorator composition.
pub struct BaseEmitter;

impl EmitVisitor for BaseEmitter {
    fn visit_if(&mut self, cond: &str, then: &str, else_: Option<&str>) -> String {
        let else_part = else_
            .map(|e| format!(" else {{ {e} }}"))
            .unwrap_or_default();
        format!("if {cond} {{ {then} }}{else_part}")
    }

    fn visit_match_arm(&mut self, pattern: &str, body: &str, _arm_idx: usize) -> String {
        format!("{pattern} => {{ {body} }}")
    }

    fn visit_for(&mut self, binding: &str, iter: &str, body: &str) -> String {
        format!("for {binding} in {iter} {{ {body} }}")
    }

    fn visit_while(&mut self, cond: &str, body: &str) -> String {
        format!("while {cond} {{ {body} }}")
    }
}

// ── CoverageVisitor ───────────────────────────────────────────────────────────

/// Coverage instrumentation decorator.
///
/// Wraps any `V: EmitVisitor` and injects `#[cfg(test)] crate::__mvl_cov::hit(N)`
/// calls at the entry of each branch body.
///
/// Counters are allocated sequentially from `start_id` upward.
/// Call [`next_counter_id`](CoverageVisitor::next_counter_id) after emission to get
/// the next available counter ID (for the cov preamble and multi-file offset).
pub struct CoverageVisitor<V: EmitVisitor> {
    inner: V,
    next_id: usize,
}

impl<V: EmitVisitor> CoverageVisitor<V> {
    /// Create a new coverage visitor wrapping `inner`, starting counter IDs at `start_id`.
    pub fn new(inner: V, start_id: usize) -> Self {
        Self {
            inner,
            next_id: start_id,
        }
    }

    /// Return the next counter ID to be allocated.
    ///
    /// Equals `start_id + number_of_branches_instrumented`. Pass this as the
    /// `start_id` for the next file's visitor in multi-file coverage runs.
    pub fn next_counter_id(&self) -> usize {
        self.next_id
    }

    fn alloc(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn hit_call(id: usize) -> String {
        format!("#[cfg(test)] crate::__mvl_cov::hit({id}); ")
    }
}

impl<V: EmitVisitor> EmitVisitor for CoverageVisitor<V> {
    fn visit_if(&mut self, cond: &str, then: &str, else_: Option<&str>) -> String {
        let true_id = self.alloc();
        let then_instrumented = format!("{}{}", Self::hit_call(true_id), then);

        let else_instrumented = else_.map(|e| {
            let false_id = self.alloc();
            format!("{}{}", Self::hit_call(false_id), e)
        });

        self.inner
            .visit_if(cond, &then_instrumented, else_instrumented.as_deref())
    }

    fn visit_match_arm(&mut self, pattern: &str, body: &str, arm_idx: usize) -> String {
        let id = self.alloc();
        let instrumented = format!("{}{}", Self::hit_call(id), body);
        self.inner.visit_match_arm(pattern, &instrumented, arm_idx)
    }

    fn visit_for(&mut self, binding: &str, iter: &str, body: &str) -> String {
        let id = self.alloc();
        let instrumented = format!("{}{}", Self::hit_call(id), body);
        self.inner.visit_for(binding, iter, &instrumented)
    }

    fn visit_while(&mut self, cond: &str, body: &str) -> String {
        let id = self.alloc();
        let instrumented = format!("{}{}", Self::hit_call(id), body);
        self.inner.visit_while(cond, &instrumented)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── BaseEmitter ───────────────────────────────────────────────────────

    #[test]
    fn base_if_no_else() {
        let mut e = BaseEmitter;
        let out = e.visit_if("x > 0", "return 1", None);
        assert_eq!(out, "if x > 0 { return 1 }");
    }

    #[test]
    fn base_if_with_else() {
        let mut e = BaseEmitter;
        let out = e.visit_if("x > 0", "return 1", Some("return 0"));
        assert_eq!(out, "if x > 0 { return 1 } else { return 0 }");
    }

    #[test]
    fn base_match_arm() {
        let mut e = BaseEmitter;
        let out = e.visit_match_arm("Some(x)", "x + 1", 0);
        assert_eq!(out, "Some(x) => { x + 1 }");
    }

    #[test]
    fn base_for_loop() {
        let mut e = BaseEmitter;
        let out = e.visit_for("item", "items.iter()", "process(item)");
        assert_eq!(out, "for item in items.iter() { process(item) }");
    }

    #[test]
    fn base_while_loop() {
        let mut e = BaseEmitter;
        let out = e.visit_while("!done", "step()");
        assert_eq!(out, "while !done { step() }");
    }

    // ── CoverageVisitor ───────────────────────────────────────────────────

    #[test]
    fn coverage_if_injects_true_branch_hit() {
        let mut v = CoverageVisitor::new(BaseEmitter, 0);
        let out = v.visit_if("x > 0", "return 1", None);
        assert!(
            out.contains("__mvl_cov::hit(0)"),
            "expected hit(0) in true branch: {out}"
        );
        assert_eq!(v.next_counter_id(), 1);
    }

    #[test]
    fn coverage_if_with_else_injects_both_branches() {
        let mut v = CoverageVisitor::new(BaseEmitter, 0);
        let out = v.visit_if("x > 0", "return 1", Some("return 0"));
        assert!(out.contains("__mvl_cov::hit(0)"), "missing true branch hit");
        assert!(
            out.contains("__mvl_cov::hit(1)"),
            "missing false branch hit"
        );
        assert_eq!(v.next_counter_id(), 2);
    }

    #[test]
    fn coverage_if_start_id_offset() {
        let mut v = CoverageVisitor::new(BaseEmitter, 10);
        let out = v.visit_if("flag", "a()", Some("b()"));
        assert!(out.contains("__mvl_cov::hit(10)"));
        assert!(out.contains("__mvl_cov::hit(11)"));
        assert_eq!(v.next_counter_id(), 12); // 10 start + 2 allocated
    }

    #[test]
    fn coverage_match_arm_injects_hit() {
        let mut v = CoverageVisitor::new(BaseEmitter, 5);
        let out = v.visit_match_arm("Ok(x)", "x", 0);
        assert!(out.contains("__mvl_cov::hit(5)"));
        assert_eq!(v.next_counter_id(), 6);
    }

    #[test]
    fn coverage_for_injects_body_hit() {
        let mut v = CoverageVisitor::new(BaseEmitter, 0);
        let out = v.visit_for("i", "0..10", "sum += i");
        assert!(out.contains("__mvl_cov::hit(0)"));
        assert_eq!(v.next_counter_id(), 1);
    }

    #[test]
    fn coverage_while_injects_body_hit() {
        let mut v = CoverageVisitor::new(BaseEmitter, 3);
        let out = v.visit_while("!done", "tick()");
        assert!(out.contains("__mvl_cov::hit(3)"));
        assert_eq!(v.next_counter_id(), 4);
    }

    #[test]
    fn coverage_counters_increment_across_calls() {
        let mut v = CoverageVisitor::new(BaseEmitter, 0);
        v.visit_if("a", "x", None);
        v.visit_match_arm("B", "y", 0);
        v.visit_if("c", "z", Some("w"));
        assert_eq!(v.next_counter_id(), 4); // 1 + 1 + 2
    }

    #[test]
    fn coverage_preserves_base_structure() {
        let mut v = CoverageVisitor::new(BaseEmitter, 0);
        let out = v.visit_if("x > 0", "return 1", Some("return 0"));
        assert!(
            out.starts_with("if x > 0 {"),
            "structure not preserved: {out}"
        );
        assert!(out.contains("else {"), "else block missing: {out}");
    }

    #[test]
    fn coverage_cfg_test_gated() {
        let mut v = CoverageVisitor::new(BaseEmitter, 0);
        let out = v.visit_if("x", "a", None);
        assert!(
            out.contains("#[cfg(test)]"),
            "hit call must be cfg(test) gated: {out}"
        );
    }
}
