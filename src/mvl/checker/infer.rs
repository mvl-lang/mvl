// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Expression type inference for the MVL type checker.
//!
//! Contains `infer_expr`, `infer_literal`, `infer_binary`, `infer_unary`,
//! and the enum-variant lookup helper.

use crate::mvl::checker::context::{CapabilityState, TypeBodyInfo, VarInfo};
use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::ifc;
use crate::mvl::checker::types::{resolve, types_compatible, Ty};
use crate::mvl::parser::ast::{BinaryOp, Expr, Literal, UnaryOp};
use crate::mvl::parser::lexer::Span;

use super::TypeChecker;

impl TypeChecker {
    // ── Expression type inference ─────────────────────────────────────────

    pub(super) fn infer_expr(&mut self, expr: &Expr) -> Ty {
        let ty = self.infer_expr_inner(expr);
        self.expr_types.insert(expr.span(), ty.clone());
        ty
    }

    fn infer_expr_inner(&mut self, expr: &Expr) -> Ty {
        match expr {
            // #11: Literals
            Expr::Literal(lit, _) => self.infer_literal(lit),

            // #11/#15: Variable reference
            Expr::Ident(name, span) => {
                if let Some((scope_idx, info)) = self.env.lookup_with_scope_index(name) {
                    // Clone early to release the borrow on `self.env` before calling self.emit.
                    let is_mutable = info.mutable;
                    let is_moved = info.moved;
                    let ty = info.ty.clone();

                    // ADR-0002: Lambdas may only capture immutable bindings.
                    // If we are inside a lambda and the variable was found in a scope
                    // that predates the lambda's own scope, it is a captured binding.
                    if let Some(&boundary) = self.lambda_scope_starts.last() {
                        if scope_idx < boundary && is_mutable {
                            self.emit(CheckError::CaptureMutabilityViolation {
                                name: name.clone(),
                                span: *span,
                            });
                        }
                    }
                    // #15: ownership — reject use after move
                    if is_moved {
                        self.emit(CheckError::UseAfterMove {
                            name: name.clone(),
                            span: *span,
                        });
                        return Ty::Unknown;
                    }
                    ty
                } else {
                    // Before emitting UndefinedVariable, check whether the ident
                    // is a known enum unit-variant or the built-in `None`.
                    if name == "None" {
                        return Ty::Option(Box::new(Ty::Unknown));
                    }
                    // Bare variant: `DivisionByZero` or path: `MathError::DivisionByZero`
                    // When the path has a namespace prefix (e.g. `Foo::Bar`), prefer the
                    // type named `Foo` over any other enum that happens to have a `Bar` variant.
                    // This prevents ambiguous-variant bugs when two enums share a variant name
                    // (e.g. user `TokenCat::Number` vs stdlib JSON `Value::Number`).
                    if let Some((ns, v)) = name.split_once("::") {
                        // Prefer the explicitly named type.
                        if self.env.types.get(ns).is_some_and(|ti| {
                            matches!(&ti.body, TypeBodyInfo::Enum(vs) if vs.iter().any(|x| x.name == v))
                        }) {
                            return Ty::Named(ns.to_string(), vec![]);
                        }
                        // Fallback: search all enums (handles aliased / re-exported types).
                        if let Some(enum_ty) = self.lookup_enum_for_variant(v) {
                            return enum_ty;
                        }
                    } else if let Some(enum_ty) = self.lookup_enum_for_variant(name.as_str()) {
                        return enum_ty;
                    }
                    // Function reference: `xs.map(double)` — ident is a known function name.
                    // Return Ty::Fn so callers like map/filter can infer the output type.
                    if let Some(fn_info) = self.env.lookup_fn(name).cloned() {
                        return Ty::Fn(
                            fn_info.params.clone(),
                            Box::new(fn_info.ret.clone()),
                            fn_info.effects.clone(),
                            fn_info.totality.clone(),
                        );
                    }
                    self.emit(CheckError::UndefinedVariable {
                        name: name.clone(),
                        span: *span,
                    });
                    Ty::Unknown
                }
            }

            // #11: Binary operations
            Expr::Binary {
                op,
                left,
                right,
                span,
            } => self.infer_binary(*op, left, right, *span),

            Expr::Unary { op, expr, span } => self.infer_unary(*op, expr, *span),

            Expr::Borrow {
                mutable,
                expr,
                span,
            } => {
                let inner = self.infer_expr(expr);
                // Fix 1: reject `&mut x` on an immutable binding (#366)
                if *mutable {
                    if let Expr::Ident(name, _) = expr.as_ref() {
                        if let Some(info) = self.env.lookup(name).cloned() {
                            if !info.mutable {
                                self.emit(CheckError::AssignToImmutable {
                                    name: name.clone(),
                                    span: *span,
                                });
                            }
                        }
                    }
                }
                // Fix 4: reject double-borrow — referencing an already-referenced value (#366)
                if let Ty::Ref(_, _) = &inner {
                    self.emit(CheckError::TypeMismatch {
                        expected: inner.display(),
                        found: format!("val {}", inner.display()),
                        span: *span,
                    });
                    return Ty::Unknown;
                }
                // Phase D (#362): check CapabilityState on the referent (error only).
                // State updates are deferred to Stmt::Let where ref_var is also set,
                // so that capability_state is always released on scope exit. Updating state
                // here (expression position) would leak when the reference is not `let`-bound.
                if let Expr::Ident(name, _) = expr.as_ref() {
                    let current = self
                        .env
                        .lookup(name)
                        .map(|i| i.capability_state.clone())
                        .unwrap_or(CapabilityState::Owned);

                    if *mutable {
                        if current != CapabilityState::Owned {
                            self.emit(CheckError::AliasingMutableBorrow {
                                name: name.clone(),
                                span: *span,
                            });
                        }
                    } else if matches!(current, CapabilityState::Ref) {
                        self.emit(CheckError::AliasingMutableBorrow {
                            name: name.clone(),
                            span: *span,
                        });
                    }
                }
                Ty::Ref(*mutable, Box::new(inner))
            }

            // #12: Field access — reject direct field access on enum or Option
            Expr::FieldAccess { expr, field, span } => {
                let ty = self.infer_expr(expr);
                // #14: Option direct access
                if ty.is_option() {
                    self.emit(CheckError::OptionDirectAccess { span: *span });
                    return Ty::Unknown;
                }
                self.field_type_checked(&ty, field, *span)
            }

            // #11: Function call
            Expr::FnCall {
                name, args, span, ..
            } => self.infer_fn_call(name, args, *span),

            Expr::MethodCall {
                receiver,
                method,
                args,
                span,
            } => {
                // Qualified module call: `json.decode(s)` where `json` is a
                // module alias from `use std.json`. Redirect to a function
                // call so stdlib lookup tables resolve `decode` correctly (#820).
                if let Expr::Ident(name, _) = receiver.as_ref() {
                    if self.module_aliases.contains_key(name.as_str()) {
                        return self.infer_fn_call(method, args, *span);
                    }
                }
                let recv_ty = self.infer_expr(receiver);
                let arg_tys: Vec<Ty> = args.iter().map(|a| self.infer_expr(a)).collect();
                // Req 9: capability check for actor-boundary crossings.
                // `channel.send(val)` — first argument must be `iso` or `val`.
                if method == "send" {
                    if let Some(first_arg) = args.first() {
                        self.check_send_capability(first_arg, *span);
                    }
                }
                // Stdlib method resolution (#43): dispatch on receiver type.
                // IFC labels propagate through method results via the receiver label.
                self.infer_method_call(&recv_ty, method, &arg_tys, *span)
            }

            // #13: Match expressions
            Expr::Match {
                scrutinee,
                arms,
                span,
            } => {
                let scrutinee_ty = self.infer_expr(scrutinee);
                self.infer_match_expr(arms, &scrutinee_ty, *span)
            }

            Expr::If {
                cond,
                then,
                else_,
                span,
            } => {
                let cond_ty = self.infer_expr(cond);
                // IFC: extract the condition's security label for implicit flow promotion.
                // Branching on Secret<Bool> must promote the result to at least Secret<T>;
                // otherwise the choice of branch would leak the guard's value.
                let cond_label = ifc::label_of(&cond_ty).map(|s| s.to_string());
                if !cond_ty.is_bool() && !matches!(cond_ty, Ty::Unknown) {
                    self.emit(CheckError::TypeMismatch {
                        expected: "Bool".to_string(),
                        found: cond_ty.display(),
                        span: cond.span(),
                    });
                }
                let then_ty = self.infer_block_type(then, None);
                // Promote branch type by joining with the condition's label (#26 implicit flow).
                let promoted_then = {
                    let label = ifc::join_opt(
                        cond_label.clone(),
                        ifc::label_of(&then_ty).map(|s| s.to_string()),
                    );
                    ifc::apply_label(label, then_ty.unlabeled().clone())
                };
                if let Some(else_expr) = else_ {
                    let else_ty = self.infer_expr(else_expr);
                    let promoted_else = {
                        let label = ifc::join_opt(
                            cond_label.clone(),
                            ifc::label_of(&else_ty).map(|s| s.to_string()),
                        );
                        ifc::apply_label(label, else_ty.unlabeled().clone())
                    };
                    if !matches!(promoted_then, Ty::Unknown)
                        && !matches!(promoted_else, Ty::Unknown)
                        && !types_compatible(&promoted_then, &promoted_else)
                    {
                        self.emit(CheckError::TypeMismatch {
                            expected: promoted_then.display(),
                            found: promoted_else.display(),
                            span: *span,
                        });
                    }
                    if matches!(promoted_then, Ty::Unknown) {
                        promoted_else
                    } else {
                        promoted_then
                    }
                } else {
                    promoted_then
                }
            }

            Expr::Block(block) => {
                // Infer the type of the last expression so that block-expressions
                // (e.g. the else-branch of an if-expression) return the correct type.
                self.infer_block_type(block, None)
            }

            // #12: Struct construction
            Expr::Construct { name, fields, span } => self.check_construction(name, fields, *span),

            Expr::List { elems, .. } => {
                let elem_ty = elems
                    .first()
                    .map(|e| self.infer_expr(e))
                    .unwrap_or(Ty::Unknown);
                for e in elems.iter().skip(1) {
                    self.infer_expr(e);
                }
                Ty::List(Box::new(elem_ty))
            }

            Expr::Map { pairs, .. } => {
                // Join the labels of all value expressions so the resulting Map
                // type reflects any sensitivity present in the values (#54, Req 6).
                // This ensures `{"k": secret_val}` is typed as
                // `Secret<Map<String,String>>` rather than `Map<String,Secret<String>>`,
                // making the standard `label_of` check work for log-sink enforcement.
                let mut joined_label: Option<String> = None;
                let (key_ty, val_ty) = pairs
                    .first()
                    .map(|(k, v)| {
                        let kt = self.infer_expr(k);
                        let vt = self.infer_expr(v);
                        joined_label = ifc::join_opt(
                            joined_label.clone(),
                            ifc::label_of(&vt).map(|s| s.to_string()),
                        );
                        (kt, vt.unlabeled().clone())
                    })
                    .unwrap_or((Ty::Unknown, Ty::Unknown));
                for (k, v) in pairs.iter().skip(1) {
                    self.infer_expr(k);
                    let vt = self.infer_expr(v);
                    joined_label = ifc::join_opt(
                        joined_label.clone(),
                        ifc::label_of(&vt).map(|s| s.to_string()),
                    );
                }
                let map_ty = Ty::Map(Box::new(key_ty), Box::new(val_ty));
                ifc::apply_label(joined_label, map_ty)
            }

            Expr::Set { elems, .. } => {
                let elem_ty = elems
                    .first()
                    .map(|e| self.infer_expr(e))
                    .unwrap_or(Ty::Unknown);
                for e in elems.iter().skip(1) {
                    self.infer_expr(e);
                }
                Ty::Set(Box::new(elem_ty))
            }

            // #14: `?` propagation
            Expr::Propagate { expr, span } => {
                let ty = self.infer_expr(expr);
                if !ty.is_propagatable() && !matches!(ty, Ty::Unknown) {
                    self.emit(CheckError::PropagateNotResult {
                        ty: ty.display(),
                        span: *span,
                    });
                    return Ty::Unknown;
                }
                // If both the expression and enclosing function return Result types,
                // verify error types are compatible — either identical or convertible via From.
                if let (Ty::Result(_, expr_err), Some(Ty::Result(_, ret_err))) = (
                    ty.unlabeled(),
                    self.current_return_ty.as_ref().map(|t| t.unlabeled()),
                ) {
                    let from_ty = expr_err.display();
                    let into_ty = ret_err.display();
                    if from_ty != into_ty
                        && !matches!(**expr_err, Ty::Unknown)
                        && !matches!(**ret_err, Ty::Unknown)
                        && !self.env.has_from_impl(&into_ty, &from_ty)
                    {
                        self.emit(CheckError::PropagateIncompatibleError {
                            from_ty,
                            into_ty,
                            span: *span,
                        });
                    }
                }
                ty.propagate_inner()
            }

            // consume(x) — explicit iso consumption; marks the binding as moved
            // so subsequent references are caught by use-after-move checking.
            Expr::Consume { expr, .. } => {
                let ty = self.infer_expr(expr);
                if let Expr::Ident(name, _) = expr.as_ref() {
                    self.env.mark_moved(name);
                }
                ty
            }

            // #894: relabel(name, expr, "tag") — applies a declared IFC transition.
            // Looks up the transition in the type environment, verifies input type,
            // and returns the output type.
            Expr::Relabel {
                name,
                expr,
                tag: _,
                span,
            } => {
                let inner_ty = self.infer_expr(expr);
                // Look up the declared relabel transition.
                if let Some((from, to)) = self.env.lookup_relabel(name) {
                    let inner_base = inner_ty.base();
                    let input_matches = match &from {
                        None => !matches!(inner_base, Ty::Labeled(..)), // bare → not labeled
                        Some(label) => matches!(inner_base, Ty::Labeled(l, _) if l == label),
                    };
                    if !input_matches && !matches!(inner_base, Ty::Unknown) {
                        self.emit(CheckError::InvalidRelabel {
                            transition: name.clone(),
                            expected_from: from.as_deref().unwrap_or("bare").to_string(),
                            found: inner_ty.display(),
                            span: *span,
                        });
                        return Ty::Unknown;
                    }
                    // Compute output type.
                    let stripped = ifc::strip_label(inner_base);
                    match to {
                        None => stripped.clone(), // bare output
                        Some(label) => Ty::Labeled(label, Box::new(stripped.clone())),
                    }
                } else {
                    self.emit(CheckError::UnknownRelabel {
                        name: name.clone(),
                        span: *span,
                    });
                    Ty::Unknown
                }
            }

            // Phase 8: spawn expression validates field initializers against the actor
            // type's declared fields (#742) and returns the actor's type name (#63/#698).
            Expr::Spawn {
                actor_type,
                fields,
                span,
                ..
            } => self.check_construction(actor_type, fields, *span),

            // Phase 8: select — evaluates to Unit (fire-and-forget arms, spec 015 §8)
            Expr::Select { arms, .. } => {
                for arm in arms {
                    self.infer_expr(&arm.expr);
                    self.infer_block_type(&arm.body, None);
                }
                Ty::Unit
            }
            Expr::Concurrently { body, .. } => {
                self.infer_block_type(body, None);
                Ty::Unit
            }

            Expr::Lambda {
                params,
                ret_type,
                body,
                ..
            } => {
                // Record the current scope depth as the lambda boundary so that
                // Expr::Ident can detect mutable captures (ADR-0002).
                let boundary = self.env.scope_depth();
                self.lambda_scope_starts.push(boundary);

                self.env.push_scope();
                let param_tys: Vec<Ty> = params
                    .iter()
                    .map(|p| {
                        let ty = resolve(&p.ty);
                        let env_ty = match &ty {
                            Ty::Ref(_, inner) => (**inner).clone(),
                            _ => ty.clone(),
                        };
                        self.env.define(
                            p.name.clone(),
                            VarInfo::new(
                                env_ty,
                                matches!(ty.base(), Ty::Ref(true, _))
                                    || matches!(
                                        p.capability,
                                        Some(crate::mvl::parser::ast::Capability::Ref)
                                            | Some(crate::mvl::parser::ast::Capability::Iso)
                                    ),
                            ),
                        );
                        ty
                    })
                    .collect();
                let ret_ty = ret_type.as_ref().map(|t| resolve(t)).unwrap_or(Ty::Unknown);
                let body_ty = self.infer_expr(body);
                // Verify body type matches declared return annotation
                if !matches!(ret_ty, Ty::Unknown)
                    && !matches!(body_ty, Ty::Unknown)
                    && !types_compatible(&ret_ty, &body_ty)
                {
                    self.emit(CheckError::TypeMismatch {
                        expected: ret_ty.display(),
                        found: body_ty.display(),
                        span: body.span(),
                    });
                }
                self.env.pop_scope();
                self.lambda_scope_starts.pop();
                Ty::Fn(param_tys, Box::new(ret_ty), vec![], None)
            }

            // Quantifier predicates only appear in contract positions (requires/ensures/invariants),
            // never in expression positions that type inference walks.
            Expr::Quantifier(..) => unreachable!("Quantifier in type-inference position"),
        }
    }

