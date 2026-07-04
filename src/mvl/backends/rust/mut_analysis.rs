// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Read-only binding analysis (`unused_mut` suppression).
//!
//! Walks a function body and returns the set of `Pattern::Ident` spans that
//! belong to `let` bindings that are only *read* — never assigned to and
//! never used as the receiver of a method call.  Combined with the
//! `Ty::Ref(true, _)` marker in [`emit_stmts`], the Rust backend can emit
//! `let name` instead of `let mut name` for MVL `ref` bindings that ended up
//! being read-only, which eliminates the Rust `unused_mut` warning on the
//! generated code.
//!
//! # Scoping
//!
//! The analysis maintains a stack of scopes so shadowed bindings are treated
//! independently.  Consider:
//!
//! ```text
//! match tok {
//!     A => { let curr: ref P = f(x); use(curr) }         // read-only
//!     B => { let curr: ref P = f(x); curr = g(curr) }    // mutated
//! }
//! ```
//!
//! A name-based analysis would mark `curr` as mutated fn-wide because of the
//! `B` arm, leaving the `A` arm with a spurious `let mut`.  This
//! implementation tracks bindings by their pattern span and resolves each
//! `Assign` / receiver use to the innermost lexically-enclosing binding.
//!
//! # Why also block on method calls?
//!
//! MVL `x.push(v)` on `x: ref List[T]` lowers to `x.push(v)` in Rust where
//! `push` takes `&mut self`.  Rust auto-borrows `x` as mutable, and refuses
//! to compile if the binding isn't `mut`.  Because the transpiler doesn't
//! track receiver mutability per-method here, any `MethodCall` on a name is
//! treated conservatively as a potential write — the binding stays `mut`.
//! Under-approximating the read-only set is always sound: we keep more
//! `mut`s than strictly necessary, but never drop one that's needed.
//!
//! # Not walked
//!
//! - Lambda bodies: captures are opaque and the closure may mutate later.
//!   Every binding referenced *inside* a lambda is conservatively marked
//!   non-read-only.

use std::collections::{HashMap, HashSet};

use crate::mvl::ir::{
    LValue, Pattern, RefExpr, TirBlock, TirElseBranch, TirExpr, TirExprKind, TirMatchBody,
    TirParam, TirStmt,
};
use crate::mvl::parser::lexer::Span;

/// Return the set of pattern spans belonging to `let` bindings that are
/// only read within `body`.
///
/// [`emit_stmts`] emits `let name = ...` (no `mut`) when the `Pattern::Ident`
/// span of a `Ty::Ref(true, _)` binding appears in this set.
pub fn compute_readonly_names(body: &TirBlock) -> HashSet<Span> {
    let mut tracker = MutTracker::default();
    tracker.visit_block(body);
    tracker
        .bindings
        .into_iter()
        .filter(|b| !b.mutated)
        .map(|b| b.span)
        .collect()
}

/// Return the set of parameter names that are only *read* within `body` —
/// never assigned, never used as a method-call receiver, and never captured
/// inside a lambda.  Parameters are seeded into the outer lexical scope so
/// inner `let` shadowing is handled correctly.
///
/// [`emit_functions::emit_tir_params`] uses this to decide whether a `ref`
/// parameter needs a Rust `mut` prefix in the emitted signature.  Skipping
/// `mut` on parameters that are never mutated removes the `unused_mut`
/// warnings that previously required a crate-level `#![allow]` (#1654).
pub fn compute_readonly_param_names(body: &TirBlock, params: &[TirParam]) -> HashSet<String> {
    let mut tracker = MutTracker::default();
    // Seed params into an outer scope so the body walk's inner scopes can
    // shadow them without losing the parameter binding.
    tracker.enter_scope();
    let mut param_indices: Vec<(String, usize)> = Vec::with_capacity(params.len());
    for p in params {
        let idx = tracker.bindings.len();
        tracker.bindings.push(BindingInfo {
            span: p.span,
            mutated: false,
            referenced: false,
        });
        if let Some(top) = tracker.scopes.last_mut() {
            top.insert(p.name.clone(), idx);
        }
        param_indices.push((p.name.clone(), idx));
    }
    tracker.visit_block(body);
    tracker.exit_scope();

    param_indices
        .into_iter()
        .filter(|(_, idx)| !tracker.bindings[*idx].mutated)
        .map(|(name, _)| name)
        .collect()
}

