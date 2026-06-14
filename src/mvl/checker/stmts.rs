// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Block, statement, and expression-statement type checking for the MVL type checker.
//! Also includes field access, struct construction, and alias resolution helpers.

use crate::mvl::checker::context::{CapabilityState, TypeBodyInfo, VariantFieldsInfo};
use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::ifc;
use crate::mvl::checker::types::{resolve, types_compatible, Ty};
use crate::mvl::parser::ast::{
    Block, ElseBranch, Expr, GenericParam, LValue, MatchArm, MatchBody, Pattern, Stmt, Totality,
};
use crate::mvl::parser::lexer::Span;

use super::TypeChecker;

impl TypeChecker {
    // ── Blocks and statements ─────────────────────────────────────────────

    /// Check whether `branch_ty` (the implicit return of one branch of an if-statement)
    /// needs to be promoted due to the condition's security label, and emit a TypeMismatch
    /// if the promoted type is incompatible with `return_ty`.
    ///
    /// Only fires when:
    /// - the condition carries a security label (`cond_label` is `Some`),
    /// - the function declares a concrete return type (`return_ty` is `Some`),
    /// - and the branch yields a non-Unit, non-Unknown result.
    pub(super) fn check_branch_label_promotion(
        &mut self,
        cond_label: Option<String>,
        branch_ty: &Ty,
        return_ty: Option<&Ty>,
        span: Span,
    ) {
        if let (Some(lbl), Some(ret)) = (cond_label, return_ty) {
            if !matches!(branch_ty.unlabeled(), Ty::Unit | Ty::Unknown) {
                let promoted = ifc::apply_label(Some(lbl), branch_ty.unlabeled().clone());
                if !matches!(promoted, Ty::Unknown) && !types_compatible(ret, &promoted) {
                    self.emit(CheckError::TypeMismatch {
                        expected: ret.display(),
                        found: promoted.display(),
                        span,
                    });
                }
            }
        }
    }

    pub(super) fn check_block(&mut self, block: &Block, expected_ty: Option<&Ty>) {
        self.env.push_scope();
        for stmt in &block.stmts {
            self.check_stmt(stmt, expected_ty);
        }
        self.env.pop_scope();
    }