    // ── Literal types (#11) ───────────────────────────────────────────────

    pub(super) fn infer_literal(&self, lit: &Literal) -> Ty {
        match lit {
            Literal::Integer(_) => Ty::Int,
            Literal::Float(_) => Ty::Float,
            Literal::Str(_) => Ty::String,
            Literal::Char(_) => Ty::Char,
            Literal::Bool(_) => Ty::Bool,
            Literal::Unit => Ty::Unit,
        }
    }

    // ── Binary operations (#11) ───────────────────────────────────────────

    pub(super) fn infer_binary(
        &mut self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
        span: Span,
    ) -> Ty {
        let lt = self.infer_expr(left);
        let rt = self.infer_expr(right);

        match op {
            // Arithmetic: both operands must be numeric and the same type.
            // Labels propagate via join: Secret<Int> + Public<Int> → Secret<Int>.
            // Named alias types (e.g. Amount = Float where ...) are resolved before checking.
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
                // Resolve named aliases so that Labeled<NamedAlias> is treated as Labeled<baseType>.
                let lt = self.resolve_alias_in_labeled(lt);
                let rt = self.resolve_alias_in_labeled(rt);
                if !matches!(lt, Ty::Unknown) && !lt.is_numeric() {
                    self.emit(CheckError::NonNumericArithmetic {
                        ty: lt.display(),
                        span: left.span(),
                    });
                    return Ty::Unknown;
                }
                if !matches!(rt, Ty::Unknown) && !rt.is_numeric() {
                    self.emit(CheckError::NonNumericArithmetic {
                        ty: rt.display(),
                        span: right.span(),
                    });
                    return Ty::Unknown;
                }
                // Compare unlabeled base types to allow mixed-label arithmetic
                let lt_inner = lt.unlabeled().clone();
                let rt_inner = rt.unlabeled().clone();
                if !matches!(lt_inner, Ty::Unknown)
                    && !matches!(rt_inner, Ty::Unknown)
                    && lt_inner != rt_inner
                {
                    self.emit(CheckError::ArithmeticTypeMismatch {
                        op: format!("{op:?}").to_lowercase(),
                        left: lt.display(),
                        right: rt.display(),
                        span,
                    });
                    return Ty::Unknown;
                }
                // Propagate the join of labels to the result (#26)
                let label = ifc::join_opt(
                    ifc::label_of(&lt).map(|s| s.to_string()),
                    ifc::label_of(&rt).map(|s| s.to_string()),
                );
                let base = if matches!(lt_inner, Ty::Unknown) {
                    rt_inner
                } else {
                    lt_inner
                };
                ifc::apply_label(label, base)
            }

            // Comparison: both sides same type → Bool
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Gt
            | BinaryOp::Le
            | BinaryOp::Ge => {
                // Constraint enforcement: unconstrained type params may not be compared.
                // `<`, `>`, `<=`, `>=` require `Ord`; `==`, `!=` require `Eq`.
                // `Ord` is a supertype of `Eq`, so `where T: Ord` satisfies an `Eq` check.
                let required_bound = match op {
                    BinaryOp::Lt | BinaryOp::Gt | BinaryOp::Le | BinaryOp::Ge => "Ord",
                    BinaryOp::Eq | BinaryOp::Ne => "Eq",
                    // Unreachable: the outer match arm already constrains `op` to the
                    // six comparison operators listed above (#991).
                    _ => unreachable!(),
                };
                for operand_ty in [&lt, &rt] {
                    if let Ty::Named(name, args) = operand_ty.unlabeled() {
                        if args.is_empty() && self.current_type_params.contains(name) {
                            let has_bound =
                                self.current_type_constraints
                                    .get(name)
                                    .is_some_and(|bounds| {
                                        bounds.iter().any(|b| {
                                            b == required_bound
                                                || (required_bound == "Eq" && b == "Ord")
                                        })
                                    });
                            if !has_bound {
                                self.emit(CheckError::MissingConstraint {
                                    type_param: name.clone(),
                                    required_bound: required_bound.to_string(),
                                    span,
                                });
                            }
                        }
                    }
                }
                if !matches!(lt, Ty::Unknown)
                    && !matches!(rt, Ty::Unknown)
                    && !types_compatible(&lt, &rt)
                {
                    self.emit(CheckError::TypeMismatch {
                        expected: lt.display(),
                        found: rt.display(),
                        span,
                    });
                }
                Ty::Bool
            }

            // Bitwise: both operands must be integer types (not Float); result is same type.
            BinaryOp::BitAnd
            | BinaryOp::BitOr
            | BinaryOp::BitXor
            | BinaryOp::Shl
            | BinaryOp::Shr => {
                let lt_inner = lt.unlabeled().clone();
                let rt_inner = rt.unlabeled().clone();
                for (ty, span) in [(&lt, left.span()), (&rt, right.span())] {
                    if !matches!(ty.unlabeled(), Ty::Unknown) && !ty.is_integer() {
                        self.emit(CheckError::TypeMismatch {
                            expected: "integer type (Int, Byte, UByte, UInt)".to_string(),
                            found: ty.display(),
                            span,
                        });
                        return Ty::Unknown;
                    }
                }
                let label = ifc::join_opt(
                    ifc::label_of(&lt).map(|s| s.to_string()),
                    ifc::label_of(&rt).map(|s| s.to_string()),
                );
                let base = if matches!(lt_inner, Ty::Unknown) {
                    rt_inner
                } else {
                    lt_inner
                };
                ifc::apply_label(label, base)
            }

            // Logic: both must be Bool (labels stripped — Bool logic yields Bool)
            BinaryOp::And | BinaryOp::Or => {
                let op_str = format!("{op:?}").to_lowercase();
                if !matches!(lt.unlabeled(), Ty::Bool | Ty::Unknown) {
                    self.emit(CheckError::LogicTypeMismatch {
                        op: op_str.clone(),
                        ty: lt.display(),
                        span: left.span(),
                    });
                }
                if !matches!(rt.unlabeled(), Ty::Bool | Ty::Unknown) {
                    self.emit(CheckError::LogicTypeMismatch {
                        op: op_str,
                        ty: rt.display(),
                        span: right.span(),
                    });
                }
                Ty::Bool
            }
        }
    }

    pub(super) fn infer_unary(&mut self, op: UnaryOp, expr: &Expr, span: Span) -> Ty {
        let ty = self.infer_expr(expr);
        match op {
            UnaryOp::Neg => {
                if !matches!(ty, Ty::Unknown) && !ty.is_numeric() {
                    self.emit(CheckError::NonNumericArithmetic {
                        ty: ty.display(),
                        span,
                    });
                    Ty::Unknown
                } else {
                    ty
                }
            }
            UnaryOp::Not => {
                if !matches!(ty.unlabeled(), Ty::Bool | Ty::Unknown) {
                    self.emit(CheckError::TypeMismatch {
                        expected: "Bool".to_string(),
                        found: ty.display(),
                        span,
                    });
                }
                Ty::Bool
            }
            UnaryOp::Deref => {
                // Deref `*expr`: if expr has type Box<T>, return T.
                match ty {
                    Ty::Named(ref name, ref args) if name == "Box" && args.len() == 1 => {
                        args[0].clone()
                    }
                    Ty::Unknown => Ty::Unknown,
                    _ => {
                        self.emit(CheckError::TypeMismatch {
                            expected: "Box<T>".to_string(),
                            found: ty.display(),
                            span,
                        });
                        Ty::Unknown
                    }
                }
            }
            UnaryOp::BitNot => {
                if !matches!(ty.unlabeled(), Ty::Unknown) && !ty.is_integer() {
                    self.emit(CheckError::TypeMismatch {
                        expected: "integer type (Int, Byte, UByte, UInt)".to_string(),
                        found: ty.display(),
                        span,
                    });
                    Ty::Unknown
                } else {
                    ty
                }
            }
        }
    }

    // ── Enum constructor resolution (#12) ────────────────────────────────
    //
    // `Some(v)`, `Ok(v)`, `Err(e)` and user-defined tuple-variant constructors
    // are parsed as `Expr::FnCall` because they syntactically look like calls.
    // `None` and unit variants are `Expr::Ident`.  We must recognise them
    // before falling through to UndefinedFunction / UndefinedVariable.

    /// Return the enum type that contains a variant named `variant`, or `None`.
    pub(super) fn lookup_enum_for_variant(&self, variant: &str) -> Option<Ty> {
        for (type_name, type_info) in &self.env.types {
            if let TypeBodyInfo::Enum(variants) = &type_info.body {
                if variants.iter().any(|v| v.name == variant) {
                    return Some(Ty::Named(type_name.clone(), vec![]));
                }
            }
        }
        None
    }
}
