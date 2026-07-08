// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emit Rust statements from MVL [`TirStmt`] nodes.
//!
//! Covers statement forms defined in 000-parser/Req 5 ("Parse Statements"):
//! `let`/`let mut`, assignment, `if`/`else`, `match`, `for`, `while`, `return`, `?`.
//!
//! The emitted Rust preserves MVL's semantic guarantees:
//! - `while` only appears in `partial fn` bodies (enforced by the type checker per Req 8)
//! - `for` iterates over labeled collections, preserving security labels per Req 11
//! - Assignments carry the label of the source expression (IFC is static, no runtime cost)
//!
//! Part of the ADR-0003 transpilation pipeline.  Spec link: 000-parser Req 1 (statement grammar).
//!
//! See ADR-0003 for the overall compilation strategy.

use super::emitter::RustEmitter;
use crate::mvl::backends::rust::emit_exprs::arms_have_str_pattern;
use crate::mvl::backends::rust::emit_types::{emit_ref_expr_for_assert, emit_ty};
use crate::mvl::backends::rust::mcdc_instr::DecisionKind;
use crate::mvl::ir::{
    BinaryOp, LValue, LetKind, Literal, LogicOp, Pattern, RefExpr, TirBlock, TirElseBranch,
    TirExpr, TirExprKind, TirMatchBody, TirStmt, Ty,
};
use crate::mvl::passes::coverage::BranchKind;
use crate::mvl::passes::mcdc::analysis::count_clauses_ref;

