// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Pattern matching and exhaustiveness checking for the MVL type checker.

use crate::mvl::checker::context::{TypeBodyInfo, TypeEnv, VarInfo, VariantFieldsInfo};
use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::types::Ty;
use crate::mvl::parser::ast::{MatchArm, MatchBody, Pattern, RefExpr};
use crate::mvl::parser::lexer::Span;

use super::TypeChecker;

/// Normalize `Ty::Named("Option"/"Result", [...])` to the structural
/// `Ty::Option`/`Ty::Result` variant. A generic method's own `self` parameter
/// (e.g. `Result[T, E]::map`) is stored in this named-wrapper shape rather
/// than the structural one a concrete call site produces — the same
/// declared/actual shape mismatch already fixed once in the WASM backend's
/// `unify_ty_params` (#2125), just never fixed here in the checker's own
/// match-pattern binding. Without this, `bind_match_pattern`'s `Some`/`Ok`/
/// `Err` arms couldn't recognize the scrutinee as an Option/Result at all,
/// fell through to `Ty::Unknown`, and any variable bound from the payload
/// (`Err(e) => ...e...`) carried no usable type downstream — invisible for
/// methods that only match against `_`/literals (`is_ok`, `is_err`) but
/// silently wrong for anything reconstructing a new value from the binding,
/// e.g. `Result[T, E]::map`'s `Err(e) => Err(e)` passthrough (#2149).
fn normalize_option_result(ty: &Ty) -> Ty {
    match ty {
        Ty::Named(name, args) if name == "Option" && args.len() == 1 => {
            Ty::Option(Box::new(args[0].clone()))
        }
        Ty::Named(name, args) if name == "Result" && args.len() == 2 => {
            Ty::Result(Box::new(args[0].clone()), Box::new(args[1].clone()))
        }
        other => other.clone(),
    }
}

impl TypeChecker {
    // ── Match exhaustiveness (#13) ────────────────────────────────────────

    pub(super) fn infer_match_expr(
        &mut self,
        arms: &[MatchArm],
        scrutinee_ty: &Ty,
        span: Span,
    ) -> Ty {
        // `match` used as an expression (e.g. a `let` RHS or lambda body): its
        // value is consumed by whatever encloses it, so suppress ResultIgnored
        // on arm tails — same reasoning as `Expr::If`'s `suppress_result_ignored`.
        self.check_match_arms(arms, scrutinee_ty, span, None, true)
    }

