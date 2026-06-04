// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! AST-based expression emitter (legacy path for prelude/stdlib functions).
//!
//! The TIR-based replacement lives in `emit_exprs.rs`.

use super::emitter::RustEmitter;
use crate::mvl::backends::rust::emit_stmts_ast::{
    emit_mcdc_guard_block_ast as emit_mcdc_guard_block,
    scrutinee_needs_clone_ast as scrutinee_needs_clone,
};
use crate::mvl::backends::rust::emit_types::{emit_label, emit_type_expr};
use crate::mvl::ir::Ty;
use crate::mvl::parser::ast::{BinaryOp, Expr, Literal, MatchArm, MatchBody, Pattern, UnaryOp};
use crate::mvl::passes::coverage::BranchKind;
use crate::mvl::passes::mcdc::analysis::count_clauses_ref;
use crate::mvl::passes::mcdc::transform::DecisionKind;

use crate::mvl::backends::{
    STDLIB_BUILTIN_METHODS, STDLIB_UFCS_METHODS, STRING_LABEL_PRESERVING_METHODS,
};

impl RustEmitter {
    /// Emit an expression into the code buffer (no trailing newline).
    pub fn emit_expr_ast(&mut self, expr: &Expr) {
        match expr {
            Expr::Literal(lit, span) => {
                // Mutation mode: inject env-var dispatch for Bool and Integer literals.
                if self.mutation.is_some() {
                    match lit {
                        Literal::Bool(b) => {
                            if let Some(mid) = self.alloc_bool_mutation(*b, span.line) {
                                let (alt, orig) = if *b {
                                    ("false", "true")
                                } else {
                                    ("true", "false")
                                };
                                self.push(&format!(
                                    r#"{{ match ::std::env::var("MVL_MUTANT").as_deref() {{ Ok("{mid}") => {alt}, _ => {orig} }} }}"#
                                ));
                                return;
                            }
                        }
                        Literal::Integer(n) => {
                            if let Some(int_variants) = self.alloc_int_mutations(*n, span.line) {
                                self.push("{ match ::std::env::var(\"MVL_MUTANT\").as_deref() {");
                                for (mid, alt) in &int_variants {
                                    self.push(&format!(" Ok(\"{mid}\") => {alt},"));
                                }
                                self.push(&format!(" _ => {n} }}"));
                                self.push(" }");
                                return;
                            }
                        }
                        _ => {}
                    }
                }
                self.emit_literal_ast(lit);
            }
            Expr::Ident(name, _) => {
                // #928: in free-function extension method bodies, `self` → `self_`.
                if name == "self" && self.self_as_free_param {
                    self.push("self_");
                } else {
                    self.push(&map_ident(name));
                }
            }
            Expr::FieldAccess { expr, field, .. } => {
                self.emit_expr_ast(expr);
                self.push(".");
                self.push(field);
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
                        let receiver_ty = self.expr_types.get(&receiver.span()).cloned();
                        // Use is_option/is_result which strip security labels (Labeled<Option<T>>
                        // and Labeled<Result<T,E>> are still Option/Result for dispatch purposes).
                        let is_opt_or_result = receiver_ty
                            .as_ref()
                            .is_some_and(|t| t.is_option() || t.is_result());
                        if is_opt_or_result {
                            self.emit_expr_ast(receiver);
                            self.push(".map(|__x| (");
                            self.emit_expr_ast(&args[0]);
                            self.push(")(__x.clone()))");
                        } else {
                            // List (and unknown types) use into_iter().collect().
                            // Set.map is not a valid MVL operation (checker returns Ty::Unknown),
                            // so this arm is only reached for List receivers in valid programs.
                            self.emit_expr_ast(receiver);
                            self.push(".into_iter().map(|__x| (");
                            self.emit_expr_ast(&args[0]);
                            self.push(")(__x.clone())).collect::<Vec<_>>()");
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
                            self.capability_params_map
                                .get(name.as_str())
                                .and_then(|b| b.first().copied())
                                .flatten()
                                .is_some()
                        } else {
                            false
                        };
                        self.emit_expr_ast(receiver);
                        self.push(".clone().into_iter().");
                        self.push(method);
                        self.push("(|__x| (");
                        self.emit_expr_ast(&args[0]);
                        if needs_borrow {
                            self.push(")(&__x.clone())).collect::<Vec<_>>()");
                        } else {
                            self.push(")(__x.clone())).collect::<Vec<_>>()");
                        }
                    }
                    // any / all — same predicate pattern but return bool, no collect.
                    "any" | "all" if args.len() == 1 => {
                        let needs_borrow = if let Expr::Ident(name, _) = &args[0] {
                            self.capability_params_map
                                .get(name.as_str())
                                .and_then(|b| b.first().copied())
                                .flatten()
                                .is_some()
                        } else {
                            false
                        };
                        self.emit_expr_ast(receiver);
                        self.push(".clone().into_iter().");
                        self.push(method);
                        self.push("(|__x| (");
                        self.emit_expr_ast(&args[0]);
                        if needs_borrow {
                            self.push(")(&__x.clone()))");
                        } else {
                            self.push(")(__x.clone()))");
                        }
                    }
                    // fold(init, f) — init cloned (value arg); f wrapped in closure
                    // so capturing closures are accepted alongside fn pointers.
                    // When f is a named function with borrow params, add & to the
                    // accumulator and/or element in the generated lambda.
                    "fold" if args.len() == 2 => {
                        let (borrow_acc, borrow_elem) = if let Expr::Ident(name, _) = &args[1] {
                            let borrows = self
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
                        self.emit_expr_ast(receiver);
                        self.push(".clone().into_iter().fold(");
                        self.emit_expr_as_arg_ast(&args[0]);
                        self.push(", |acc, __x| (");
                        self.emit_expr_ast(&args[1]);
                        self.push(")(");
                        if borrow_acc {
                            self.push("&");
                        }
                        self.push("acc, ");
                        if borrow_elem {
                            self.push("&");
                        }
                        self.push("__x))");
                    }
                    // windows(n)/chunks(n) — Rust returns &[T] slices; collect into Vec<Vec<T>>.
                    // MVL passes n as Int (i64); Rust requires usize, so cast.
                    "windows" | "chunks" => {
                        self.emit_expr_ast(receiver);
                        self.push(".");
                        self.push(method);
                        self.push("(");
                        if let Some(arg) = args.first() {
                            self.emit_expr_ast(arg);
                            self.push(" as usize");
                        }
                        self.push(").map(|w| w.to_vec()).collect::<Vec<_>>()");
                    }
                    // partition(f) — turbofish needed so Rust can infer the element type
                    "partition" => {
                        self.emit_expr_ast(receiver);
                        self.push(".into_iter().partition::<Vec<_>, _>(|__x| ");
                        if let Some(arg) = args.first() {
                            self.emit_expr_ast(arg);
                        }
                        self.push("(__x.clone()))");
                    }
                    // group_by(f) — no native Rust equivalent; fold into HashMap
                    "group_by" => {
                        self.push("{ let mut __m = std::collections::HashMap::new(); for __v in ");
                        self.emit_expr_ast(receiver);
                        self.push(".into_iter() { __m.entry(");
                        if let Some(arg) = args.first() {
                            // Phase B: if the key function takes a reference for its first
                            // parameter, emit `&__v.clone()` instead of `__v.clone()`.
                            let needs_borrow = if let Expr::Ident(name, _) = arg {
                                self.capability_params_map
                                    .get(name.as_str())
                                    .and_then(|b| b.first().copied())
                                    .flatten()
                                    .is_some()
                            } else {
                                false
                            };
                            self.emit_expr_ast(arg);
                            if needs_borrow {
                                self.push("(&__v.clone())");
                            } else {
                                self.push("(__v.clone())");
                            }
                        }
                        self.push(").or_insert_with(Vec::new).push(__v); } __m }");
                    }
                    // and_then(f) — Option<T> and Result<T,E>
                    "and_then" if args.len() == 1 => {
                        self.emit_expr_ast(receiver);
                        self.push(".and_then(|__x| (");
                        self.emit_expr_ast(&args[0]);
                        self.push(")(__x.clone()))");
                    }
                    // sort() — sort_by with partial_cmp for numeric stability
                    "sort" if args.is_empty() => {
                        self.push("{let mut __v=(");
                        self.emit_expr_ast(receiver);
                        self.push(");__v.sort_by(|__a,__b|__a.partial_cmp(__b).unwrap_or(std::cmp::Ordering::Equal));__v}");
                    }

                    // ── Operator-level methods ────────────────────────────────────────
                    //
                    // Bitwise ops on Int/Byte: emitted as Rust operators for LLVM
                    // visibility and future intrinsic optimisation.
                    "bit_and" if args.len() == 1 => {
                        self.push("(");
                        self.emit_expr_ast(receiver);
                        self.push(" & ");
                        self.emit_expr_ast(&args[0]);
                        self.push(")");
                    }
                    "bit_or" if args.len() == 1 => {
                        self.push("(");
                        self.emit_expr_ast(receiver);
                        self.push(" | ");
                        self.emit_expr_ast(&args[0]);
                        self.push(")");
                    }
                    "bit_xor" if args.len() == 1 => {
                        self.push("(");
                        self.emit_expr_ast(receiver);
                        self.push(" ^ ");
                        self.emit_expr_ast(&args[0]);
                        self.push(")");
                    }
                    "bit_not" if args.is_empty() => {
                        self.push("(!");
                        self.emit_expr_ast(receiver);
                        self.push(")");
                    }
                    // wrapping_shl/shr avoids debug-mode panic for out-of-range shift counts
                    "shift_left" if args.len() == 1 => {
                        self.push("(");
                        self.emit_expr_ast(receiver);
                        self.push(".wrapping_shl(");
                        self.emit_expr_ast(&args[0]);
                        self.push(" as u32))");
                    }
                    "shift_right" if args.len() == 1 => {
                        self.push("(");
                        self.emit_expr_ast(receiver);
                        self.push(".wrapping_shr(");
                        self.emit_expr_ast(&args[0]);
                        self.push(" as u32))");
                    }
                    // is_zero() — i64 has no is_zero(); emit comparison
                    "is_zero" if args.is_empty() => {
                        self.push("(");
                        self.emit_expr_ast(receiver);
                        self.push(" == 0)");
                    }
                    // to_int() on Byte (u8→i64) or Float (f64→i64, truncating)
                    "to_int" if args.is_empty() => {
                        self.push("(");
                        self.emit_expr_ast(receiver);
                        self.push(" as i64)");
                    }
                    // to_float() on Int (i64→f64); i64::from() unwraps IFC labels transparently
                    "to_float" if args.is_empty() => {
                        self.push("(i64::from(");
                        self.emit_expr_ast(receiver);
                        self.push(".clone()) as f64)");
                    }
                    // pow(e) — direct Rust using checker type info (#554).
                    // i64: .pow(e as u32); f64: .powf(e).
                    "pow" if args.len() == 1 => {
                        let receiver_ty = self.expr_types.get(&receiver.span()).cloned();
                        self.emit_expr_ast(receiver);
                        match receiver_ty.as_ref() {
                            Some(Ty::Float) => {
                                self.push(".powf(");
                                self.emit_expr_as_arg_ast(&args[0]);
                                self.push(")");
                            }
                            _ => {
                                self.push(".pow(");
                                self.emit_expr_as_arg_ast(&args[0]);
                                self.push(" as u32)");
                            }
                        }
                    }
                    // clamp(low, high) — Rust's clamp panics on inverted bounds; safe wrapper
                    "clamp" if args.len() == 2 => {
                        self.emit_safe_clamp_ast(receiver, &args[0], &args[1]);
                    }
                    // contains(x) — direct Rust using checker type info (#554).
                    // String: .contains(arg.as_str()); List/Set: .contains(&arg).
                    "contains" if args.len() == 1 => {
                        let receiver_ty = self.expr_types.get(&receiver.span()).cloned();
                        self.emit_expr_ast(receiver);
                        match receiver_ty.as_ref() {
                            Some(Ty::String) => {
                                // emit_args_no_into avoids .into() before .as_str().
                                // (x.into()).as_str() is ambiguous (E0282) when the arg
                                // is a String variable — Rust cannot infer the Into target
                                // without a constraining function parameter type.
                                self.push(".contains((");
                                self.emit_args_no_into_ast(args);
                                self.push(").as_str())");
                            }
                            _ => {
                                self.push(".contains(&(");
                                self.emit_args_ast(args);
                                self.push("))");
                            }
                        }
                    }

                    // concat(x) — type-aware dispatch (#928):
                    //   String: str_concat(receiver, other)
                    //   List:   list_concat(receiver, other)
                    "concat" if args.len() == 1 => {
                        let receiver_ty = self.expr_types.get(&receiver.span()).cloned();
                        let rust_fn = match receiver_ty.as_ref() {
                            Some(Ty::List(_)) => "list_concat",
                            _ => "str_concat",
                        };
                        self.push(rust_fn);
                        self.push("(");
                        self.emit_expr_ast(receiver);
                        self.push(".clone().into()");
                        self.push(", ");
                        self.emit_args_ast(args);
                        self.push(")");
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
                        let receiver_ty = self.expr_types.get(&receiver.span()).cloned();
                        match receiver_ty.as_ref() {
                            Some(Ty::Map(_, _)) => {
                                self.emit_expr_ast(receiver);
                                self.push(".get(&(");
                                self.emit_expr_ast(&args[0]);
                                self.push(").clone()).cloned()");
                            }
                            _ => {
                                self.push("{ let __mvl_i = (");
                                self.emit_expr_ast(&args[0]);
                                self.push("); if __mvl_i < 0 { None } else { (");
                                self.emit_expr_ast(receiver);
                                self.push(").get(__mvl_i as usize).cloned() } }");
                            }
                        }
                    }

                    // len() — direct Rust using checker type info (#554).
                    // String: .chars().count() as i64; List/Map/Set: .len() as i64.
                    // Labeled types: propagate label via field access.
                    "len" if args.is_empty() => {
                        let receiver_ty = self.expr_types.get(&receiver.span()).cloned();
                        self.emit_len_direct_ast(receiver, receiver_ty.as_ref());
                    }

                    // insert(k, v) — Map: emit HashMap::insert (returns Option, discarded).
                    "insert" if args.len() == 2 => {
                        self.push("{ let _ = ");
                        self.emit_expr_ast(receiver);
                        self.push(".insert(");
                        self.emit_expr_as_arg_ast(&args[0]);
                        self.push(", ");
                        self.emit_expr_as_arg_ast(&args[1]);
                        self.push("); }");
                    }

                    // insert(x) — Set: emit HashSet::insert (returns bool, discarded).
                    "insert" if args.len() == 1 => {
                        self.push("{ let _ = ");
                        self.emit_expr_ast(receiver);
                        self.push(".insert(");
                        self.emit_expr_as_arg_ast(&args[0]);
                        self.push("); }");
                    }

                    // put(key, value) — Map: insert + return updated map (MVL value semantics).
                    "put" if args.len() == 2 => {
                        self.push("{ let mut __m = ");
                        self.emit_expr_ast(receiver);
                        self.push(".clone(); __m.insert(");
                        self.emit_expr_as_arg_ast(&args[0]);
                        self.push(", ");
                        self.emit_expr_as_arg_ast(&args[1]);
                        self.push("); __m }");
                    }

                    // without(key) — Map: remove key + return updated map (MVL value semantics).
                    "without" if args.len() == 1 => {
                        self.push("{ let mut __m = ");
                        self.emit_expr_ast(receiver);
                        self.push(".clone(); __m.remove(&(");
                        self.emit_expr_ast(&args[0]);
                        self.push(").clone()); __m }");
                    }

                    // remove(key) — Map: HashMap::remove returns Option<V> (correct for MVL).
                    //               Set: HashSet::remove returns bool (discarded as stmt).
                    "remove" if args.len() == 1 => {
                        self.emit_expr_ast(receiver);
                        self.push(".remove(&(");
                        self.emit_expr_ast(&args[0]);
                        self.push(").clone())");
                    }

                    // contains_key(k) — Map-only. Borrows key for HashMap::contains_key.
                    "contains_key" if args.len() == 1 => {
                        self.emit_expr_ast(receiver);
                        self.push(".contains_key(&(");
                        self.emit_expr_ast(&args[0]);
                        self.push(").clone())");
                    }

                    // keys() — Map: collect HashMap::keys() iterator into Vec.
                    "keys" if args.is_empty() => {
                        self.emit_expr_ast(receiver);
                        self.push(".keys().cloned().collect::<Vec<_>>()");
                    }

                    // values() — Map: collect HashMap::values() iterator into Vec.
                    "values" if args.is_empty() => {
                        self.emit_expr_ast(receiver);
                        self.push(".values().cloned().collect::<Vec<_>>()");
                    }

                    // to_list() — Set: collect HashSet::iter() into Vec.
                    "to_list" if args.is_empty() => {
                        self.emit_expr_ast(receiver);
                        self.push(".iter().cloned().collect::<Vec<_>>()");
                    }

                    // is_empty() — Vec, HashMap, HashSet all have is_empty() → bool. ✓
                    // Falls through to generic dispatch below (no special handling needed).

                    // intersection(b) / union(b) / difference(b) — Set operations.
                    // These return iterators; collect into HashSet.
                    // Clone the argument to avoid consuming it when used multiple times.
                    "intersection" if args.len() == 1 => {
                        let b = &args[0];
                        self.push("{ let __b = ");
                        self.emit_expr_ast(b);
                        self.push(".clone(); ");
                        self.emit_expr_ast(receiver);
                        self.push(
                            ".intersection(&__b).cloned().collect::<std::collections::HashSet<_>>() }",
                        );
                    }
                    "union" if args.len() == 1 => {
                        let b = &args[0];
                        self.push("{ let __b = ");
                        self.emit_expr_ast(b);
                        self.push(".clone(); ");
                        self.emit_expr_ast(receiver);
                        self.push(
                            ".union(&__b).cloned().collect::<std::collections::HashSet<_>>() }",
                        );
                    }
                    "difference" if args.len() == 1 => {
                        let b = &args[0];
                        self.push("{ let __b = ");
                        self.emit_expr_ast(b);
                        self.push(".clone(); ");
                        self.emit_expr_ast(receiver);
                        self.push(
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
                        let elem_is_labeled = self
                            .expr_types
                            .get(&receiver.span())
                            .is_some_and(|ty| matches!(ty, Ty::List(inner) if matches!(inner.as_ref(), Ty::Labeled(..))));
                        self.emit_expr_ast(receiver);
                        self.push(".push(");
                        if elem_is_labeled {
                            self.emit_expr_as_fn_arg_ast(&args[0]);
                        } else {
                            self.emit_expr_as_arg_ast(&args[0]);
                        }
                        self.push(")");
                    }
                    "extend" | "append" if args.len() == 1 => {
                        let elem_is_labeled = self
                            .expr_types
                            .get(&receiver.span())
                            .is_some_and(|ty| matches!(ty, Ty::List(inner) if matches!(inner.as_ref(), Ty::Labeled(..))));
                        self.emit_expr_ast(receiver);
                        self.push(".");
                        self.push(method);
                        self.push("(");
                        if elem_is_labeled {
                            self.emit_expr_as_fn_arg_ast(&args[0]);
                        } else {
                            self.emit_expr_as_arg_ast(&args[0]);
                        }
                        self.push(")");
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
                        let wrap_label: Option<String> =
                            if STRING_LABEL_PRESERVING_METHODS.contains(&m) {
                                self.expr_types.get(&receiver.span()).and_then(|ty| {
                                    if let Ty::Labeled(label, inner) = ty {
                                        if matches!(inner.as_ref(), Ty::String) {
                                            Some(emit_label(label.as_str()).to_string())
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
                            self.push(&format!("{lname}::new("));
                        }
                        self.push(method);
                        self.push("(");
                        self.emit_expr_ast(receiver);
                        self.push(".clone().into()");
                        if !args.is_empty() {
                            self.push(", ");
                            self.emit_args_ast(args);
                        }
                        self.push(")");
                        if wrap_label.is_some() {
                            self.push(")");
                        }
                    }

                    // ── Builtin stdlib method dispatch (#928) ───────────────────────────
                    // Builtin kernel methods are implemented by the runtime as free
                    // functions (e.g. `str_concat`, `list_get`).  Emit as
                    // `runtime_fn(receiver.clone().into(), args)`.
                    m if STDLIB_BUILTIN_METHODS.iter().any(|(mvl, _)| *mvl == m) => {
                        let (_, rust_fn) = STDLIB_BUILTIN_METHODS
                            .iter()
                            .find(|(mvl, _)| *mvl == m)
                            .unwrap();
                        self.push(rust_fn);
                        self.push("(");
                        self.emit_expr_ast(receiver);
                        self.push(".clone().into()");
                        if !args.is_empty() {
                            self.push(", ");
                            self.emit_args_ast(args);
                        }
                        self.push(")");
                    }

                    // ── Generic Rust method fallthrough ───────────────────────────────
                    _ => {
                        // If `method` is a known stdlib builtin (see `BUILTINS` in
                        // backends.rs), it should have an arm above.  Reaching here
                        // means the Rust backend is missing an emit arm — add it.
                        // Known gaps are tracked in issue #1095.
                        // #959: if `method` is a fn-typed struct field, emit `(receiver.field)(args)`
                        // instead of `receiver.field(args)` — Rust interprets the latter as a method
                        // call on the struct and cannot find the method in the impl block.
                        let is_fn_typed_field = if let Some(Ty::Named(type_name, type_args)) =
                            self.expr_types.get(&receiver.span())
                        {
                            type_args.is_empty()
                                && self
                                    .fn_typed_struct_fields
                                    .contains(&(type_name.clone(), method.clone()))
                        } else {
                            false
                        };
                        if is_fn_typed_field {
                            self.push("(");
                        }
                        self.emit_expr_ast(receiver);
                        self.push(".");
                        self.push(method);
                        if is_fn_typed_field {
                            self.push(")(");
                        } else {
                            self.push("(");
                        }
                        self.emit_args_ast(args);
                        self.push(")");
                    }
                }
            }
            Expr::FnCall {
                name,
                type_args,
                args,
                ..
            } => {
                // panic! is a Rust macro: first arg must be a bare string literal.
                // format → mvl_format to avoid collision with Rust's format! macro (#901).
                if name.as_str() == "format" {
                    self.push("mvl_format(");
                    self.emit_args_ast(args);
                    self.push(")");
                } else if name.as_str() == "panic" {
                    self.push(&format!("{name}!"));
                    self.push("(");
                    self.emit_args_for_macro_ast(args);
                    self.push(")");
                } else if matches!(name.as_str(), "assert_eq" | "assert_ne") {
                    // assert_eq!/assert_ne! compare same-typed values.  String literal args
                    // must NOT get `.into()` — it makes the type ambiguous in assert macros
                    // because `String` has many cross-type `PartialEq` impls (PathBuf, Box<str>,
                    // Arc<str>…) so Rust can't determine which `From<String>` to use.
                    self.push(&format!("{}!", name));
                    self.push("(");
                    self.emit_args_no_into_ast(args);
                    self.push(")");
                } else if name.as_str() == "map_new" || name.as_str() == "Map::new" {
                    // map_new[K, V]() / Map::new() → std::collections::HashMap::new().
                    // Type is inferred from the let-binding annotation.
                    self.push("std::collections::HashMap::new()");
                } else if name.as_str() == "from_int" {
                    // from_int(n: Int) -> Byte — wrapping cast i64 → u8.
                    // The checker enforces exactly 1 Int argument; assert here as a
                    // second line of defence so a missed check produces a clear panic
                    // rather than silent `( as u8)` which would be a Rust syntax error.
                    debug_assert_eq!(args.len(), 1, "from_int requires exactly one argument");
                    self.push("(");
                    if let Some(arg) = args.first() {
                        self.emit_expr_ast(arg);
                    }
                    self.push(" as u8)");
                } else if name == "String::from_chars" {
                    // #928: static builtin method → runtime free function.
                    self.push("str_from_chars(");
                    self.emit_args_ast(args);
                    self.push(")");
                } else if name == "String::from_bytes" {
                    self.push("str_from_bytes(");
                    self.emit_args_ast(args);
                    self.push(")");
                } else {
                    let is_extern = self.extern_fns.contains(name.as_str());
                    if is_extern {
                        self.push("unsafe { ");
                    }
                    // Phase 8: free calls to actor methods from within the actor state
                    // impl must be prefixed with `self.` (e.g. `log(seq)` → `self.log(seq)`).
                    if !is_extern && self.actor_methods.contains(name.as_str()) {
                        self.push("self.");
                    }
                    // #420: Use fully-qualified path for stdlib functions that would
                    // otherwise be shadowed by a locally-defined built-in of the same name.
                    if let Some(qualified) = self.stdlib_fn_qualified.get(name.as_str()).cloned() {
                        self.push(&qualified);
                    } else {
                        self.push(&map_fn_name(name));
                    }
                    if !type_args.is_empty() {
                        self.push("::<");
                        let strs: Vec<String> = type_args.iter().map(emit_type_expr).collect();
                        self.push(&strs.join(", "));
                        self.push(">");
                    }
                    self.push("(");
                    // Phase B: look up borrow flags for this callee so we can emit `&x`
                    // instead of `x.clone()` for reference parameters.
                    let borrows: Vec<Option<bool>> = self
                        .capability_params_map
                        .get(name.as_str())
                        .cloned()
                        .unwrap_or_default();
                    self.emit_args_with_borrows_ast(args, &borrows);
                    self.push(")");
                    if is_extern {
                        self.push(" }");
                    }
                }
            }
            Expr::Borrow { mutable, expr, .. } => {
                // Fix 7: parenthesise compound inner expressions to preserve precedence.
                // e.g. `&(a + b)` must not emit `&a + b` (which Rust parses as `(&a) + b`).
                let needs_parens =
                    !matches!(expr.as_ref(), Expr::Ident(_, _) | Expr::FieldAccess { .. });
                if *mutable {
                    self.push(if needs_parens { "&mut (" } else { "&mut " });
                } else {
                    self.push(if needs_parens { "&(" } else { "&" });
                }
                self.emit_expr_ast(expr);
                if needs_parens {
                    self.push(")");
                }
            }
            Expr::Unary { op, expr, .. } => match op {
                UnaryOp::Neg => {
                    self.push("-");
                    self.emit_expr_ast(expr);
                }
                UnaryOp::Not => {
                    self.push("(!");
                    self.emit_expr_ast(expr);
                    self.push(")");
                }
                UnaryOp::Deref => {
                    self.push("*(");
                    self.emit_expr_ast(expr);
                    self.push(")");
                }
                UnaryOp::BitNot => {
                    // Rust uses `!` for bitwise NOT on integer types.
                    self.push("!");
                    self.emit_expr_ast(expr);
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
                    self.alloc_binary_mutations(*op, span.line)
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
                    self.push(&format!("{{ let {lvar} = &("));
                    self.emit_expr_ast(left);
                    self.push(&format!("); let {rvar} = &("));
                    self.emit_expr_ast(right);
                    self.push("); match ::std::env::var(\"MVL_MUTANT\").as_deref() {");
                    for (mid, alt_op) in &mut_variants {
                        self.push(&format!(" Ok(\"{mid}\") => (*{lvar} {alt_op} *{rvar}),"));
                    }
                    self.push(&format!(
                        " _ => (*{lvar} {} *{rvar}), }} }}",
                        emit_binary_op(*op)
                    ));
                } else {
                    // For Int arithmetic, emit checked methods to match LLVM backend
                    // overflow behaviour (trap on overflow rather than wrapping).
                    let is_int_arith = matches!(op, BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul)
                        && matches!(self.expr_types.get(span), Some(Ty::Int));
                    if is_int_arith {
                        let method = match op {
                            BinaryOp::Add => "checked_add",
                            BinaryOp::Sub => "checked_sub",
                            BinaryOp::Mul => "checked_mul",
                            // Unreachable: `is_int_arith` is only true for Add/Sub/Mul (#991).
                            _ => unreachable!(),
                        };
                        // Use <i64>::clone(&(expr)) to coerce both i64 and &i64
                        // (pattern-bound variables in match arms on &Enum) to i64.
                        self.push("(<i64>::clone(&(");
                        self.emit_expr_ast(left);
                        self.push(&format!(")).{method}(<i64>::clone(&("));
                        self.emit_expr_ast(right);
                        self.push("))).expect(\"integer overflow\"))");
                    } else {
                        self.push("(");
                        self.emit_expr_ast(left);
                        self.push(" ");
                        self.push(emit_binary_op(*op));
                        self.push(" ");
                        // Rust requires `String + &str` for string concatenation.
                        // If the left side is a string-literal-rooted chain, borrow the right.
                        if *op == BinaryOp::Add && is_string_add_chain(left) {
                            self.push("&(");
                            self.emit_expr_ast(right);
                            self.push(")");
                        } else {
                            self.emit_expr_ast(right);
                        }
                        self.push(")");
                    }
                }
            }
            Expr::If {
                cond, then, else_, ..
            } => {
                self.push("if ");
                self.emit_expr_ast(cond);
                self.push(" {");
                self.nl();
                self.push_indent();
                self.emit_block_as_value_ast(&then.stmts);
                self.pop_indent();
                self.indent();
                self.push("}");
                if let Some(else_expr) = else_ {
                    self.push(" else ");
                    self.emit_expr_ast(else_expr);
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
                    .map(|i| self.alloc_branch(span.line, BranchKind::MatchArm(i)))
                    .collect();
                let has_str_pattern = arms_have_str_pattern(arms);
                // Emit scrutinee first so any compound conditions inside it allocate
                // MC/DC IDs before the match-level decisions (mirrors analysis order).
                self.push("match ");
                self.emit_expr_ast(scrutinee);
                // Clone when the scrutinee is a self.field access (can't move out of &self)
                // or a capability param (val/ref → &T/&mut T in Rust). Without clone,
                // match ergonomics yield reference bindings that fail E0507/E0277.
                if scrutinee_needs_clone(scrutinee)
                    || matches!(scrutinee.as_ref(), Expr::Ident(name, _) if self.capability_param_names.contains(name))
                {
                    self.push(".clone()");
                }
                // Allocate MC/DC arm-coverage decision after scrutinee.
                let match_mcdc_id: Option<usize> = if arms.len() >= 2 {
                    self.alloc_mcdc_decision(span.line, arms.len(), DecisionKind::Match, vec![])
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
                if has_str_pattern {
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
                    self.emit_match_arm_ast(arm, arm_idx, *cov_id, match_mcdc_id, *guard_mcdc_id);
                }
                self.pop_indent();
                self.indent();
                self.push("}");
            }
            Expr::Block(block) => {
                self.push("{");
                self.nl();
                self.push_indent();
                self.emit_block_as_value_ast(&block.stmts);
                self.pop_indent();
                self.indent();
                self.push("}");
            }
            Expr::Propagate { expr, .. } => {
                self.emit_expr_ast(expr);
                self.push("?");
            }
            Expr::Construct { name, fields, .. } => {
                self.push(name);
                self.push(" { ");
                // Emit directly into cg so nested FnCall expressions can look up
                // borrow_params_map. A fresh RustEmitter::new() would have an empty
                // map, causing borrow-inferred parameters inside field values to be
                // emitted as .clone() instead of &x (#465).
                for (i, (fname, fexpr)) in fields.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.push(&format!("{fname}: "));
                    // Clone field values: placing a value into a struct field is a
                    // move in Rust. MVL value semantics require the source binding to
                    // remain valid. Spec 009 Req 2: clone ALL non-Copy arguments.
                    self.emit_expr_as_arg_ast(fexpr);
                }
                self.push(" }");
            }
            Expr::List { elems, .. } => {
                self.push("vec![");
                self.emit_args_ast(elems);
                self.push("]");
            }
            Expr::Map { pairs, .. } => {
                self.push("std::collections::HashMap::from([");
                // Emit directly into cg for the same reason as Expr::Construct above.
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.push("(");
                    self.emit_expr_ast(k);
                    self.push(", ");
                    self.emit_expr_ast(v);
                    // `.clone().into()` coerces IFC-label wrappers (Clean<String>, etc.) to
                    // their plain inner type so map values match HashMap<String, String>
                    // signatures in stdlib functions like log_info / log_warn.
                    // Clone is required because MVL strings have value semantics — the
                    // same variable may be used after the map literal (e.g. as a log field
                    // and then as a function argument).
                    self.push(".clone().into()");
                    self.push(")");
                }
                self.push("])");
            }
            Expr::Set { elems, .. } => {
                self.push("std::collections::HashSet::from([");
                self.emit_args_ast(elems);
                self.push("])");
            }
            Expr::Consume { expr, .. } => {
                // `consume` mirrors Pony's `consume` for iso; just emit the inner expr in Phase 1
                self.emit_expr_ast(expr);
            }
            // `relabel name(expr, "tag")` — IFC label bridge (#894).
            // At Rust codegen level, the newtype wrappers are the runtime representation:
            //   - Unwrap transitions (Labeled → bare): emit `(expr).0`
            //   - Wrap transitions (bare → Labeled): emit `LabelName((expr))`
            // Standard transitions: trust/release unwrap; classify/taint wrap.
            // Capability labels: db_url/config_path/api_endpoint/audit_target wrap;
            // undb_url/unconfig_path/unapi_endpoint/unaudit_target unwrap.
            Expr::Relabel { name, expr, .. } => {
                match name.as_str() {
                    // Unwrap: strip the label newtype to get the inner value.
                    "trust" | "release" | "undb_url" | "unconfig_path" | "unapi_endpoint"
                    | "unaudit_target" => {
                        self.push("(");
                        self.emit_expr_ast(expr);
                        // .clone() is needed when the label wrapper is behind a shared
                        // reference (e.g. `self.api_key` in `&self` methods, or match
                        // bindings from `val` parameters). Always cloning is correct;
                        // the Rust compiler elides trivial clones on owned values.
                        self.push(").0.clone()");
                    }
                    // Wrap: construct the label newtype around the value.
                    "classify" => {
                        self.push("Secret((");
                        self.emit_expr_ast(expr);
                        self.push("))");
                    }
                    "taint" => {
                        self.push("Tainted((");
                        self.emit_expr_ast(expr);
                        self.push("))");
                    }
                    // Capability label wraps: db_url, config_path, api_endpoint, audit_target
                    "db_url" => {
                        self.push("DbUrl((");
                        self.emit_expr_ast(expr);
                        self.push("))");
                    }
                    "config_path" => {
                        self.push("ConfigPath((");
                        self.emit_expr_ast(expr);
                        self.push("))");
                    }
                    "api_endpoint" => {
                        self.push("ApiEndpoint((");
                        self.emit_expr_ast(expr);
                        self.push("))");
                    }
                    "audit_target" => {
                        self.push("AuditTarget((");
                        self.emit_expr_ast(expr);
                        self.push("))");
                    }
                    // Unknown relabel transitions are rejected by CheckError::UnknownRelabel
                    // before transpilation (#990), so this arm is unreachable in well-typed programs.
                    _ => {
                        unreachable!(
                            "relabel '{name}': unknown transition — blocked by checker (#990)"
                        );
                    }
                }
            }
            Expr::Lambda {
                params,
                ret_type,
                body,
                ..
            } => {
                self.push("|");
                let param_strs: Vec<String> = params
                    .iter()
                    .map(|p| {
                        let ty_str = emit_type_expr(&p.ty);
                        format!("{}: {ty_str}", p.name)
                    })
                    .collect();
                self.push(&param_strs.join(", "));
                self.push("|");
                if let Some(ret) = ret_type {
                    self.push(" -> ");
                    self.push(&emit_type_expr(ret));
                }
                self.push(" ");
                self.emit_expr_ast(body);
            }
            Expr::Spawn {
                actor_type, fields, ..
            } => {
                // Phase 8: `actor Counter { count: 0 }` → `_start_counter(CounterState { count: 0, _self_ref: None })`
                let snake =
                    crate::mvl::backends::rust::emit_actors::actor_name_to_snake(actor_type);
                self.push(&format!("_start_{snake}({actor_type}State {{"));
                for (i, (field_name, val)) in fields.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.push(&format!("{field_name}: "));
                    self.emit_expr_as_arg_ast(val);
                }
                // `_self_ref` is always None at construction; `_start_<name>` sets it
                // after the channel is created (so the handle can be cloned into state).
                if !fields.is_empty() {
                    self.push(", ");
                }
                self.push("_self_ref: None");
                self.push("})");
            }
            // Phase 8 (#743): select { arm => { body } … } — first-ready-wins stub.
            // Emits the first arm's handler body; full scheduler (try_recv polling)
            // is deferred until bidirectional channel receive is implemented.
            // Quantifier predicates are only valid in contract positions — never emitted.
            Expr::Quantifier(..) => unreachable!("Quantifier in codegen position"),
            Expr::Select { arms, .. } => {
                self.push("{");
                self.nl();
                self.push_indent();
                if let Some(first) = arms.first() {
                    self.emit_block_stmts_ast(&first.body.stmts);
                }
                self.pop_indent();
                self.indent();
                self.push("}");
            }
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

impl RustEmitter {
    fn emit_literal_ast(&mut self, lit: &Literal) {
        match lit {
            Literal::Integer(n) => self.push(&n.to_string()),
            Literal::Float(f) => {
                // Ensure float literals have a decimal point in Rust
                let s = format!("{f}");
                if s.contains('.') || s.contains('e') {
                    self.push(&s);
                } else {
                    self.push(&format!("{s}.0"));
                }
            }
            Literal::Str(s) => self.push(&format!("\"{}\".to_string()", escape_str(s))),
            Literal::Char(c) => self.push(&format!("'{}'", escape_char(*c))),
            Literal::Bool(b) => self.push(if *b { "true" } else { "false" }),
            Literal::Unit => self.push("()"),
        }
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

impl RustEmitter {
    /// Emit a literal in pattern position.  String literals must be bare `"s"`
    /// (not `"s".to_string()`) because Rust patterns cannot contain method calls.
    fn emit_literal_in_pattern_ast(&mut self, lit: &Literal) {
        match lit {
            Literal::Str(s) => self.push(&format!("\"{}\"", escape_str(s))),
            other => self.emit_literal_ast(other),
        }
    }

    // ── Arguments ─────────────────────────────────────────────────────────────

    fn emit_args_ast(&mut self, args: &[Expr]) {
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                self.push(", ");
            }
            self.emit_expr_as_fn_arg_ast(arg);
        }
    }

    /// Emit arguments without `.into()` on string literals.
    ///
    /// Used for `assert_eq!`/`assert_ne!` where `.into()` causes E0283: `String`
    /// has many cross-type `PartialEq<X>` impls so Rust can't determine which
    /// `From<String>` conversion to use.  Emitting `.to_string()` (no `.into()`)
    /// makes the concrete `String` type visible to the type checker.
    fn emit_args_no_into_ast(&mut self, args: &[Expr]) {
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                self.push(", ");
            }
            if let Expr::Literal(crate::mvl::parser::ast::Literal::Str(s), _) = arg {
                self.push(&format!("\"{}\".to_string()", escape_str(s)));
            } else {
                self.emit_expr_as_arg_ast(arg);
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
    fn emit_args_with_borrows_ast(&mut self, args: &[Expr], borrows: &[Option<bool>]) {
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                self.push(", ");
            }
            match borrows.get(i).copied().flatten() {
                Some(mutable) => self.emit_expr_as_borrow_arg_ast(arg, mutable),
                None => self.emit_expr_as_fn_arg_ast(arg),
            }
        }
    }

    /// Emit an expression as a reference argument (`&x` or `&mut x`).
    ///
    /// For identifiers and field accesses the prefix is prepended directly.
    /// For temporaries (function call results, struct literals, block expressions)
    /// the expression is wrapped in `&(…)` / `&mut (…)` — valid in Rust because
    /// temporaries live until the end of the enclosing statement.
    fn emit_expr_as_borrow_arg_ast(&mut self, expr: &Expr, mutable: bool) {
        match expr {
            // Fix 3: already a borrow expression — emit as-is, no extra & needed
            Expr::Borrow { .. } => self.emit_expr_ast(expr),
            Expr::Ident(_, _) | Expr::FieldAccess { .. } => {
                self.push(if mutable { "&mut " } else { "&" });
                self.emit_expr_ast(expr);
            }
            _ => {
                self.push(if mutable { "&mut (" } else { "&(" });
                self.emit_expr_ast(expr);
                self.push(")");
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
    fn emit_expr_as_arg_ast(&mut self, expr: &Expr) {
        match expr {
            Expr::Literal(Literal::Str(s), _) => {
                self.push(&format!("\"{}\".to_string().into()", escape_str(s)));
            }
            // Phase 8: `self` used as a tag argument inside an actor behavior.
            // The actor's own handle is stored in `_self_ref`; clone it to pass.
            Expr::Ident(name, _) if name == "self" && !self.actor_self_type.is_empty() => {
                // Upgrade the weak self-ref to a strong handle for the duration of this call.
                // Safe: we are inside a dispatch, so at least one external sender is alive.
                let ty = self.actor_self_type.clone();
                self.push(&format!(
                    "{ty} {{ _sender: self._self_ref.as_ref().unwrap().upgrade().unwrap() }}"
                ));
            }
            // Identifiers: check if this is the last use — if so, move instead of clone.
            Expr::Ident(_, span) => {
                self.emit_expr_ast(expr);
                if !self.last_uses.contains(span) {
                    self.push(".clone()");
                }
            }
            // Field accesses: conservatively clone (partial moves are complex in Rust).
            Expr::FieldAccess { .. } => {
                self.emit_expr_ast(expr);
                self.push(".clone()");
            }
            _ => {
                // Temporaries (function call results, struct literals, block expressions)
                // are rvalues that Rust moves into the callee — no `.clone()` needed.
                // The value is freshly created and has no other owner in the caller.
                self.emit_expr_ast(expr);
            }
        }
    }

    /// Emit an expression as an argument to a regular function call (not a macro).
    ///
    /// Adds `.into()` so that unlabeled (Public) values coerce to labeled parameters
    /// (e.g. `String` → `Clean<String>`) via `From<T> for Label<T>` in mvl_runtime::ifc.
    /// This is safe for function calls because the parameter type constrains `.into()`'s
    /// target, preventing the E0283 ambiguity that arises in macros like `println!`.
    fn emit_expr_as_fn_arg_ast(&mut self, expr: &Expr) {
        use crate::mvl::ir::Ty;
        match expr {
            Expr::Literal(Literal::Str(s), _) => {
                self.push(&format!("\"{}\".to_string().into()", escape_str(s)));
            }
            // Phase 8: `self` used as a tag argument inside an actor behavior.
            Expr::Ident(name, _) if name == "self" && !self.actor_self_type.is_empty() => {
                // Upgrade the weak self-ref to a strong handle for the duration of this call.
                // Safe: we are inside a dispatch, so at least one external sender is alive.
                let ty = self.actor_self_type.clone();
                self.push(&format!(
                    "{ty} {{ _sender: self._self_ref.as_ref().unwrap().upgrade().unwrap() }}"
                ));
            }
            // `self` in a type-attached method (`&self` receiver) cannot be moved — always
            // clone first so `self.clone().into()` works for any `T: Clone` type.
            // (Emitted as free function for built-in types, so `self_as_free_param` handles
            // that case separately via the general ident path below.)
            Expr::Ident(name, _)
                if name == "self"
                    && self.actor_self_type.is_empty()
                    && !self.self_as_free_param =>
            {
                self.push("self.clone().into()");
            }
            // Function-typed identifiers (callbacks, named function references) must NOT
            // get `.into()` — Rust function items do not implement `Into<_>` generically.
            Expr::Ident(_, span) if matches!(self.expr_types.get(span), Some(Ty::Fn(..))) => {
                self.emit_expr_ast(expr);
                if !self.last_uses.contains(span) {
                    self.push(".clone()");
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
                    self.expr_types.get(span),
                    Some(Ty::Option(_) | Ty::Result(_, _))
                ) =>
            {
                self.emit_expr_ast(expr);
                if !self.last_uses.contains(span) {
                    self.push(".clone()");
                }
            }
            // Value identifiers: `.into()` allows unlabeled (Public) values to coerce into
            // labeled parameters (e.g. `String` → `Clean<String>`).
            Expr::Ident(name, span) => {
                self.emit_expr_ast(expr);
                if !self.last_uses.contains(span) {
                    self.push(".clone().into()");
                } else if self.capability_param_names.contains(name.as_str()) {
                    // Last use of a `&T` (val-in-type-position) parameter: clone before
                    // into() to avoid the unsatisfied `&T: Into<T>` bound that arises
                    // when a sibling-module callee takes the value by owned `T`.
                    self.push(".clone().into()");
                } else {
                    self.push(".into()");
                }
            }
            Expr::FieldAccess { .. } => {
                self.emit_expr_ast(expr);
                self.push(".clone().into()");
            }
            _ => {
                self.emit_expr_ast(expr);
            }
        }
    }

    /// Emit arguments for Rust macros like `println!` where the first argument
    /// must be a bare string literal (not a `.to_string()` expression).
    fn emit_args_for_macro_ast(&mut self, args: &[Expr]) {
        if args.is_empty() {
            return;
        }
        match &args[0] {
            Expr::Literal(Literal::Str(s), _) => {
                // First arg is a string literal: emit bare, then remaining args as values.
                self.push(&format!("\"{}\"", escape_str(s)));
                for arg in &args[1..] {
                    self.push(", ");
                    self.emit_expr_as_arg_ast(arg);
                }
            }
            _ => {
                // First arg is not a string literal: generate one "{}" placeholder per
                // argument so the format string matches the argument count (#198).
                let placeholders = vec!["{}"; args.len()].join(" ");
                self.push(&format!("\"{placeholders}\""));
                for arg in args {
                    self.push(", ");
                    self.emit_expr_as_arg_ast(arg);
                }
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

impl RustEmitter {
    fn emit_match_arm_ast(
        &mut self,
        arm: &MatchArm,
        arm_idx: usize,
        cov_id: Option<usize>,
        match_mcdc_id: Option<usize>,
        guard_mcdc_id: Option<usize>,
    ) {
        self.indent();
        self.emit_pattern_ast(&arm.pattern);
        if let Some(guard) = &arm.guard {
            self.push(" if ");
            if let Some(gid) = guard_mcdc_id {
                let n = count_clauses_ref(guard);
                self.push(&emit_mcdc_guard_block(guard, gid, n));
            } else {
                use crate::mvl::backends::rust::emit_types::emit_ref_expr_for_assert;
                self.push(&emit_ref_expr_for_assert(guard, "_"));
            }
        }
        self.push(" => ");
        match &arm.body {
            MatchBody::Expr(e) => {
                // Wrap in a block to inject coverage and MC/DC hits.
                self.push("{ ");
                if let Some(id) = cov_id {
                    self.push(&format!("#[cfg(test)] crate::__mvl_cov::hit({id}); "));
                }
                if let Some(mid) = match_mcdc_id {
                    self.push(&format!(
                        "#[cfg(test)] crate::__mvl_mcdc::record({mid}usize, {arm_idx}u32); "
                    ));
                }
                self.emit_expr_ast(e);
                self.push(" }");
                self.push(",");
                self.nl();
            }
            MatchBody::Block(block) => {
                self.push("{");
                self.nl();
                self.push_indent();
                if let Some(id) = cov_id {
                    self.emit_cov_hit(id);
                }
                if let Some(mid) = match_mcdc_id {
                    self.line(&format!(
                        "#[cfg(test)] crate::__mvl_mcdc::record({mid}usize, {arm_idx}u32);"
                    ));
                }
                // Use emit_block_as_value so the final Stmt::Expr is a tail
                // expression (no semicolon) and becomes the arm's return value.
                // Mirrors the same fix in emit_stmts.rs for Stmt::Match arms.
                self.emit_block_as_value_ast(&block.stmts);
                self.pop_indent();
                self.indent();
                self.push("},");
                self.nl();
            }
        }
    }

    // ── Patterns ─────────────────────────────────────────────────────────────

    pub fn emit_pattern_ast(&mut self, pat: &Pattern) {
        match pat {
            Pattern::Wildcard(_) => self.push("_"),
            Pattern::Ident(name, _) => self.push(&map_ident(name)),
            Pattern::Literal(lit, _) => self.emit_literal_in_pattern_ast(lit),
            Pattern::Tuple { elems, .. } => {
                self.push("(");
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.emit_pattern_ast(e);
                }
                self.push(")");
            }
            Pattern::TupleStruct { name, fields, .. } => {
                self.push(name);
                self.push("(");
                for (i, f) in fields.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.emit_pattern_ast(f);
                }
                self.push(")");
            }
            Pattern::Struct { name, fields, .. } => {
                self.push(name);
                self.push(" { ");
                for (i, (fname, fpat)) in fields.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.push(fname);
                    self.push(": ");
                    self.emit_pattern_ast(fpat);
                }
                self.push(" }");
            }
            Pattern::Some { inner, .. } => {
                self.push("Some(");
                self.emit_pattern_ast(inner);
                self.push(")");
            }
            Pattern::None(_) => self.push("None"),
            Pattern::Ok { inner, .. } => {
                self.push("Ok(");
                self.emit_pattern_ast(inner);
                self.push(")");
            }
            Pattern::Err { inner, .. } => {
                self.push("Err(");
                self.emit_pattern_ast(inner);
                self.push(")");
            }
        }
    }

    // ── Block statements (used in if/match body/function body) ────────────────

    pub fn emit_block_stmts_ast(&mut self, stmts: &[crate::mvl::parser::ast::Stmt]) {
        for stmt in stmts {
            self.emit_stmt_ast(stmt);
        }
    }

    /// Emit block statements where the final `Stmt::Expr` is a tail expression
    /// (no semicolon), so it becomes the implicit return value of the block.
    pub fn emit_block_as_value_ast(&mut self, stmts: &[crate::mvl::parser::ast::Stmt]) {
        use crate::mvl::parser::ast::Stmt;
        if stmts.is_empty() {
            return;
        }
        let (head, tail) = stmts.split_at(stmts.len() - 1);
        for stmt in head {
            self.emit_stmt_ast(stmt);
        }
        match &tail[0] {
            Stmt::Expr { expr, .. } => {
                self.indent();
                self.emit_expr_ast(expr);
                self.nl();
            }
            other => self.emit_stmt_ast(other),
        }
    }
}

// ── Name mappings ─────────────────────────────────────────────────────────

fn map_ident(name: &str) -> String {
    name.to_string()
}

fn map_fn_name(name: &str) -> String {
    // Built-in MVL functions mapped to Rust / stdlib equivalents
    match name {
        "panic" => "panic!".to_string(),
        "assert" => "assert!".to_string(),
        "assert_eq" => "assert_eq!".to_string(),
        "assert_ne" => "assert_ne!".to_string(),
        _ => name.to_string(),
    }
}

impl RustEmitter {
    /// Emit `receiver.len()` as direct Rust using the receiver's checker type (#554).
    ///
    /// - `String` → `.chars().count() as i64` (Unicode codepoint count, not byte count).
    /// - `List/Map/Set` → `.len() as i64`.
    /// - `Labeled(label, inner)` → `Label((&receiver).0.method as i64)` preserving IFC label.
    /// - Unknown → `.len() as i64` (safe fallback for any Rust collection).
    fn emit_len_direct_ast(&mut self, receiver: &Expr, ty: Option<&Ty>) {
        // Wrap in parens: `as i64` has low precedence and `.clone()` cannot follow
        // an unparenthesised cast, e.g. `xs.len() as i64.clone()` is a parse error.
        match ty {
            Some(Ty::String) => {
                self.push("(");
                self.emit_expr_ast(receiver);
                self.push(".chars().count() as i64)");
            }
            Some(Ty::Labeled(label, inner)) => {
                let label_name = emit_label(label.as_str());
                let method = match inner.as_ref() {
                    Ty::String => ".chars().count()",
                    _ => ".len()",
                };
                self.push(&format!("{label_name}((&("));
                self.emit_expr_ast(receiver);
                self.push(&format!(")).0{method} as i64)"));
            }
            _ => {
                self.push("(");
                self.emit_expr_ast(receiver);
                self.push(".len() as i64)");
            }
        }
    }

    /// Emit `n.clamp(low, high)` as a safe Rust block expression.
    ///
    /// - If `low > high`, the value is returned unchanged (graceful degradation).
    /// - Otherwise, clamps the value to `[low, high]`.
    /// - Never panics (unlike Rust's `i64::clamp`/`f64::clamp` which panic on inverted bounds).
    fn emit_safe_clamp_ast(&mut self, receiver: &Expr, low: &Expr, high: &Expr) {
        self.push("{let _mvl_n=(");
        self.emit_expr_ast(receiver);
        self.push(");let _mvl_lo=(");
        self.emit_expr_ast(low);
        self.push(");let _mvl_hi=(");
        self.emit_expr_ast(high);
        self.push(");if _mvl_lo>_mvl_hi{_mvl_n}else{_mvl_n.clamp(_mvl_lo,_mvl_hi)}}");
    }
}