impl RustEmitter {
    /// Emit a single statement (with indentation and trailing newline).
    pub fn emit_stmt(&mut self, stmt: &TirStmt) {
        match stmt {
            // Ghost bindings are specification-only — erased before codegen (Phase 4, #627).
            TirStmt::Let {
                kind: LetKind::Ghost,
                ..
            } => {}
            TirStmt::Let {
                kind: LetKind::Regular,
                pattern,
                ty,
                init,
                ..
            } => {
                self.indent();
                // `Ty::Ref(true, _)` encodes mutability — emit `let mut` and strip the ref wrapper.
                // `Ty::Ref(false, _)` encodes MVL `val` (read-only borrow) — strip the ref too so
                // the binding is emitted owned; the init expression is owned in Rust, and a bare
                // `let x: &T = <owned>` would fail (E0308).  Val semantics at the local-binding
                // site are indistinguishable from ownership once we already own the RHS (#1707
                // phase 8).
                let (is_mutable, ty_for_emit) = match ty {
                    Ty::Ref(true, inner) => (true, inner.as_ref()),
                    Ty::Ref(false, inner) => (false, inner.as_ref()),
                    _ => (false, ty),
                };
                // Suppress `mut` when the analysis in `mut_analysis` proved that
                // this `ref` binding is only read within the function body — no
                // assignments, no method-call receivers.  Rust would emit an
                // `unused_mut` warning for these otherwise.  Keyed by pattern
                // span so shadowed bindings are analysed independently.
                let is_readonly = matches!(pattern, Pattern::Ident(_, span)
                    if self.readonly_names.contains(span));
                if is_mutable && !is_readonly {
                    self.push("let mut ");
                } else {
                    self.push("let ");
                }
                self.emit_pattern(pattern);
                // Fn types: omit the annotation so Rust infers the concrete
                // closure type — `fn(T)->U` rejects capturing closures (#1313).
                if !matches!(ty_for_emit, Ty::Fn(..)) {
                    self.push(": ");
                    self.push(&emit_ty(ty_for_emit));
                }
                self.push(" = ");
                // Refined alias wrapping: `let port: Port = 5558` → `Port::new(5558)` (#1326)
                let refined_wrap = self.refined_alias_base(ty_for_emit).is_some()
                    && self.refined_alias_base(&init.ty).is_none();
                // Refined alias unwrapping: `let n: Int = port` → `port.0` (#1326)
                let refined_unwrap = self.refined_alias_base(ty_for_emit).is_none()
                    && self.refined_alias_base(&init.ty).is_some();
                if refined_wrap {
                    if let Ty::Named(name, _) = ty_for_emit {
                        self.push(&format!("{}::new(", name));
                    }
                }
                // For Fn types: wrap Lambda in move so closure owns captures
                let wrap_move = matches!(ty_for_emit, Ty::Fn(..))
                    && matches!(&init.kind, TirExprKind::Lambda { .. });
                if wrap_move {
                    self.push("move ");
                }
                self.emit_expr(init);
                // When the init is a field access on a borrowed receiver — either a
                // capability parameter (`acc.items` where `acc: &ParseAcc`) or the
                // implicit `self` in an actor method (`self.workers` where self is
                // `&mut ActorState`) — the field is behind a reference and cannot be
                // moved.  `.clone()` produces an owned copy for mutation + reassign.
                let field_needs_clone =
                    if let TirExprKind::FieldAccess { expr: inner, .. } = &init.kind {
                        if let TirExprKind::Var(name) = &inner.kind {
                            self.capability_param_names.contains(name.as_str())
                                || (name == "self" && !self.actor_self_type.is_empty())
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                // MVL value semantics on a `let x: T = other_var;` — Rust would
                // otherwise MOVE `other_var` into `x`, leaving subsequent reads
                // of `other_var` invalid.  When the init is a bare `Var` that
                // is NOT its last use, insert `.clone()` so the source binding
                // stays live for later reads (last-use analysis at
                // [`compute_last_uses`], #1707 phase 10).
                let var_needs_clone = matches!(&init.kind, TirExprKind::Var(_))
                    && !self.last_uses.contains(&init.span);
                if field_needs_clone || var_needs_clone {
                    self.push(".clone()");
                }
                if refined_unwrap {
                    self.push(".0");
                }
                if refined_wrap {
                    self.push(")");
                }
                // When the declared type is a security label (e.g. Tainted<String>) and the
                // init is a plain value or function call, `.into()` converts it to the labeled
                // type.  Skip the coercion when the init's static type already matches the
                // declared type — `.into()` is a no-op there and, for a generic FnCall,
                // blocks Rust from inferring the type parameter (#1707 phase 11).
                let needs_into = matches!(ty, Ty::Labeled(..))
                    && init.ty != *ty
                    && matches!(
                        &init.kind,
                        TirExprKind::Literal(Literal::Str(_))
                            | TirExprKind::FnCall { .. }
                            | TirExprKind::MethodCall { .. }
                    );
                if needs_into {
                    self.push(".into()");
                }
                self.push(";");
                self.nl();
            }

            TirStmt::Assign { target, value, .. } => {
                self.indent();
                self.emit_lvalue(target);
                self.push(" = ");
                self.emit_expr(value);
                self.push(";");
                self.nl();
            }

            TirStmt::Return { value, .. } => {
                self.indent();
                if let Some(v) = value {
                    self.push("return ");
                    // Wrap Lambda in move when returning from function so
                    // closure owns captured variables (#1313).
                    if matches!(&v.kind, TirExprKind::Lambda { .. }) {
                        self.push("move ");
                    }
                    self.emit_expr(v);
                    self.push(";");
                } else {
                    self.push("return;");
                }
                self.nl();
            }

            TirStmt::If {
                cond,
                then,
                else_,
                span,
                ..
            } => {
                let true_id = self.alloc_branch(span.line, BranchKind::IfTrue);
                let false_id = else_
                    .as_ref()
                    .and(self.alloc_branch(span.line, BranchKind::IfFalse));
                if self.emit_mcdc_if(cond, then, else_, span.line, true_id, false_id) {
                    // MC/DC instrumented emission handled the full if-statement.
                } else {
                    self.indent();
                    self.push("if ");
                    self.emit_expr(cond);
                    self.push(" {");
                    self.nl();
                    self.push_indent();
                    if let Some(id) = true_id {
                        self.emit_cov_hit(id);
                    }
                    self.emit_block_as_value(&then.stmts);
                    self.pop_indent();
                    self.indent();
                    self.push("}");
                    if let Some(else_branch) = else_ {
                        self.push(" else ");
                        self.emit_else_branch(else_branch, false_id);
                    }
                    self.nl();
                }
            }

            TirStmt::Match {
                scrutinee,
                arms,
                span,
                ..
            } => {
                // Allocate branch coverage IDs for each arm up-front (avoids borrow conflict).
                let arm_ids: Vec<Option<usize>> = (0..arms.len())
                    .map(|i| self.alloc_branch(span.line, BranchKind::MatchArm(i)))
                    .collect();
                let has_str_arm = arms_have_str_pattern(arms);

                // Emit scrutinee first so any compound conditions inside it allocate
                // MC/DC IDs before the match-level decisions (mirrors analysis order).
                self.indent();
                self.push("match ");
                self.emit_expr(scrutinee);
                // Clone when the scrutinee is a self.field access (can't move out of &self)
                // or a capability param (val/ref → &T/&mut T in Rust). Without clone,
                // match ergonomics yield reference bindings that fail E0507/E0277.
                if scrutinee_needs_clone(scrutinee)
                    || matches!(&scrutinee.kind, TirExprKind::Var(name) if self.capability_param_names.contains(name))
                {
                    self.push(".clone()");
                }

                // Allocate MC/DC arm-coverage decision (Match kind, one "clause" per arm).
                let match_mcdc_id: Option<usize> = if arms.len() >= 2 {
                    self.alloc_mcdc_decision(span.line, arms.len(), DecisionKind::Match, vec![])
                } else {
                    None
                };
                // Pre-allocate MatchGuard decision IDs for compound guards — all arms in
                // order before any body emission (mirrors analysis pre-allocation order).
                let guard_mcdc_ids: Vec<Option<usize>> = arms
                    .iter()
                    .map(|arm| {
                        arm.guard.as_ref().and_then(|g| {
                            let n = count_clauses_ref(g);
                            if n >= 2 {
                                self.alloc_mcdc_decision(
                                    arm.span.line,
                                    n,
                                    DecisionKind::MatchGuard,
                                    vec![],
                                )
                            } else {
                                None
                            }
                        })
                    })
                    .collect();

                if has_str_arm {
                    self.push(".as_str()");
                }
                self.push(" {");
                self.nl();
                self.push_indent();
                for ((arm_idx, arm), (cov_id, guard_mcdc_id)) in arms
                    .iter()
                    .enumerate()
                    .zip(arm_ids.iter().zip(guard_mcdc_ids.iter()))
                {
                    self.indent();
                    self.emit_pattern(&arm.pattern);
                    if let Some(guard) = &arm.guard {
                        self.push(" if ");
                        if let Some(&gid) = guard_mcdc_id.as_ref() {
                            let n = count_clauses_ref(guard);
                            self.push(&emit_mcdc_guard_block(guard, gid, n));
                        } else {
                            self.push(&emit_ref_expr_for_assert(guard, "_"));
                        }
                    }
                    self.push(" => ");
                    match &arm.body {
                        TirMatchBody::Expr(e) => {
                            // Wrap in a block to inject coverage and MC/DC hits.
                            self.push("{ ");
                            if let Some(&id) = cov_id.as_ref() {
                                self.push(&format!("#[cfg(test)] crate::__mvl_cov::hit({id}); "));
                            }
                            if let Some(mid) = match_mcdc_id {
                                self.push(&format!(
                                    "#[cfg(test)] crate::__mvl_mcdc::record({mid}usize, {arm_idx}u32); "
                                ));
                            }
                            self.emit_expr(e);
                            self.push(" }");
                            self.push(",");
                            self.nl();
                        }
                        TirMatchBody::Block(block) => {
                            self.push("{");
                            self.nl();
                            self.push_indent();
                            if let Some(&id) = cov_id.as_ref() {
                                self.emit_cov_hit(id);
                            }
                            if let Some(mid) = match_mcdc_id {
                                self.line(&format!(
                                    "#[cfg(test)] crate::__mvl_mcdc::record({mid}usize, {arm_idx}u32);"
                                ));
                            }
                            // Use emit_block_as_value so the final TirStmt::Expr is a tail
                            // expression (no semicolon) and becomes the arm's return value.
                            self.emit_block_as_value(&block.stmts);
                            self.pop_indent();
                            self.indent();
                            self.push("}");
                            self.nl();
                        }
                    }
                }
                self.pop_indent();
                self.indent();
                self.push("}");
                self.nl();
            }

            TirStmt::For {
                pattern,
                iter,
                body,
                span,
                ..
            } => {
                let for_id = self.alloc_branch(span.line, BranchKind::ForBody);
                self.indent();
                self.push("for ");
                self.emit_pattern(pattern);
                // MVL value semantics: the iterable is conceptually copied, not consumed.
                // Wrap the entire expression in parens before `.clone()` so the pattern
                // works for all expression forms (ident, field access, function call, etc.).
                // Spec 009 Req 7.
                self.push(" in (");
                self.emit_expr(iter);
                self.push(").clone() {");
                self.nl();
                self.push_indent();
                if let Some(id) = for_id {
                    self.emit_cov_hit(id);
                }
                self.emit_block_stmts(&body.stmts);
                self.pop_indent();
                self.indent();
                self.push("}");
                self.nl();
            }

            TirStmt::While {
                cond, body, span, ..
            } => {
                let while_id = self.alloc_branch(span.line, BranchKind::WhileBody);
                if self.emit_mcdc_while(cond, body, span.line, while_id) {
                    // MC/DC instrumented emission handled the full while-loop.
                } else {
                    self.indent();
                    // `while true { … }` → `loop { … }` to avoid the Rust while_true lint.
                    let is_unconditional =
                        matches!(&cond.kind, TirExprKind::Literal(Literal::Bool(true)));
                    if is_unconditional {
                        self.push("loop {");
                    } else {
                        self.push("while ");
                        self.emit_expr(cond);
                        self.push(" {");
                    }
                    self.nl();
                    self.push_indent();
                    if let Some(id) = while_id {
                        self.emit_cov_hit(id);
                    }
                    self.emit_block_stmts(&body.stmts);
                    self.pop_indent();
                    self.indent();
                    self.push("}");
                    self.nl();
                }
            }

            TirStmt::Expr { expr, .. } => {
                self.indent();
                self.emit_expr(expr);
                // Determine if this needs a semicolon: add one for non-block expressions
                // that are used as statements (not the implicit tail expression).
                // Phase 1: always add semicolon for safety; tail expressions in Rust
                // blocks without semicolons are handled by emit_fn_decl's body emitter.
                self.push(";");
                self.nl();
            }
        }
    }
}

/// Returns true when the match scrutinee is a direct field access on `self`
/// (e.g. `self.best_ask`). Such expressions cannot be moved out of `&mut self`,
/// so the emitter appends `.clone()` to the scrutinee.
pub(crate) fn scrutinee_needs_clone(expr: &TirExpr) -> bool {
    if let TirExprKind::FieldAccess { expr: base, .. } = &expr.kind {
        matches!(&base.kind, TirExprKind::Var(n) if n == "self" || n == "self_")
    } else {
        false
    }
}

impl RustEmitter {
    fn emit_lvalue(&mut self, lv: &LValue) {
        match lv {
            LValue::Ident(name, _) => self.push(name),
            LValue::Field { base, field, .. } => {
                self.emit_lvalue(base);
                self.push(".");
                self.push(field);
            }
        }
    }

    fn emit_else_branch(&mut self, branch: &TirElseBranch, cov_id: Option<usize>) {
        match branch {
            TirElseBranch::Block(block) => {
                self.push("{");
                self.nl();
                self.push_indent();
                if let Some(id) = cov_id {
                    self.emit_cov_hit(id);
                }
                self.emit_block_as_value(&block.stmts);
                self.pop_indent();
                self.indent();
                self.push("}");
            }
            TirElseBranch::If(stmt) => {
                // Emit the `if` inline (no leading indent, no trailing newline) so
                // the caller's `} else ` and this `if` land on the same line.
                // The false-branch coverage hit for the outer if is injected here
                // before the inner condition is tested.
                //
                // When the outer if's IfFalse probe needs emitting, we wrap in a
                // block: `else { hit(N); if ... }` so the probe fires before the
                // inner condition is evaluated.
                let needs_block = cov_id.is_some();
                if needs_block {
                    self.push("{");
                    self.nl();
                    self.push_indent();
                    if let Some(id) = cov_id {
                        self.emit_cov_hit(id);
                    }
                    self.indent();
                }
                match stmt.as_ref() {
                    TirStmt::If {
                        cond,
                        then,
                        else_,
                        span,
                        ..
                    } => {
                        // Allocate IDs for the inner else-if's own branches.
                        let inner_true_id = self.alloc_branch(span.line, BranchKind::IfTrue);
                        let inner_false_id = else_
                            .as_ref()
                            .and(self.alloc_branch(span.line, BranchKind::IfFalse));
                        // Instrument compound else-if conditions with MC/DC (same as
                        // top-level if — mirrors analysis order so decision IDs align).
                        // Must wrap in `{ }` because `else` requires a block in Rust.
                        let mut check_clauses = Vec::new();
                        collect_clauses_tir(cond, &mut check_clauses);
                        if check_clauses.len() >= 2 && self.mcdc.is_some() {
                            if !needs_block {
                                self.push("{");
                                self.nl();
                                self.push_indent();
                            }
                            self.emit_mcdc_if(
                                cond,
                                then,
                                else_,
                                span.line,
                                inner_true_id,
                                inner_false_id,
                            );
                            self.pop_indent();
                            self.indent();
                            self.push("}");
                            return;
                        }
                        self.push("if ");
                        self.emit_expr(cond);
                        self.push(" {");
                        self.nl();
                        self.push_indent();
                        if let Some(id) = inner_true_id {
                            self.emit_cov_hit(id);
                        }
                        self.emit_block_as_value(&then.stmts);
                        self.pop_indent();
                        self.indent();
                        self.push("}");
                        if let Some(inner_else) = else_ {
                            self.push(" else ");
                            self.emit_else_branch(inner_else, inner_false_id);
                        }
                    }
                    other => unreachable!(
                        "TirElseBranch::If must always wrap TirStmt::If; got {:?}",
                        other
                    ),
                }
                if needs_block {
                    self.pop_indent();
                    self.indent();
                    self.push("}");
                }
            }
        }
    }
}

// ── MC/DC guard instrumentation ──────────────────────────────────────────

/// Build a Rust block expression (usable as a match arm guard) that tracks
/// MC/DC clause values for a compound `RefExpr` guard.
///
/// The generated block evaluates the guard with short-circuit semantics,
/// records an observation, and evaluates to the boolean guard outcome.
/// Returned as a `String` because `RefExpr` uses string-building throughout.
pub(crate) fn emit_mcdc_guard_block(guard: &RefExpr, decision_id: usize, n: usize) -> String {
    let decls = format!(
        "let mut __d{decision_id}_c = [false; {n}]; \
         let mut __d{decision_id}_e = [false; {n}];"
    );
    let mut idx = 0usize;
    let sc = sc_ref_outcome_str(guard, decision_id, &mut idx);

    // Build 2N+1 bit observation encoding identical to emit_mcdc_record.
    let vals: Vec<String> = (0..n)
        .map(|i| format!("((__d{decision_id}_c[{i}] as u32) << {i}u32)"))
        .collect();
    let evals: Vec<String> = (0..n)
        .map(|i| format!("((__d{decision_id}_e[{i}] as u32) << {}u32)", n + i))
        .collect();
    let outcome_bit = format!("((__d{decision_id}_outcome as u32) << {}u32)", 2 * n);
    let encoding = vals
        .into_iter()
        .chain(evals)
        .chain(std::iter::once(outcome_bit))
        .collect::<Vec<_>>()
        .join(" | ");

    format!(
        "{{ {decls} \
         let __d{decision_id}_outcome: bool = {sc}; \
         #[cfg(test)] crate::__mvl_mcdc::record({decision_id}usize, {encoding}); \
         __d{decision_id}_outcome }}"
    )
}

/// Recursively build the short-circuit evaluation tree for a `RefExpr`.
///
/// Returns a Rust expression string that evaluates the guard with short-circuit
/// semantics while setting `__d{id}_c[i]` and `__d{id}_e[i]` for each leaf.
fn sc_ref_outcome_str(expr: &RefExpr, id: usize, idx: &mut usize) -> String {
    match expr {
        RefExpr::LogicOp {
            op: LogicOp::And,
            left,
            right,
            ..
        } => {
            let l = sc_ref_outcome_str(left, id, idx);
            let r = sc_ref_outcome_str(right, id, idx);
            format!("(if {{ {l} }} {{ {r} }} else {{ false }})")
        }
        RefExpr::LogicOp {
            op: LogicOp::Or,
            left,
            right,
            ..
        } => {
            let l = sc_ref_outcome_str(left, id, idx);
            let r = sc_ref_outcome_str(right, id, idx);
            format!("(if {{ {l} }} {{ true }} else {{ {r} }})")
        }
        RefExpr::Grouped { inner, .. } => sc_ref_outcome_str(inner, id, idx),
        _ => {
            let i = *idx;
            *idx += 1;
            let val = emit_ref_expr_for_assert(expr, "_");
            format!("{{ __d{id}_e[{i}] = true; __d{id}_c[{i}] = {val}; __d{id}_c[{i}] }}")
        }
    }
}

// ── TIR clause counting (parallel to AST collect_clauses) ────────────────

/// Count atomic boolean clauses in a TirExpr compound condition.
fn count_clauses_tir(expr: &TirExpr) -> usize {
    match &expr.kind {
        TirExprKind::Binary {
            op: BinaryOp::And | BinaryOp::Or,
            left,
            right,
        } => count_clauses_tir(left) + count_clauses_tir(right),
        _ => 1,
    }
}

/// Collect TirExpr leaf references from a compound boolean TirExpr.
fn collect_clauses_tir<'a>(expr: &'a TirExpr, clauses: &mut Vec<&'a TirExpr>) {
    match &expr.kind {
        TirExprKind::Binary {
            op: BinaryOp::And | BinaryOp::Or,
            left,
            right,
        } => {
            collect_clauses_tir(left, clauses);
            collect_clauses_tir(right, clauses);
        }
        _ => clauses.push(expr),
    }
}

// ── MC/DC instrumentation helpers ────────────────────────────────────────

impl RustEmitter {
    /// Emit an if-statement with MC/DC clause tracking for compound conditions.
    ///
    /// Returns `true` when MC/DC instrumentation was applied (compound condition
    /// with an active MC/DC map).  Returns `false` when the caller should fall
    /// back to normal emission (simple condition or MC/DC inactive).
    fn emit_mcdc_if(
        &mut self,
        cond: &TirExpr,
        then: &TirBlock,
        else_: &Option<TirElseBranch>,
        line: u32,
        true_id: Option<usize>,
        false_id: Option<usize>,
    ) -> bool {
        let n = count_clauses_tir(cond);
        if n <= 1 || self.mcdc.is_none() {
            return false;
        }
        let mut clauses: Vec<&TirExpr> = Vec::new();
        collect_clauses_tir(cond, &mut clauses);
        let coupled = crate::mvl::passes::mcdc::transform::detect_coupled_pairs_tir(
            &clauses,
            Some(&self.mcdc_fn_field_reads),
        );
        let Some(decision_id) = self.alloc_mcdc_decision(line, n, DecisionKind::If, coupled) else {
            return false;
        };

        // Emit clause value and eval-flag arrays (short-circuit — only observed clauses are set).
        self.line(&format!("let mut __d{decision_id}_c = [false; {n}];"));
        self.line(&format!("let mut __d{decision_id}_e = [false; {n}];"));

        // Emit outcome via short-circuit tree that sets c[i]/e[i] for each leaf when reached.
        self.indent();
        self.push(&format!("let __d{decision_id}_outcome: bool = "));
        self.emit_mcdc_sc_outcome(cond, decision_id, &mut 0, &mut 0);
        self.push(";");
        self.nl();

        // Emit observation record.
        self.emit_mcdc_record(decision_id, n);

        // Emit: if __d{id}_outcome { ... }
        self.indent();
        self.push(&format!("if __d{decision_id}_outcome {{"));
        self.nl();
        self.push_indent();
        if let Some(id) = true_id {
            self.emit_cov_hit(id);
        }
        self.emit_block_as_value(&then.stmts);
        self.pop_indent();
        self.indent();
        self.push("}");
        if let Some(else_branch) = else_ {
            self.push(" else ");
            self.emit_else_branch(else_branch, false_id);
        }
        self.nl();
        true
    }