    /// Check a block and return the type of its final expression (or Unit).
    ///
    /// Used for if-expression then-branches where the block's value matters.
    /// The last `Stmt::Expr` provides the block's type; earlier statements
    /// are checked normally. Unlike `check_block`, the final expression is
    /// NOT flagged as `ResultIgnored` because its value is consumed.
    pub(super) fn infer_block_type(&mut self, block: &Block, return_ty: Option<&Ty>) -> Ty {
        self.env.push_scope();
        let stmts = &block.stmts;
        let n = stmts.len();
        let mut last_ty = Ty::Unit;

        // An empty block implicitly returns Unit.  Reject non-Unit return types so
        // the transpiler never needs to emit todo!("empty body") (#990).
        if stmts.is_empty() {
            if let Some(ret) = return_ty {
                let resolved = self.resolve_alias(ret.clone());
                if !matches!(resolved, Ty::Unit | Ty::Unknown) {
                    self.emit(CheckError::TypeMismatch {
                        expected: ret.display(),
                        found: "Unit".to_string(),
                        span: block.span,
                    });
                }
            }
        }

        for (i, stmt) in stmts.iter().enumerate() {
            if i + 1 == n {
                // Tail-position statement: infer its type so the block propagates the
                // correct return value.  `match` and `if/else` in tail position produce
                // their arm/branch values, not Unit.
                match stmt {
                    Stmt::Expr { expr, .. } => {
                        last_ty = self.infer_expr(expr);

                        // Check implicit return type against declared return type.
                        // Resolve named alias types (e.g. PositiveInt = Int) before comparing
                        // so that `Int` is accepted where a refined alias is declared.
                        // For Result types, ResultIgnored below is the more specific error.
                        if let Some(ret) = return_ty {
                            let resolved_ret = self.resolve_alias(ret.clone());
                            if !matches!(last_ty, Ty::Unknown)
                                && !matches!(resolved_ret, Ty::Unknown)
                                && !last_ty.is_result()
                                && !self.types_compatible_resolved(&resolved_ret, &last_ty)
                            {
                                self.emit(CheckError::TypeMismatch {
                                    expected: ret.display(),
                                    found: last_ty.display(),
                                    span: expr.span(),
                                });
                            }
                        }

                        // Suppress ResultIgnored only when the block's expected return
                        // type is itself compatible with Result (the value is used).
                        // If the expected return type is Unit or incompatible, the
                        // caller is discarding the Result — emit ResultIgnored as usual.
                        if last_ty.is_result() {
                            let consumed_by_caller = return_ty
                                .map(|rt| self.types_compatible_resolved(rt, &last_ty))
                                .unwrap_or(false);
                            if !consumed_by_caller {
                                self.emit(CheckError::ResultIgnored { span: expr.span() });
                            }
                        }
                        break;
                    }

                    Stmt::Match {
                        scrutinee,
                        arms,
                        span,
                    } => {
                        // `match` in tail position: check arms and infer the block's type.
                        let scrutinee_ty = self.infer_expr(scrutinee);
                        last_ty = self.check_match_arms(arms, &scrutinee_ty, *span, return_ty);
                        if let Some(ret) = return_ty {
                            let resolved_ret = self.resolve_alias(ret.clone());
                            if !matches!(last_ty, Ty::Unknown)
                                && !matches!(resolved_ret, Ty::Unknown)
                                && !last_ty.is_result()
                                && !self.types_compatible_resolved(&resolved_ret, &last_ty)
                            {
                                self.emit(CheckError::TypeMismatch {
                                    expected: ret.display(),
                                    found: last_ty.display(),
                                    span: *span,
                                });
                            }
                        }
                        // Mirror the ResultIgnored check from Stmt::Expr: a tail match
                        // that produces an unhandled Result must still be flagged.
                        if last_ty.is_result() {
                            let consumed_by_caller = return_ty
                                .map(|rt| self.types_compatible_resolved(rt, &last_ty))
                                .unwrap_or(false);
                            if !consumed_by_caller {
                                self.emit(CheckError::ResultIgnored { span: *span });
                            }
                        }
                        break;
                    }

                    Stmt::If {
                        cond,
                        then,
                        else_,
                        span,
                    } => {
                        // `if/else` in tail position: delegate to helper so that
                        // `else if` chains are also inferred recursively.
                        last_ty = self.infer_tail_if(cond, then, else_, *span, return_ty);
                        // Check the overall result against the declared return type.
                        if let Some(ret) = return_ty {
                            let resolved_ret = self.resolve_alias(ret.clone());
                            if !matches!(last_ty, Ty::Unknown)
                                && !matches!(resolved_ret, Ty::Unknown)
                                && !last_ty.is_result()
                                && !self.types_compatible_resolved(&resolved_ret, &last_ty)
                            {
                                self.emit(CheckError::TypeMismatch {
                                    expected: ret.display(),
                                    found: last_ty.display(),
                                    span: *span,
                                });
                            }
                        }
                        break;
                    }

                    _ => {
                        // A tail `return` statement means the block always diverges
                        // and never falls through.  Use Unknown (the "skip" sentinel)
                        // so callers don't see a spurious `Unit` type — the return
                        // value's compatibility with `return_ty` is already checked
                        // inside `check_stmt` for `Stmt::Return`.
                        if matches!(stmt, Stmt::Return { .. }) {
                            last_ty = Ty::Unknown;
                        }
                        self.check_stmt(stmt, return_ty);
                        break;
                    }
                }
            }
            self.check_stmt(stmt, return_ty);
        }
        self.env.pop_scope();
        last_ty
    }

    /// Infer the type of an `if/else` in tail position, handling `else if` chains
    /// recursively so that every branch contributes to the block's return type.
    /// Returns the inferred type (the then-branch type when branches are compatible).
    pub(super) fn infer_tail_if(
        &mut self,
        cond: &Expr,
        then: &Block,
        else_: &Option<ElseBranch>,
        span: Span,
        return_ty: Option<&Ty>,
    ) -> Ty {
        let cond_ty = self.infer_expr(cond);
        if !cond_ty.is_bool() && !matches!(cond_ty, Ty::Unknown) {
            self.emit(CheckError::TypeMismatch {
                expected: "Bool".to_string(),
                found: cond_ty.display(),
                span: cond.span(),
            });
        }
        let cond_label = ifc::label_of(&cond_ty).map(|s| s.to_string());
        let then_ty = self.infer_block_type(then, return_ty);
        self.check_branch_label_promotion(cond_label.clone(), &then_ty, return_ty, span);
        let result_ty = then_ty;
        if let Some(else_branch) = else_ {
            match else_branch {
                ElseBranch::Block(b) => {
                    let else_ty = self.infer_block_type(b, return_ty);
                    self.check_branch_label_promotion(
                        cond_label.clone(),
                        &else_ty,
                        return_ty,
                        span,
                    );
                    if !matches!(result_ty, Ty::Unknown)
                        && !matches!(else_ty, Ty::Unknown)
                        && !types_compatible(&result_ty, &else_ty)
                    {
                        self.emit(CheckError::TypeMismatch {
                            expected: result_ty.display(),
                            found: else_ty.display(),
                            span,
                        });
                    }
                }
                ElseBranch::If(nested_if) => {
                    // `else if` chain: recurse so the nested if's type is also
                    // inferred and checked for compatibility with the then-branch.
                    if let Stmt::If {
                        cond: c,
                        then: t,
                        else_: e,
                        span: s,
                    } = nested_if.as_ref()
                    {
                        let nested_ty = self.infer_tail_if(c, t, e, *s, return_ty);
                        self.check_branch_label_promotion(
                            cond_label.clone(),
                            &nested_ty,
                            return_ty,
                            span,
                        );
                        if !matches!(result_ty, Ty::Unknown)
                            && !matches!(nested_ty, Ty::Unknown)
                            && !types_compatible(&result_ty, &nested_ty)
                        {
                            self.emit(CheckError::TypeMismatch {
                                expected: result_ty.display(),
                                found: nested_ty.display(),
                                span,
                            });
                        }
                    } else {
                        // Shouldn't happen by construction (ElseBranch::If always wraps
                        // Stmt::If), but fall back gracefully.
                        self.check_stmt(nested_if, return_ty);
                    }
                }
            }
        }
        result_ty
    }

