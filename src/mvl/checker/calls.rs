// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Function and method call type inference for the MVL type checker.

use crate::mvl::checker::context::TypeBodyInfo;
use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::ifc;
use crate::mvl::checker::types::{types_compatible, Ty};
use crate::mvl::parser::ast::{Expr, Totality};
use crate::mvl::parser::lexer::Span;

use super::TypeChecker;

impl TypeChecker {
    // ── Function calls (#11) ──────────────────────────────────────────────

    pub(super) fn infer_fn_call(&mut self, name: &str, args: &[Expr], span: Span) -> Ty {
        // Infer all argument types (for side-effect error collection)
        let arg_tys: Vec<Ty> = args.iter().map(|a| self.infer_expr(a)).collect();

        if let Some(fn_info) = self.env.lookup_fn(name).cloned() {
            // 003-information-flow/Req 6: public I/O sinks MUST accept only bare types (#956).
            // Any labeled argument (Tainted, Secret, or user-defined) must be relabeled
            // before passing to a sink.  Driven by the declarative `sink` modifier.
            if fn_info.is_sink {
                for (arg, arg_ty) in args.iter().zip(arg_tys.iter()) {
                    if let Some(label) = ifc::label_of(arg_ty) {
                        self.emit(CheckError::LoggingLabelViolation {
                            label: label.to_string(),
                            span: arg.span(),
                        });
                    }
                }
            }
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
            // Type check: skip for generics (type params not yet substituted).
            if !is_generic {
                for (i, (expected, found)) in fn_info.params.iter().zip(arg_tys.iter()).enumerate()
                {
                    // ADR-0024: for label-transparent functions, strip the security label
                    // from the argument before type-checking ONLY when the parameter is
                    // bare (unlabeled). If the parameter is labeled, the argument must
                    // match the label exactly — no label stripping (#894).
                    let found_check =
                        if fn_info.label_transparent && ifc::label_of(expected).is_none() {
                            ifc::strip_label(found)
                        } else {
                            found
                        };
                    if !types_compatible(expected, found_check) {
                        self.emit(CheckError::TypeMismatch {
                            expected: expected.display(),
                            found: found.display(),
                            span: args[i].span(),
                        });
                    }
                }
            }

            // Req 7/8: Effect propagation — caller must declare all effects of callee.
            // Req 3: Parametrized effects — declared `/data` covers required `/data/file.txt`
            // (prefix subsetting via `effect_satisfies`).
            for required in &fn_info.effects {
                let covered = self
                    .current_fn_effects
                    .iter()
                    .any(|declared| self.effect_satisfies(declared, required));
                if !covered {
                    if self.current_fn_effects.is_empty() {
                        // Pure function calling effectful one (#19)
                        self.emit(CheckError::UndeclaredEffect {
                            callee: name.to_string(),
                            effect: required.to_string(),
                            span,
                        });
                    } else {
                        // Caller has some effects but not this one (#20)
                        self.emit(CheckError::MissingEffect {
                            caller: self.current_fn_name.clone(),
                            callee: name.to_string(),
                            effect: required.to_string(),
                            span,
                        });
                    }
                }
            }

            // Req 8: Total function must not call partial functions.
            if matches!(fn_info.totality, Some(Totality::Partial))
                && !matches!(self.current_fn_totality, Some(Totality::Partial))
            {
                self.emit(CheckError::PartialCallInTotal {
                    callee: name.to_string(),
                    span,
                });
            }

            // L5-08: for generic functions the declared return type is a type-parameter
            // name (e.g. `T`), not a concrete type.  Return Unknown so the call site
            // unifies freely with any annotation or context type.
            // This check must come BEFORE label_transparent to avoid applying label
            // propagation to an unresolved type variable.
            if is_generic {
                return Ty::Unknown;
            }
            // ADR-0024: all functions are label-transparent by default (universal propagation).
            //
            // Propagate only the EXCESS label from each argument — the label that exceeds
            // the function's declared parameter type.  This ensures:
            //   - format("{}", secret)  → Secret[String]  (param=String, arg=Secret[String]: excess=Secret)
            //   - write(p, tainted)     → Result[Unit,E]  (param=Tainted[String], arg=Tainted[String]: no excess)
            //   - hash_password(clean)  → Secret[String]  (no excess from arg, but ret has Secret label)
            //
            // For variadic builtins (params=[]) all argument labels are propagated directly.
            // The excess is then joined with the return type's own label, and applied to the
            // stripped (unlabeled) return type.
            if fn_info.label_transparent {
                // In the user-defined label model (#894), label-transparent functions
                // propagate labels from bare-typed arguments to the return type.
                // Excess = arg has a label and param is bare (no label declared).
                let arg_label = if fn_info.params.is_empty() {
                    // No declared params: propagate all arg labels.
                    arg_tys.iter().fold(None, |acc, ty| {
                        ifc::join_opt(acc, ifc::label_of(ty).map(|s| s.to_string()))
                    })
                } else {
                    // Non-variadic: propagate label from args where param is bare.
                    fn_info.params.iter().zip(arg_tys.iter()).fold(
                        None,
                        |acc, (param_ty, arg_ty)| {
                            // Only propagate if param has no label (bare parameter).
                            if ifc::label_of(param_ty).is_none() {
                                let excess = ifc::label_of(arg_ty).map(|s| s.to_string());
                                ifc::join_opt(acc, excess)
                            } else {
                                acc
                            }
                        },
                    )
                };
                let ret_label = ifc::label_of(&fn_info.ret).map(|s| s.to_string());
                let combined = ifc::join_opt(arg_label, ret_label);
                return ifc::apply_label(combined, ifc::strip_label(&fn_info.ret).clone());
            }
            fn_info.ret.clone()
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
                // Byte constructor: from_int(n: Int) -> Byte  (wrapping cast)
                "from_int" => {
                    if arg_count != 1 {
                        self.emit(CheckError::WrongArgCount {
                            name: "from_int".to_string(),
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
                        if !types_compatible(expected, found) {
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
                        .current_fn_effects
                        .iter()
                        .any(|declared| self.effect_satisfies(declared, required));
                    if !covered {
                        if self.current_fn_effects.is_empty() {
                            self.emit(CheckError::UndeclaredEffect {
                                callee: name.to_string(),
                                effect: required.to_string(),
                                span,
                            });
                        } else {
                            self.emit(CheckError::MissingEffect {
                                caller: self.current_fn_name.clone(),
                                callee: name.to_string(),
                                effect: required.to_string(),
                                span,
                            });
                        }
                    }
                }
                // Req 8: total function must not call a partial HOF parameter.
                // Note: hof_totality is None when the HOF param was declared via TypeExpr::Fn
                // syntax (e.g. `f: fn(Int) -> Int`) because the parser does not yet support
                // totality annotations in function-type expressions. In that case this guard
                // is a no-op — a known Phase 1 gap tracked in #711.
                if matches!(hof_totality, Some(Totality::Partial))
                    && !matches!(self.current_fn_totality, Some(Totality::Partial))
                {
                    self.emit(CheckError::PartialCallInTotal {
                        callee: name.to_string(),
                        span,
                    });
                }
                return ret_ty;
            }
            // #928: Static method call `Type::method(args)` — resolve via method_table.
            if let Some((type_name, method_name)) = name.split_once("::") {
                if let Some(methods) = self.method_table.get(type_name) {
                    if let Some(fn_info) = methods.get(method_name) {
                        let is_generic = !fn_info.type_params.is_empty();
                        if is_generic {
                            return Ty::Unknown;
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
            Ty::Byte => Self::byte_method_ty(method),
            Ty::UByte => Self::ubyte_method_ty(method),
            Ty::UInt => Self::uint_method_ty(method),
            Ty::Float => Self::float_method_ty(method),
            Ty::String => Self::string_method_ty(method, arg_tys),
            Ty::List(elem_ty) => Self::list_method_ty(elem_ty.as_ref(), method, arg_tys),
            Ty::Option(inner) => Self::option_method_ty(inner.as_ref(), method, arg_tys),
            Ty::Result(ok_ty, _) => Self::result_method_ty(ok_ty.as_ref(), method, arg_tys),
            Ty::Map(k_ty, v_ty) => Self::map_method_ty(k_ty.as_ref(), v_ty.as_ref(), method),
            Ty::Set(t_ty) => Self::set_method_ty(t_ty.as_ref(), method),
            Ty::Named(type_name, _) => {
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
                        if !types_compatible(expected, found) {
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
                            .current_fn_effects
                            .iter()
                            .any(|declared| self.effect_satisfies(declared, required));
                        if !covered {
                            if self.current_fn_effects.is_empty() {
                                self.emit(CheckError::UndeclaredEffect {
                                    callee: format!("{type_name}.{method}"),
                                    effect: required.to_string(),
                                    span,
                                });
                            } else {
                                self.emit(CheckError::MissingEffect {
                                    caller: self.current_fn_name.clone(),
                                    callee: format!("{type_name}.{method}"),
                                    effect: required.to_string(),
                                    span,
                                });
                            }
                        }
                    }
                    // Totality: total caller must not call partial method.
                    if matches!(method_info.totality, Some(Totality::Partial))
                        && !matches!(self.current_fn_totality, Some(Totality::Partial))
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
        // #985: For closed builtin types, Ty::Unknown from the method dispatch means
        // "method not found" — emit a diagnostic instead of silently propagating Unknown.
        if matches!(result, Ty::Unknown)
            && matches!(
                base,
                Ty::Int
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