/// Return the set of parameter names that are never *referenced* within
/// `body`, `requires`, or `ensures` — i.e. neither read nor written nor
/// mentioned in any pre/post condition.  Used by
/// [`emit_functions::emit_tir_params`] to prefix unreferenced parameter
/// names with `_` in emitted Rust (Rust idiom for "documented but unused"),
/// removing the `unused_variables` warnings that previously required a
/// crate-level `#![allow]` (#1658).
///
/// Requires/ensures are walked as reads because they are emitted as
/// `assert!` calls in the generated body — a param mentioned only in a
/// contract is still referenced from the emitted Rust's point of view.
pub fn compute_unused_param_names(
    body: &TirBlock,
    params: &[TirParam],
    requires: &[RefExpr],
    ensures: &[RefExpr],
) -> HashSet<String> {
    let mut tracker = MutTracker::default();
    tracker.enter_scope();
    let mut param_indices: Vec<(String, usize)> = Vec::with_capacity(params.len());
    for p in params {
        let idx = tracker.bindings.len();
        tracker.bindings.push(BindingInfo {
            span: p.span,
            mutated: false,
            referenced: false,
        });
        if let Some(top) = tracker.scopes.last_mut() {
            top.insert(p.name.clone(), idx);
        }
        param_indices.push((p.name.clone(), idx));
    }
    tracker.visit_block(body);
    for pred in requires {
        tracker.visit_refexpr(pred);
    }
    for pred in ensures {
        tracker.visit_refexpr(pred);
    }
    tracker.exit_scope();

    param_indices
        .into_iter()
        .filter(|(_, idx)| !tracker.bindings[*idx].referenced)
        .map(|(name, _)| name)
        .collect()
}

// ── Internal tracker ─────────────────────────────────────────────────────────

#[derive(Debug)]
struct BindingInfo {
    span: Span,
    mutated: bool,
    /// True when the name has been read, written, method-called, captured
    /// in a lambda, or referenced in a pre/post condition — anything that
    /// makes the corresponding Rust parameter *used* from `rustc`'s point
    /// of view.  Distinct from `mutated`: every mutation is a reference,
    /// but not every reference is a mutation.
    referenced: bool,
}

#[derive(Default)]
struct MutTracker {
    /// Every binding registered by a `let` — indexed by insertion order.
    bindings: Vec<BindingInfo>,
    /// Stack of scopes: each entry maps binding name → index into `bindings`.
    /// The topmost scope reflects the current lexical position; shadowing
    /// updates the top scope's entry, hiding any outer binding of the same
    /// name until the current scope is popped.
    scopes: Vec<HashMap<String, usize>>,
}

impl MutTracker {
    fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn exit_scope(&mut self) {
        self.scopes.pop();
    }

    /// Register a new `let` binding in the current scope, returning nothing.
    /// The binding starts life as read-only; visits to `Assign` / method-call
    /// receivers later may flip it.  Non-`Ident` patterns are ignored — the
    /// emitter never asks about their read-only status because destructuring
    /// patterns don't currently receive the `mut` treatment.
    fn declare(&mut self, pat: &Pattern) {
        if let Pattern::Ident(name, span) = pat {
            let idx = self.bindings.len();
            self.bindings.push(BindingInfo {
                span: *span,
                mutated: false,
                referenced: false,
            });
            if let Some(top) = self.scopes.last_mut() {
                top.insert(name.clone(), idx);
            }
        }
    }

    /// Resolve `name` to the innermost enclosing binding index, if any.
    fn resolve(&self, name: &str) -> Option<usize> {
        for scope in self.scopes.iter().rev() {
            if let Some(&idx) = scope.get(name) {
                return Some(idx);
            }
        }
        None
    }

    fn mark_mutated(&mut self, name: &str) {
        if let Some(idx) = self.resolve(name) {
            // Mutation implies reference.
            self.bindings[idx].mutated = true;
            self.bindings[idx].referenced = true;
        }
    }

    /// Mark `name` as referenced (read, method-called, captured, or
    /// mentioned in a pre/post condition).  Does not affect `mutated`.
    fn mark_referenced(&mut self, name: &str) {
        if let Some(idx) = self.resolve(name) {
            self.bindings[idx].referenced = true;
        }
    }

    // ── Structural walk ──────────────────────────────────────────────────────

    fn visit_block(&mut self, block: &TirBlock) {
        self.enter_scope();
        for stmt in &block.stmts {
            self.visit_stmt(stmt);
        }
        self.exit_scope();
    }

