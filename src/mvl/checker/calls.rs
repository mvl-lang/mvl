// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Function and method call type inference for the MVL type checker.

use std::collections::HashMap;

use crate::mvl::checker::context::TypeBodyInfo;
use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::ifc;
use crate::mvl::checker::types::{resolve, Ty};
use crate::mvl::parser::ast::{Effect, Expr, Totality, TypeExpr};
use crate::mvl::parser::lexer::Span;

use super::TypeChecker;

// ── Generic type instantiation helpers (#1066) ────────────────────────────────

/// Apply a type-parameter substitution to a resolved `Ty`.
///
/// Names listed in `type_params` that are absent from `subst` (i.e. could not
/// be inferred from the call-site arguments) are replaced with `Ty::Unknown` so
/// that downstream compatibility checks degrade gracefully instead of failing
/// on an unresolved `Named("T", [])` sentinel.
fn substitute_ty(ty: &Ty, subst: &HashMap<String, Ty>, type_params: &[String]) -> Ty {
    match ty {
        // Bare named type: check if it's a type parameter.
        Ty::Named(name, args) if args.is_empty() => {
            if let Some(concrete) = subst.get(name.as_str()) {
                concrete.clone()
            } else if type_params.contains(name) {
                Ty::Unknown // unresolved type param — error-recovery sentinel
            } else {
                ty.clone()
            }
        }
        // Generic named type: recurse into type arguments.
        Ty::Named(name, args) => Ty::Named(
            name.clone(),
            args.iter()
                .map(|a| substitute_ty(a, subst, type_params))
                .collect(),
        ),
        Ty::Option(inner) => Ty::Option(Box::new(substitute_ty(inner, subst, type_params))),
        Ty::Result(ok, err) => Ty::Result(
            Box::new(substitute_ty(ok, subst, type_params)),
            Box::new(substitute_ty(err, subst, type_params)),
        ),
        Ty::List(elem) => Ty::List(Box::new(substitute_ty(elem, subst, type_params))),
        Ty::Map(k, v) => Ty::Map(
            Box::new(substitute_ty(k, subst, type_params)),
            Box::new(substitute_ty(v, subst, type_params)),
        ),
        Ty::Set(elem) => Ty::Set(Box::new(substitute_ty(elem, subst, type_params))),
        Ty::Labeled(label, inner) => Ty::Labeled(
            label.clone(),
            Box::new(substitute_ty(inner, subst, type_params)),
        ),
        Ty::Refined(base, pred) => Ty::Refined(
            Box::new(substitute_ty(base, subst, type_params)),
            pred.clone(),
        ),
        Ty::Fn(params, ret, effects, totality) => Ty::Fn(
            params
                .iter()
                .map(|p| substitute_ty(p, subst, type_params))
                .collect(),
            Box::new(substitute_ty(ret, subst, type_params)),
            effects.clone(),
            totality.clone(),
        ),
        Ty::Ref(mutable, inner) => {
            Ty::Ref(*mutable, Box::new(substitute_ty(inner, subst, type_params)))
        }
        // Primitives, Never, Unknown, Session — no type params inside.
        _ => ty.clone(),
    }
}

/// Infer type-parameter bindings by structurally matching declared parameter
/// types against the concrete argument types at a call site.
///
/// First binding for each parameter wins; conflicting second bindings are
/// silently ignored (the subsequent per-argument compatibility check will
/// report the mismatch).
fn infer_type_params(
    type_params: &[String],
    param_tys: &[Ty],
    arg_tys: &[Ty],
    subst: &mut HashMap<String, Ty>,
) {
    for (param_ty, arg_ty) in param_tys.iter().zip(arg_tys.iter()) {
        infer_type_param_pair(type_params, param_ty, arg_ty, subst);
    }
}