    pub(super) fn check_stmt(&mut self, stmt: &Stmt, return_ty: Option<&Ty>) {
        match stmt {
            Stmt::Let {
                pattern, ty, init, ..
            } => {
                let init_ty = self.infer_expr(init);
                let ann_ty = resolve(ty);
                // Phase C (#305, #363): scope-depth check for any reference assignment.
                // Covers both implicit borrow (`let r: val T = x` where x: T) and explicit
                // borrow / ref-copy (`let r: val T = val x` or `let r: val T = existing_ref`).
                let is_ref_assignment = if let Ty::Ref(_, inner_ty) = &ann_ty {
                    types_compatible(inner_ty, &init_ty) || types_compatible(&ann_ty, &init_ty)
                } else {
                    false
                };
                if is_ref_assignment {
                    self.check_capability_scope(pattern, init);
                } else if !self.types_compatible_resolved(&ann_ty, &init_ty) {
                    self.emit(CheckError::TypeMismatch {
                        expected: ann_ty.display(),
                        found: init_ty.display(),
                        span: init.span(),
                    });
                } else if ann_ty.is_linear_in_env(&self.env.types) {
                    // Move semantics (Spec 001 Req 4): `let t: T = s` moves ownership
                    // from `s` to `t`, making `s` unavailable. No `consume()` required —
                    // consume() is only for `iso` capability transfers (Spec 014 Req 2).
                    if let Expr::Ident(src, _) = init {
                        self.env.mark_moved(src);
                    }
                }
                // Gap 2 (#1068): check if this binding shadows a live linear value.
                // Skip if the init expression references the shadowed name — the old
                // value is being consumed by the initializer (builder/accumulator pattern).
                if let Pattern::Ident(name, _) = pattern {
                    if let Some(info) = self.env.lookup(name) {
                        if !info.moved
                            && info.ty.is_linear_in_env(&self.env.types)
                            && !expr_references_name(init, name)
                        {
                            self.emit(CheckError::LinearShadowDrop {
                                name: name.clone(),
                                ty: info.ty.display(),
                                span: init.span(),
                            });
                        }
                    }
                }
                // Mutability is encoded in the type: `ref T` = mutable, everything else = immutable.
                // Strip the `ref`/`val` wrapper so the binding carries the inner type T —
                // reading `x: ref T` in expressions should yield T, not ref T.
                let is_mutable =
                    matches!(ann_ty.base(), crate::mvl::checker::types::Ty::Ref(true, _));
                let bind_ty = match &ann_ty {
                    Ty::Ref(_, inner) => (**inner).clone(),
                    other => other.clone(),
                };
                self.bind_pattern(pattern, &bind_ty, is_mutable);
                // Phase D (#362, #660): record which variable the new binding borrows so that
                // `pop_scope()` can release the borrow when the binding goes out of scope.
                // Also update the referent's capability_state here (not in Expr::Borrow) so
                // that state is only set when ref_var is simultaneously recorded.
                //
                // Two cases:
                //  (a) Explicit borrow: `let v: val T = val x` — init is Expr::Borrow.
                //      Aliasing is already checked in Expr::Borrow (infer.rs).
                //  (b) Implicit borrow: `let v: val T = x` — init is Expr::Ident.
                //      Aliasing must be checked here; state transitions are the same.
                if let (Pattern::Ident(bound_name, _), Expr::Borrow { expr, mutable, .. }) =
                    (pattern, init)
                {
                    if let Expr::Ident(borrowed_name, _) = expr.as_ref() {
                        if let Some(bound_info) = self.env.lookup_mut_var(bound_name) {
                            bound_info.ref_var = Some(borrowed_name.clone());
                        }
                        if let Some(referent) = self.env.lookup_mut_var(borrowed_name) {
                            referent.capability_state = if *mutable {
                                CapabilityState::Ref
                            } else {
                                match referent.capability_state.clone() {
                                    CapabilityState::Val(n) => CapabilityState::Val(n + 1),
                                    _ => CapabilityState::Val(1),
                                }
                            };
                        }
                    }
                } else if is_ref_assignment {
                    // Case (b): implicit borrow driven by type annotation.
                    if let (
                        Pattern::Ident(bound_name, _),
                        Expr::Ident(borrowed_name, borrow_span),
                    ) = (pattern, init)
                    {
                        let is_mutable = matches!(ann_ty, Ty::Ref(true, _));
                        // Check aliasing violation before updating state.
                        let current = self
                            .env
                            .lookup(borrowed_name)
                            .map(|i| i.capability_state.clone())
                            .unwrap_or(CapabilityState::Owned);
                        if is_mutable {
                            if current != CapabilityState::Owned {
                                self.emit(CheckError::AliasingMutableBorrow {
                                    name: borrowed_name.clone(),
                                    span: *borrow_span,
                                });
                            }
                        } else if matches!(current, CapabilityState::Ref) {
                            self.emit(CheckError::AliasingMutableBorrow {
                                name: borrowed_name.clone(),
                                span: *borrow_span,
                            });
                        }
                        // Record ref_var so pop_scope() can release the capability.
                        if let Some(bound_info) = self.env.lookup_mut_var(bound_name) {
                            bound_info.ref_var = Some(borrowed_name.clone());
                        }
                        // Transition the referent's capability state.
                        if let Some(referent) = self.env.lookup_mut_var(borrowed_name) {
                            referent.capability_state = if is_mutable {
                                CapabilityState::Ref
                            } else {
                                match referent.capability_state.clone() {
                                    CapabilityState::Val(n) => CapabilityState::Val(n + 1),
                                    _ => CapabilityState::Val(1),
                                }
                            };
                        }
                    }
                }
                // #14: ResultIgnored — if the init expression is a Result and
                // it's not being used at all, that would be caught at Stmt::Expr.
                // Here the Result is being bound, which is acceptable.
            }

            // #17: immutability enforcement
            Stmt::Assign {
                target,
                value,
                span,
            } => {
                let val_ty = self.infer_expr(value);
                // Move semantics: reassignment of linear type marks source as moved.
                if val_ty.is_linear_in_env(&self.env.types) {
                    if let Expr::Ident(src, _) = value {
                        self.env.mark_moved(src);
                    }
                }
                self.check_assignment(target, &val_ty, *span);
            }

            Stmt::Return { value, span } => {
                if let Some(expr) = value {
                    let found = self.infer_expr(expr);
                    // Use `return_ty` if available; fall back to the function-level
                    // `current_return_ty` so that early `return` inside a for/while
                    // loop body is still checked against the function's return type.
                    let effective_ret = return_ty.or(self.fn_context().return_ty.as_ref());
                    if let Some(ret) = effective_ret {
                        if !self.types_compatible_resolved(ret, &found) {
                            self.emit(CheckError::TypeMismatch {
                                expected: ret.display(),
                                found: found.display(),
                                span: *span,
                            });
                        }
                    }
                }
            }

            Stmt::If {
                cond,
                then,
                else_,
                span,
            } => {
                let cond_ty = self.infer_expr(cond);
                if !cond_ty.is_bool() && !matches!(cond_ty, Ty::Unknown) {
                    self.emit(CheckError::TypeMismatch {
                        expected: "Bool".to_string(),
                        found: cond_ty.display(),
                        span: cond.span(),
                    });
                }
                // Extract the condition's security label for implicit return-type promotion.
                // Branching on Secret<Bool> or Tainted<Bool> means the choice of branch
                // reveals the condition's value; non-Unit results must be promoted.
                let cond_label = ifc::label_of(&cond_ty).map(|s| s.to_string());

                // Pass None: non-tail if-branch body types don't constrain the
                // function return. Early `return` inside branches uses
                // `current_return_ty` as fallback (see Stmt::Return above).
                let then_ty = self.infer_block_type(then, None);
                self.check_branch_label_promotion(cond_label.clone(), &then_ty, return_ty, *span);

                if let Some(else_branch) = else_ {
                    match else_branch {
                        ElseBranch::Block(b) => {
                            let else_ty = self.infer_block_type(b, None);
                            self.check_branch_label_promotion(
                                cond_label, &else_ty, return_ty, *span,
                            );
                        }
                        ElseBranch::If(s) => self.check_stmt(s, None),
                    }
                }
            }

            Stmt::Match {
                scrutinee,
                arms,
                span,
            } => {
                let scrutinee_ty = self.infer_expr(scrutinee);
                // Pass None: non-tail match arm bodies don't constrain the function
                // return. Early `return` in arms uses `current_return_ty` fallback.
                self.check_match_arms(arms, &scrutinee_ty, *span, None);
            }

            Stmt::For {
                pattern,
                iter,
                invariants,
                body,
                span: _,
            } => {
                let iter_ty = self.infer_expr(iter);
                for inv in invariants {
                    self.infer_expr(inv);
                }
                let iter_span = iter.span();
                let elem_ty = self.check_iterator_type(&iter_ty, iter_span);
                self.env.push_scope();
                self.bind_pattern(pattern, &elem_ty, false);
                // Pass None: the loop body's tail type doesn't constrain the
                // function return; early `return` inside the body uses
                // `current_return_ty` as fallback in Stmt::Return.
                self.check_block(body, None);
                self.env.pop_scope();
            }

            Stmt::While {
                cond,
                invariants,
                decreases,
                body,
                span,
            } => {
                let cond_ty = self.infer_expr(cond);
                if !cond_ty.is_bool() && !matches!(cond_ty, Ty::Unknown) {
                    self.emit(CheckError::TypeMismatch {
                        expected: "Bool".to_string(),
                        found: cond_ty.display(),
                        span: cond.span(),
                    });
                }
                // Record types for invariant and decreases expressions so TIR lowering can find them.
                for inv in invariants {
                    self.infer_expr(inv);
                }
                if let Some(dec) = decreases {
                    self.infer_expr(dec);
                }
                // Req 8: reject `while` in total functions unless a `decreases` measure
                // is provided (Phase 5, #628 — decreases enables bounded while loops).
                if !matches!(self.fn_context().totality, Some(Totality::Partial))
                    && decreases.is_none()
                {
                    self.emit(CheckError::UnboundedLoopInTotal { span: *span });
                }
                // Same reasoning as Stmt::For: loop body tail type ≠ fn return type.
                self.check_block(body, None);
            }

            // #14: Reject bare Result expressions (ResultIgnored)
            Stmt::Expr { expr, .. } => {
                let ty = self.infer_expr(expr);
                if ty.is_result() {
                    self.emit(CheckError::ResultIgnored { span: expr.span() });
                }
            }
        }
    }

