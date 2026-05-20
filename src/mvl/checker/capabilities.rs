// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Capability scope checking and reference lifetime helpers for the MVL type checker.

use crate::mvl::checker::errors::CheckError;
use crate::mvl::parser::ast::{
    Block, Capability, ElseBranch, Expr, MatchArm, MatchBody, Pattern, Stmt,
};
use crate::mvl::parser::lexer::Span;
use std::collections::HashSet;

use super::TypeChecker;

impl TypeChecker {
    // ── Lambda capture immutability (ADR-0002) ────────────────────────────

    /// Verify that the lambda body does not capture mutable outer bindings.
    ///
    /// ADR-0002 prescribes "Lambdas with immutable captures only".  This helper
    /// walks `expr` collecting every `Expr::Ident` that is NOT one of the lambda
    /// parameter names; if any such captured name is bound as `mutable` in the
    /// current environment, we emit [`CheckError::CaptureMutabilityViolation`].
    ///
    /// Note: superseded by the scope-index check in `Expr::Ident` (via `lambda_scope_starts`).
    /// Retained for reference; callers should prefer the scope-based approach.
    #[allow(dead_code)]
    pub(super) fn check_lambda_captures(&mut self, expr: &Expr, param_names: &[&str]) {
        let captures = collect_free_var_refs(expr, param_names);
        for (name, span) in captures {
            if let Some(info) = self.env.lookup(&name) {
                if info.mutable {
                    self.emit(CheckError::CaptureMutabilityViolation { name, span });
                }
            }
        }
    }

    // ── Reference capability checking (#22) ───────────────────────────────

    /// Verify that an argument to `channel.send()` has a sendable capability.
    ///
    /// Only `iso` and `val` may cross actor boundaries via `channel.send()`; `ref` may not.
    /// `tag` is sendable per ADR-0029 (identity-only, no read/write access).
    /// `consume` wrapping is detected by looking for `Expr::Consume` (or equivalent).
    ///
    /// # Scope limitation
    /// Currently only checks simple identifier arguments (e.g. `channel.send(x)`).
    /// Complex expressions like `channel.send(get_payload())` or `channel.send(obj.field)`
    /// are not checked. See #73 for tracking.
    pub(super) fn check_send_capability(&mut self, arg: &Expr, span: Span) {
        if let Expr::Ident(name, _) = arg {
            if let Some(info) = self.env.lookup(name).cloned() {
                // Only ref is non-sendable; iso, val, tag, and unannotated are sendable (ADR-0029 §1)
                if let Some(Capability::Ref) = &info.capability {
                    self.emit(CheckError::CapabilityViolation {
                        param: name.clone(),
                        capability: "ref".to_string(),
                        span,
                    });
                }
            }
        }
    }