    /// Check match arms for exhaustiveness and return the result type.
    pub(super) fn check_match_arms(
        &mut self,
        arms: &[MatchArm],
        scrutinee_ty: &Ty,
        span: Span,
        return_ty: Option<&Ty>,
        suppress_result_ignored: bool,
    ) -> Ty {
        // Check each arm body. Arms are mutually exclusive at runtime (only
        // one pattern matches), so a move inside one arm must not be visible
        // while checking the next — snapshot before each arm, restore
        // before the next, and union the outcomes across all arms once
        // every arm has been checked (#1991 follow-up: branch-aware move
        // tracking, needed once container literals/`.insert()` started
        // participating in move semantics).
        let mut arm_tys: Vec<Ty> = Vec::new();
        let pre_snapshot = self.env.snapshot_moved();
        let mut merged_snapshot: Option<Vec<std::collections::HashMap<String, bool>>> = None;
        for arm in arms {
            self.env.restore_moved(&pre_snapshot);
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
                MatchBody::Block(b) => {
                    self.infer_block_type(b, return_ty, suppress_result_ignored)
                }
            };
            self.env.pop_scope();
            let post_snapshot = self.env.snapshot_moved();
            merged_snapshot = Some(match merged_snapshot {
                None => post_snapshot,
                Some(prev) => TypeEnv::union_moved_snapshots(&prev, &post_snapshot),
            });
            arm_tys.push(body_ty);
        }
        if let Some(final_snapshot) = merged_snapshot {
            self.env.restore_moved(&final_snapshot);
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
                let has_some = unguarded.iter().any(|a| pattern_has_some(&a.pattern));
                let has_none = unguarded.iter().any(|a| pattern_has_none(&a.pattern));
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
                let has_ok = unguarded.iter().any(|a| pattern_has_ok(&a.pattern));
                let has_err = unguarded.iter().any(|a| pattern_has_err(&a.pattern));
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
                    match &type_info.body {
                        TypeBodyInfo::Enum(variants) => {
                            let variant_names: Vec<String> =
                                variants.iter().map(|v| v.name.clone()).collect();

                            // A wildcard is any Pattern::Wildcard OR a bare ident not in the enum's variants
                            if unguarded
                                .iter()
                                .any(|a| is_wildcard_pattern(&a.pattern, &variant_names))
                            {
                                return;
                            }

                            // Collect which variant names are explicitly covered (Or patterns cover many)
                            let covered: Vec<String> = unguarded
                                .iter()
                                .flat_map(|arm| covered_variant_names(&arm.pattern, &variant_names))
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
                        // #1787: peel alias types so `type MyOpt = Option[Int]` matches
                        // are still checked for exhaustiveness against the underlying sum.
                        TypeBodyInfo::Alias(inner) => {
                            let inner_ty = inner.clone();
                            self.check_exhaustiveness(arms, &inner_ty, span);
                        }
                        TypeBodyInfo::Struct { .. } => {}
                    }
                }
                // Unknown type → no exhaustiveness check
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
            Pattern::Literal(_, _) => {}
            Pattern::Or { patterns, .. } => {
                // Bind from the first alternative; all must bind the same names/types.
                if let Some(first) = patterns.first() {
                    self.bind_pattern(first, ty, mutable);
                }
            }
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
                let inner_ty = match normalize_option_result(scrutinee_ty.unlabeled()) {
                    Ty::Option(t) => *t,
                    _ => Ty::Unknown,
                };
                self.bind_match_pattern(inner, &inner_ty);
            }
            Pattern::Ok { inner, .. } => {
                let inner_ty = match normalize_option_result(scrutinee_ty.unlabeled()) {
                    Ty::Result(ok, _) => *ok,
                    _ => Ty::Unknown,
                };
                self.bind_match_pattern(inner, &inner_ty);
            }
            Pattern::Err { inner, .. } => {
                let inner_ty = match normalize_option_result(scrutinee_ty.unlabeled()) {
                    Ty::Result(_, err) => *err,
                    _ => Ty::Unknown,
                };
                self.bind_match_pattern(inner, &inner_ty);
            }
            Pattern::TupleStruct { name, fields, .. } => {
                // Look up the enum variant to get concrete field types so that
                // function-typed fields (e.g. `Filtered(lo, hi, pred: fn(Int)->Bool)`)
                // are bound with the correct type and can be called as HOF.
                //
                // When the pattern is qualified (e.g. `CsvError::ParseError`), look up
                // the named type directly before falling back to scanning all types.
                // Multiple stdlib types can share a variant name (e.g. JsonError::ParseError
                // and CsvError::ParseError both have "ParseError") and HashMap iteration
                // order is non-deterministic, so an unanchored search can bind fields to
                // the wrong types (#1410).
                let lookup_variant_fields = |type_info: &TypeBodyInfo, vname: &str| {
                    if let TypeBodyInfo::Enum(variants) = type_info {
                        variants.iter().find(|v| v.name == vname).and_then(|v| {
                            if let VariantFieldsInfo::Tuple(tys) = &v.fields {
                                Some(tys.clone())
                            } else {
                                None
                            }
                        })
                    } else {
                        None
                    }
                };
                let variant_name = name.split("::").last().unwrap_or(name.as_str());
                let field_tys: Vec<Ty> = if let Some((type_name, _)) = name.split_once("::") {
                    // Qualified name: prefer the explicitly-named type to avoid ambiguity.
                    self.env
                        .types
                        .get(type_name)
                        .and_then(|ti| lookup_variant_fields(&ti.body, variant_name))
                        .or_else(|| {
                            self.env
                                .types
                                .values()
                                .find_map(|ti| lookup_variant_fields(&ti.body, variant_name))
                        })
                        .unwrap_or_default()
                } else {
                    self.env
                        .types
                        .values()
                        .find_map(|ti| lookup_variant_fields(&ti.body, variant_name))
                        .unwrap_or_default()
                };
                for (i, p) in fields.iter().enumerate() {
                    let ty = field_tys.get(i).cloned().unwrap_or(Ty::Unknown);
                    self.bind_match_pattern(p, &ty);
                }
            }
            Pattern::Struct { name, fields, .. } => {
                // Mirror TupleStruct above: look up the enum variant's declared
                // struct-shaped field types by name so a bound field (e.g.
                // `AuthError::AccountLocked { attempts }`) gets its real type
                // instead of Unknown — Unknown silently breaks any method call
                // or use of the binding downstream (to_string(), arithmetic, ...).
                let lookup_variant_struct_fields = |type_info: &TypeBodyInfo, vname: &str| {
                    if let TypeBodyInfo::Enum(variants) = type_info {
                        variants.iter().find(|v| v.name == vname).and_then(|v| {
                            if let VariantFieldsInfo::Struct(named) = &v.fields {
                                Some(named.clone())
                            } else {
                                None
                            }
                        })
                    } else {
                        None
                    }
                };
                let variant_name = name.split("::").last().unwrap_or(name.as_str());
                let field_infos: Vec<super::context::FieldInfo> =
                    if let Some((type_name, _)) = name.split_once("::") {
                        self.env
                            .types
                            .get(type_name)
                            .and_then(|ti| lookup_variant_struct_fields(&ti.body, variant_name))
                            .or_else(|| {
                                self.env.types.values().find_map(|ti| {
                                    lookup_variant_struct_fields(&ti.body, variant_name)
                                })
                            })
                            .unwrap_or_default()
                    } else {
                        self.env
                            .types
                            .values()
                            .find_map(|ti| lookup_variant_struct_fields(&ti.body, variant_name))
                            .unwrap_or_default()
                    };
                for (field_name, p) in fields {
                    let ty = field_infos
                        .iter()
                        .find(|fi| &fi.name == field_name)
                        .map(|fi| fi.ty.clone())
                        .unwrap_or(Ty::Unknown);
                    self.bind_match_pattern(p, &ty);
                }
            }
            Pattern::Or { patterns, .. } => {
                // Bind from the first alternative; exhaustiveness checker validates consistency.
                if let Some(first) = patterns.first() {
                    self.bind_match_pattern(first, scrutinee_ty);
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
            RefExpr::Integer { .. } | RefExpr::Float { .. } | RefExpr::Bool { .. } => {}
            RefExpr::BoundedForall { span, .. } | RefExpr::BoundedExists { span, .. } => {
                self.emit(CheckError::QuantifierOutsideGhost { span: *span });
            }
            RefExpr::BitwiseOp { left, right, .. } => {
                self.check_guard_ref_expr(left);
                self.check_guard_ref_expr(right);
            }
            RefExpr::BitwiseNot { inner, .. } => {
                self.check_guard_ref_expr(inner);
            }
            RefExpr::StringOp { receiver, .. } => {
                self.check_guard_ref_expr(receiver);
            }
            RefExpr::ArrayGet { list, index, .. } => {
                self.check_guard_ref_expr(list);
                self.check_guard_ref_expr(index);
            }
            RefExpr::RegexMatch { receiver, .. } => {
                self.check_guard_ref_expr(receiver);
            }
            RefExpr::Abs { inner, .. } => {
                self.check_guard_ref_expr(inner);
            }
            RefExpr::Min { left, right, .. } | RefExpr::Max { left, right, .. } => {
                self.check_guard_ref_expr(left);
                self.check_guard_ref_expr(right);
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
            Pattern::Or { patterns, .. } => {
                if let Some(first) = patterns.first() {
                    self.bind_sub_patterns(first, mutable);
                }
            }
            _ => {}
        }
    }
}

// ── Pattern helpers (used by check_exhaustiveness) ────────────────────────────

fn pattern_has_some(p: &Pattern) -> bool {
    match p {
        Pattern::Some { .. } => true,
        Pattern::TupleStruct { name, .. } if name == "Some" => true,
        Pattern::Or { patterns, .. } => patterns.iter().any(pattern_has_some),
        _ => false,
    }
}

fn pattern_has_none(p: &Pattern) -> bool {
    match p {
        Pattern::None(_) => true,
        Pattern::Ident(n, _) if n == "None" => true,
        Pattern::Or { patterns, .. } => patterns.iter().any(pattern_has_none),
        _ => false,
    }
}

fn pattern_has_ok(p: &Pattern) -> bool {
    match p {
        Pattern::Ok { .. } => true,
        Pattern::TupleStruct { name, .. } if name == "Ok" => true,
        Pattern::Or { patterns, .. } => patterns.iter().any(pattern_has_ok),
        _ => false,
    }
}

fn pattern_has_err(p: &Pattern) -> bool {
    match p {
        Pattern::Err { .. } => true,
        Pattern::TupleStruct { name, .. } if name == "Err" => true,
        Pattern::Or { patterns, .. } => patterns.iter().any(pattern_has_err),
        _ => false,
    }
}

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
        // An Or pattern is a wildcard if any alternative is a wildcard.
        Pattern::Or { patterns, .. } => patterns
            .iter()
            .any(|p| is_wildcard_pattern(p, variant_names)),
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
        // Or pattern: the first alternative determines coverage (caller iterates all alternatives
        // separately via covered_variant_names for exhaustiveness).
        Pattern::Or { .. } => None,
        _ => None,
    }
}

/// Collect all variant names covered by a pattern (handles `Or` patterns covering multiple).
fn covered_variant_names(pattern: &Pattern, variant_names: &[String]) -> Vec<String> {
    match pattern {
        Pattern::Or { patterns, .. } => patterns
            .iter()
            .flat_map(|p| covered_variant_names(p, variant_names))
            .collect(),
        _ => covered_variant_name(pattern, variant_names)
            .into_iter()
            .collect(),
    }
}