    // ── Assignment target (#17 immutability) ─────────────────────────────

    pub(super) fn check_assignment(&mut self, target: &LValue, val_ty: &Ty, span: Span) {
        match target {
            LValue::Ident(name, _) => {
                if let Some(info) = self.env.lookup(name).cloned() {
                    if !info.mutable {
                        self.emit(CheckError::AssignToImmutable {
                            name: name.clone(),
                            span,
                        });
                    }
                    // #17: also verify the assigned value is type-compatible
                    if !self.types_compatible_resolved(&info.ty, val_ty) {
                        self.emit(CheckError::TypeMismatch {
                            expected: info.ty.display(),
                            found: val_ty.display(),
                            span,
                        });
                    }
                } else {
                    self.emit(CheckError::UndefinedVariable {
                        name: name.clone(),
                        span,
                    });
                }
            }
            LValue::Field {
                base,
                field,
                span: field_span,
            } => {
                let base_ty = self.infer_lvalue(base);
                // Check that the specific field is mutable.
                self.check_field_mutation(&base_ty, field, *field_span);
                // Check the assigned value against the FIELD type (not the base struct type).
                // Recursing with val_ty into check_assignment on the base would incorrectly
                // compare the base struct type against the field value type.
                let field_ty = self.field_type(&base_ty, field).unwrap_or(Ty::Unknown);
                if !matches!(field_ty, Ty::Unknown) && !types_compatible(&field_ty, val_ty) {
                    self.emit(CheckError::TypeMismatch {
                        expected: field_ty.display(),
                        found: val_ty.display(),
                        span,
                    });
                }
            }
        }
    }

