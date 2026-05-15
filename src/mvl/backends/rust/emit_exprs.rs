// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emit Rust expressions from MVL [`Expr`] nodes.

use crate::mvl::backends::rust::emit_stmts::emit_mcdc_guard_block;
use crate::mvl::backends::rust::emit_types::{emit_label, emit_type_expr};
use crate::mvl::backends::rust::emitter::RustEmitter;
use crate::mvl::backends::rust::mcdc_instr::DecisionKind;
use crate::mvl::checker::types::Ty;
use crate::mvl::parser::ast::{BinaryOp, Expr, Literal, MatchArm, MatchBody, Pattern, UnaryOp};
use crate::mvl::passes::coverage::BranchKind;
use crate::mvl::passes::mcdc::analysis::count_clauses_ref;

/// Methods implemented as pure MVL functions in std/strings.mvl and std/lists.mvl.
///
/// When the transpiler sees `receiver.method(args)` for one of these names it
/// emits a UFCS free-function call instead: `method(receiver.clone().into(), args)`.
/// The `.into()` coercion allows IFC-label wrapper types (`Clean<String>`, etc.) to
/// flow into functions that take the plain inner type — `From<Label<T>> for T` is
/// implemented in `mvl_runtime::ifc`.
///
/// Phase 4 (ADR-0003): replaces per-method hardcoded Rust emission with an explicit
/// trust boundary declared as `pub builtin fn` in std/strings.mvl and std/lists.mvl.
const STDLIB_UFCS_METHODS: &[&str] = &[
    // std/strings.mvl
    "trim",
    "to_upper",
    "to_lower",
    "chars",
    "concat",
    "starts_with",
    "ends_with",
    "find",
    "replace",
    "split",
    "substring",
    "parse_int",
    "parse_float",
    // std/lists.mvl
    "slice",
    "take",
    "skip",
    "first",
    "last",
    "flatten",
    "reverse",
];

/// String methods that return a `String` with the same IFC label as their receiver.
/// When the receiver is `Label<String>`, the call result must be re-wrapped in `Label::new(…)`
/// because the UFCS trampoline (`method(receiver.clone().into(), …)`) strips the label via
/// `.into()` before passing to the stdlib function (which returns plain `String`).
const STRING_LABEL_PRESERVING_METHODS: &[&str] = &[
    "trim",
    "to_upper",
    "to_lower",
    "concat",
    "replace",
    "substring",
];