    /// Emit a while-loop with MC/DC clause tracking, restructured as `loop { … }`.
    ///
    /// Returns `true` when MC/DC instrumentation was applied, `false` to fall back.
    fn emit_mcdc_while(
        &mut self,
        cond: &TirExpr,
        body: &TirBlock,
        line: u32,
        while_id: Option<usize>,
    ) -> bool {
        let n = count_clauses_tir(cond);
        if n <= 1 || self.mcdc.is_none() {
            return false;
        }
        let mut clauses: Vec<&TirExpr> = Vec::new();
        collect_clauses_tir(cond, &mut clauses);
        let coupled = crate::mvl::passes::mcdc::transform::detect_coupled_pairs_tir(
            &clauses,
            Some(&self.mcdc_fn_field_reads),
        );
        let Some(decision_id) = self.alloc_mcdc_decision(line, n, DecisionKind::While, coupled)
        else {
            return false;
        };

        // Restructure as `loop` so clause locals can be re-evaluated each iteration.
        self.indent();
        self.push("loop {");
        self.nl();
        self.push_indent();

        // Clause value and eval-flag arrays.
        self.line(&format!("let mut __d{decision_id}_c = [false; {n}];"));
        self.line(&format!("let mut __d{decision_id}_e = [false; {n}];"));

        // Outcome via short-circuit tree.
        self.indent();
        self.push(&format!("let __d{decision_id}_outcome: bool = "));
        self.emit_mcdc_sc_outcome(cond, decision_id, &mut 0, &mut 0);
        self.push(";");
        self.nl();

        // Record observation.
        self.emit_mcdc_record(decision_id, n);

        // Break when condition is false.
        self.line(&format!("if !__d{decision_id}_outcome {{ break; }}"));

        // Coverage hit (body entry).
        if let Some(id) = while_id {
            self.emit_cov_hit(id);
        }

        self.emit_block_stmts(&body.stmts);
        self.pop_indent();
        self.indent();
        self.push("}");
        self.nl();
        true
    }