    /// Resolve a named type through the type environment if it is a type alias.
    /// Returns the alias base type (with Refined stripped), or the original type if not an alias.
    /// Recursively resolves chained aliases (e.g. `Port → PositiveInt → Int`).
    /// Used for return-type and arithmetic checks where named aliases should be transparent.
    pub(super) fn resolve_alias(&self, ty: Ty) -> Ty {
        if let Ty::Named(ref name, _) = ty {
            if let Some(type_info) = self.env.lookup_type(name) {
                if let TypeBodyInfo::Alias(inner) = &type_info.body {
                    // Recurse to resolve chained aliases.
                    return self.resolve_alias(inner.base().clone());
                }
            }
        }
        ty
    }

    /// Type compatibility that sees through type aliases (#1324).
    ///
    /// A refined type alias like `Port = Int where ...` is treated as structurally
    /// compatible with its base type `Int` in both directions:
    /// - `Int` where `Port` expected: allowed (refinement checked separately)
    /// - `Port` where `Int` expected: always safe (widening)
    pub(super) fn types_compatible_resolved(&self, expected: &Ty, found: &Ty) -> bool {
        if types_compatible(expected, found) {
            return true;
        }
        let expected_resolved = self.resolve_alias(expected.clone());
        let found_resolved = self.resolve_alias(found.clone());
        types_compatible(&expected_resolved, &found_resolved)
    }