/// Emit an expression into the code buffer (no trailing newline).
pub fn emit_expr(cg: &mut RustEmitter, expr: &Expr) {
    match expr {
        Expr::Literal(lit, span) => {
            // Mutation mode: inject env-var dispatch for Bool and Integer literals.
            if cg.mutation.is_some() {
                match lit {
                    Literal::Bool(b) => {
                        if let Some(mid) = cg.alloc_bool_mutation(*b, span.line) {
                            let (alt, orig) = if *b {
                                ("false", "true")
                            } else {
                                ("true", "false")
                            };
                            cg.push(&format!(
                                r#"{{ match ::std::env::var("MVL_MUTANT").as_deref() {{ Ok("{mid}") => {alt}, _ => {orig} }} }}"#
                            ));
                            return;
                        }
                    }
                    Literal::Integer(n) => {
                        if let Some(int_variants) = cg.alloc_int_mutations(*n, span.line) {
                            cg.push("{ match ::std::env::var(\"MVL_MUTANT\").as_deref() {");
                            for (mid, alt) in &int_variants {
                                cg.push(&format!(" Ok(\"{mid}\") => {alt},"));
                            }
                            cg.push(&format!(" _ => {n} }}"));
                            cg.push(" }");
                            return;
                        }
                    }
                    _ => {}
                }
            }
            emit_literal(cg, lit);
        }
        Expr::Ident(name, _) => cg.push(&map_ident(name)),
        Expr::FieldAccess { expr, field, .. } => {
            emit_expr(cg, expr);
            cg.push(".");
            cg.push(field);
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            // Methods that don't map directly to a Rust method of the same name.
            //
            // Phase 4 note: string and list methods that have pure MVL implementations
            // in std/strings.mvl and std/lists.mvl are dispatched via UFCS in the `_`
            // fallback below: `s.trim()` → `trim(s)`, `xs.take(n)` → `take(xs, n)`.
            // The actual Rust implementation is the transpiled MVL stdlib function.
            match method.as_str() {
                // ── Higher-order collection methods ──────────────────────────────

                // map(f) — direct Rust dispatch using checker type info (#554).
                // Option/Result use .map(); List and unknown types use into_iter().collect().
                "map" if args.len() == 1 => {
                    let receiver_ty = cg.expr_types.get(&receiver.span()).cloned();
                    match receiver_ty.as_ref() {
                        Some(Ty::Option(_)) | Some(Ty::Result(_, _)) => {
                            emit_expr(cg, receiver);
                            cg.push(".map(|__x| (");
                            emit_expr(cg, &args[0]);
                            cg.push(")(__x.clone()))");
                        }
                        _ => {
                            // List (and unknown types) use into_iter().collect().
                            // Set.map is not a valid MVL operation (checker returns Ty::Unknown),
                            // so this arm is only reached for List receivers in valid programs.
                            emit_expr(cg, receiver);
                            cg.push(".into_iter().map(|__x| (");
                            emit_expr(cg, &args[0]);
                            cg.push(")(__x.clone())).collect::<Vec<_>>()");
                        }
                    }
                }

                // ── Pure MVL higher-order list methods ────────────────────────────
                //
                // Emitted as Rust native iterator chains rather than UFCS calls to
                // std/lists.mvl free functions.  This allows the predicate/mapper
                // argument to be ANY callable (fn pointer or capturing closure),
                // because Rust's .filter() / .fold() etc. accept FnMut, not just
                // bare fn pointers.
                //
                // filter / take_while / skip_while — predicate applied to a clone
                // of each element; result collected back into Vec.
                "filter" | "take_while" | "skip_while" if args.len() == 1 => {
                    let needs_borrow = if let Expr::Ident(name, _) = &args[0] {
                        cg.capability_params_map
                            .get(name.as_str())
                            .and_then(|b| b.first().copied())
                            .flatten()
                            .is_some()
                    } else {
                        false
                    };
                    emit_expr(cg, receiver);
                    cg.push(".clone().into_iter().");
                    cg.push(method);
                    cg.push("(|__x| (");
                    emit_expr(cg, &args[0]);
                    if needs_borrow {
                        cg.push(")(&__x.clone())).collect::<Vec<_>>()");
                    } else {
                        cg.push(")(__x.clone())).collect::<Vec<_>>()");
                    }
                }
                // any / all — same predicate pattern but return bool, no collect.
                "any" | "all" if args.len() == 1 => {
                    let needs_borrow = if let Expr::Ident(name, _) = &args[0] {
                        cg.capability_params_map
                            .get(name.as_str())
                            .and_then(|b| b.first().copied())
                            .flatten()
                            .is_some()
                    } else {
                        false
                    };
                    emit_expr(cg, receiver);
                    cg.push(".clone().into_iter().");
                    cg.push(method);
                    cg.push("(|__x| (");
                    emit_expr(cg, &args[0]);
                    if needs_borrow {
                        cg.push(")(&__x.clone()))");
                    } else {
                        cg.push(")(__x.clone()))");
                    }
                }
                // fold(init, f) — init cloned (value arg); f wrapped in closure
                // so capturing closures are accepted alongside fn pointers.
                // When f is a named function with borrow params, add & to the
                // accumulator and/or element in the generated lambda.
                "fold" if args.len() == 2 => {
                    let (borrow_acc, borrow_elem) = if let Expr::Ident(name, _) = &args[1] {
                        let borrows = cg
                            .capability_params_map
                            .get(name.as_str())
                            .cloned()
                            .unwrap_or_default();
                        let b0 = borrows.first().copied().flatten().is_some();
                        let b1 = borrows.get(1).copied().flatten().is_some();
                        (b0, b1)
                    } else {
                        (false, false)
                    };
                    emit_expr(cg, receiver);
                    cg.push(".clone().into_iter().fold(");
                    emit_expr_as_arg(cg, &args[0]);
                    cg.push(", |acc, __x| (");
                    emit_expr(cg, &args[1]);
                    cg.push(")(");
                    if borrow_acc {
                        cg.push("&");
                    }
                    cg.push("acc, ");
                    if borrow_elem {
                        cg.push("&");
                    }
                    cg.push("__x))");
                }
                // windows(n)/chunks(n) — Rust returns &[T] slices; collect into Vec<Vec<T>>
                "windows" | "chunks" => {
                    emit_expr(cg, receiver);
                    cg.push(".");
                    cg.push(method);
                    cg.push("(");
                    emit_args(cg, args);
                    cg.push(").map(|w| w.to_vec()).collect::<Vec<_>>()");
                }
                // partition(f) — turbofish needed so Rust can infer the element type
                "partition" => {
                    emit_expr(cg, receiver);
                    cg.push(".into_iter().partition::<Vec<_>, _>(|__x| ");
                    if let Some(arg) = args.first() {
                        emit_expr(cg, arg);
                    }
                    cg.push("(__x.clone()))");
                }
                // group_by(f) — no native Rust equivalent; fold into HashMap
                "group_by" => {
                    cg.push("{ let mut __m = std::collections::HashMap::new(); for __v in ");
                    emit_expr(cg, receiver);
                    cg.push(".into_iter() { __m.entry(");
                    if let Some(arg) = args.first() {
                        // Phase B: if the key function takes a reference for its first
                        // parameter, emit `&__v.clone()` instead of `__v.clone()`.
                        let needs_borrow = if let Expr::Ident(name, _) = arg {
                            cg.capability_params_map
                                .get(name.as_str())
                                .and_then(|b| b.first().copied())
                                .flatten()
                                .is_some()
                        } else {
                            false
                        };
                        emit_expr(cg, arg);
                        if needs_borrow {
                            cg.push("(&__v.clone())");
                        } else {
                            cg.push("(__v.clone())");
                        }
                    }
                    cg.push(").or_insert_with(Vec::new).push(__v); } __m }");
                }
                // and_then(f) — Option<T> and Result<T,E>
                "and_then" if args.len() == 1 => {
                    emit_expr(cg, receiver);
                    cg.push(".and_then(|__x| (");
                    emit_expr(cg, &args[0]);
                    cg.push(")(__x.clone()))");
                }
                // sort() — sort_by with partial_cmp for numeric stability
                "sort" if args.is_empty() => {
                    cg.push("{let mut __v=(");
                    emit_expr(cg, receiver);
                    cg.push(");__v.sort_by(|__a,__b|__a.partial_cmp(__b).unwrap_or(std::cmp::Ordering::Equal));__v}");
                }

                // ── Operator-level methods ────────────────────────────────────────
                //
                // Bitwise ops on Int/Byte: emitted as Rust operators for LLVM
                // visibility and future intrinsic optimisation.
                "bit_and" if args.len() == 1 => {
                    cg.push("(");
                    emit_expr(cg, receiver);
                    cg.push(" & ");
                    emit_expr(cg, &args[0]);
                    cg.push(")");
                }
                "bit_or" if args.len() == 1 => {
                    cg.push("(");
                    emit_expr(cg, receiver);
                    cg.push(" | ");
                    emit_expr(cg, &args[0]);
                    cg.push(")");
                }
                "bit_xor" if args.len() == 1 => {
                    cg.push("(");
                    emit_expr(cg, receiver);
                    cg.push(" ^ ");
                    emit_expr(cg, &args[0]);
                    cg.push(")");
                }
                "bit_not" if args.is_empty() => {
                    cg.push("(!");
                    emit_expr(cg, receiver);
                    cg.push(")");
                }
                // wrapping_shl/shr avoids debug-mode panic for out-of-range shift counts
                "shift_left" if args.len() == 1 => {
                    cg.push("(");
                    emit_expr(cg, receiver);
                    cg.push(".wrapping_shl(");
                    emit_expr(cg, &args[0]);
                    cg.push(" as u32))");
                }
                "shift_right" if args.len() == 1 => {
                    cg.push("(");
                    emit_expr(cg, receiver);
                    cg.push(".wrapping_shr(");
                    emit_expr(cg, &args[0]);
                    cg.push(" as u32))");
                }
                // is_zero() — i64 has no is_zero(); emit comparison
                "is_zero" if args.is_empty() => {
                    cg.push("(");
                    emit_expr(cg, receiver);
                    cg.push(" == 0)");
                }
                // to_int() on Byte (u8→i64) or Float (f64→i64, truncating)
                "to_int" if args.is_empty() => {
                    cg.push("(");
                    emit_expr(cg, receiver);
                    cg.push(" as i64)");
                }
                // to_float() on Int (i64→f64); i64::from() unwraps IFC labels transparently
                "to_float" if args.is_empty() => {
                    cg.push("(i64::from(");
                    emit_expr(cg, receiver);
                    cg.push(".clone()) as f64)");
                }
                // pow(e) — direct Rust using checker type info (#554).
                // i64: .pow(e as u32); f64: .powf(e).
                "pow" if args.len() == 1 => {
                    let receiver_ty = cg.expr_types.get(&receiver.span()).cloned();
                    emit_expr(cg, receiver);
                    match receiver_ty.as_ref() {
                        Some(Ty::Float) => {
                            cg.push(".powf(");
                            emit_expr_as_arg(cg, &args[0]);
                            cg.push(")");
                        }
                        _ => {
                            cg.push(".pow(");
                            emit_expr_as_arg(cg, &args[0]);
                            cg.push(" as u32)");
                        }
                    }
                }
                // clamp(low, high) — Rust's clamp panics on inverted bounds; safe wrapper
                "clamp" if args.len() == 2 => {
                    emit_safe_clamp(cg, receiver, &args[0], &args[1]);
                }
                // contains(x) — direct Rust using checker type info (#554).
                // String: .contains(arg.as_str()); List/Set: .contains(&arg).
                "contains" if args.len() == 1 => {
                    let receiver_ty = cg.expr_types.get(&receiver.span()).cloned();
                    emit_expr(cg, receiver);
                    match receiver_ty.as_ref() {
                        Some(Ty::String) => {
                            // emit_args_no_into avoids .into() before .as_str().
                            // (x.into()).as_str() is ambiguous (E0282) when the arg
                            // is a String variable — Rust cannot infer the Into target
                            // without a constraining function parameter type.
                            cg.push(".contains((");
                            emit_args_no_into(cg, args);
                            cg.push(").as_str())");
                        }
                        _ => {
                            cg.push(".contains(&(");
                            emit_args(cg, args);
                            cg.push("))");
                        }
                    }
                }

                // ── Map / Set / List unified method traits ────────────────────────
                //
                // These methods share a name across multiple collection types (Map,
                // List) or need special Rust handling (cloned(), as i64 cast, etc.).
                // Trait-dispatch lets Rust pick the right impl at compile time without
                // the transpiler needing type information about the receiver.

                // get(key) — direct Rust using checker type info (#554).
                // Map: .get(&key).cloned(); List: bounds-checked index.
                "get" if args.len() == 1 => {
                    let receiver_ty = cg.expr_types.get(&receiver.span()).cloned();
                    match receiver_ty.as_ref() {
                        Some(Ty::Map(_, _)) => {
                            emit_expr(cg, receiver);
                            cg.push(".get(&(");
                            emit_expr(cg, &args[0]);
                            cg.push(").clone()).cloned()");
                        }
                        _ => {
                            cg.push("{ let __mvl_i = (");
                            emit_expr(cg, &args[0]);
                            cg.push("); if __mvl_i < 0 { None } else { (");
                            emit_expr(cg, receiver);
                            cg.push(").get(__mvl_i as usize).cloned() } }");
                        }
                    }
                }

                // len() — direct Rust using checker type info (#554).
                // String: .chars().count() as i64; List/Map/Set: .len() as i64.
                // Labeled types: propagate label via field access.
                "len" if args.is_empty() => {
                    let receiver_ty = cg.expr_types.get(&receiver.span()).cloned();
                    emit_len_direct(cg, receiver, receiver_ty.as_ref());
                }

                // insert(k, v) — Map: emit HashMap::insert (returns Option, discarded).
                "insert" if args.len() == 2 => {
                    cg.push("{ let _ = ");
                    emit_expr(cg, receiver);
                    cg.push(".insert(");
                    emit_expr_as_arg(cg, &args[0]);
                    cg.push(", ");
                    emit_expr_as_arg(cg, &args[1]);
                    cg.push("); }");
                }

                // insert(x) — Set: emit HashSet::insert (returns bool, discarded).
                "insert" if args.len() == 1 => {
                    cg.push("{ let _ = ");
                    emit_expr(cg, receiver);
                    cg.push(".insert(");
                    emit_expr_as_arg(cg, &args[0]);
                    cg.push("); }");
                }

                // remove(key) — Map: HashMap::remove returns Option<V> (correct for MVL).
                //               Set: HashSet::remove returns bool (discarded as stmt).
                "remove" if args.len() == 1 => {
                    emit_expr(cg, receiver);
                    cg.push(".remove(&(");
                    emit_expr(cg, &args[0]);
                    cg.push(").clone())");
                }

                // contains_key(k) — Map-only. Borrows key for HashMap::contains_key.
                "contains_key" if args.len() == 1 => {
                    emit_expr(cg, receiver);
                    cg.push(".contains_key(&(");
                    emit_expr(cg, &args[0]);
                    cg.push(").clone())");
                }

                // keys() — Map: collect HashMap::keys() iterator into Vec.
                "keys" if args.is_empty() => {
                    emit_expr(cg, receiver);
                    cg.push(".keys().cloned().collect::<Vec<_>>()");
                }

                // values() — Map: collect HashMap::values() iterator into Vec.
                "values" if args.is_empty() => {
                    emit_expr(cg, receiver);
                    cg.push(".values().cloned().collect::<Vec<_>>()");
                }

                // to_list() — Set: collect HashSet::iter() into Vec.
                "to_list" if args.is_empty() => {
                    emit_expr(cg, receiver);
                    cg.push(".iter().cloned().collect::<Vec<_>>()");
                }

                // is_empty() — Vec, HashMap, HashSet all have is_empty() → bool. ✓
                // Falls through to generic dispatch below (no special handling needed).

                // intersection(b) / union(b) / difference(b) — Set operations.
                // These return iterators; collect into HashSet.
                "intersection" if args.len() == 1 => {
                    let b = &args[0];
                    cg.push("{ let __b = ");
                    emit_expr(cg, b);
                    cg.push("; ");
                    emit_expr(cg, receiver);
                    cg.push(
                        ".intersection(&__b).cloned().collect::<std::collections::HashSet<_>>() }",
                    );
                }
                "union" if args.len() == 1 => {
                    let b = &args[0];
                    cg.push("{ let __b = ");
                    emit_expr(cg, b);
                    cg.push("; ");
                    emit_expr(cg, receiver);
                    cg.push(".union(&__b).cloned().collect::<std::collections::HashSet<_>>() }");
                }
                "difference" if args.len() == 1 => {
                    let b = &args[0];
                    cg.push("{ let __b = ");
                    emit_expr(cg, b);
                    cg.push("; ");
                    emit_expr(cg, receiver);
                    cg.push(
                        ".difference(&__b).cloned().collect::<std::collections::HashSet<_>>() }",
                    );
                }

                // push(elem) / extend(iter) / append(other) — collection mutators.
                //
                // emit_expr_as_fn_arg adds `.into()` for all ident args, which causes
                // E0283 when the element type is plain (e.g. Vec<i64>.push(n.into())):
                // Rust cannot infer which Into impl to use.  Only add `.into()` when
                // the receiver element type is labeled (e.g. Vec<Clean<String>>).
                "push" if args.len() == 1 => {
                    let elem_is_labeled = cg
                        .expr_types
                        .get(&receiver.span())
                        .is_some_and(|ty| matches!(ty, Ty::List(inner) if matches!(inner.as_ref(), Ty::Labeled(..))));
                    emit_expr(cg, receiver);
                    cg.push(".push(");
                    if elem_is_labeled {
                        emit_expr_as_fn_arg(cg, &args[0]);
                    } else {
                        emit_expr_as_arg(cg, &args[0]);
                    }
                    cg.push(")");
                }
                "extend" | "append" if args.len() == 1 => {
                    let elem_is_labeled = cg
                        .expr_types
                        .get(&receiver.span())
                        .is_some_and(|ty| matches!(ty, Ty::List(inner) if matches!(inner.as_ref(), Ty::Labeled(..))));
                    emit_expr(cg, receiver);
                    cg.push(".");
                    cg.push(method);
                    cg.push("(");
                    if elem_is_labeled {
                        emit_expr_as_fn_arg(cg, &args[0]);
                    } else {
                        emit_expr_as_arg(cg, &args[0]);
                    }
                    cg.push(")");
                }

                // ── UFCS dispatch for pure MVL stdlib methods ─────────────────────
                //
                // Methods implemented in std/strings.mvl and std/lists.mvl are
                // compiled to free Rust functions and emitted in the prelude of every
                // generated file. `s.trim()` → `trim(s.clone().into(), args)`.
                //
                // `.into()` after the receiver clone allows IFC-label wrapper types
                // (`Clean<String>`, `Public<String>`, etc.) to coerce into the plain
                // inner type expected by the MVL stdlib function. `From<Label<T>> for T`
                // is implemented in `mvl_runtime::ifc` for all label variants.
                //
                // For string-transforming methods (`to_lower`, `trim`, `concat`, etc.) the
                // checker propagates the receiver's IFC label to the result (the result is
                // as sensitive as the input).  But the UFCS trampoline strips the label via
                // `.into()`, so the raw call returns plain `String`.  We detect this case via
                // `expr_types` and re-wrap: `Label::new(method(receiver.clone().into(), …))`.
                m if STDLIB_UFCS_METHODS.contains(&m) => {
                    // Check whether we must re-wrap the result in a label newtype.
                    let wrap_label: Option<String> = if STRING_LABEL_PRESERVING_METHODS.contains(&m)
                    {
                        cg.expr_types.get(&receiver.span()).and_then(|ty| {
                            if let Ty::Labeled(label, inner) = ty {
                                if matches!(inner.as_ref(), Ty::String) {
                                    Some(emit_label(*label).to_string())
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        })
                    } else {
                        None
                    };
                    if let Some(ref lname) = wrap_label {
                        cg.push(&format!("{lname}::new("));
                    }
                    cg.push(method);
                    cg.push("(");
                    emit_expr(cg, receiver);
                    cg.push(".clone().into()");
                    if !args.is_empty() {
                        cg.push(", ");
                        emit_args(cg, args);
                    }
                    cg.push(")");
                    if wrap_label.is_some() {
                        cg.push(")");
                    }
                }

                // ── Generic Rust method fallthrough ───────────────────────────────
                _ => {
                    emit_expr(cg, receiver);
                    cg.push(".");
                    cg.push(method);
                    cg.push("(");
                    emit_args(cg, args);
                    cg.push(")");
                }
            }
        }
        Expr::FnCall {
            name,
            type_args,
            args,
            ..
        } => {
            // println!/print!/eprintln!/format! are Rust macros: first arg must be a bare string
            // literal, not a `.to_string()` expression.
            if matches!(
                name.as_str(),
                "println" | "print" | "eprintln" | "eprint" | "format" | "panic"
            ) {
                cg.push(&format!("{name}!"));
                cg.push("(");
                emit_args_for_macro(cg, args);
                cg.push(")");
            } else if matches!(name.as_str(), "assert_eq" | "assert_ne") {
                // assert_eq!/assert_ne! compare same-typed values.  String literal args
                // must NOT get `.into()` — it makes the type ambiguous in assert macros
                // because `String` has many cross-type `PartialEq` impls (PathBuf, Box<str>,
                // Arc<str>…) so Rust can't determine which `From<String>` to use.
                cg.push(&format!("{}!", name));
                cg.push("(");
                emit_args_no_into(cg, args);
                cg.push(")");
            } else if name.as_str() == "from_int" {
                // from_int(n: Int) -> Byte — wrapping cast i64 → u8.
                // The checker enforces exactly 1 Int argument; assert here as a
                // second line of defence so a missed check produces a clear panic
                // rather than silent `( as u8)` which would be a Rust syntax error.
                debug_assert_eq!(args.len(), 1, "from_int requires exactly one argument");
                cg.push("(");
                if let Some(arg) = args.first() {
                    emit_expr(cg, arg);
                }
                cg.push(" as u8)");
            } else {
                let is_extern = cg.extern_fns.contains(name.as_str());
                if is_extern {
                    cg.push("unsafe { ");
                }
                // Phase 8: free calls to actor methods from within the actor state
                // impl must be prefixed with `self.` (e.g. `log(seq)` → `self.log(seq)`).
                if !is_extern && cg.actor_methods.contains(name.as_str()) {
                    cg.push("self.");
                }
                // #420: Use fully-qualified path for stdlib functions that would
                // otherwise be shadowed by a locally-defined built-in of the same name.
                if let Some(qualified) = cg.stdlib_fn_qualified.get(name.as_str()).cloned() {
                    cg.push(&qualified);
                } else {
                    cg.push(&map_fn_name(name));
                }
                if !type_args.is_empty() {
                    cg.push("::<");
                    let strs: Vec<String> = type_args.iter().map(emit_type_expr).collect();
                    cg.push(&strs.join(", "));
                    cg.push(">");
                }
                cg.push("(");
                // Phase B: look up borrow flags for this callee so we can emit `&x`
                // instead of `x.clone()` for reference parameters.
                let borrows: Vec<Option<bool>> = cg
                    .capability_params_map
                    .get(name.as_str())
                    .cloned()
                    .unwrap_or_default();
                emit_args_with_borrows(cg, args, &borrows);
                cg.push(")");
                if is_extern {
                    cg.push(" }");
                }
            }
        }
        Expr::Borrow { mutable, expr, .. } => {
            // Fix 7: parenthesise compound inner expressions to preserve precedence.
            // e.g. `&(a + b)` must not emit `&a + b` (which Rust parses as `(&a) + b`).
            let needs_parens =
                !matches!(expr.as_ref(), Expr::Ident(_, _) | Expr::FieldAccess { .. });
            if *mutable {
                cg.push(if needs_parens { "&mut (" } else { "&mut " });
            } else {
                cg.push(if needs_parens { "&(" } else { "&" });
            }
            emit_expr(cg, expr);
            if needs_parens {
                cg.push(")");
            }
        }
        Expr::Unary { op, expr, .. } => match op {
            UnaryOp::Neg => {
                cg.push("-");
                emit_expr(cg, expr);
            }
            UnaryOp::Not => {
                cg.push("!");
                emit_expr(cg, expr);
            }
            UnaryOp::Deref => {
                cg.push("*(");
                emit_expr(cg, expr);
                cg.push(")");
            }
            UnaryOp::BitNot => {
                // Rust uses `!` for bitwise NOT on integer types.
                cg.push("!");
                emit_expr(cg, expr);
            }
        },
        Expr::Binary {
            op,
            left,
            right,
            span,
        } => {
            // Mutation mode: inject env-var dispatch for behavioral operator alternatives.
            // String concatenation (`+` on string-literal-rooted chains) is excluded from
            // mutation: the `&`/`*` hoisting pattern cannot satisfy Rust's `String + &str`
            // ownership requirement, and the arithmetic alternatives (-, *, /) don't
            // type-check on strings. Such expressions fall through to the regular path.
            let mut_variants_opt = if *op == BinaryOp::Add && is_string_add_chain(left) {
                None
            } else {
                cg.alloc_binary_mutations(*op, span.line)
            };
            if let Some(mut_variants) = mut_variants_opt {
                // Hoist operands into temp bindings so each sub-expression is emitted
                // (and its nested mutations allocated) exactly once — not N+1 times.
                //
                // `first_id` is the lowest ID allocated for this binary node. IDs are
                // globally monotonic, so `__mvl_l_{first_id}` / `__mvl_r_{first_id}` are
                // unique even across deeply nested binary expressions.
                //
                // `&(expr)` rather than a plain `let` binding is used because operands may
                // be non-Copy MVL enum values (e.g. struct fields). Dereferencing with `*`
                // in each arm provides the operand by reference without moving it. This is
                // valid for all numeric and boolean types that binary operators act on.
                let first_id = mut_variants
                    .first()
                    .expect("alloc_binary_mutations guarantees a non-empty variant list")
                    .0
                    .clone();
                let lvar = format!("__mvl_l_{first_id}");
                let rvar = format!("__mvl_r_{first_id}");
                cg.push(&format!("{{ let {lvar} = &("));
                emit_expr(cg, left);
                cg.push(&format!("); let {rvar} = &("));
                emit_expr(cg, right);
                cg.push("); match ::std::env::var(\"MVL_MUTANT\").as_deref() {");
                for (mid, alt_op) in &mut_variants {
                    cg.push(&format!(" Ok(\"{mid}\") => (*{lvar} {alt_op} *{rvar}),"));
                }
                cg.push(&format!(
                    " _ => (*{lvar} {} *{rvar}), }} }}",
                    emit_binary_op(*op)
                ));
            } else {
                cg.push("(");
                emit_expr(cg, left);
                cg.push(" ");
                cg.push(emit_binary_op(*op));
                cg.push(" ");
                // Rust requires `String + &str` for string concatenation.
                // If the left side is a string-literal-rooted chain, borrow the right.
                if *op == BinaryOp::Add && is_string_add_chain(left) {
                    cg.push("&(");
                    emit_expr(cg, right);
                    cg.push(")");
                } else {
                    emit_expr(cg, right);
                }
                cg.push(")");
            }
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            cg.push("if ");
            emit_expr(cg, cond);
            cg.push(" {");
            cg.nl();
            cg.push_indent();
            emit_block_as_value(cg, &then.stmts);
            cg.pop_indent();
            cg.indent();
            cg.push("}");
            if let Some(else_expr) = else_ {
                cg.push(" else ");
                emit_expr(cg, else_expr);
            }
        }
        Expr::Match {
            scrutinee,
            arms,
            span,
            ..
        } => {
            // Allocate branch coverage IDs for each arm up-front.
            let arm_ids: Vec<Option<usize>> = (0..arms.len())
                .map(|i| cg.alloc_branch(span.line, BranchKind::MatchArm(i)))
                .collect();
            let has_str_pattern = arms_have_str_pattern(arms);
            // Emit scrutinee first so any compound conditions inside it allocate
            // MC/DC IDs before the match-level decisions (mirrors analysis order).
            cg.push("match ");
            emit_expr(cg, scrutinee);
            // Allocate MC/DC arm-coverage decision after scrutinee.
            let match_mcdc_id: Option<usize> = if arms.len() >= 2 {
                cg.alloc_mcdc_decision(span.line, arms.len(), DecisionKind::Match, vec![])
            } else {
                None
            };
            // Pre-allocate MatchGuard decision IDs (all arms, before body emission).
            let guard_mcdc_ids: Vec<Option<usize>> = arms
                .iter()
                .map(|arm| {
                    arm.guard.as_ref().and_then(|g| {
                        let n = count_clauses_ref(g);
                        if n >= 2 {
                            cg.alloc_mcdc_decision(
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
            if has_str_pattern {
                cg.push(".as_str()");
            }
            cg.push(" {");
            cg.nl();
            cg.push_indent();
            for ((arm_idx, arm), (cov_id, guard_mcdc_id)) in arms
                .iter()
                .enumerate()
                .zip(arm_ids.iter().zip(guard_mcdc_ids.iter()))
            {
                emit_match_arm(cg, arm, arm_idx, *cov_id, match_mcdc_id, *guard_mcdc_id);
            }
            cg.pop_indent();
            cg.indent();
            cg.push("}");
        }
        Expr::Block(block) => {
            cg.push("{");
            cg.nl();
            cg.push_indent();
            emit_block_as_value(cg, &block.stmts);
            cg.pop_indent();
            cg.indent();
            cg.push("}");
        }
        Expr::Propagate { expr, .. } => {
            emit_expr(cg, expr);
            cg.push("?");
        }
        Expr::Construct { name, fields, .. } => {
            cg.push(name);
            cg.push(" { ");
            // Emit directly into cg so nested FnCall expressions can look up
            // borrow_params_map. A fresh RustEmitter::new() would have an empty
            // map, causing borrow-inferred parameters inside field values to be
            // emitted as .clone() instead of &x (#465).
            for (i, (fname, fexpr)) in fields.iter().enumerate() {
                if i > 0 {
                    cg.push(", ");
                }
                cg.push(&format!("{fname}: "));
                // Clone field values: placing a value into a struct field is a
                // move in Rust. MVL value semantics require the source binding to
                // remain valid. Spec 009 Req 2: clone ALL non-Copy arguments.
                emit_expr_as_arg(cg, fexpr);
            }
            cg.push(" }");
        }
        Expr::List { elems, .. } => {
            cg.push("vec![");
            emit_args(cg, elems);
            cg.push("]");
        }
        Expr::Map { pairs, .. } => {
            cg.push("std::collections::HashMap::from([");
            // Emit directly into cg for the same reason as Expr::Construct above.
            for (i, (k, v)) in pairs.iter().enumerate() {
                if i > 0 {
                    cg.push(", ");
                }
                cg.push("(");
                emit_expr(cg, k);
                cg.push(", ");
                emit_expr(cg, v);
                // `.into()` coerces IFC-label wrappers (Clean<String>, etc.) to
                // their plain inner type so map values match HashMap<String, String>
                // signatures in stdlib functions like log_info / log_warn.
                cg.push(".into()");
                cg.push(")");
            }
            cg.push("])");
        }
        Expr::Set { elems, .. } => {
            cg.push("std::collections::HashSet::from([");
            emit_args(cg, elems);
            cg.push("])");
        }
        Expr::Consume { expr, .. } => {
            // `consume` mirrors Pony's `consume` for iso; just emit the inner expr in Phase 1
            emit_expr(cg, expr);
        }
        Expr::Declassify { expr, .. } => {
            cg.push("declassify(");
            emit_expr(cg, expr);
            cg.push(")");
        }
        Expr::Sanitize { expr, .. } => {
            cg.push("sanitize(");
            emit_expr(cg, expr);
            cg.push(")");
        }
        Expr::Lambda {
            params,
            ret_type,
            body,
            ..
        } => {
            cg.push("|");
            let param_strs: Vec<String> = params
                .iter()
                .map(|p| {
                    let ty_str = emit_type_expr(&p.ty);
                    format!("{}: {ty_str}", p.name)
                })
                .collect();
            cg.push(&param_strs.join(", "));
            cg.push("|");
            if let Some(ret) = ret_type {
                cg.push(" -> ");
                cg.push(&emit_type_expr(ret));
            }
            cg.push(" ");
            emit_expr(cg, body);
        }
        Expr::Spawn {
            actor_type, fields, ..
        } => {
            // Phase 8: `actor Counter { count: 0 }` → `_start_counter(CounterState { count: 0, _self_ref: None })`
            let snake = crate::mvl::backends::rust::emit_actors::actor_name_to_snake(actor_type);
            cg.push(&format!("_start_{snake}({actor_type}State {{"));
            for (i, (field_name, val)) in fields.iter().enumerate() {
                if i > 0 {
                    cg.push(", ");
                }
                cg.push(&format!("{field_name}: "));
                emit_expr(cg, val);
            }
            // `_self_ref` is always None at construction; `_start_<name>` sets it
            // after the channel is created (so the handle can be cloned into state).
            if !fields.is_empty() {
                cg.push(", ");
            }
            cg.push("_self_ref: None");
            cg.push("})");
        }
        // Phase 8 (#743): concurrently { body } — structured concurrency scope.
        // Sequential fallback: emit body as a plain block.  Full thread::scope
        // isolation is deferred until the actor scheduler is complete.
        Expr::Concurrently { body, .. } => {
            cg.push("{");
            cg.nl();
            cg.push_indent();
            emit_block_stmts(cg, &body.stmts);
            cg.pop_indent();
            cg.indent();
            cg.push("}");
        }
        // Phase 8 (#743): select { arm => { body } … } — first-ready-wins stub.
        // Emits the first arm's handler body; full scheduler (try_recv polling)
        // is deferred until bidirectional channel receive is implemented.
        Expr::Select { arms, .. } => {
            cg.push("{");
            cg.nl();
            cg.push_indent();
            if let Some(first) = arms.first() {
                emit_block_stmts(cg, &first.body.stmts);
            }
            cg.pop_indent();
            cg.indent();
            cg.push("}");
        }
    }
}

// ── Literal ───────────────────────────────────────────────────────────────

/// Re-escape a decoded string value for insertion into a Rust string literal.
fn escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            other => out.push(other),
        }
    }
    out
}

/// Re-escape a decoded char value for insertion into a Rust char literal.
fn escape_char(c: char) -> String {
    match c {
        '\\' => "\\\\".to_string(),
        '\'' => "\\'".to_string(),
        '\n' => "\\n".to_string(),
        '\t' => "\\t".to_string(),
        '\r' => "\\r".to_string(),
        '\0' => "\\0".to_string(),
        other => other.to_string(),
    }
}

fn emit_literal(cg: &mut RustEmitter, lit: &Literal) {
    match lit {
        Literal::Integer(n) => cg.push(&n.to_string()),
        Literal::Float(f) => {
            // Ensure float literals have a decimal point in Rust
            let s = format!("{f}");
            if s.contains('.') || s.contains('e') {
                cg.push(&s);
            } else {
                cg.push(&format!("{s}.0"));
            }
        }
        Literal::Str(s) => cg.push(&format!("\"{}\".to_string()", escape_str(s))),
        Literal::Char(c) => cg.push(&format!("'{}'", escape_char(*c))),
        Literal::Bool(b) => cg.push(if *b { "true" } else { "false" }),
        Literal::Unit => cg.push("()"),
    }
}

/// Returns true if any match arm uses a string literal pattern.
///
/// When true, the scrutinee must be coerced to `&str` via `.as_str()` so that
/// Rust's pattern matching works (both `String` and IFC-labeled strings expose
/// `.as_str()`).  Called from both `Expr::Match` and `Stmt::Match` codegen.
pub fn arms_have_str_pattern(arms: &[MatchArm]) -> bool {
    arms.iter()
        .any(|a| matches!(&a.pattern, Pattern::Literal(Literal::Str(_), _)))
}

/// Emit a literal in pattern position.  String literals must be bare `"s"`
/// (not `"s".to_string()`) because Rust patterns cannot contain method calls.
fn emit_literal_in_pattern(cg: &mut RustEmitter, lit: &Literal) {
    match lit {
        Literal::Str(s) => cg.push(&format!("\"{}\"", escape_str(s))),
        other => emit_literal(cg, other),
    }
}

// ── Arguments ─────────────────────────────────────────────────────────────

fn emit_args(cg: &mut RustEmitter, args: &[Expr]) {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            cg.push(", ");
        }
        emit_expr_as_fn_arg(cg, arg);
    }
}

/// Emit arguments without `.into()` on string literals.
///
/// Used for `assert_eq!`/`assert_ne!` where `.into()` causes E0283: `String`
/// has many cross-type `PartialEq<X>` impls so Rust can't determine which
/// `From<String>` conversion to use.  Emitting `.to_string()` (no `.into()`)
/// makes the concrete `String` type visible to the type checker.
fn emit_args_no_into(cg: &mut RustEmitter, args: &[Expr]) {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            cg.push(", ");
        }
        if let Expr::Literal(crate::mvl::parser::ast::Literal::Str(s), _) = arg {
            cg.push(&format!("\"{}\".to_string()", escape_str(s)));
        } else {
            emit_expr_as_arg(cg, arg);
        }
    }
}

/// Emit arguments for a function call, using per-parameter borrow kinds (Phase B).
///
/// * `Some(false)` — emit `&x` (shared reference).
/// * `Some(true)`  — emit `&mut x` (mutable reference).
/// * `None`        — emit normally (`x.clone()` / move).
///
/// When `borrows` is shorter than `args` (unknown callee / variadic),
/// the remaining args fall through to normal value-argument emission.
fn emit_args_with_borrows(cg: &mut RustEmitter, args: &[Expr], borrows: &[Option<bool>]) {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            cg.push(", ");
        }
        match borrows.get(i).copied().flatten() {
            Some(mutable) => emit_expr_as_borrow_arg(cg, arg, mutable),
            None => emit_expr_as_fn_arg(cg, arg),
        }
    }
}

