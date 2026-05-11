// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Block, statement, and expression-statement type checking for the MVL type checker.
//! Also includes field access, struct construction, and alias resolution helpers.

use crate::mvl::checker::context::{BorrowState, TypeBodyInfo};
use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::ifc;
use crate::mvl::checker::types::{resolve, types_compatible, Ty};
use crate::mvl::parser::ast::{
    Block, ElseBranch, Expr, LValue, LetKind, Pattern, SecurityLabel, Stmt, Totality,
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
        cond_label: Option<SecurityLabel>,
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
                                && !types_compatible(&resolved_ret, &last_ty)
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
                                .map(|rt| types_compatible(rt, &last_ty))
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
                                && !types_compatible(&resolved_ret, &last_ty)
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
                                .map(|rt| types_compatible(rt, &last_ty))
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
                                && !types_compatible(&resolved_ret, &last_ty)
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
        let cond_label = ifc::label_of(&cond_ty);
        let then_ty = self.infer_block_type(then, return_ty);
        self.check_branch_label_promotion(cond_label, &then_ty, return_ty, span);
        let result_ty = then_ty;
        if let Some(else_branch) = else_ {
            match else_branch {
                ElseBranch::Block(b) => {
                    let else_ty = self.infer_block_type(b, return_ty);
                    self.check_branch_label_promotion(cond_label, &else_ty, return_ty, span);
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
                        self.check_branch_label_promotion(cond_label, &nested_ty, return_ty, span);
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
                kind,
                pattern,
                ty,
                init,
                ..
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
                    self.check_borrow_lifetime(pattern, init);
                } else if !types_compatible(&ann_ty, &init_ty) {
                    self.emit(CheckError::TypeMismatch {
                        expected: ann_ty.display(),
                        found: init_ty.display(),
                        span: init.span(),
                    });
                }
                self.bind_pattern(
                    pattern,
                    &ann_ty,
                    matches!(kind, LetKind::Regular { mutable: true }),
                );
                // Phase D (#362): record which variable the new binding borrows so that
                // `pop_scope()` can release the borrow when the binding goes out of scope.
                // Also update the referent's borrow_state here (not in Expr::Borrow) so
                // that state is only set when borrows_var is simultaneously recorded.
                if let (Pattern::Ident(bound_name, _), Expr::Borrow { expr, mutable, .. }) =
                    (pattern, init)
                {
                    if let Expr::Ident(borrowed_name, _) = expr.as_ref() {
                        if let Some(bound_info) = self.env.lookup_mut_var(bound_name) {
                            bound_info.borrows_var = Some(borrowed_name.clone());
                        }
                        if let Some(referent) = self.env.lookup_mut_var(borrowed_name) {
                            referent.borrow_state = if *mutable {
                                BorrowState::MutablyBorrowed
                            } else {
                                match referent.borrow_state.clone() {
                                    BorrowState::SharedBorrowed(n) => {
                                        BorrowState::SharedBorrowed(n + 1)
                                    }
                                    _ => BorrowState::SharedBorrowed(1),
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
                self.check_assignment(target, &val_ty, *span);
            }

            Stmt::Return { value, span } => {
                if let Some(expr) = value {
                    let found = self.infer_expr(expr);
                    // Use `return_ty` if available; fall back to the function-level
                    // `current_return_ty` so that early `return` inside a for/while
                    // loop body is still checked against the function's return type.
                    let effective_ret = return_ty.or(self.current_return_ty.as_ref());
                    if let Some(ret) = effective_ret {
                        if !types_compatible(ret, &found) {
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
                let cond_label = ifc::label_of(&cond_ty);

                // Pass None: non-tail if-branch body types don't constrain the
                // function return. Early `return` inside branches uses
                // `current_return_ty` as fallback (see Stmt::Return above).
                let then_ty = self.infer_block_type(then, None);
                self.check_branch_label_promotion(cond_label, &then_ty, return_ty, *span);

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
                body,
                span,
            } => {
                // Req 8: `for` loops are bounded (total) — reject in `partial` functions.
                if matches!(self.current_fn_totality, Some(Totality::Partial)) {
                    self.emit(CheckError::ForLoopInPartialFn { span: *span });
                }
                let iter_ty = self.infer_expr(iter);
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
                cond, body, span, ..
            } => {
                let cond_ty = self.infer_expr(cond);
                if !cond_ty.is_bool() && !matches!(cond_ty, Ty::Unknown) {
                    self.emit(CheckError::TypeMismatch {
                        expected: "Bool".to_string(),
                        found: cond_ty.display(),
                        span: cond.span(),
                    });
                }
                // Req 8: reject `while` in total functions (only `for` is bounded).
                // Unannotated `fn` is implicitly total and also rejects while loops.
                if !matches!(self.current_fn_totality, Some(Totality::Partial)) {
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
                    if !types_compatible(&info.ty, val_ty) {
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
    /// Used for return-type and arithmetic checks where named aliases should be transparent.
    pub(super) fn resolve_alias(&self, ty: Ty) -> Ty {
        if let Ty::Named(ref name, _) = ty {
            if let Some(type_info) = self.env.lookup_type(name) {
                if let TypeBodyInfo::Alias(inner) = &type_info.body {
                    return inner.base().clone();
                }
            }
        }
        ty
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
                if let TypeBodyInfo::Struct(fields) = &type_info.body {
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
                if let TypeBodyInfo::Struct(fields) = &type_info.body {
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
                        TypeBodyInfo::Struct(fields) => {
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

        if let Some(type_info) = self.env.lookup_type(name).cloned() {
            match &type_info.body {
                TypeBodyInfo::Struct(declared_fields) => {
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
                    Ty::Named(name.to_string(), vec![])
                }
                TypeBodyInfo::Enum(_) => {
                    // Enum variant construction — name might be "EnumType::Variant"
                    // For now just return the type
                    Ty::Named(name.to_string(), vec![])
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