    /// Resolve named aliases inside a Labeled wrapper.
    /// E.g. `Public<Amount>` where `Amount = Float where ...` → `Public<Float>`.
    pub(super) fn resolve_alias_in_labeled(&self, ty: Ty) -> Ty {
        if let Ty::Labeled(label, inner) = ty {
            Ty::Labeled(label, Box::new(self.resolve_alias(*inner)))
        } else {
            self.resolve_alias(ty)
        }
    }

    pub(super) fn infer_lvalue(&self, target: &LValue) -> Ty {
        match target {
            LValue::Ident(name, _) => self
                .env
                .lookup(name)
                .map(|i| i.ty.clone())
                .unwrap_or(Ty::Unknown),
            LValue::Field { base, field, .. } => {
                let base_ty = self.infer_lvalue(base);
                self.field_type(&base_ty, field).unwrap_or(Ty::Unknown)
            }
        }
    }

    pub(super) fn check_field_mutation(&mut self, ty: &Ty, field: &str, span: Span) {
        let base = ty.unlabeled();
        if let Ty::Named(name, _) = base {
            if let Some(type_info) = self.env.lookup_type(name).cloned() {
                if let TypeBodyInfo::Struct { fields, .. } = &type_info.body {
                    if let Some(fi) = fields.iter().find(|f| f.name == field) {
                        if !fi.mutable {
                            self.emit(CheckError::MutateImmutableField {
                                ty: name.clone(),
                                field: field.to_string(),
                                span,
                            });
                        }
                    }
                }
            }
        }
    }

    // ── Field access (#12) ────────────────────────────────────────────────

    /// Look up a field type without emitting errors.
    pub(super) fn field_type(&self, ty: &Ty, field: &str) -> Option<Ty> {
        let base = ty.unlabeled();
        if let Ty::Named(name, _) = base {
            if let Some(type_info) = self.env.lookup_type(name) {
                if let TypeBodyInfo::Struct { fields, .. } = &type_info.body {
                    return fields
                        .iter()
                        .find(|f| f.name == field)
                        .map(|f| f.ty.clone());
                }
            }
        }
        None
    }

    /// Look up a field type, emitting errors for violations.
    pub(super) fn field_type_checked(&mut self, ty: &Ty, field: &str, span: Span) -> Ty {
        let base = ty.unlabeled().clone();
        match &base {
            Ty::Named(name, _) => {
                if let Some(type_info) = self.env.lookup_type(name).cloned() {
                    match &type_info.body {
                        TypeBodyInfo::Struct { fields, .. } => {
                            if let Some(fi) = fields.iter().find(|f| f.name == field) {
                                fi.ty.clone()
                            } else {
                                self.emit(CheckError::FieldNotFound {
                                    ty: name.clone(),
                                    field: field.to_string(),
                                    span,
                                });
                                Ty::Unknown
                            }
                        }
                        TypeBodyInfo::Enum(_) => {
                            self.emit(CheckError::FieldAccessOnEnum {
                                ty: name.clone(),
                                span,
                            });
                            Ty::Unknown
                        }
                        TypeBodyInfo::Alias(inner) => {
                            self.field_type_checked(&inner.clone(), field, span)
                        }
                    }
                } else {
                    // Unknown named type — already reported elsewhere
                    Ty::Unknown
                }
            }
            Ty::Unknown => Ty::Unknown,
            other => {
                self.emit(CheckError::FieldNotFound {
                    ty: other.display(),
                    field: field.to_string(),
                    span,
                });
                Ty::Unknown
            }
        }
    }

    // ── Struct construction (#12) ─────────────────────────────────────────