/// Emit an expression as a reference argument (`&x` or `&mut x`).
///
/// For identifiers and field accesses the prefix is prepended directly.
/// For temporaries (function call results, struct literals, block expressions)
/// the expression is wrapped in `&(…)` / `&mut (…)` — valid in Rust because
/// temporaries live until the end of the enclosing statement.
fn emit_expr_as_borrow_arg(cg: &mut RustEmitter, expr: &Expr, mutable: bool) {
    match expr {
        // Fix 3: already a borrow expression — emit as-is, no extra & needed
        Expr::Borrow { .. } => emit_expr(cg, expr),
        Expr::Ident(_, _) | Expr::FieldAccess { .. } => {
            cg.push(if mutable { "&mut " } else { "&" });
            emit_expr(cg, expr);
        }
        _ => {
            cg.push(if mutable { "&mut (" } else { "&(" });
            emit_expr(cg, expr);
            cg.push(")");
        }
    }
}

/// Emit an expression in function-argument position.
///
/// MVL has value semantics: passing a value to a function is a copy, not a
/// move.  In Rust, non-Copy types (structs, enums, Vec, String) are moved by
/// default.  We insert `.clone()` for identifiers and field accesses so the
/// caller retains ownership after the call, matching MVL semantics.
/// `.clone()` on Copy types (i64, bool, char) is a no-op and is optimised
/// away by the compiler.
///
/// String literals get `.to_string().into()` for label type coercion.
///
/// # Phase A: last-use move elision (Spec 009 Req 2)
///
/// When an `Expr::Ident`'s span appears in [`RustEmitter::last_uses`], the variable
/// is used for the last time in this function.  Emitting a Rust move (no
/// `.clone()`) is sound: the caller's binding is consumed but never read again.
fn emit_expr_as_arg(cg: &mut RustEmitter, expr: &Expr) {
    match expr {
        Expr::Literal(Literal::Str(s), _) => {
            cg.push(&format!("\"{}\".to_string().into()", escape_str(s)));
        }
        // Phase 8: `self` used as a tag argument inside an actor behavior.
        // The actor's own handle is stored in `_self_ref`; clone it to pass.
        Expr::Ident(name, _) if name == "self" && !cg.actor_self_type.is_empty() => {
            cg.push("self._self_ref.as_ref().unwrap().clone()");
        }
        // Identifiers: check if this is the last use — if so, move instead of clone.
        Expr::Ident(_, span) => {
            emit_expr(cg, expr);
            if !cg.last_uses.contains(span) {
                cg.push(".clone()");
            }
        }
        // Field accesses: conservatively clone (partial moves are complex in Rust).
        Expr::FieldAccess { .. } => {
            emit_expr(cg, expr);
            cg.push(".clone()");
        }
        _ => {
            // Temporaries (function call results, struct literals, block expressions)
            // are rvalues that Rust moves into the callee — no `.clone()` needed.
            // The value is freshly created and has no other owner in the caller.
            emit_expr(cg, expr);
        }
    }
}