/// Recursively infer bindings from a single (param_type, arg_type) pair.
fn infer_type_param_pair(
    type_params: &[String],
    param_ty: &Ty,
    arg_ty: &Ty,
    subst: &mut HashMap<String, Ty>,
) {
    // Peel off the param label (if any) before matching; the arg may carry
    // a label of its own which becomes part of the binding.
    let param_base = param_ty.unlabeled();
    match param_base {
        // T (bare type parameter) → bind to the concrete arg type (labels included).
        Ty::Named(name, args) if args.is_empty() && type_params.contains(name) => {
            subst.entry(name.clone()).or_insert_with(|| arg_ty.clone());
        }
        // Named[A, B, ...] → recurse into type arguments.
        Ty::Named(_, param_args) => {
            if let Ty::Named(_, arg_args) = arg_ty.unlabeled() {
                for (p, a) in param_args.iter().zip(arg_args.iter()) {
                    infer_type_param_pair(type_params, p, a, subst);
                }
            }
        }
        Ty::Option(inner_p) => {
            if let Ty::Option(inner_a) = arg_ty.unlabeled() {
                infer_type_param_pair(type_params, inner_p, inner_a, subst);
            }
        }
        Ty::Result(ok_p, err_p) => {
            if let Ty::Result(ok_a, err_a) = arg_ty.unlabeled() {
                infer_type_param_pair(type_params, ok_p, ok_a, subst);
                infer_type_param_pair(type_params, err_p, err_a, subst);
            }
        }
        Ty::List(inner_p) => {
            if let Ty::List(inner_a) = arg_ty.unlabeled() {
                infer_type_param_pair(type_params, inner_p, inner_a, subst);
            }
        }
        Ty::Map(k_p, v_p) => {
            if let Ty::Map(k_a, v_a) = arg_ty.unlabeled() {
                infer_type_param_pair(type_params, k_p, k_a, subst);
                infer_type_param_pair(type_params, v_p, v_a, subst);
            }
        }
        Ty::Set(inner_p) => {
            if let Ty::Set(inner_a) = arg_ty.unlabeled() {
                infer_type_param_pair(type_params, inner_p, inner_a, subst);
            }
        }
        Ty::Fn(params_p, ret_p, _, _) => {
            if let Ty::Fn(params_a, ret_a, _, _) = arg_ty.unlabeled() {
                for (p, a) in params_p.iter().zip(params_a.iter()) {
                    infer_type_param_pair(type_params, p, a, subst);
                }
                infer_type_param_pair(type_params, ret_p, ret_a, subst);
            }
        }
        _ => {} // primitives, Unknown, Never — no type params to extract
    }
}

impl TypeChecker {
    // ── Function calls (#11) ──────────────────────────────────────────────