    pub(super) fn check_construction(
        &mut self,
        name: &str,
        fields: &[(String, Expr)],
        span: Span,
    ) -> Ty {
        // Infer all provided field values
        let provided: Vec<(String, Ty)> = fields
            .iter()
            .map(|(fname, fexpr)| (fname.clone(), self.infer_expr(fexpr)))
            .collect();

        // Resolve the lookup name: for "EnumType::Variant { … }" construction,
        // split on "::" and look up the base enum type.
        let (lookup_name, variant_name) = if let Some((base, var)) = name.split_once("::") {
            (base, Some(var))
        } else {
            (name, None)
        };

        if let Some(type_info) = self.env.lookup_type(lookup_name).cloned() {
            match &type_info.body {
                TypeBodyInfo::Struct {
                    fields: declared_fields,
                    ..
                } => {
                    // Check that all declared fields are provided
                    for df in declared_fields.iter() {
                        if !provided.iter().any(|(pname, _)| pname == &df.name) {
                            self.emit(CheckError::MissingField {
                                ty: name.to_string(),
                                field: df.name.clone(),
                                span,
                            });
                        }
                    }
                    // Check no extra fields are provided
                    for (pname, pty) in &provided {
                        if let Some(df) = declared_fields.iter().find(|f| &f.name == pname) {
                            if !types_compatible(&df.ty, pty) {
                                self.emit(CheckError::TypeMismatch {
                                    expected: df.ty.display(),
                                    found: pty.display(),
                                    span,
                                });
                            }
                        } else {
                            self.emit(CheckError::UnknownField {
                                ty: name.to_string(),
                                field: pname.clone(),
                                span,
                            });
                        }
                    }
                    Ty::Named(lookup_name.to_string(), vec![])
                }
                TypeBodyInfo::Enum(variants) => {
                    // Named-field enum variant construction: "EnumType::Variant { field: val }"
                    // Find the variant and type-check its fields.
                    if let Some(var_name) = variant_name {
                        if let Some(vi) = variants.iter().find(|v| v.name == var_name) {
                            if let VariantFieldsInfo::Struct(declared_fields) = &vi.fields {
                                // Infer generic type params from the provided field values.
                                let param_names: Vec<String> = type_info
                                    .params
                                    .iter()
                                    .map(GenericParam::name)
                                    .map(str::to_string)
                                    .collect();
                                let mut subst = std::collections::HashMap::<String, Ty>::new();
                                for df in declared_fields.iter() {
                                    if let Some((_, pty)) =
                                        provided.iter().find(|(pn, _)| pn == &df.name)
                                    {
                                        infer_type_param(&param_names, &df.ty, pty, &mut subst);
                                    }
                                }

                                for df in declared_fields.iter() {
                                    if !provided.iter().any(|(pname, _)| pname == &df.name) {
                                        self.emit(CheckError::MissingField {
                                            ty: name.to_string(),
                                            field: df.name.clone(),
                                            span,
                                        });
                                    }
                                }
                                for (pname, pty) in &provided {
                                    if let Some(df) =
                                        declared_fields.iter().find(|f| &f.name == pname)
                                    {
                                        let expected = apply_subst(&df.ty, &param_names, &subst);
                                        if !types_compatible(&expected, pty) {
                                            self.emit(CheckError::TypeMismatch {
                                                expected: expected.display(),
                                                found: pty.display(),
                                                span,
                                            });
                                        }
                                    } else {
                                        self.emit(CheckError::UnknownField {
                                            ty: name.to_string(),
                                            field: pname.clone(),
                                            span,
                                        });
                                    }
                                }

                                // Return the enum type with inferred type args.
                                let type_args: Vec<Ty> = param_names
                                    .iter()
                                    .map(|n| subst.get(n).cloned().unwrap_or(Ty::Unknown))
                                    .collect();
                                return Ty::Named(lookup_name.to_string(), type_args);
                            }
                        }
                    }
                    Ty::Named(lookup_name.to_string(), vec![])
                }
                TypeBodyInfo::Alias(inner) => inner.clone(),
            }
        } else {
            // Unknown type
            self.emit(CheckError::UndefinedType {
                name: name.to_string(),
                span,
            });
            Ty::Unknown
        }
    }
}

// ── Free helpers ────────────────────────────────────────────────────────────