/// Emit an expression as an argument to a regular function call (not a macro).
///
/// Adds `.into()` so that unlabeled (Public) values coerce to labeled parameters
/// (e.g. `String` → `Clean<String>`) via `From<T> for Label<T>` in mvl_runtime::ifc.
/// This is safe for function calls because the parameter type constrains `.into()`'s
/// target, preventing the E0283 ambiguity that arises in macros like `println!`.
fn emit_expr_as_fn_arg(cg: &mut RustEmitter, expr: &Expr) {
    use crate::mvl::checker::types::Ty;
    match expr {
        Expr::Literal(Literal::Str(s), _) => {
            cg.push(&format!("\"{}\".to_string().into()", escape_str(s)));
        }
        // Phase 8: `self` used as a tag argument inside an actor behavior.
        Expr::Ident(name, _) if name == "self" && !cg.actor_self_type.is_empty() => {
            cg.push("self._self_ref.as_ref().unwrap().clone()");
        }
        // Function-typed identifiers (callbacks, named function references) must NOT
        // get `.into()` — Rust function items do not implement `Into<_>` generically.
        Expr::Ident(_, span) if matches!(cg.expr_types.get(span), Some(Ty::Fn(..))) => {
            emit_expr(cg, expr);
            if !cg.last_uses.contains(span) {
                cg.push(".clone()");
            }
        }
        // Option/Result identifiers must NOT get `.into()` — the IFC label impl
        // `From<T> for Label<T>` creates multiple conversion candidates for compound
        // types (e.g. `Option<i64>.into()` could be `Option<i64>` or `Clean<Option<i64>>`),
        // causing E0283 when the parameter type is generic (e.g. `is_some<T>(Option<T>)`).
        //
        // Safety invariant: suppressing `.into()` here does NOT create an IFC label bypass.
        // The type checker rejects label-mismatched calls (e.g. Secret[Option[T]] passed to
        // Option[T]) before codegen runs — see tests::secret_option_to_unlabeled_option_rejected
        // and tests::secret_result_to_unlabeled_result_rejected in tests/type_checker.rs (#714).
        Expr::Ident(_, span)
            if matches!(
                cg.expr_types.get(span),
                Some(Ty::Option(_) | Ty::Result(_, _))
            ) =>
        {
            emit_expr(cg, expr);
            if !cg.last_uses.contains(span) {
                cg.push(".clone()");
            }
        }
        // Value identifiers: `.into()` allows unlabeled (Public) values to coerce into
        // labeled parameters (e.g. `String` → `Clean<String>`).
        Expr::Ident(_, span) => {
            emit_expr(cg, expr);
            if !cg.last_uses.contains(span) {
                cg.push(".clone().into()");
            } else {
                cg.push(".into()");
            }
        }
        Expr::FieldAccess { .. } => {
            emit_expr(cg, expr);
            cg.push(".clone().into()");
        }
        _ => {
            emit_expr(cg, expr);
        }
    }
}