    pub(super) fn infer_fn_call(
        &mut self,
        name: &str,
        type_args: &[TypeExpr],
        args: &[Expr],
        span: Span,
    ) -> Ty {
        // Infer all argument types (for side-effect error collection)
        let arg_tys: Vec<Ty> = args.iter().map(|a| self.infer_expr(a)).collect();

        if let Some(fn_info) = self.env.lookup_fn(name).cloned() {
            let is_generic = !fn_info.type_params.is_empty();
            // #989: Always check arity, even for generic functions.
            if fn_info.params.len() != arg_tys.len() {
                self.emit(CheckError::WrongArgCount {
                    name: name.to_string(),
                    expected: fn_info.params.len(),
                    found: arg_tys.len(),
                    span,
                });
                return fn_info.ret.clone();
            }

            // #1066: For generic functions, instantiate type parameters before checking.
            // Build a substitution map (T → concrete type) from explicit type args or
            // by inferring from argument types, then substitute into param and return types.
            let (inst_params, inst_ret): (Vec<Ty>, Ty) = if is_generic {
                let subst: HashMap<String, Ty> = if !type_args.is_empty() {
                    // Explicit type arguments: match positionally to type_params.
                    fn_info
                        .type_params
                        .iter()
                        .zip(type_args.iter())
                        .map(|(tp_name, te)| (tp_name.clone(), resolve(te)))
                        .collect()
                } else {
                    // Infer from argument types by structural matching.
                    let mut s = HashMap::new();
                    infer_type_params(&fn_info.type_params, &fn_info.params, &arg_tys, &mut s);
                    s
                };
                let inst_params = fn_info
                    .params
                    .iter()
                    .map(|ty| substitute_ty(ty, &subst, &fn_info.type_params))
                    .collect();
                let inst_ret = substitute_ty(&fn_info.ret, &subst, &fn_info.type_params);
                (inst_params, inst_ret)
            } else {
                (fn_info.params.clone(), fn_info.ret.clone())
            };

            // Type check arguments against instantiated param types.
            // #1007: Strip arg label only when the (instantiated) param is bare.
            for (i, (expected, found)) in inst_params.iter().zip(arg_tys.iter()).enumerate() {
                let found_check = if ifc::label_of(expected).is_none() {
                    ifc::strip_label(found)
                } else {
                    found
                };
                if !self.types_compatible_resolved(expected, found_check) {
                    self.emit(CheckError::TypeMismatch {
                        expected: expected.display(),
                        found: found.display(),
                        span: args[i].span(),
                    });
                }
            }

            // Req 7/8: Effect propagation — caller must declare all effects of callee.
            // Req 3: Parametrized effects — declared `/data` covers required `/data/file.txt`
            // (prefix subsetting via `effect_satisfies`).
            for required in &fn_info.effects {
                let covered = self
                    .fn_context()
                    .effects
                    .iter()
                    .any(|declared| self.effect_satisfies(declared, required));
                if !covered {
                    if self.fn_context().effects.is_empty() {
                        // Pure function calling effectful one (#19)
                        self.emit(CheckError::UndeclaredEffect {
                            callee: name.to_string(),
                            effect: required.to_string(),
                            span,
                        });
                    } else {
                        // Caller has some effects but not this one (#20)
                        self.emit(CheckError::MissingEffect {
                            caller: self.fn_context().fn_name.clone(),
                            callee: name.to_string(),
                            effect: required.to_string(),
                            span,
                        });
                    }
                }
            }

            // #1068 Gap 3: accumulate callee effects into the innermost lambda body.
            if let Some(acc) = self.lambda_body_effects.last_mut() {
                acc.extend(fn_info.effects.iter().cloned());
            }

            // Req 8: Total function must not call partial functions.
            if matches!(fn_info.totality, Some(Totality::Partial))
                && !matches!(self.fn_context().totality, Some(Totality::Partial))
            {
                self.emit(CheckError::PartialCallInTotal {
                    callee: name.to_string(),
                    span,
                });
            }

            // #1007: all functions propagate labels unconditionally.
            //
            // Propagate only the EXCESS label from each argument — the label that exceeds
            // the instantiated parameter type.  This ensures:
            //   - format("{}", secret)  → Secret[String]  (param=String, arg=Secret[String]: excess=Secret)
            //   - write(p, tainted)     → Result[Unit,E]  (param=Tainted[String], arg=Tainted[String]: no excess)
            //   - hash_password(clean)  → Secret[String]  (no excess from arg, but ret has Secret label)
            //   - identity(secret)      → Secret[String]  (T=Secret[String]: inst_ret=Secret[String])
            //
            // For variadic builtins (params=[]) all argument labels are propagated directly.
            // The excess is then joined with the return type's own label, and applied to the
            // stripped (unlabeled) return type.
            let arg_label = if inst_params.is_empty() {
                // No declared params: propagate all arg labels.
                arg_tys.iter().fold(None, |acc, ty| {
                    ifc::join_opt(acc, ifc::label_of(ty).map(|s| s.to_string()))
                })
            } else {
                // Non-variadic: propagate label from args where instantiated param is bare.
                // `Unknown` params (e.g. builtin generics stored as Unknown) are treated as
                // non-propagating — we don't know the declared type so we don't assume bare.
                inst_params
                    .iter()
                    .zip(arg_tys.iter())
                    .fold(None, |acc, (param_ty, arg_ty)| {
                        if ifc::label_of(param_ty).is_none() && !matches!(param_ty, Ty::Unknown) {
                            let excess = ifc::label_of(arg_ty).map(|s| s.to_string());
                            ifc::join_opt(acc, excess)
                        } else {
                            acc
                        }
                    })
            };
            let ret_label = ifc::label_of(&inst_ret).map(|s| s.to_string());
            let combined = ifc::join_opt(arg_label, ret_label);
            ifc::apply_label(combined, ifc::strip_label(&inst_ret).clone())
        } else {
            // ── Built-in enum constructors ────────────────────────────────
            // These are not in the function table but are valid expressions.
            let arg_count = arg_tys.len();
            // Use first().cloned() rather than into_iter().next() so that arg_tys is
            // still available below for HOF per-argument type checking.
            let first_arg = arg_tys.first().cloned().unwrap_or(Ty::Unknown);
            match name {
                "Some" => return Ty::Option(Box::new(first_arg)),
                "Ok" => return Ty::Result(Box::new(first_arg), Box::new(Ty::Unknown)),
                "Err" => return Ty::Result(Box::new(Ty::Unknown), Box::new(first_arg)),
                // Byte constructors:
                //   from_int(n)          — safe, requires n ∈ [0, 255] (prover-enforced)
                //   wrapping_from_int(n) — intentional truncation, any Int
                "from_int" | "wrapping_from_int" => {
                    if arg_count != 1 {
                        self.emit(CheckError::WrongArgCount {
                            name: name.to_string(),
                            expected: 1,
                            found: arg_count,
                            span,
                        });
                    } else if !matches!(first_arg, Ty::Int) {
                        self.emit(CheckError::TypeMismatch {
                            expected: "Int".to_string(),
                            found: first_arg.display(),
                            span,
                        });
                    }
                    return Ty::Byte;
                }
                // Box::new(x) wraps x in a heap-allocated Box<T> (for recursive ADTs)
                "Box::new" => {
                    if arg_count != 1 {
                        self.emit(CheckError::WrongArgCount {
                            name: "Box::new".to_string(),
                            expected: 1,
                            found: arg_count,
                            span,
                        });
                    }
                    return Ty::Named("Box".to_string(), vec![first_arg]);
                }
                _ => {}
            }
            // User-defined enum tuple-variant constructor (bare or path form).
            // When qualified (`Type::Variant`), prefer the specific type to avoid
            // returning the wrong enum when multiple enums share a variant name
            // (e.g. JsonError::ParseError vs TomlError::ParseError — #822).
            if let Some((type_prefix, variant)) = name.split_once("::") {
                if let Some(type_info) = self.env.types.get(type_prefix) {
                    if let TypeBodyInfo::Enum(variants) = &type_info.body {
                        if variants.iter().any(|v| v.name == variant) {
                            return Ty::Named(type_prefix.to_string(), vec![]);
                        }
                    }
                }
            }
            let variant_name = name.split_once("::").map_or(name, |(_, v)| v);
            if let Some(enum_ty) = self.lookup_enum_for_variant(variant_name) {
                return enum_ty;
            }
            // HOF: `name` may be a local variable with a function type fn(T...) -> R.
            // This covers stdlib patterns like `map(xs, f)` where `f: fn(T) -> U`.
            // resolve_alias() dereferences fn-type aliases (type Pred = fn(Int) -> Bool) — #953.
            let hof_var_ty = self
                .env
                .lookup(name)
                .map(|vi| self.resolve_alias(vi.ty.clone()));
            if let Some(hof_fn) = hof_var_ty.and_then(|ty| {
                if let Ty::Fn(pts, rt, effects, totality) = ty {
                    Some((pts, *rt, effects, totality))
                } else {
                    None
                }
            }) {
                let (param_tys, ret_ty, hof_effects, hof_totality) = hof_fn;
                if arg_count != param_tys.len() {
                    self.emit(CheckError::WrongArgCount {
                        name: name.to_string(),
                        expected: param_tys.len(),
                        found: arg_count,
                        span,
                    });
                } else {
                    // Check each argument type against the declared parameter type.
                    for (i, (expected, found)) in param_tys.iter().zip(arg_tys.iter()).enumerate() {
                        if !self.types_compatible_resolved(expected, found) {
                            self.emit(CheckError::TypeMismatch {
                                expected: expected.display(),
                                found: found.display(),
                                span: args[i].span(),
                            });
                        }
                    }
                }
                // Req 7: propagate callee effects — HOF call sites obey same rules as named calls.
                for required in &hof_effects {
                    let covered = self
                        .fn_context()
                        .effects
                        .iter()
                        .any(|declared| self.effect_satisfies(declared, required));
                    if !covered {
                        if self.fn_context().effects.is_empty() {
                            self.emit(CheckError::UndeclaredEffect {
                                callee: name.to_string(),
                                effect: required.to_string(),
                                span,
                            });
                        } else {
                            self.emit(CheckError::MissingEffect {
                                caller: self.fn_context().fn_name.clone(),
                                callee: name.to_string(),
                                effect: required.to_string(),
                                span,
                            });
                        }
                    }
                }
                // #1068 Gap 3: accumulate HOF callee effects into innermost lambda body.
                if let Some(acc) = self.lambda_body_effects.last_mut() {
                    acc.extend(hof_effects.iter().cloned());
                }
                // Req 8: total function must not call a partial HOF parameter.
                // Note: hof_totality is None when the HOF param was declared via TypeExpr::Fn
                // syntax (e.g. `f: fn(Int) -> Int`) because the parser does not yet support
                // totality annotations in function-type expressions. In that case this guard
                // is a no-op — a known Phase 1 gap tracked in #711.
                if matches!(hof_totality, Some(Totality::Partial))
                    && !matches!(self.fn_context().totality, Some(Totality::Partial))
                {
                    self.emit(CheckError::PartialCallInTotal {
                        callee: name.to_string(),
                        span,
                    });
                }
                return ret_ty;
            }
            // #928: Static method call `Type::method(args)` — resolve via method_table.
            // #1066: Instantiate generic static methods the same way as generic functions.
            if let Some((type_name, method_name)) = name.split_once("::") {
                if let Some(methods) = self.method_table.get(type_name) {
                    if let Some(fn_info) = methods.get(method_name).cloned() {
                        if !fn_info.type_params.is_empty() {
                            let mut subst = HashMap::new();
                            infer_type_params(
                                &fn_info.type_params,
                                &fn_info.params,
                                &arg_tys,
                                &mut subst,
                            );
                            return substitute_ty(&fn_info.ret, &subst, &fn_info.type_params);
                        }
                        return fn_info.ret.clone();
                    }
                }
            }
            // Not in function table — could be builtin or foreign; emit Unknown
            self.emit(CheckError::UndefinedFunction {
                name: name.to_string(),
                span,
            });
            Ty::Unknown
        }
    }

