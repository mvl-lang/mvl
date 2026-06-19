// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emit Rust method calls from MVL [`TirExprKind::MethodCall`] nodes.

use super::emitter::RustEmitter;
use crate::mvl::backends::rust::emit_types::emit_label;
use crate::mvl::backends::{
    is_stdlib_method, is_stdlib_ufcs_method, rust_emit_by_name, STRING_LABEL_PRESERVING_METHODS,
};
use crate::mvl::ir::{TirExpr, TirExprKind, Ty};

impl RustEmitter {
    /// Emit a Rust method call from a MVL [`TirExprKind::MethodCall`].
    ///
    /// Dispatches on the method name with type-aware selection for higher-order
    /// collection methods, type coercions, stdlib integration, and generic fallthrough.
    pub(super) fn emit_method_call(&mut self, receiver: &TirExpr, method: &str, args: &[TirExpr]) {
        // Methods that don't map directly to a Rust method of the same name.
        //
        // Phase 4 note: string and list methods that have pure MVL implementations
        // in std/strings.mvl and std/lists.mvl are dispatched via UFCS in the `_`
        // fallback below: `s.trim()` → `trim(s)`, `xs.take(n)` → `take(xs, n)`.
        // The actual Rust implementation is the transpiled MVL stdlib function.
        match method {
            // ── Higher-order collection methods ──────────────────────────────

            // map(f) — direct Rust dispatch using checker type info (#554).
            // Option/Result use .map(); Set uses into_iter().collect::<HashSet>();
            // List and unknown types use into_iter().collect::<Vec>().
            "map" if args.len() == 1 => {
                let receiver_ty = Some(receiver.ty.clone());
                // Use is_option/is_result which strip security labels (Labeled<Option<T>>
                // and Labeled<Result<T,E>> are still Option/Result for dispatch purposes).
                let is_opt_or_result = receiver_ty
                    .as_ref()
                    .is_some_and(|t| t.is_option() || t.is_result());
                let is_set = receiver_ty
                    .as_ref()
                    .is_some_and(|t| matches!(t.unlabeled(), Ty::Set(_)));
                if is_opt_or_result {
                    self.emit_expr(receiver);
                    self.push(".map(|__x| (");
                    self.emit_expr(&args[0]);
                    self.push(")(__x.clone()))");
                } else if is_set {
                    // Set.map: into_iter + collect into HashSet.
                    self.emit_expr(receiver);
                    self.push(".into_iter().map(|__x| (");
                    self.emit_expr(&args[0]);
                    self.push(")(__x.clone())).collect::<std::collections::HashSet<_>>()");
                } else {
                    // List (and unknown types) use into_iter().collect().
                    self.emit_expr(receiver);
                    self.push(".into_iter().map(|__x| (");
                    self.emit_expr(&args[0]);
                    self.push(")(__x.clone())).collect::<Vec<_>>()");
                }
            }
            // map_values(f) — Map only: transform values, keep keys.
            "map_values" if args.len() == 1 => {
                self.emit_expr(receiver);
                self.push(".clone().into_iter().map(|(__k, __v)| (__k, (");
                self.emit_expr(&args[0]);
                self.push(")(__v.clone()))).collect::<std::collections::HashMap<_, _>>()");
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
            // of each element; result collected back into Vec (or HashMap/HashSet).
            "filter" | "take_while" | "skip_while" if args.len() == 1 => {
                let receiver_ty = Some(receiver.ty.clone());
                let is_map = method == "filter"
                    && receiver_ty
                        .as_ref()
                        .is_some_and(|t| matches!(t.unlabeled(), Ty::Map(_, _)));
                let is_set = method == "filter"
                    && receiver_ty
                        .as_ref()
                        .is_some_and(|t| matches!(t.unlabeled(), Ty::Set(_)));
                if is_map {
                    // Map.filter(f: fn(V) -> Bool): destructure entry, test value.
                    self.emit_expr(receiver);
                    self.push(".clone().into_iter().filter(|(_k, __v)| (");
                    self.emit_expr(&args[0]);
                    self.push(")(__v.clone())).collect::<std::collections::HashMap<_, _>>()");
                } else if is_set {
                    // Set.filter(f: fn(T) -> Bool): same as List but collect HashSet.
                    self.emit_expr(receiver);
                    self.push(".clone().into_iter().filter(|__x| (");
                    self.emit_expr(&args[0]);
                    self.push(")(__x.clone())).collect::<std::collections::HashSet<_>>()");
                } else {
                    let needs_borrow = if let TirExprKind::Var(name) = &args[0].kind {
                        self.capability_params_map
                            .get(name.as_str())
                            .and_then(|b| b.first().copied())
                            .flatten()
                            .is_some()
                    } else {
                        false
                    };
                    self.emit_expr(receiver);
                    self.push(".clone().into_iter().");
                    self.push(method);
                    self.push("(|__x| (");
                    self.emit_expr(&args[0]);
                    if needs_borrow {
                        self.push(")(&__x.clone())).collect::<Vec<_>>()");
                    } else {
                        self.push(")(__x.clone())).collect::<Vec<_>>()");
                    }
                }
            }
            // any / all — same predicate pattern but return bool, no collect.
            "any" | "all" if args.len() == 1 => {
                let receiver_ty = Some(receiver.ty.clone());
                let is_map = receiver_ty
                    .as_ref()
                    .is_some_and(|t| matches!(t.unlabeled(), Ty::Map(_, _)));
                if is_map {
                    // Map.any/all(f: fn(V) -> Bool): destructure entry, test value.
                    self.emit_expr(receiver);
                    self.push(".clone().into_iter().");
                    self.push(method);
                    self.push("(|(_k, __v)| (");
                    self.emit_expr(&args[0]);
                    self.push(")(__v.clone()))");
                } else {
                    // List, Set, and other types iterate elements directly.
                    let needs_borrow = if let TirExprKind::Var(name) = &args[0].kind {
                        self.capability_params_map
                            .get(name.as_str())
                            .and_then(|b| b.first().copied())
                            .flatten()
                            .is_some()
                    } else {
                        false
                    };
                    self.emit_expr(receiver);
                    self.push(".clone().into_iter().");
                    self.push(method);
                    self.push("(|__x| (");
                    self.emit_expr(&args[0]);
                    if needs_borrow {
                        self.push(")(&__x.clone()))");
                    } else {
                        self.push(")(__x.clone()))");
                    }
                }
            }
            // fold(init, f) — init cloned (value arg); f wrapped in closure
            // so capturing closures are accepted alongside fn pointers.
            // When f is a named function with borrow params, add & to the
            // accumulator and/or element in the generated lambda.
            "fold" if args.len() == 2 => {
                let receiver_ty = Some(receiver.ty.clone());
                let is_map = receiver_ty
                    .as_ref()
                    .is_some_and(|t| matches!(t.unlabeled(), Ty::Map(_, _)));
                if is_map {
                    // Map.fold(init, f: fn(U, V) -> U): destructure entry, fold over values.
                    self.emit_expr(receiver);
                    self.push(".clone().into_iter().fold(");
                    self.emit_expr_as_arg(&args[0]);
                    self.push(", |acc, (_k, __v)| (");
                    self.emit_expr(&args[1]);
                    self.push(")(acc, __v))");
                } else {
                    // List, Set, and other types iterate elements directly.
                    let (borrow_acc, borrow_elem) = if let TirExprKind::Var(name) = &args[1].kind {
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
                    self.emit_expr(receiver);
                    self.push(".clone().into_iter().fold(");
                    self.emit_expr_as_arg(&args[0]);
                    self.push(", |acc, __x| (");
                    self.emit_expr(&args[1]);
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
            }
            // windows(n)/chunks(n) — Rust returns &[T] slices; collect into Vec<Vec<T>>.
            // MVL passes n as Int (i64); Rust requires usize, so cast.
            "windows" | "chunks" => {
                self.emit_expr(receiver);
                self.push(".");
                self.push(method);
                self.push("(");
                if let Some(arg) = args.first() {
                    self.emit_expr(arg);
                    self.push(" as usize");
                }
                self.push(").map(|w| w.to_vec()).collect::<Vec<_>>()");
            }
            // enumerate() -> List[Indexed[T]]   (struct, not tuple — #1383)
            "enumerate" if args.is_empty() => {
                self.emit_expr(receiver);
                self.push(".into_iter().enumerate().map(|(__i, __v)| Indexed { index: __i as i64, value: __v }).collect::<Vec<_>>()");
            }
            // zip(other) -> List[Pair[T, U]]   (struct, not tuple — #1383)
            "zip" if args.len() == 1 => {
                self.emit_expr(receiver);
                self.push(".into_iter().zip(");
                self.emit_expr(&args[0]);
                self.push(".into_iter()).map(|(__a, __b)| Pair { first: __a, second: __b }).collect::<Vec<_>>()");
            }
            // partition(f) -> Partitioned[T]   (struct, not tuple — #1380)
            "partition" => {
                self.push("{ let (__matching, __rest): (Vec<_>, Vec<_>) = ");
                self.emit_expr(receiver);
                self.push(".into_iter().partition(|__x| ");
                if let Some(arg) = args.first() {
                    self.emit_expr(arg);
                }
                self.push("(__x.clone())); Partitioned { matching: __matching, rest: __rest } }");
            }
            // group_by(f) — no native Rust equivalent; fold into HashMap
            "group_by" => {
                self.push("{ let mut __m = std::collections::HashMap::new(); for __v in ");
                self.emit_expr(receiver);
                self.push(".into_iter() { __m.entry(");
                if let Some(arg) = args.first() {
                    // Phase B: if the key function takes a reference for its first
                    // parameter, emit `&__v.clone()` instead of `__v.clone()`.
                    let needs_borrow = if let TirExprKind::Var(name) = &arg.kind {
                        self.capability_params_map
                            .get(name.as_str())
                            .and_then(|b| b.first().copied())
                            .flatten()
                            .is_some()
                    } else {
                        false
                    };
                    self.push("(");
                    self.emit_expr(arg);
                    self.push(")");
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
                self.emit_expr(receiver);
                self.push(".and_then(|__x| (");
                self.emit_expr(&args[0]);
                self.push(")(__x.clone()))");
            }
            // sort() — sort_by with partial_cmp for numeric stability
            "sort" if args.is_empty() => {
                self.push("{let mut __v=(");
                self.emit_expr(receiver);
                self.push(");__v.sort_by(|__a,__b|__a.partial_cmp(__b).unwrap_or(std::cmp::Ordering::Equal));__v}");
            }
            // min() — smallest element via partial_cmp
            "min" if args.is_empty() => {
                self.emit_expr(receiver);
                self.push(".into_iter().min_by(|__a,__b|__a.partial_cmp(__b).unwrap_or(std::cmp::Ordering::Equal))");
            }
            // max() — largest element via partial_cmp
            "max" if args.is_empty() => {
                self.emit_expr(receiver);
                self.push(".into_iter().max_by(|__a,__b|__a.partial_cmp(__b).unwrap_or(std::cmp::Ordering::Equal))");
            }
            // join(sep) — join strings with separator
            "join" if args.len() == 1 => {
                self.emit_expr(receiver);
                self.push(".join(&");
                self.emit_expr(&args[0]);
                self.push(")");
            }

            // ── Operator-level methods ────────────────────────────────────────
            //
            // Bitwise ops on Int/Byte: emitted as Rust operators for LLVM
            // visibility and future intrinsic optimisation.
            "bit_and" if args.len() == 1 => {
                self.push("(");
                self.emit_expr(receiver);
                self.push(" & ");
                self.emit_expr(&args[0]);
                self.push(")");
            }
            "bit_or" if args.len() == 1 => {
                self.push("(");
                self.emit_expr(receiver);
                self.push(" | ");
                self.emit_expr(&args[0]);
                self.push(")");
            }
            "bit_xor" if args.len() == 1 => {
                self.push("(");
                self.emit_expr(receiver);
                self.push(" ^ ");
                self.emit_expr(&args[0]);
                self.push(")");
            }
            "bit_not" if args.is_empty() => {
                self.push("(!");
                self.emit_expr(receiver);
                self.push(")");
            }
            // wrapping_shl/shr avoids debug-mode panic for out-of-range shift counts
            "shift_left" if args.len() == 1 => {
                self.push("(");
                self.emit_expr(receiver);
                self.push(".wrapping_shl(");
                self.emit_expr(&args[0]);
                self.push(" as u32))");
            }
            "shift_right" if args.len() == 1 => {
                self.push("(");
                self.emit_expr(receiver);
                self.push(".wrapping_shr(");
                self.emit_expr(&args[0]);
                self.push(" as u32))");
            }
            // is_zero() — i64 has no is_zero(); emit comparison
            "is_zero" if args.is_empty() => {
                self.push("(");
                self.emit_expr(receiver);
                self.push(" == 0)");
            }
            // to_int() on Byte (u8→i64) or Float (f64→i64, truncating)
            "to_int" if args.is_empty() => {
                self.push("(");
                self.emit_expr(receiver);
                self.push(" as i64)");
            }
            // to_float() on Int (i64→f64); i64::from() unwraps IFC labels transparently
            "to_float" if args.is_empty() => {
                self.push("(i64::from(");
                self.emit_expr(receiver);
                self.push(".clone()) as f64)");
            }
            // pow(e) — direct Rust using checker type info (#554).
            // i64: .pow(e as u32); f64: .powf(e).
            "pow" if args.len() == 1 => {
                let receiver_ty = Some(receiver.ty.clone());
                self.emit_expr(receiver);
                match receiver_ty.as_ref() {
                    Some(Ty::Float) => {
                        self.push(".powf(");
                        self.emit_expr_as_arg(&args[0]);
                        self.push(")");
                    }
                    _ => {
                        self.push(".pow(");
                        self.emit_expr_as_arg(&args[0]);
                        self.push(" as u32)");
                    }
                }
            }
            // clamp(low, high) — Rust's clamp panics on inverted bounds; safe wrapper
            "clamp" if args.len() == 2 => {
                self.emit_safe_clamp(receiver, &args[0], &args[1]);
            }
            // contains(x) — direct Rust using checker type info (#554).
            // String: .contains(arg.as_str()); List/Set: .contains(&arg).
            "contains" if args.len() == 1 => {
                let receiver_ty = Some(receiver.ty.clone());
                self.emit_expr(receiver);
                match receiver_ty.as_ref() {
                    Some(Ty::String) => {
                        // emit_args_no_into avoids .into() before .as_str().
                        self.push(".contains((");
                        self.emit_args_no_into(args);
                        self.push(").as_str())");
                    }
                    _ => {
                        self.push(".contains(&(");
                        self.emit_args(args);
                        self.push("))");
                    }
                }
            }

            // concat(x) — type-aware dispatch (#928):
            //   String: str_concat(receiver, other)
            //   List:   list_concat(receiver, other)
            "concat" if args.len() == 1 => {
                let receiver_ty = Some(receiver.ty.clone());
                let rust_fn = match receiver_ty.as_ref() {
                    Some(Ty::List(_)) => "list_concat",
                    _ => "str_concat",
                };
                self.push(rust_fn);
                self.push("(");
                self.emit_expr(receiver);
                self.push(".clone().into()");
                self.push(", ");
                self.emit_args(args);
                self.push(")");
            }

            // ── Map / Set / List unified method traits ────────────────────────

            // get(key) — direct Rust using checker type info (#554).
            // Map: .get(&key).cloned(); List: bounds-checked index.
            "get" if args.len() == 1 => {
                let receiver_ty = Some(receiver.ty.clone());
                match receiver_ty.as_ref() {
                    Some(Ty::Map(_, _)) => {
                        self.emit_expr(receiver);
                        self.push(".get(&(");
                        self.emit_expr(&args[0]);
                        self.push(").clone()).cloned()");
                    }
                    _ => {
                        self.push("{ let __mvl_i = (");
                        self.emit_expr(&args[0]);
                        self.push("); if __mvl_i < 0 { None } else { (");
                        self.emit_expr(receiver);
                        self.push(").get(__mvl_i as usize).cloned() } }");
                    }
                }
            }

            // len() — direct Rust using checker type info (#554).
            // String: .chars().count() as i64; List/Map/Set: .len() as i64.
            // Labeled types: propagate label via field access.
            "len" if args.is_empty() => {
                let receiver_ty = Some(receiver.ty.clone());
                self.emit_len_direct(receiver, receiver_ty.as_ref());
            }

            // insert(k, v) — Map: emit HashMap::insert (returns Option, discarded).
            "insert" if args.len() == 2 => {
                self.push("{ let _ = ");
                self.emit_expr(receiver);
                self.push(".insert(");
                self.emit_expr_as_arg(&args[0]);
                self.push(", ");
                self.emit_expr_as_arg(&args[1]);
                self.push("); }");
            }

            // insert(x) — Set: emit HashSet::insert (returns bool, discarded).
            "insert" if args.len() == 1 => {
                self.push("{ let _ = ");
                self.emit_expr(receiver);
                self.push(".insert(");
                self.emit_expr_as_arg(&args[0]);
                self.push("); }");
            }

            // put(key, value) — Map: insert + return updated map (MVL value semantics).
            "put" if args.len() == 2 => {
                self.push("{ let mut __m = ");
                self.emit_expr(receiver);
                self.push(".clone(); __m.insert(");
                self.emit_expr_as_arg(&args[0]);
                self.push(", ");
                self.emit_expr_as_arg(&args[1]);
                self.push("); __m }");
            }

            // without(key) — Map: remove key + return updated map (MVL value semantics).
            "without" if args.len() == 1 => {
                self.push("{ let mut __m = ");
                self.emit_expr(receiver);
                self.push(".clone(); __m.remove(&(");
                self.emit_expr(&args[0]);
                self.push(").clone()); __m }");
            }

            // remove(key) — Map: HashMap::remove returns Option<V> (correct for MVL).
            //               Set: HashSet::remove returns bool (discarded as stmt).
            "remove" if args.len() == 1 => {
                self.emit_expr(receiver);
                self.push(".remove(&(");
                self.emit_expr(&args[0]);
                self.push(").clone())");
            }

            // contains_key(k) — Map-only. Borrows key for HashMap::contains_key.
            "contains_key" if args.len() == 1 => {
                self.emit_expr(receiver);
                self.push(".contains_key(&(");
                self.emit_expr(&args[0]);
                self.push(").clone())");
            }

            // keys() — Map: collect HashMap::keys() iterator into Vec.
            "keys" if args.is_empty() => {
                self.emit_expr(receiver);
                self.push(".keys().cloned().collect::<Vec<_>>()");
            }

            // values() — Map: collect HashMap::values() iterator into Vec.
            "values" if args.is_empty() => {
                self.emit_expr(receiver);
                self.push(".values().cloned().collect::<Vec<_>>()");
            }

            // entries() -> List[Entry[K, V]]   (struct, not tuple — #1383)
            "entries" if args.is_empty() => {
                self.emit_expr(receiver);
                self.push(".into_iter().map(|(__k, __v)| Entry { key: __k, value: __v }).collect::<Vec<_>>()");
            }

            // to_list() — Set: collect HashSet::iter() into Vec.
            "to_list" if args.is_empty() => {
                self.emit_expr(receiver);
                self.push(".iter().cloned().collect::<Vec<_>>()");
            }

            // intersection(b) / union(b) / difference(b) — Set operations.
            // Type-guarded: only fire for Set receivers so that user-defined types
            // with methods named `union`/`intersection` fall through to generic emit
            // (e.g. Span::union, Span::intersect — #1371).
            "intersection" if args.len() == 1 && matches!(receiver.ty, Ty::Set(_)) => {
                let b = &args[0];
                self.push("{ let __b = ");
                self.emit_expr(b);
                self.push(".clone(); ");
                self.emit_expr(receiver);
                self.push(
                    ".intersection(&__b).cloned().collect::<std::collections::HashSet<_>>() }",
                );
            }
            "union" if args.len() == 1 && matches!(receiver.ty, Ty::Set(_)) => {
                let b = &args[0];
                self.push("{ let __b = ");
                self.emit_expr(b);
                self.push(".clone(); ");
                self.emit_expr(receiver);
                self.push(".union(&__b).cloned().collect::<std::collections::HashSet<_>>() }");
            }
            "difference" if args.len() == 1 && matches!(receiver.ty, Ty::Set(_)) => {
                let b = &args[0];
                self.push("{ let __b = ");
                self.emit_expr(b);
                self.push(".clone(); ");
                self.emit_expr(receiver);
                self.push(".difference(&__b).cloned().collect::<std::collections::HashSet<_>>() }");
            }

            // set(i, value) — in-place index assignment.
            "set" if args.len() == 2 => {
                self.push("{ let __mvl_i = (");
                self.emit_expr(&args[0]);
                self.push("); (");
                self.emit_expr(receiver);
                self.push(")[__mvl_i as usize] = ");
                self.emit_expr_as_arg(&args[1]);
                self.push("; }");
            }

            // push(elem) / extend(iter) / append(other) — collection mutators.
            "push" if args.len() == 1 => {
                let elem_is_labeled = matches!(&receiver.ty, Ty::List(inner) if matches!(inner.as_ref(), Ty::Labeled(..)));
                self.emit_expr(receiver);
                self.push(".push(");
                if elem_is_labeled {
                    self.emit_expr_as_fn_arg(&args[0]);
                } else {
                    self.emit_expr_as_arg(&args[0]);
                }
                self.push(")");
            }
            "extend" | "append" if args.len() == 1 => {
                let elem_is_labeled = matches!(&receiver.ty, Ty::List(inner) if matches!(inner.as_ref(), Ty::Labeled(..)));
                self.emit_expr(receiver);
                self.push(".");
                self.push(method);
                self.push("(");
                if elem_is_labeled {
                    self.emit_expr_as_fn_arg(&args[0]);
                } else {
                    self.emit_expr_as_arg(&args[0]);
                }
                self.push(")");
            }

            // into_inner() / as_inner() on IFC label wrapper types.
            // These are generated as methods on the label newtype struct by emit_types.rs.
            // Without this case, labeled receivers hit the `is_builtin_receiver` UFCS branch
            // (since e.g. `Tainted<String>.unlabeled()` is String) and are incorrectly
            // emitted as free function calls: `into_inner(v)` instead of `v.into_inner()`.
            //
            // into_inner() takes ownership (self), so we must clone the receiver first
            // when it's behind a reference. Since MVL uses clone-on-read semantics
            // everywhere, always emitting .clone() is safe and consistent. (#1453)
            "into_inner" | "as_inner"
                if args.is_empty() && matches!(receiver.ty, Ty::Labeled(..)) =>
            {
                self.emit_expr(receiver);
                self.push(".clone().");
                self.push(method);
                self.push("()");
            }

            // ── UFCS dispatch for pure MVL stdlib methods ─────────────────────
            m if is_stdlib_ufcs_method(m) => {
                // Check whether we must re-wrap the result in a label newtype.
                let wrap_label: Option<String> = if STRING_LABEL_PRESERVING_METHODS.contains(&m) {
                    {
                        let ty = &receiver.ty;
                        if let Ty::Labeled(label, inner) = ty {
                            if matches!(inner.as_ref(), Ty::String) {
                                Some(emit_label(label.as_str()).to_string())
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                } else {
                    None
                };
                if let Some(ref lname) = wrap_label {
                    self.push(&format!("{lname}::new("));
                }
                self.push(method);
                self.push("(");
                self.emit_expr(receiver);
                self.push(".clone().into()");
                if !args.is_empty() {
                    self.push(", ");
                    self.emit_args(args);
                }
                self.push(")");
                if wrap_label.is_some() {
                    self.push(")");
                }
            }

            // ── Builtin stdlib method dispatch (#928) ───────────────────────────
            // Kernel builtins with `rust_emit` hints in `BUILTINS` are dispatched
            // as `runtime_fn(receiver.clone().into(), args)`.
            m if rust_emit_by_name(m).is_some() => {
                // Label-preserving methods on String need re-wrapping (#1267).
                let wrap_label: Option<String> = if STRING_LABEL_PRESERVING_METHODS.contains(&m) {
                    if let Ty::Labeled(label, inner) = &receiver.ty {
                        if matches!(inner.as_ref(), Ty::String) {
                            Some(emit_label(label.as_str()).to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(ref lname) = wrap_label {
                    self.push(&format!("{lname}::new("));
                }
                self.push(rust_emit_by_name(m).unwrap());
                self.push("(");
                self.emit_expr(receiver);
                self.push(".clone().into()");
                if !args.is_empty() {
                    self.push(", ");
                    self.emit_args(args);
                }
                self.push(")");
                if wrap_label.is_some() {
                    self.push(")");
                }
            }

            // ── Generic Rust method fallthrough ───────────────────────────────
            _ => {
                // #992 / #1433: Pure MVL extension methods on builtin types are
                // transpiled as top-level free functions and must be called via UFCS
                // (`method(receiver.clone().into(), args)`), not as native Rust
                // methods. Detect this case by checking the receiver type is a
                // builtin AND the method is not a registered kernel builtin.
                let is_builtin_receiver = matches!(
                    receiver.ty.unlabeled(),
                    Ty::String
                        | Ty::Int
                        | Ty::Float
                        | Ty::Bool
                        | Ty::Byte
                        | Ty::UByte
                        | Ty::UInt
                        | Ty::List(_)
                        | Ty::Map(_, _)
                        | Ty::Set(_)
                        | Ty::Option(_)
                        | Ty::Result(_, _)
                );
                if is_builtin_receiver && !is_stdlib_method(method) {
                    self.push(method);
                    self.push("(");
                    self.emit_expr(receiver);
                    self.push(".clone().into()");
                    if !args.is_empty() {
                        self.push(", ");
                        self.emit_args(args);
                    }
                    self.push(")");
                } else {
                    let is_fn_typed_field = if let Ty::Named(type_name, type_args) = &receiver.ty {
                        type_args.is_empty()
                            && self
                                .fn_typed_struct_fields
                                .contains(&(type_name.clone(), method.to_owned()))
                    } else {
                        false
                    };
                    if is_fn_typed_field {
                        self.push("(");
                    }
                    self.emit_expr(receiver);
                    self.push(".");
                    self.push(method);
                    if is_fn_typed_field {
                        self.push(")(");
                    } else {
                        self.push("(");
                    }
                    self.emit_args(args);
                    self.push(")");
                }
            }
        }
    }

    /// Emit `receiver.len()` as direct Rust using the receiver's checker type (#554).
    fn emit_len_direct(&mut self, receiver: &TirExpr, ty: Option<&Ty>) {
        match ty {
            Some(Ty::String) => {
                self.push("(");
                self.emit_expr(receiver);
                self.push(".chars().count() as i64)");
            }
            Some(Ty::Labeled(label, inner)) => {
                let label_name = emit_label(label.as_str());
                let method = match inner.as_ref() {
                    Ty::String => ".chars().count()",
                    _ => ".len()",
                };
                self.push(&format!("{label_name}((&("));
                self.emit_expr(receiver);
                self.push(&format!(")).0{method} as i64)"));
            }
            _ => {
                self.push("(");
                self.emit_expr(receiver);
                self.push(".len() as i64)");
            }
        }
    }

    /// Emit `n.clamp(low, high)` as a safe Rust block expression.
    fn emit_safe_clamp(&mut self, receiver: &TirExpr, low: &TirExpr, high: &TirExpr) {
        self.push("{let _mvl_n=(");
        self.emit_expr(receiver);
        self.push(");let _mvl_lo=(");
        self.emit_expr(low);
        self.push(");let _mvl_hi=(");
        self.emit_expr(high);
        self.push(");if _mvl_lo>_mvl_hi{_mvl_n}else{_mvl_n.clamp(_mvl_lo,_mvl_hi)}}");
    }
}