/// Emit arguments for Rust macros like `println!` where the first argument
/// must be a bare string literal (not a `.to_string()` expression).
fn emit_args_for_macro(cg: &mut RustEmitter, args: &[Expr]) {
    if args.is_empty() {
        return;
    }
    match &args[0] {
        Expr::Literal(Literal::Str(s), _) => {
            // First arg is a string literal: emit bare, then remaining args as values.
            cg.push(&format!("\"{}\"", escape_str(s)));
            for arg in &args[1..] {
                cg.push(", ");
                emit_expr_as_arg(cg, arg);
            }
        }
        _ => {
            // First arg is not a string literal: generate one "{}" placeholder per
            // argument so the format string matches the argument count (#198).
            let placeholders = vec!["{}"; args.len()].join(" ");
            cg.push(&format!("\"{placeholders}\""));
            for arg in args {
                cg.push(", ");
                emit_expr_as_arg(cg, arg);
            }
        }
    }
}

// ── Binary operators ──────────────────────────────────────────────────────

/// Return true when `expr` is the left side of a string concatenation chain.
/// A chain is rooted in a string literal: `"literal"`, `"a" + b`, etc.
/// Used to decide whether the right operand of `+` needs a borrow (`&rhs`)
/// to satisfy Rust's `String + &str` requirement.
fn is_string_add_chain(expr: &Expr) -> bool {
    match expr {
        Expr::Literal(Literal::Str(_), _) => true,
        Expr::Binary {
            op: BinaryOp::Add,
            left,
            ..
        } => is_string_add_chain(left),
        _ => false,
    }
}