    /// Phase C (#305, #363): scope-depth check for reference assignments.
    ///
    /// Emits `ReferenceOutlivesOwner` when the referent variable is defined at a
    /// deeper (shorter-lived) scope than the reference binding, or when the referent
    /// is block-local and leaves scope before the binding is made.
    ///
    /// Handles both implicit borrow (`let r: val T = x`) and explicit borrow
    /// (`let r: val T = val x`) via `referent_ident`'s `Expr::Borrow` unwrapping.
    pub(super) fn check_capability_scope(&mut self, pattern: &Pattern, init: &Expr) {
        let Pattern::Ident(ref_name, _) = pattern else {
            return;
        };
        let Some(owner_name) = referent_ident(init) else {
            return;
        };
        // scope_depth() returns scopes.len() (raw count); VarInfo.scope_depth is 0-based (scopes.len()-1).
        let r_depth = self.env.scope_depth().saturating_sub(1);
        let owner_too_deep = match self.env.lookup(owner_name) {
            Some(info) => info.scope_depth > r_depth,
            // Not in scope: defined inside the init block → always dangling.
            None => true,
        };
        if owner_too_deep {
            self.emit(CheckError::ReferenceOutlivesOwner {
                ref_name: ref_name.clone(),
                owner_name: owner_name.to_owned(),
                span: init.span(),
            });
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Walk `expr` and return all `(name, span)` pairs for `Expr::Ident` references
/// whose name is NOT in `param_names` (i.e. free variables / potential captures).
///
/// Nested lambdas are NOT recursed into — their params shadow outer names and
/// their own captures will be checked when that lambda is visited by the checker.
///
/// Used by `check_lambda_captures` (superseded approach — see `lambda_scope_starts`).
#[allow(dead_code)]
fn collect_free_var_refs(expr: &Expr, param_names: &[&str]) -> Vec<(String, Span)> {
    let mut out = Vec::new();
    collect_refs_expr(expr, param_names, &mut out);
    out
}

#[allow(dead_code)]
fn collect_refs_expr(expr: &Expr, params: &[&str], out: &mut Vec<(String, Span)>) {
    match expr {
        Expr::Ident(name, span) => {
            if !params.contains(&name.as_str()) {
                out.push((name.clone(), *span));
            }
        }
        Expr::Lambda { .. } => {
            // Do NOT recurse: the nested lambda is checked independently.
        }
        Expr::Literal(..) => {}
        Expr::FieldAccess { expr: e, .. } => collect_refs_expr(e, params, out),
        Expr::MethodCall { receiver, args, .. } => {
            collect_refs_expr(receiver, params, out);
            for a in args {
                collect_refs_expr(a, params, out);
            }
        }
        Expr::FnCall { args, .. } => {
            for a in args {
                collect_refs_expr(a, params, out);
            }
        }
        Expr::Unary { expr: e, .. }
        | Expr::Propagate { expr: e, .. }
        | Expr::Consume { expr: e, .. }
        | Expr::Relabel { expr: e, .. }
        | Expr::Borrow { expr: e, .. } => collect_refs_expr(e, params, out),
        Expr::Binary { left, right, .. } => {
            collect_refs_expr(left, params, out);
            collect_refs_expr(right, params, out);
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            collect_refs_expr(cond, params, out);
            collect_refs_block(then, params, out);
            if let Some(e) = else_ {
                collect_refs_expr(e, params, out);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_refs_expr(scrutinee, params, out);
            for arm in arms {
                match &arm.body {
                    MatchBody::Expr(e) => collect_refs_expr(e, params, out),
                    MatchBody::Block(b) => collect_refs_block(b, params, out),
                }
            }
        }
        Expr::Block(b) => collect_refs_block(b, params, out),
        Expr::Construct { fields, .. } => {
            for (_, e) in fields {
                collect_refs_expr(e, params, out);
            }
        }
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            for e in elems {
                collect_refs_expr(e, params, out);
            }
        }
        Expr::Map { pairs, .. } => {
            for (k, v) in pairs {
                collect_refs_expr(k, params, out);
                collect_refs_expr(v, params, out);
            }
        }
        Expr::Spawn { fields, .. } => {
            for (_, v) in fields {
                collect_refs_expr(v, params, out);
            }
        }
        Expr::Select { arms, .. } => {
            for arm in arms {
                collect_refs_expr(&arm.expr, params, out);
                collect_refs_block(&arm.body, params, out);
            }
        }
        Expr::Concurrently { body, .. } => collect_refs_block(body, params, out),
    }
}

#[allow(dead_code)]
fn collect_refs_block(block: &Block, params: &[&str], out: &mut Vec<(String, Span)>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { init, .. } => collect_refs_expr(init, params, out),
            Stmt::Assign { value, .. } => collect_refs_expr(value, params, out),
            Stmt::Return { value, .. } => {
                if let Some(e) = value {
                    collect_refs_expr(e, params, out);
                }
            }
            Stmt::If {
                cond, then, else_, ..
            } => {
                collect_refs_expr(cond, params, out);
                collect_refs_block(then, params, out);
                match else_ {
                    Some(ElseBranch::Block(b)) => collect_refs_block(b, params, out),
                    Some(ElseBranch::If(s)) => {
                        collect_refs_block(
                            &Block {
                                stmts: vec![*s.clone()],
                                span: s.span(),
                            },
                            params,
                            out,
                        );
                    }
                    None => {}
                }
            }
            Stmt::Match {
                scrutinee, arms, ..
            } => {
                collect_refs_expr(scrutinee, params, out);
                for arm in arms {
                    match &arm.body {
                        MatchBody::Expr(e) => collect_refs_expr(e, params, out),
                        MatchBody::Block(b) => collect_refs_block(b, params, out),
                    }
                }
            }
            Stmt::For { iter, body, .. } => {
                collect_refs_expr(iter, params, out);
                collect_refs_block(body, params, out);
            }
            Stmt::While { cond, body, .. } => {
                collect_refs_expr(cond, params, out);
                collect_refs_block(body, params, out);
            }
            Stmt::Expr { expr, .. } => collect_refs_expr(expr, params, out),
        }
    }
}

/// Extract the "root identifier" from an expression used as a `val`/`ref` init.
///
/// For `let r: val T = x`, returns `Some("x")`.
/// For `let r: val T = { ...; x }`, returns `Some("x")` (the block's tail ident).
/// Returns `None` for complex expressions where the referent cannot be named.
///
/// Used by Phase C scope-depth checking (#305, #363).
fn referent_ident(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name),
        // `val x` and `ref x` — unwrap to the inner expression.
        Expr::Borrow { expr, .. } => referent_ident(expr),
        Expr::Block(block) => block.stmts.last().and_then(|s| match s {
            Stmt::Expr { expr, .. } => referent_ident(expr),
            _ => None,
        }),
        _ => None,
    }
}