    /// Emit MC/DC instrumentation for a compound boolean function-return expression.
    ///
    /// When a production function returns `Bool` and its body ends with a compound
    /// `&&`/`||` expression, this wraps the expression with clause arrays, the
    /// short-circuit evaluation tree, and an observation record — identical in
    /// structure to `emit_mcdc_if` but without any control-flow branching.
    ///
    /// Returns `true` when instrumentation was applied; `false` to fall back to
    /// normal emission (simple expression, non-Bool return, or MC/DC inactive).
    pub fn emit_mcdc_return_expr(&mut self, expr: &TirExpr, ret_ty: &Ty, line: u32) -> bool {
        let is_bool = matches!(ret_ty, Ty::Bool);
        if !is_bool {
            return false;
        }
        let n = count_clauses_tir(expr);
        if n <= 1 || self.mcdc.is_none() {
            return false;
        }
        let mut clauses: Vec<&TirExpr> = Vec::new();
        collect_clauses_tir(expr, &mut clauses);
        let coupled = crate::mvl::passes::mcdc::transform::detect_coupled_pairs_tir(
            &clauses,
            Some(&self.mcdc_fn_field_reads),
        );
        let Some(decision_id) = self.alloc_mcdc_decision(line, n, DecisionKind::Return, coupled)
        else {
            return false;
        };

        self.line(&format!("let mut __d{decision_id}_c = [false; {n}];"));
        self.line(&format!("let mut __d{decision_id}_e = [false; {n}];"));
        self.indent();
        self.push(&format!("let __d{decision_id}_outcome: bool = "));
        self.emit_mcdc_sc_outcome(expr, decision_id, &mut 0, &mut 0);
        self.push(";");
        self.nl();
        self.emit_mcdc_record(decision_id, n);
        self.indent();
        self.push(&format!("__d{decision_id}_outcome"));
        self.nl();
        true
    }