    fn visit_stmt(&mut self, stmt: &TirStmt) {
        match stmt {
            TirStmt::Let { pattern, init, .. } => {
                // Walk the initializer in the outer scope, then register the
                // binding so subsequent statements can resolve it.
                self.visit_expr(init);
                self.declare(pattern);
            }
            TirStmt::Assign { target, value, .. } => {
                self.visit_lvalue_as_target(target);
                self.visit_expr(value);
            }
            TirStmt::Return { value, .. } => {
                if let Some(e) = value {
                    self.visit_expr(e);
                }
            }
            TirStmt::If {
                cond, then, else_, ..
            } => {
                self.visit_expr(cond);
                self.visit_block(then);
                if let Some(else_branch) = else_ {
                    match else_branch {
                        TirElseBranch::Block(b) => self.visit_block(b),
                        TirElseBranch::If(s) => self.visit_stmt(s),
                    }
                }
            }
            TirStmt::Match {
                scrutinee, arms, ..
            } => {
                self.visit_expr(scrutinee);
                for arm in arms {
                    // Each arm body is its own scope; guards are `RefExpr`
                    // (spec-only, erased before codegen) and skipped.
                    match &arm.body {
                        TirMatchBody::Block(b) => self.visit_block(b),
                        TirMatchBody::Expr(e) => {
                            self.enter_scope();
                            self.visit_expr(e);
                            self.exit_scope();
                        }
                    }
                }
            }
            TirStmt::For { iter, body, .. } => {
                self.visit_expr(iter);
                self.visit_block(body);
            }
            TirStmt::While {
                cond,
                decreases,
                body,
                ..
            } => {
                self.visit_expr(cond);
                if let Some(d) = decreases {
                    self.visit_expr(d);
                }
                self.visit_block(body);
            }
            TirStmt::Expr { expr, .. } => self.visit_expr(expr),
        }
    }

    /// Walk a refinement/contract predicate (`requires` or `ensures`),
    /// marking any resolved `Ident` reference as *referenced* (not
    /// mutated).  Emitted `assert!` calls preserve these references in
    /// the generated body, so a param mentioned only in a contract must
    /// not be `_`-prefixed in the emitted signature (#1658).
    fn visit_refexpr(&mut self, expr: &RefExpr) {
        match expr {
            RefExpr::Ident { name, .. } => self.mark_referenced(name),
            RefExpr::Len { ident, .. } => self.mark_referenced(ident),
            RefExpr::LogicOp { left, right, .. }
            | RefExpr::Compare { left, right, .. }
            | RefExpr::ArithOp { left, right, .. } => {
                self.visit_refexpr(left);
                self.visit_refexpr(right);
            }
            RefExpr::Not { inner, .. }
            | RefExpr::Grouped { inner, .. }
            | RefExpr::Old { inner, .. } => self.visit_refexpr(inner),
            RefExpr::FieldAccess { object, .. } => self.visit_refexpr(object),
            RefExpr::Forall { body, .. } | RefExpr::Exists { body, .. } => {
                // Quantifier body may reference outer names (the quantified
                // variable itself is out of the tracker's scope, so
                // `resolve` will simply miss it — no false positive).
                self.visit_refexpr(body);
            }
            RefExpr::Integer { .. } | RefExpr::Float { .. } | RefExpr::Bool { .. } => {}
        }
    }

    /// Both `Ident` and `Field { base: Ident(name), ... }` writes count as a
    /// mutation of the outermost binding name.
    fn visit_lvalue_as_target(&mut self, lv: &LValue) {
        match lv {
            LValue::Ident(name, _) => self.mark_mutated(name),
            LValue::Field { base, .. } => self.visit_lvalue_as_target(base),
        }
    }