/// Check whether the tail return expression of a block flows from one of the
/// given reference-parameter names.
///
/// Returns `None` if every path through the block tail returns a value whose
/// origin is one of `ref_params`.  Returns `Some(span)` pointing at the first
/// sub-expression that is NOT traceable to a reference parameter.
///
/// Also scans all statements for early `return` expressions that don't flow
/// from a reference parameter (catches returns before the tail position).
///
/// Used by Phase C return-flow checking (#364).
pub(super) fn block_return_flows_from_ref_param(
    block: &Block,
    ref_params: &HashSet<&str>,
) -> Option<Span> {
    // First, scan every statement for embedded early returns that don't flow
    // from a reference parameter.
    if let Some(bad) = block_early_return_violation(block, ref_params) {
        return Some(bad);
    }
    // Then check the tail expression (the implicit return value of the block).
    match block.stmts.last() {
        None => Some(block.span),
        Some(stmt) => stmt_return_flows_from_ref_param(stmt, block.span, ref_params),
    }
}

/// Check whether `stmt`, when in tail position, produces a value that flows
/// from one of the reference parameters in `ref_params`.
///
/// Returns `None` if the value flows from a reference parameter, or
/// `Some(span)` pointing at the first sub-expression that does not.
fn stmt_return_flows_from_ref_param(
    stmt: &Stmt,
    fallback_span: Span,
    ref_params: &HashSet<&str>,
) -> Option<Span> {
    match stmt {
        Stmt::Expr { expr, .. } => expr_return_flows_from_ref_param(expr, ref_params),
        Stmt::Return {
            value: Some(expr), ..
        } => expr_return_flows_from_ref_param(expr, ref_params),
        Stmt::Return { value: None, span } => Some(*span),
        Stmt::If {
            then, else_, span, ..
        } => {
            if let Some(bad) = block_return_flows_from_ref_param(then, ref_params) {
                return Some(bad);
            }
            match else_ {
                None => Some(*span),
                Some(ElseBranch::Block(b)) => block_return_flows_from_ref_param(b, ref_params),
                Some(ElseBranch::If(inner)) => {
                    stmt_return_flows_from_ref_param(inner, *span, ref_params)
                }
            }
        }
        Stmt::Match { arms, span, .. } => {
            if arms.is_empty() {
                return Some(*span);
            }
            check_match_arms_flow(arms, ref_params)
        }
        _ => Some(fallback_span),
    }
}