    /// Emit the short-circuit evaluation tree for a compound boolean TirExpr condition.
    ///
    /// Sets `__d{id}_c[i]` (clause value) and `__d{id}_e[i]` (evaluated flag) for
    /// each leaf clause *only when it is actually reached* by short-circuit execution.
    /// Emits a Rust block expression whose value is the overall boolean outcome.
    ///
    /// `clause_idx` counts leaf clauses (left-to-right); `tmp_idx` numbers internal
    /// temporaries so nested `&&`/`||` nodes use distinct names.
    fn emit_mcdc_sc_outcome(
        &mut self,
        expr: &TirExpr,
        decision_id: usize,
        clause_idx: &mut usize,
        tmp_idx: &mut usize,
    ) {
        if let TirExprKind::Binary {
            op, left, right, ..
        } = &expr.kind
        {
            if matches!(op, BinaryOp::And | BinaryOp::Or) {
                let t = *tmp_idx;
                *tmp_idx += 1;
                self.push("{");
                self.push(&format!(" let __d{decision_id}_t{t} = "));
                self.emit_mcdc_sc_outcome(left, decision_id, clause_idx, tmp_idx);
                self.push(";");
                if *op == BinaryOp::And {
                    self.push(&format!(" if __d{decision_id}_t{t} {{ "));
                    self.emit_mcdc_sc_outcome(right, decision_id, clause_idx, tmp_idx);
                    self.push(" } else { false }");
                } else {
                    self.push(&format!(" if __d{decision_id}_t{t} {{ true }} else {{ "));
                    self.emit_mcdc_sc_outcome(right, decision_id, clause_idx, tmp_idx);
                    self.push(" }");
                }
                self.push(" }");
                return;
            }
        }
        // Leaf: set eval flag, record value, return value.
        let i = *clause_idx;
        *clause_idx += 1;
        self.push(&format!(
            "{{ __d{decision_id}_e[{i}] = true; __d{decision_id}_c[{i}] = "
        ));
        self.emit_expr(expr);
        self.push(&format!("; __d{decision_id}_c[{i}] }}"));
    }

    /// Emit the `__mvl_mcdc::record(…)` call for a decision with `n` clauses.
    ///
    /// Encoding (u32): bits 0..n-1 = clause vals, bits n..2n-1 = eval flags, bit 2n = outcome.
    fn emit_mcdc_record(&mut self, decision_id: usize, n: usize) {
        let vals: Vec<String> = (0..n)
            .map(|i| format!("((__d{decision_id}_c[{i}] as u32) << {i}u32)"))
            .collect();
        let evals: Vec<String> = (0..n)
            .map(|i| format!("((__d{decision_id}_e[{i}] as u32) << {}u32)", n + i))
            .collect();
        let outcome = format!("((__d{decision_id}_outcome as u32) << {}u32)", 2 * n);
        let encoded = vals
            .into_iter()
            .chain(evals)
            .chain(std::iter::once(outcome))
            .collect::<Vec<_>>()
            .join(" | ");
        self.line(&format!(
            "#[cfg(test)] crate::__mvl_mcdc::record({decision_id}usize, {encoded});"
        ));
    }
}