    fn visit_expr(&mut self, expr: &TirExpr) {
        match &expr.kind {
            TirExprKind::Var(name) => {
                // Pure read: not a mutation, but is a reference — needed
                // for `compute_unused_param_names` (#1658).
                self.mark_referenced(name);
            }
            TirExprKind::MethodCall { receiver, args, .. } => {
                // The receiver is treated as a potential mutation site: MVL
                // methods that take `self` by mutable reference will require
                // the binding to be `mut` in Rust.
                if let TirExprKind::Var(name) = &receiver.kind {
                    self.mark_mutated(name);
                } else {
                    self.visit_expr(receiver);
                }
                for a in args {
                    self.visit_expr(a);
                }
            }
            TirExprKind::FieldAccess { expr, .. } => self.visit_expr(expr),
            TirExprKind::FnCall { name, args, .. } => {
                // `name` may resolve to a fn-typed parameter (e.g. `f(x)`
                // where `f: fn(T) -> U` is a parameter).  `mark_referenced`
                // is a no-op when `name` isn't a local binding — global
                // function names simply miss the resolver.
                self.mark_referenced(name);
                for a in args {
                    self.visit_expr(a);
                }
            }
            TirExprKind::Unary { expr, .. } => self.visit_expr(expr),
            TirExprKind::Binary { left, right, .. } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            TirExprKind::If {
                cond, then, else_, ..
            } => {
                self.visit_expr(cond);
                self.visit_block(then);
                if let Some(e) = else_ {
                    self.visit_expr(e);
                }
            }
            TirExprKind::Match { scrutinee, arms } => {
                self.visit_expr(scrutinee);
                for arm in arms {
                    match &arm.body {
                        TirMatchBody::Block(b) => self.visit_block(b),
                        TirMatchBody::Expr(e) => {
                            self.enter_scope();
                            self.visit_expr(e);
                            self.exit_scope();
                        }
                    }
                }
            }
            TirExprKind::Block(b) => self.visit_block(b),
            TirExprKind::Lambda { body, .. } => {
                // Bindings referenced inside a lambda are conservatively
                // treated as mutated — the closure may outlive the enclosing
                // scope and be invoked arbitrarily.
                let mut inner = NameCollector::default();
                inner.visit_expr(body);
                for name in inner.names {
                    self.mark_mutated(&name);
                }
            }
            TirExprKind::Propagate(e) => self.visit_expr(e),
            TirExprKind::Construct { fields, .. } => {
                for (_, v) in fields {
                    self.visit_expr(v);
                }
            }
            _ => {
                walk_remaining_expr(self, expr);
            }
        }
    }
}

/// Collect every `Var` name referenced inside a subtree (used to conservatively
/// escalate lambda captures).
#[derive(Default)]
struct NameCollector {
    names: HashSet<String>,
}

impl<'a> crate::mvl::ir::visit::Visit<'a> for NameCollector {
    fn visit_tir_expr(&mut self, e: &'a TirExpr) {
        match &e.kind {
            TirExprKind::Var(name) => {
                self.names.insert(name.clone());
            }
            // The shared `walk_tir_expr` treats `FieldAccess` as a leaf
            // (see `ir/visit.rs:79`) — recurse manually so `obj.field`
            // inside a lambda body still marks the outer `obj` binding
            // as captured (#1658, snake_game regression).
            TirExprKind::FieldAccess { expr, .. } => {
                self.visit_tir_expr(expr);
                return;
            }
            _ => {}
        }
        crate::mvl::ir::visit::walk_tir_expr(self, e);
    }
}

impl NameCollector {
    fn visit_expr(&mut self, expr: &TirExpr) {
        use crate::mvl::ir::visit::Visit;
        self.visit_tir_expr(expr);
    }
}

/// Fallback walker for [`TirExprKind`] variants that carry nested exprs but
/// aren't handled explicitly above (currently: `List`, `Map`, `Set`,
/// `Relabel`, `Cast`, etc.).  Uses the shared TIR [`Visit`] machinery so that
/// nested `Var` / `MethodCall` / `Let` / `Assign` nodes get discovered.
fn walk_remaining_expr(tracker: &mut MutTracker, expr: &TirExpr) {
    use crate::mvl::ir::visit::{walk_tir_expr, Visit};

    struct Adapter<'a>(&'a mut MutTracker);

    impl<'a> Visit<'a> for Adapter<'_> {
        fn visit_tir_expr(&mut self, e: &'a TirExpr) {
            match &e.kind {
                TirExprKind::Var(_)
                | TirExprKind::MethodCall { .. }
                | TirExprKind::FieldAccess { .. }
                | TirExprKind::FnCall { .. }
                | TirExprKind::Unary { .. }
                | TirExprKind::Binary { .. }
                | TirExprKind::If { .. }
                | TirExprKind::Match { .. }
                | TirExprKind::Block(_)
                | TirExprKind::Lambda { .. }
                | TirExprKind::Propagate(_)
                | TirExprKind::Construct { .. } => {
                    self.0.visit_expr(e);
                }
                _ => walk_tir_expr(self, e),
            }
        }
    }

    Adapter(tracker).visit_tir_expr(expr);
}