fn emit_binary_op(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::Gt => ">",
        BinaryOp::Le => "<=",
        BinaryOp::Ge => ">=",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
        BinaryOp::BitAnd => "&",
        BinaryOp::BitOr => "|",
        BinaryOp::BitXor => "^",
        BinaryOp::Shl => "<<",
        // Rust's >> is sign-aware based on type (u8 → logical, i64 → arithmetic).
        BinaryOp::Shr => ">>",
    }
}

// ── Match arms ────────────────────────────────────────────────────────────

fn emit_match_arm(
    cg: &mut RustEmitter,
    arm: &MatchArm,
    arm_idx: usize,
    cov_id: Option<usize>,
    match_mcdc_id: Option<usize>,
    guard_mcdc_id: Option<usize>,
) {
    cg.indent();
    emit_pattern(cg, &arm.pattern);
    if let Some(guard) = &arm.guard {
        cg.push(" if ");
        if let Some(gid) = guard_mcdc_id {
            let n = count_clauses_ref(guard);
            cg.push(&emit_mcdc_guard_block(guard, gid, n));
        } else {
            use crate::mvl::backends::rust::emit_types::emit_ref_expr_for_assert;
            cg.push(&emit_ref_expr_for_assert(guard, "_"));
        }
    }
    cg.push(" => ");
    match &arm.body {
        MatchBody::Expr(e) => {
            // Wrap in a block to inject coverage and MC/DC hits.
            cg.push("{ ");
            if let Some(id) = cov_id {
                cg.push(&format!("#[cfg(test)] crate::__mvl_cov::hit({id}); "));
            }
            if let Some(mid) = match_mcdc_id {
                cg.push(&format!(
                    "#[cfg(test)] crate::__mvl_mcdc::record({mid}usize, {arm_idx}u32); "
                ));
            }
            emit_expr(cg, e);
            cg.push(" }");
            cg.push(",");
            cg.nl();
        }
        MatchBody::Block(block) => {
            cg.push("{");
            cg.nl();
            cg.push_indent();
            if let Some(id) = cov_id {
                cg.emit_cov_hit(id);
            }
            if let Some(mid) = match_mcdc_id {
                cg.line(&format!(
                    "#[cfg(test)] crate::__mvl_mcdc::record({mid}usize, {arm_idx}u32);"
                ));
            }
            emit_block_stmts(cg, &block.stmts);
            cg.pop_indent();
            cg.indent();
            cg.push("},");
            cg.nl();
        }
    }
}