/// Returns `true` if `expr` contains a reference to `name` (shallow walk).
///
/// Used by shadow-drop detection: `let x: T = f(x)` should NOT trigger a
/// shadow-drop error because `x` is consumed by `f(x)` in the initializer.
fn expr_references_name(expr: &Expr, name: &str) -> bool {
    match expr {
        Expr::Ident(n, _) => n == name,
        Expr::FnCall { args, .. } => args.iter().any(|a| expr_references_name(a, name)),
        Expr::MethodCall { receiver, args, .. } => {
            expr_references_name(receiver, name)
                || args.iter().any(|a| expr_references_name(a, name))
        }
        Expr::FieldAccess { expr: inner, .. }
        | Expr::Consume { expr: inner, .. }
        | Expr::Unary { expr: inner, .. }
        | Expr::Propagate { expr: inner, .. }
        | Expr::Borrow { expr: inner, .. }
        | Expr::Relabel { expr: inner, .. } => expr_references_name(inner, name),
        Expr::Binary { left, right, .. } => {
            expr_references_name(left, name) || expr_references_name(right, name)
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            expr_references_name(cond, name)
                || block_references_name(then, name)
                || else_
                    .as_ref()
                    .is_some_and(|e| expr_references_name(e, name))
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            expr_references_name(scrutinee, name)
                || arms.iter().any(|a| match_arm_references_name(a, name))
        }
        Expr::Block(b) => block_references_name(b, name),
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            elems.iter().any(|e| expr_references_name(e, name))
        }
        Expr::Map { pairs, .. } => pairs
            .iter()
            .any(|(k, v)| expr_references_name(k, name) || expr_references_name(v, name)),
        Expr::Construct { fields, .. } | Expr::Spawn { fields, .. } => {
            fields.iter().any(|(_, v)| expr_references_name(v, name))
        }
        Expr::Lambda { .. } => false, // lambdas capture, don't consume
        _ => false,
    }
}

fn block_references_name(block: &Block, name: &str) -> bool {
    block.stmts.iter().any(|s| stmt_references_name(s, name))
}

fn stmt_references_name(stmt: &Stmt, name: &str) -> bool {
    match stmt {
        Stmt::Expr { expr, .. } => expr_references_name(expr, name),
        Stmt::Return { value, .. } => value
            .as_ref()
            .is_some_and(|e| expr_references_name(e, name)),
        Stmt::Let { init, .. } => expr_references_name(init, name),
        Stmt::Assign { value, .. } => expr_references_name(value, name),
        Stmt::If {
            cond, then, else_, ..
        } => {
            expr_references_name(cond, name)
                || block_references_name(then, name)
                || else_.as_ref().is_some_and(|eb| match eb {
                    ElseBranch::Block(b) => block_references_name(b, name),
                    ElseBranch::If(s) => stmt_references_name(s, name),
                })
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            expr_references_name(scrutinee, name)
                || arms.iter().any(|a| match_arm_references_name(a, name))
        }
        _ => false,
    }
}

fn match_arm_references_name(arm: &MatchArm, name: &str) -> bool {
    match &arm.body {
        MatchBody::Expr(e) => expr_references_name(e, name),
        MatchBody::Block(b) => block_references_name(b, name),
    }
}

/// Infer a single generic type-parameter binding from a declared/actual type pair.
///
/// Handles the common case of `T` (bare parameter) and recursively handles
/// compound declared types like `List[T]`, `Option[T]`, etc.
fn infer_type_param(
    params: &[String],
    declared: &Ty,
    actual: &Ty,
    subst: &mut std::collections::HashMap<String, Ty>,
) {
    match declared {
        Ty::Named(name, args) if args.is_empty() && params.contains(name) => {
            subst.entry(name.clone()).or_insert_with(|| actual.clone());
        }
        Ty::Named(_, decl_args) => {
            if let Ty::Named(_, actual_args) = actual {
                for (d, a) in decl_args.iter().zip(actual_args.iter()) {
                    infer_type_param(params, d, a, subst);
                }
            }
        }
        Ty::List(elem) => {
            if let Ty::List(aelem) = actual {
                infer_type_param(params, elem, aelem, subst);
            }
        }
        Ty::Option(inner) => {
            if let Ty::Option(ainner) = actual {
                infer_type_param(params, inner, ainner, subst);
            }
        }
        _ => {}
    }
}

/// Substitute inferred type parameters in a declared type.
fn apply_subst(ty: &Ty, params: &[String], subst: &std::collections::HashMap<String, Ty>) -> Ty {
    match ty {
        Ty::Named(name, args) if args.is_empty() && params.contains(name) => {
            subst.get(name.as_str()).cloned().unwrap_or(Ty::Unknown)
        }
        Ty::Named(name, args) => Ty::Named(
            name.clone(),
            args.iter().map(|a| apply_subst(a, params, subst)).collect(),
        ),
        Ty::List(elem) => Ty::List(Box::new(apply_subst(elem, params, subst))),
        Ty::Option(inner) => Ty::Option(Box::new(apply_subst(inner, params, subst))),
        Ty::Result(ok, err) => Ty::Result(
            Box::new(apply_subst(ok, params, subst)),
            Box::new(apply_subst(err, params, subst)),
        ),
        _ => ty.clone(),
    }
}