/// Check whether `expr` produces a value that flows from one of the reference
/// parameters in `ref_params`.
///
/// Returns `None` if the value flows from a reference parameter, or
/// `Some(span)` pointing at the first sub-expression that does not.
fn expr_return_flows_from_ref_param(expr: &Expr, ref_params: &HashSet<&str>) -> Option<Span> {
    match expr {
        Expr::Ident(name, _) => {
            if ref_params.contains(name.as_str()) {
                None
            } else {
                Some(expr.span())
            }
        }
        // A borrow expression `&inner` is transparent: the underlying value
        // still needs to flow from a reference parameter.
        Expr::Borrow { expr: inner, .. } => expr_return_flows_from_ref_param(inner, ref_params),
        Expr::If {
            then, else_, span, ..
        } => {
            if let Some(bad) = block_return_flows_from_ref_param(then, ref_params) {
                return Some(bad);
            }
            match else_ {
                None => Some(*span),
                Some(else_expr) => expr_return_flows_from_ref_param(else_expr, ref_params),
            }
        }
        Expr::Match { arms, span, .. } => {
            if arms.is_empty() {
                return Some(*span);
            }
            check_match_arms_flow(arms, ref_params)
        }
        Expr::Block(block) => block_return_flows_from_ref_param(block, ref_params),
        _ => Some(expr.span()),
    }
}

/// Check each arm of a match expression; return the span of the first arm
/// whose body does not flow from a reference parameter, or `None` if all
/// arms are valid.
fn check_match_arms_flow(arms: &[MatchArm], ref_params: &HashSet<&str>) -> Option<Span> {
    for arm in arms {
        let bad = match &arm.body {
            MatchBody::Expr(e) => expr_return_flows_from_ref_param(e, ref_params),
            MatchBody::Block(b) => block_return_flows_from_ref_param(b, ref_params),
        };
        if let Some(bad_span) = bad {
            return Some(bad_span);
        }
    }
    None
}

/// Walk every statement in `block` (at any depth) and return the span of the
/// first `Stmt::Return` whose value does not flow from `ref_params`, or `None`
/// if every explicit return is valid.
///
/// This catches early `return` statements that appear before the tail position.
fn block_early_return_violation(block: &Block, ref_params: &HashSet<&str>) -> Option<Span> {
    for stmt in &block.stmts {
        if let Some(bad) = stmt_early_return_violation(stmt, ref_params) {
            return Some(bad);
        }
    }
    None
}

fn stmt_early_return_violation(stmt: &Stmt, ref_params: &HashSet<&str>) -> Option<Span> {
    match stmt {
        Stmt::Return {
            value: Some(expr), ..
        } => expr_return_flows_from_ref_param(expr, ref_params),
        Stmt::Return { value: None, span } => Some(*span),
        Stmt::If { then, else_, .. } => {
            block_early_return_violation(then, ref_params).or_else(|| match else_ {
                None => None,
                Some(ElseBranch::Block(b)) => block_early_return_violation(b, ref_params),
                Some(ElseBranch::If(inner)) => stmt_early_return_violation(inner, ref_params),
            })
        }
        Stmt::Match { arms, .. } => {
            for arm in arms {
                if let MatchBody::Block(b) = &arm.body {
                    if let Some(bad) = block_early_return_violation(b, ref_params) {
                        return Some(bad);
                    }
                }
            }
            None
        }
        Stmt::Expr { expr, .. } => expr_early_return_violation(expr, ref_params),
        _ => None,
    }
}

fn expr_early_return_violation(expr: &Expr, ref_params: &HashSet<&str>) -> Option<Span> {
    match expr {
        Expr::If { then, else_, .. } => {
            block_early_return_violation(then, ref_params).or_else(|| match else_ {
                None => None,
                Some(e) => expr_early_return_violation(e, ref_params),
            })
        }
        Expr::Match { arms, .. } => {
            for arm in arms {
                if let MatchBody::Block(b) = &arm.body {
                    if let Some(bad) = block_early_return_violation(b, ref_params) {
                        return Some(bad);
                    }
                }
            }
            None
        }
        Expr::Block(b) => block_early_return_violation(b, ref_params),
        _ => None,
    }
}
