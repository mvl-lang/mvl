// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Pattern matching and exhaustiveness checking for the MVL type checker.

use crate::mvl::checker::context::{TypeBodyInfo, VarInfo, VariantFieldsInfo};
use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::types::Ty;
use crate::mvl::parser::ast::{MatchArm, MatchBody, Pattern, RefExpr};
use crate::mvl::parser::lexer::Span;

use super::TypeChecker;

impl TypeChecker {
    // ── Match exhaustiveness (#13) ────────────────────────────────────────

    pub(super) fn infer_match_expr(
        &mut self,
        arms: &[MatchArm],
        scrutinee_ty: &Ty,
        span: Span,
    ) -> Ty {
        self.check_match_arms(arms, scrutinee_ty, span, None)
    }

    /// Check match arms for exhaustiveness and return the result type.
    pub(super) fn check_match_arms(
        &mut self,
        arms: &[MatchArm],
        scrutinee_ty: &Ty,
        span: Span,
        return_ty: Option<&Ty>,
    ) -> Ty {
        // Check each arm body
        let mut arm_tys: Vec<Ty> = Vec::new();
        for arm in arms {
            self.env.push_scope();
            self.bind_match_pattern(&arm.pattern, scrutinee_ty);
            // Validate guard expression variables are in scope (#938).
            if let Some(guard) = &arm.guard {
                self.check_guard_ref_expr(guard);
            }
            let body_ty = match &arm.body {
                MatchBody::Expr(e) => self.infer_expr(e),
                // Use infer_block_type so the last Stmt::Expr is treated as
                // the arm's return value rather than a discarded statement.
                // This prevents false ResultIgnored errors on Ok(...)/Err(...)
                // that appear at the end of match arm blocks.
                MatchBody::Block(b) => self.infer_block_type(b, return_ty),
            };
            self.env.pop_scope();
            arm_tys.push(body_ty);
        }

        // Exhaustiveness check
        self.check_exhaustiveness(arms, scrutinee_ty, span);

        arm_tys
            .into_iter()
            .find(|t| !matches!(t, Ty::Unknown))
            .unwrap_or(Ty::Unknown)
    }

    pub(super) fn check_exhaustiveness(
        &mut self,
        arms: &[MatchArm],
        scrutinee_ty: &Ty,
        span: Span,
    ) {
        let base = scrutinee_ty.unlabeled().clone();

        // Guarded arms don't guarantee coverage — a guard may fail, so
        // only unguarded arms count toward exhaustiveness (#938).
        let unguarded: Vec<&MatchArm> = arms.iter().filter(|a| a.guard.is_none()).collect();

        match &base {
            // Option<T>: must cover Some(_) and None
            Ty::Option(_) => {
                // A bare `_` or non-Option-variant ident is a wildcard → exhaustive
                if unguarded
                    .iter()
                    .any(|a| is_wildcard_pattern(&a.pattern, &[]))
                {
                    return;
                }
                let has_some = unguarded.iter().any(|a| {
                    matches!(a.pattern, Pattern::Some { .. })
                        || matches!(&a.pattern, Pattern::TupleStruct { name, .. } if name == "Some")
                });
                let has_none = unguarded.iter().any(|a| {
                    matches!(a.pattern, Pattern::None(_))
                        || matches!(&a.pattern, Pattern::Ident(n, _) if n == "None")
                });
                let mut missing = Vec::new();
                if !has_some {
                    missing.push("Some(_)".to_string());
                }
                if !has_none {
                    missing.push("None".to_string());
                }
                if !missing.is_empty() {
                    self.emit(CheckError::NonExhaustiveMatch { missing, span });
                }
            }

            // Result<T,E>: must cover Ok(_) and Err(_)
            Ty::Result(_, _) => {
                if unguarded
                    .iter()
                    .any(|a| is_wildcard_pattern(&a.pattern, &[]))
                {
                    return;
                }
                let has_ok = unguarded.iter().any(|a| {
                    matches!(a.pattern, Pattern::Ok { .. })
                        || matches!(&a.pattern, Pattern::TupleStruct { name, .. } if name == "Ok")
                });
                let has_err = unguarded.iter().any(|a| {
                    matches!(a.pattern, Pattern::Err { .. })
                        || matches!(&a.pattern, Pattern::TupleStruct { name, .. } if name == "Err")
                });
                let mut missing = Vec::new();
                if !has_ok {
                    missing.push("Ok(_)".to_string());
                }
                if !has_err {
                    missing.push("Err(_)".to_string());
                }
                if !missing.is_empty() {
                    self.emit(CheckError::NonExhaustiveMatch { missing, span });
                }
            }

            // Named enum: collect which variants are covered
            Ty::Named(name, _) => {
                if let Some(type_info) = self.env.lookup_type(name).cloned() {
                    if let TypeBodyInfo::Enum(variants) = &type_info.body {
                        let variant_names: Vec<String> =
                            variants.iter().map(|v| v.name.clone()).collect();

                        // A wildcard is any Pattern::Wildcard OR a bare ident not in the enum's variants
                        if unguarded
                            .iter()
                            .any(|a| is_wildcard_pattern(&a.pattern, &variant_names))
                        {
                            return;
                        }

                        // Collect which variant names are explicitly covered
                        let covered: Vec<String> = unguarded
                            .iter()
                            .filter_map(|arm| covered_variant_name(&arm.pattern, &variant_names))
                            .collect();

                        let missing: Vec<String> = variant_names
                            .iter()
                            .filter(|v| !covered.contains(v))
                            .cloned()
                            .collect();
                        if !missing.is_empty() {
                            self.emit(CheckError::NonExhaustiveMatch { missing, span });
                        }
                    }
                }
                // Unknown type or non-enum → no exhaustiveness check
            }

            _ => {} // literals, bools, tuples — skip exhaustiveness
        }
    }