    // ── Method resolution (#43: string + collection ops) ─────────────────

    /// Resolve the return type of a method call based on the receiver type.
    ///
    /// All collection methods return `Option<T>` where there is any possibility
    /// of absence (e.g. `.get`, `.first`) — never panic on valid input.
    /// IFC labels on the receiver propagate to the result via `apply_label`.
    pub(super) fn infer_method_call(
        &mut self,
        recv_ty: &Ty,
        method: &str,
        arg_tys: &[Ty],
        span: Span,
    ) -> Ty {
        // Validate concat(other: String) — exactly one String argument.
        // Other String methods have flexible or zero args and don't need pre-validation here.
        if matches!(recv_ty.unlabeled(), Ty::String) && method == "concat" {
            if arg_tys.len() != 1 {
                self.emit(CheckError::WrongArgCount {
                    name: "String.concat".to_string(),
                    expected: 1,
                    found: arg_tys.len(),
                    span,
                });
                return Ty::Unknown;
            }
            if !matches!(arg_tys[0].unlabeled(), Ty::String) {
                self.emit(CheckError::TypeMismatch {
                    expected: "String".to_string(),
                    found: arg_tys[0].display(),
                    span,
                });
                return Ty::Unknown;
            }
        }
        // Join receiver label with all argument labels (Req 7: result sensitivity is
        // the join of all inputs, e.g. `public_str.replace("x", secret_arg)` → Secret<String>).
        let recv_label = ifc::label_of(recv_ty).map(|s| s.to_string());
        let arg_label = arg_tys.iter().fold(None, |acc, ty| {
            ifc::join_opt(acc, ifc::label_of(ty).map(|s| s.to_string()))
        });
        let label = ifc::join_opt(recv_label, arg_label);
        let base = recv_ty.unlabeled();
        let result = match base {
            Ty::Int => Self::int_method_ty(method),
            Ty::Bool => Self::bool_method_ty(method),
            Ty::Byte => Self::byte_method_ty(method),
            Ty::UByte => Self::ubyte_method_ty(method),
            Ty::UInt => Self::uint_method_ty(method),
            Ty::Float => Self::float_method_ty(method),
            Ty::String => Self::string_method_ty(method, arg_tys),
            Ty::List(elem_ty) => Self::list_method_ty(elem_ty.as_ref(), method, arg_tys),
            Ty::Option(inner) => Self::option_method_ty(inner.as_ref(), method, arg_tys),
            Ty::Result(ok_ty, _) => Self::result_method_ty(ok_ty.as_ref(), method, arg_tys),
            Ty::Map(k_ty, v_ty) => {
                Self::map_method_ty(k_ty.as_ref(), v_ty.as_ref(), method, arg_tys)
            }
            Ty::Set(t_ty) => Self::set_method_ty(t_ty.as_ref(), method, arg_tys),
            Ty::Named(type_name, _) => {
                // Req 7: calling any method on an actor type sends to its mailbox — requires Send (#1126).
                // Actor behaviors are not in method_table, so this check must come first.
                if self.actor_type_names.contains(type_name.as_str()) {
                    // actor_id() is a pure sync read of the handle's ID — no mailbox send (#1128).
                    if method == "actor_id" {
                        return Ty::Int;
                    }
                    let send_eff = Effect::new("Send", span);
                    let covered = self
                        .fn_context()
                        .effects
                        .iter()
                        .any(|declared| self.effect_satisfies(declared, &send_eff));
                    if !covered {
                        if self.fn_context().effects.is_empty() {
                            self.emit(CheckError::UndeclaredEffect {
                                callee: format!("{type_name}.{method}"),
                                effect: "Send".to_string(),
                                span,
                            });
                        } else {
                            self.emit(CheckError::MissingEffect {
                                caller: self.fn_context().fn_name.clone(),
                                callee: format!("{type_name}.{method}"),
                                effect: "Send".to_string(),
                                span,
                            });
                        }
                    }
                }
                // User-defined type-attached method (#868): look up method table.
                // Clone to release the borrow on `self` before calling self.emit().
                let method_info = self
                    .method_table
                    .get(type_name.as_str())
                    .and_then(|m| m.get(method))
                    .cloned();
                if let Some(method_info) = method_info {
                    // Arity: params[0] is `self` (implicit at the call site).
                    let expected_args = method_info.params.len().saturating_sub(1);
                    if expected_args != arg_tys.len() {
                        self.emit(CheckError::WrongArgCount {
                            name: format!("{type_name}.{method}"),
                            expected: expected_args,
                            found: arg_tys.len(),
                            span,
                        });
                        return method_info.ret.clone();
                    }
                    // Per-argument type check (skip self at index 0).
                    for (expected, found) in method_info.params[1..].iter().zip(arg_tys.iter()) {
                        if !self.types_compatible_resolved(expected, found) {
                            self.emit(CheckError::TypeMismatch {
                                expected: expected.display(),
                                found: found.display(),
                                span,
                            });
                        }
                    }
                    // Effect propagation: caller must declare all effects of this method.
                    for required in &method_info.effects {
                        let covered = self
                            .fn_context()
                            .effects
                            .iter()
                            .any(|declared| self.effect_satisfies(declared, required));
                        if !covered {
                            if self.fn_context().effects.is_empty() {
                                self.emit(CheckError::UndeclaredEffect {
                                    callee: format!("{type_name}.{method}"),
                                    effect: required.to_string(),
                                    span,
                                });
                            } else {
                                self.emit(CheckError::MissingEffect {
                                    caller: self.fn_context().fn_name.clone(),
                                    callee: format!("{type_name}.{method}"),
                                    effect: required.to_string(),
                                    span,
                                });
                            }
                        }
                    }
                    // #1068 Gap 3: accumulate method effects into innermost lambda body.
                    if let Some(acc) = self.lambda_body_effects.last_mut() {
                        acc.extend(method_info.effects.iter().cloned());
                    }
                    // Totality: total caller must not call partial method.
                    if matches!(method_info.totality, Some(Totality::Partial))
                        && !matches!(self.fn_context().totality, Some(Totality::Partial))
                    {
                        self.emit(CheckError::PartialCallInTotal {
                            callee: format!("{type_name}.{method}"),
                            span,
                        });
                    }
                    method_info.ret.clone()
                } else {
                    // Unknown method on a type that HAS declared methods: emit a
                    // diagnostic (#875 review). Types with NO declared methods are
                    // left silent — they may use externally-injected behaviors
                    // (stub/adapter types like `UserStore` in auth_handler.mvl).
                    if self.method_table.contains_key(type_name.as_str()) {
                        self.emit(CheckError::UndefinedFunction {
                            name: format!("{type_name}.{method}"),
                            span,
                        });
                    }
                    Ty::Unknown
                }
            }
            _ => Ty::Unknown,
        };
        // #992 / #1433: For builtin types, if static dispatch returned Unknown,
        // check method_table for pure MVL extension methods declared in std/*.mvl.
        // This removes the 4-way sync requirement for pure MVL extension methods.
        let result = if matches!(result, Ty::Unknown) {
            let builtin_name: &str = match base {
                Ty::String => "String",
                Ty::Int => "Int",
                Ty::Float => "Float",
                Ty::Bool => "Bool",
                Ty::Byte => "Byte",
                Ty::UByte => "UByte",
                Ty::UInt => "UInt",
                Ty::List(_) => "List",
                Ty::Map(_, _) => "Map",
                Ty::Set(_) => "Set",
                Ty::Option(_) => "Option",
                Ty::Result(_, _) => "Result",
                _ => "",
            };
            if !builtin_name.is_empty() {
                if let Some(mi) = self
                    .method_table
                    .get(builtin_name)
                    .and_then(|m| m.get(method))
                    .cloned()
                {
                    let expected_args = mi.params.len().saturating_sub(1);
                    if expected_args != arg_tys.len() {
                        self.emit(CheckError::WrongArgCount {
                            name: format!("{builtin_name}.{method}"),
                            expected: expected_args,
                            found: arg_tys.len(),
                            span,
                        });
                        return mi.ret;
                    }
                    for (expected, found) in mi.params[1..].iter().zip(arg_tys.iter()) {
                        if !self.types_compatible_resolved(expected, found) {
                            self.emit(CheckError::TypeMismatch {
                                expected: expected.display(),
                                found: found.display(),
                                span,
                            });
                        }
                    }
                    let effects = mi.effects.clone();
                    for required in &effects {
                        let covered = self
                            .fn_context()
                            .effects
                            .iter()
                            .any(|declared| self.effect_satisfies(declared, required));
                        if !covered {
                            if self.fn_context().effects.is_empty() {
                                self.emit(CheckError::UndeclaredEffect {
                                    callee: format!("{builtin_name}.{method}"),
                                    effect: required.to_string(),
                                    span,
                                });
                            } else {
                                self.emit(CheckError::MissingEffect {
                                    caller: self.fn_context().fn_name.clone(),
                                    callee: format!("{builtin_name}.{method}"),
                                    effect: required.to_string(),
                                    span,
                                });
                            }
                        }
                    }
                    if let Some(acc) = self.lambda_body_effects.last_mut() {
                        acc.extend(effects.iter().cloned());
                    }
                    if matches!(mi.totality, Some(Totality::Partial))
                        && !matches!(self.fn_context().totality, Some(Totality::Partial))
                    {
                        self.emit(CheckError::PartialCallInTotal {
                            callee: format!("{builtin_name}.{method}"),
                            span,
                        });
                    }
                    mi.ret
                } else {
                    Ty::Unknown
                }
            } else {
                Ty::Unknown
            }
        } else {
            result
        };
        // #985: For closed builtin types, Ty::Unknown from the method dispatch means
        // "method not found" — emit a diagnostic instead of silently propagating Unknown.
        if matches!(result, Ty::Unknown)
            && matches!(
                base,
                Ty::Int
                    | Ty::Bool
                    | Ty::Byte
                    | Ty::UByte
                    | Ty::UInt
                    | Ty::Float
                    | Ty::String
                    | Ty::List(..)
                    | Ty::Map(..)
                    | Ty::Set(..)
                    | Ty::Option(..)
                    | Ty::Result(..)
            )
        {
            self.emit(CheckError::UnknownMethod {
                receiver_ty: base.display(),
                method: method.to_string(),
                span,
            });
        }
        // Only apply label when we resolved a concrete type.
        // Leaving Ty::Unknown unwrapped preserves the "Unknown = unresolved" sentinel;
        // wrapping it (e.g. Tainted<Unknown>) confuses downstream operators like `?`.
        if matches!(result, Ty::Unknown) {
            result
        } else {
            ifc::apply_label(label, result)
        }
    }
}