// ── Patterns ─────────────────────────────────────────────────────────────

pub fn emit_pattern(cg: &mut RustEmitter, pat: &Pattern) {
    match pat {
        Pattern::Wildcard(_) => cg.push("_"),
        Pattern::Ident(name, _) => cg.push(&map_ident(name)),
        Pattern::Literal(lit, _) => emit_literal_in_pattern(cg, lit),
        Pattern::Tuple { elems, .. } => {
            cg.push("(");
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    cg.push(", ");
                }
                emit_pattern(cg, e);
            }
            cg.push(")");
        }
        Pattern::TupleStruct { name, fields, .. } => {
            cg.push(name);
            cg.push("(");
            for (i, f) in fields.iter().enumerate() {
                if i > 0 {
                    cg.push(", ");
                }
                emit_pattern(cg, f);
            }
            cg.push(")");
        }
        Pattern::Struct { name, fields, .. } => {
            cg.push(name);
            cg.push(" { ");
            for (i, (fname, fpat)) in fields.iter().enumerate() {
                if i > 0 {
                    cg.push(", ");
                }
                cg.push(fname);
                cg.push(": ");
                emit_pattern(cg, fpat);
            }
            cg.push(" }");
        }
        Pattern::Some { inner, .. } => {
            cg.push("Some(");
            emit_pattern(cg, inner);
            cg.push(")");
        }
        Pattern::None(_) => cg.push("None"),
        Pattern::Ok { inner, .. } => {
            cg.push("Ok(");
            emit_pattern(cg, inner);
            cg.push(")");
        }
        Pattern::Err { inner, .. } => {
            cg.push("Err(");
            emit_pattern(cg, inner);
            cg.push(")");
        }
    }
}