    // ── Pattern binding ───────────────────────────────────────────────────

    pub(super) fn bind_pattern(&mut self, pattern: &Pattern, ty: &Ty, mutable: bool) {
        match pattern {
            Pattern::Ident(name, _) => {
                self.env
                    .define(name.clone(), VarInfo::new(ty.clone(), mutable));
            }
            Pattern::Wildcard(_) => {}
            Pattern::Tuple { elems, .. } => {
                if let Ty::Tuple(elem_tys) = ty.unlabeled() {
                    for (p, t) in elems.iter().zip(elem_tys.iter()) {
                        self.bind_pattern(p, t, mutable);
                    }
                } else {
                    for p in elems {
                        self.bind_pattern(p, &Ty::Unknown, mutable);
                    }
                }
            }
            Pattern::Literal(_, _) => {}
            _ => {
                // For struct/tuple-struct patterns, just bind sub-patterns as Unknown
                self.bind_sub_patterns(pattern, mutable);
            }
        }
    }

    pub(super) fn bind_match_pattern(&mut self, pattern: &Pattern, scrutinee_ty: &Ty) {
        match pattern {
            Pattern::Ident(name, _) => {
                self.env
                    .define(name.clone(), VarInfo::new(scrutinee_ty.clone(), false));
            }
            Pattern::Wildcard(_) | Pattern::Literal(_, _) | Pattern::None(_) => {}
            Pattern::Some { inner, .. } => {
                let inner_ty = match scrutinee_ty.unlabeled() {
                    Ty::Option(t) => *t.clone(),
                    _ => Ty::Unknown,
                };
                self.bind_match_pattern(inner, &inner_ty);
            }
            Pattern::Ok { inner, .. } => {
                let inner_ty = match scrutinee_ty.unlabeled() {
                    Ty::Result(ok, _) => *ok.clone(),
                    _ => Ty::Unknown,
                };
                self.bind_match_pattern(inner, &inner_ty);
            }
            Pattern::Err { inner, .. } => {
                let inner_ty = match scrutinee_ty.unlabeled() {
                    Ty::Result(_, err) => *err.clone(),
                    _ => Ty::Unknown,
                };
                self.bind_match_pattern(inner, &inner_ty);
            }
            Pattern::TupleStruct { name, fields, .. } => {
                // Look up the enum variant to get concrete field types so that
                // function-typed fields (e.g. `Filtered(lo, hi, pred: fn(Int)->Bool)`)
                // are bound with the correct type and can be called as HOF.
                let variant_name = name.split("::").last().unwrap_or(name.as_str());
                let field_tys: Vec<Ty> = self
                    .env
                    .types
                    .values()
                    .find_map(|ti| {
                        if let TypeBodyInfo::Enum(variants) = &ti.body {
                            variants
                                .iter()
                                .find(|v| v.name == variant_name)
                                .and_then(|v| {
                                    if let VariantFieldsInfo::Tuple(tys) = &v.fields {
                                        Some(tys.clone())
                                    } else {
                                        None
                                    }
                                })
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default();
                for (i, p) in fields.iter().enumerate() {
                    let ty = field_tys.get(i).cloned().unwrap_or(Ty::Unknown);
                    self.bind_match_pattern(p, &ty);
                }
            }
            Pattern::Struct { fields, .. } => {
                for (_, p) in fields {
                    self.bind_match_pattern(p, &Ty::Unknown);
                }
            }
            Pattern::Tuple { elems, .. } => {
                let elem_tys = match scrutinee_ty.unlabeled() {
                    Ty::Tuple(ts) => ts.clone(),
                    _ => vec![],
                };
                for (i, p) in elems.iter().enumerate() {
                    let ty = elem_tys.get(i).cloned().unwrap_or(Ty::Unknown);
                    self.bind_match_pattern(p, &ty);
                }
            }
        }
    }

    /// Validate a match guard's RefExpr — check that referenced identifiers
    /// are in scope.  Since RefExpr is a predicate language (Compare, LogicOp,
    /// Not, etc.), the result is always boolean by construction (#938).
    fn check_guard_ref_expr(&mut self, expr: &RefExpr) {
        match expr {
            RefExpr::Ident { name, span } => {
                if self.env.lookup(name).is_none() {
                    self.emit(CheckError::UndefinedVariable {
                        name: name.clone(),
                        span: *span,
                    });
                }
            }
            RefExpr::LogicOp { left, right, .. }
            | RefExpr::Compare { left, right, .. }
            | RefExpr::ArithOp { left, right, .. } => {
                self.check_guard_ref_expr(left);
                self.check_guard_ref_expr(right);
            }
            RefExpr::Not { inner, .. }
            | RefExpr::Grouped { inner, .. }
            | RefExpr::Old { inner, .. } => {
                self.check_guard_ref_expr(inner);
            }
            RefExpr::FieldAccess { object, .. } => {
                self.check_guard_ref_expr(object);
            }
            RefExpr::Len { ident, span } => {
                if self.env.lookup(ident).is_none() {
                    self.emit(CheckError::UndefinedVariable {
                        name: ident.clone(),
                        span: *span,
                    });
                }
            }
            RefExpr::Integer { .. } | RefExpr::Float { .. } => {}
            RefExpr::Forall { body, .. } | RefExpr::Exists { body, .. } => {
                self.check_guard_ref_expr(body);
            }
        }
    }

    pub(super) fn bind_sub_patterns(&mut self, pattern: &Pattern, mutable: bool) {
        match pattern {
            Pattern::TupleStruct { fields, .. } => {
                for p in fields {
                    self.bind_pattern(p, &Ty::Unknown, mutable);
                }
            }
            Pattern::Struct { fields, .. } => {
                for (_, p) in fields {
                    self.bind_pattern(p, &Ty::Unknown, mutable);
                }
            }
            Pattern::Some { inner, .. }
            | Pattern::Ok { inner, .. }
            | Pattern::Err { inner, .. } => {
                self.bind_pattern(inner, &Ty::Unknown, mutable);
            }
            _ => {}
        }
    }
}

// ── Pattern helpers (used by check_exhaustiveness) ────────────────────────────

fn is_wildcard_pattern(pattern: &Pattern, variant_names: &[String]) -> bool {
    match pattern {
        Pattern::Wildcard(_) => true,
        Pattern::Ident(name, _) => {
            // Qualified names like "Enum::Variant" are never wildcards
            if name.contains("::") {
                return false;
            }
            !variant_names.contains(name)
        }
        _ => false,
    }
}

/// Extract the variant name that a pattern explicitly covers, given the set of
/// known variant names.  Returns `None` for non-variant or wildcard patterns.
/// Handles qualified names like `Enum::Variant(...)` by extracting the short name.
fn covered_variant_name(pattern: &Pattern, variant_names: &[String]) -> Option<String> {
    match pattern {
        Pattern::TupleStruct { name, .. } | Pattern::Struct { name, .. } => {
            let short = name.rsplit("::").next().unwrap_or(name.as_str());
            if variant_names.contains(&short.to_string()) {
                Some(short.to_string())
            } else {
                Some(name.clone())
            }
        }
        // A bare ident (qualified or not) that IS a known variant name counts as that variant
        Pattern::Ident(name, _) => {
            let short = name.rsplit("::").next().unwrap_or(name.as_str());
            if variant_names.contains(&short.to_string()) {
                Some(short.to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}