// ── Block statements (used in if/match body/function body) ────────────────

pub fn emit_block_stmts(cg: &mut RustEmitter, stmts: &[crate::mvl::parser::ast::Stmt]) {
    use crate::mvl::backends::rust::emit_stmts::emit_stmt;
    for stmt in stmts {
        emit_stmt(cg, stmt);
    }
}

/// Emit block statements where the final `Stmt::Expr` is a tail expression
/// (no semicolon), so it becomes the implicit return value of the block.
pub fn emit_block_as_value(cg: &mut RustEmitter, stmts: &[crate::mvl::parser::ast::Stmt]) {
    use crate::mvl::backends::rust::emit_stmts::emit_stmt;
    use crate::mvl::parser::ast::Stmt;
    if stmts.is_empty() {
        return;
    }
    let (head, tail) = stmts.split_at(stmts.len() - 1);
    for stmt in head {
        emit_stmt(cg, stmt);
    }
    match &tail[0] {
        Stmt::Expr { expr, .. } => {
            cg.indent();
            emit_expr(cg, expr);
            cg.nl();
        }
        other => emit_stmt(cg, other),
    }
}

// ── Name mappings ─────────────────────────────────────────────────────────

fn map_ident(name: &str) -> String {
    // MVL `self` inside refinements → Rust parameter name is substituted upstream;
    // as an expression ident, pass through as-is
    name.to_string()
}

fn map_fn_name(name: &str) -> String {
    // Built-in MVL functions mapped to Rust / stdlib equivalents
    match name {
        "println" => "println!".to_string(),
        "panic" => "panic!".to_string(),
        "assert" => "assert!".to_string(),
        "assert_eq" => "assert_eq!".to_string(),
        "assert_ne" => "assert_ne!".to_string(),
        _ => name.to_string(),
    }
}

/// Emit `receiver.len()` as direct Rust using the receiver's checker type (#554).
///
/// - `String` → `.chars().count() as i64` (Unicode codepoint count, not byte count).
/// - `List/Map/Set` → `.len() as i64`.
/// - `Labeled(label, inner)` → `Label((&receiver).0.method as i64)` preserving IFC label.
/// - Unknown → `.len() as i64` (safe fallback for any Rust collection).
fn emit_len_direct(cg: &mut RustEmitter, receiver: &Expr, ty: Option<&Ty>) {
    // Wrap in parens: `as i64` has low precedence and `.clone()` cannot follow
    // an unparenthesised cast, e.g. `xs.len() as i64.clone()` is a parse error.
    match ty {
        Some(Ty::String) => {
            cg.push("(");
            emit_expr(cg, receiver);
            cg.push(".chars().count() as i64)");
        }
        Some(Ty::Labeled(label, inner)) => {
            let label_name = emit_label(*label);
            let method = match inner.as_ref() {
                Ty::String => ".chars().count()",
                _ => ".len()",
            };
            cg.push(&format!("{label_name}((&("));
            emit_expr(cg, receiver);
            cg.push(&format!(")).0{method} as i64)"));
        }
        _ => {
            cg.push("(");
            emit_expr(cg, receiver);
            cg.push(".len() as i64)");
        }
    }
}

/// Emit `n.clamp(low, high)` as a safe Rust block expression.
///
/// - If `low > high`, the value is returned unchanged (graceful degradation).
/// - Otherwise, clamps the value to `[low, high]`.
/// - Never panics (unlike Rust's `i64::clamp`/`f64::clamp` which panic on inverted bounds).
fn emit_safe_clamp(cg: &mut RustEmitter, receiver: &Expr, low: &Expr, high: &Expr) {
    cg.push("{let _mvl_n=(");
    emit_expr(cg, receiver);
    cg.push(");let _mvl_lo=(");
    emit_expr(cg, low);
    cg.push(");let _mvl_hi=(");
    emit_expr(cg, high);
    cg.push(");if _mvl_lo>_mvl_hi{_mvl_n}else{_mvl_n.clamp(_mvl_lo,_mvl_hi)}}");
}
