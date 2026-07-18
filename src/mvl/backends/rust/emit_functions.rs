// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emit Rust function declarations from MVL function nodes.
//!
//! `emit_fn_decl` works on [`TirFn`] (TIR-typed functions).
//! `emit_fn_decl_ast` works on [`FnDecl`] (AST functions, used for prelude/stdlib).
//!
//! Phase 1 mappings:
//! - Effects (`! DB + Console`) → `/// # Effects: DB, Console` doc comment
//! - Totality (`total`) → `/// # Totality: total` doc comment
//! - Capabilities (`iso`, `val`, `ref`, `tag`) → `// capability: iso` comment on param
//! - Type params with constraints → Rust generic bounds
//! - Return refinement → `assert!` at end of body

use super::emitter::RustEmitter;
use crate::mvl::backends::rust::emit_types::{
    emit_label, emit_ref_expr_for_assert, emit_ty, is_runtime_checkable,
};
use crate::mvl::backends::rust::last_use::compute_last_uses;
use crate::mvl::backends::rust::mut_analysis::{
    compute_readonly_names, compute_readonly_param_names, compute_unreferenced_binder_spans,
    compute_unused_param_names,
};
use crate::mvl::ir::{
    Capability, Constraint, GenericParam, Literal, TirBlock, TirExprKind, TirFn, TirParam, TirStmt,
    Totality, Ty,
};
use crate::mvl::passes::coverage::BranchKind;

// ── TIR version ───────────────────────────────────────────────────────────

/// Emit a function return type from a resolved `Ty`.
///
/// MVL's `fn(T) -> U` becomes `impl Fn(T) -> U` in return position so
/// functions can return capturing closures (#1313).
fn emit_fn_return_ty(ty: &Ty) -> String {
    match ty {
        Ty::Fn(params, ret, _, _) => {
            let params_str: Vec<String> = params.iter().map(emit_ty).collect();
            format!("impl Fn({}) -> {}", params_str.join(", "), emit_ty(ret))
        }
        _ => emit_ty(ty),
    }
}

/// Return `true` when `block` contains a `select { … }` expression at any depth.
///
/// `select` arm receiver expressions are not emitted by the Rust backend, so
/// parameters referenced only there would produce `unused_variables` warnings.
/// The emitter adds `#[allow(unused_variables)]` on functions that contain a
/// select to suppress those warnings (#1678).
fn body_has_select(block: &TirBlock) -> bool {
    block.stmts.iter().any(stmt_has_select)
}

fn stmt_has_select(stmt: &TirStmt) -> bool {
    match stmt {
        TirStmt::Expr { expr, .. } => expr_has_select(expr),
        TirStmt::Let { init, .. } => expr_has_select(init),
        TirStmt::If {
            cond, then, else_, ..
        } => {
            expr_has_select(cond)
                || body_has_select(then)
                || else_.as_ref().is_some_and(|e| match e {
                    crate::mvl::ir::TirElseBranch::Block(b) => body_has_select(b),
                    crate::mvl::ir::TirElseBranch::If(s) => stmt_has_select(s),
                })
        }
        TirStmt::While { body, .. } | TirStmt::For { body, .. } => body_has_select(body),
        _ => false,
    }
}

fn expr_has_select(expr: &crate::mvl::ir::TirExpr) -> bool {
    matches!(expr.kind, TirExprKind::Select { .. })
}

/// Emit a function-parameter type from a resolved `Ty`.
///
/// MVL's `fn(T) -> U` stays as a bare Rust function pointer in parameter
/// position — enum/struct fields use `fn(T) -> U`, and parameters must match.
fn emit_fn_param_ty(ty: &Ty) -> String {
    match ty {
        Ty::Fn(params, ret, _, _) => {
            let params_str: Vec<String> = params.iter().map(emit_ty).collect();
            format!("fn({}) -> {}", params_str.join(", "), emit_ty(ret))
        }
        _ => emit_ty(ty),
    }
}

impl RustEmitter {
    /// Resolve the Rust function name for `fd`, applying a package prefix when this
    /// function is involved in a cross-package name collision (#1475).
    ///
    /// Returns `fd.name` unchanged when there is no collision.
    fn pkg_fn_rust_name(&self, fd: &TirFn) -> String {
        if fd.pkg_name.is_none() {
            return fd.name.clone();
        }
        let ret_key = emit_ty(&fd.ret_ty);
        self.pkg_fn_dispatch
            .get(&(fd.original_name.clone(), ret_key))
            .cloned()
            .unwrap_or_else(|| fd.name.clone())
    }

    /// Emit a TIR function declaration.
    pub fn emit_fn_decl(&mut self, fd: &TirFn) {
        // Track current function name and test status for coverage metadata.
        self.current_fn = fd.name.clone();
        self.current_fn_is_test = fd.is_test;
        // #1048: inject mvl_join_actors() at the end of fn main() when actors are present.
        self.inject_actor_join = self.has_actors && fd.name == "main";

        let borrows: Vec<Option<bool>> = self
            .capability_params_map
            .get(&fd.name)
            .cloned()
            .unwrap_or_default();

        let mutated_params = collect_mutated_map_params_tir(&fd.body, &self.capability_params_map);
        let readonly_params = compute_readonly_param_names(&fd.body, &fd.params);
        let unused_params =
            compute_unused_param_names(&fd.body, &fd.params, &fd.requires, &fd.ensures);

        if fd.is_test {
            if !fd.effects.is_empty() {
                self.line(&format!(
                    "/// # Effects: {}",
                    fd.effects
                        .iter()
                        .map(|e| e.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                self.line("/// MVL effect annotations — informational in Phase 1.");
            }
            self.line("#[test]");
            let generics = emit_generics_with_tir_params(
                &fd.type_params,
                &fd.constraints,
                &fd.params,
                &fd.ret_ty,
            );
            let params_str = emit_tir_params(
                &fd.params,
                &borrows,
                &mutated_params,
                &readonly_params,
                &unused_params,
            );
            let ret_str = emit_fn_return_ty(&fd.ret_ty);
            self.line(&format!(
                "fn {}{generics}({params_str}) -> {ret_str} {{",
                fd.name
            ));
            self.push_indent();
            self.emit_fn_body_tir(fd);
            self.pop_indent();
            self.line("}");
            return;
        }

        // Doc comments for MVL-specific annotations that Rust cannot express directly
        if let Some(Totality::Total) = &fd.totality {
            self.line("/// # Totality");
            self.line(
                "/// This function is declared `total` in MVL: it must terminate for all inputs.",
            );
        }
        if !fd.effects.is_empty() {
            self.line(&format!(
                "/// # Effects: {}",
                fd.effects
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            self.line("/// MVL effect annotations — informational in Phase 1.");
        }

        // Function signature
        let generics =
            emit_generics_with_tir_params(&fd.type_params, &fd.constraints, &fd.params, &fd.ret_ty);
        let ret_str = emit_fn_return_ty(&fd.ret_ty);

        if let Some(recv_ty) = &fd.receiver_type {
            let is_builtin_type = matches!(
                recv_ty.as_str(),
                "String"
                    | "Int"
                    | "Float"
                    | "Bool"
                    | "Byte"
                    | "UByte"
                    | "UInt"
                    | "List"
                    | "Map"
                    | "Set"
                    | "Option"
                    | "Result"
            );
            if !is_builtin_type {
                let has_self = fd.params.first().is_some_and(|p| p.name == "self");
                let (param_start, self_prefix) = if has_self {
                    let self_borrow = borrows.first().copied().flatten();
                    let prefix = match self_borrow {
                        Some(true) => "&mut self",
                        Some(false) => "&self",
                        None => match fd.params[0].capability {
                            Some(Capability::Val) | Some(Capability::Ref) => "&self",
                            _ => "self",
                        },
                    };
                    (1usize, prefix)
                } else {
                    (0usize, "")
                };
                let rest_params = emit_tir_params(
                    fd.params.get(param_start..).unwrap_or(&[]),
                    borrows.get(param_start..).unwrap_or(&[]),
                    &mutated_params,
                    &readonly_params,
                    &unused_params,
                );
                let params_str = match (self_prefix.is_empty(), rest_params.is_empty()) {
                    (true, true) => String::new(),
                    (true, false) => rest_params,
                    (false, true) => self_prefix.to_string(),
                    (false, false) => format!("{self_prefix}, {rest_params}"),
                };
                self.line(&format!("impl {recv_ty} {{"));
                self.push_indent();
                self.line(&format!(
                    "pub fn {}{generics}({params_str}) -> {ret_str} {{",
                    fd.name
                ));
                self.push_indent();
                if let Some(id) = self.alloc_branch(fd.span.line, BranchKind::FnEntry) {
                    self.emit_cov_hit(id);
                }
                for req_pred in fd.requires.iter().filter(|p| is_runtime_checkable(p)) {
                    let pred_str = emit_ref_expr_for_assert(req_pred, "self");
                    let msg = pred_str.replace('{', "{{").replace('}', "}}");
                    self.line(&format!("assert!({pred_str}, \"requires: {msg}\");"));
                }
                self.emit_fn_body_tir(fd);
                self.pop_indent();
                self.line("}");
                self.pop_indent();
                self.line("}");
                return;
            }
        }

        let has_self_param = fd.receiver_type.is_some();
        if has_self_param {
            self.self_as_free_param = true;
        }

        let params_str = emit_tir_params(
            &fd.params,
            &borrows,
            &mutated_params,
            &readonly_params,
            &unused_params,
        );
        let rust_name = self.pkg_fn_rust_name(fd);
        // select arm receiver expressions are dropped in Rust emission; params
        // referenced only there would produce unused_variables warnings (#1678).
        if body_has_select(&fd.body) {
            self.line("#[allow(unused_variables)]");
        }
        self.line(&format!(
            "pub fn {}{generics}({params_str}) -> {ret_str} {{",
            rust_name
        ));
        self.push_indent();
        if let Some(id) = self.alloc_branch(fd.span.line, BranchKind::FnEntry) {
            self.emit_cov_hit(id);
        }
        for req_pred in fd.requires.iter().filter(|p| is_runtime_checkable(p)) {
            let pred_str = emit_ref_expr_for_assert(req_pred, "self");
            let msg = pred_str.replace('{', "{{").replace('}', "}}");
            self.line(&format!("assert!({pred_str}, \"requires: {msg}\");"));
        }
        self.emit_fn_body_tir(fd);
        self.pop_indent();
        self.line("}");

        if has_self_param {
            self.self_as_free_param = false;
        }
    }

    /// Emit the statements and return-refinement check for a TIR function body.
    fn emit_fn_body_tir(&mut self, fd: &TirFn) {
        self.last_uses = compute_last_uses(&fd.body);
        self.readonly_names = compute_readonly_names(&fd.body, &self.capability_params_map);
        let (unreferenced_let, unreferenced_arm) =
            compute_unreferenced_binder_spans(&fd.body, &fd.params);
        self.unreferenced_let_spans = unreferenced_let;
        self.unreferenced_arm_spans = unreferenced_arm;

        self.capability_param_names.clear();
        let borrows = self
            .capability_params_map
            .get(&fd.name)
            .cloned()
            .unwrap_or_default();
        for (i, param) in fd.params.iter().enumerate() {
            if borrows.get(i).copied().flatten().is_some() {
                self.capability_param_names.insert(param.name.clone());
            }
        }

        // #960: for HOF params (fn-typed parameters), temporarily insert their inner
        // parameter borrow flags into capability_params_map.
        // #1467: also resolve named fn-type aliases (`type Dispatcher = fn(val T) -> U`)
        // so the HOF param's inner signature is visible through the alias.
        let mut hof_param_entries: Vec<(String, Option<Vec<Option<bool>>>)> = Vec::new();
        for param in &fd.params {
            let resolved = self.resolve_fn_alias(&param.ty).unwrap_or(&param.ty);
            if let Ty::Fn(fn_params, _, _, _) = resolved {
                let flags: Vec<Option<bool>> = fn_params
                    .iter()
                    .map(|p| {
                        if let Ty::Ref(mutable, _) = p {
                            Some(*mutable)
                        } else {
                            None
                        }
                    })
                    .collect();
                if flags.iter().any(|b| b.is_some()) {
                    let previous = self.capability_params_map.insert(param.name.clone(), flags);
                    hof_param_entries.push((param.name.clone(), previous));
                }
            }
        }

        let needs_actor_scope = self.inject_actor_join;
        if needs_actor_scope {
            self.inject_actor_join = false;
            self.indent();
            self.push("{");
            self.nl();
            self.push_indent();
        }

        let stmts = &fd.body.stmts;
        if stmts.is_empty() {
            let is_unit = matches!(fd.ret_ty, Ty::Unit);
            if !is_unit {
                unreachable!("non-Unit function with empty body — blocked by checker (#990)");
            }
        } else {
            let (head, tail) = stmts.split_at(stmts.len() - 1);

            // MVL has no `break` keyword, so a `while true { ... }` loop is
            // unconditionally divergent — the only ways out are `return` or
            // panic.  When the immediately preceding statement is such a loop
            // and the tail is a value expression, that tail is unreachable.
            // Drop it: the loop emits as Rust `loop { ... }` which has type `!`
            // and satisfies any return type without a fallback expression.
            // This is the canonical MVL idiom (CLAUDE.md "while true + return")
            // and Rust's `unreachable_code` lint would otherwise warn on every
            // such function.
            let tail_is_unreachable = matches!(
                (head.last(), &tail[0]),
                (
                    Some(TirStmt::While { cond, .. }),
                    TirStmt::Expr { .. },
                ) if matches!(&cond.kind, TirExprKind::Literal(Literal::Bool(true)))
            );

            if tail_is_unreachable {
                // Emit only `head` — the `while true { ... }` loop becomes
                // Rust `loop { ... }` (type `!`) and is the function's tail.
                self.emit_block_stmts(head);
            } else {
                self.emit_block_stmts(head);

                let last = &tail[0];
                let is_unit = matches!(fd.ret_ty, Ty::Unit);
                let has_ensures = fd.ensures.iter().any(is_runtime_checkable) && !is_unit;
                match last {
                    TirStmt::Expr { expr, .. } => {
                        if !self.emit_mcdc_return_expr(expr, &fd.ret_ty, expr.span.line) {
                            if has_ensures {
                                self.indent();
                                self.push("let _result = ");
                                self.emit_expr_tail_with_return_type_tir(
                                    expr, &fd.ret_ty, &fd.params,
                                );
                                self.push(";");
                                self.nl();
                                for ens_pred in
                                    fd.ensures.iter().filter(|p| is_runtime_checkable(p))
                                {
                                    let pred_str = emit_ref_expr_for_assert(ens_pred, "_result");
                                    let msg = pred_str.replace('{', "{{").replace('}', "}}");
                                    self.line(&format!("assert!({pred_str}, \"ensures: {msg}\");"));
                                }
                                self.line("_result");
                            } else {
                                self.indent();
                                self.emit_expr_tail_with_return_type_tir(
                                    expr, &fd.ret_ty, &fd.params,
                                );
                                self.nl();
                            }
                        }
                    }
                    other => {
                        self.emit_block_stmts(std::slice::from_ref(other));
                    }
                }
            }
        }

        if needs_actor_scope {
            self.pop_indent();
            self.indent();
            self.push("}");
            self.nl();
            self.indent();
            self.push("mvl_join_actors()");
            self.nl();
        }

        if let Some(pred) = &fd.return_refinement {
            let pred_str = emit_ref_expr_for_assert(pred, "_return_val");
            self.line(&format!(
                "// return refinement: assert!({pred_str}) — checked by MVL type checker"
            ));
        }

        for (name, previous) in hof_param_entries {
            match previous {
                Some(v) => {
                    self.capability_params_map.insert(name, v);
                }
                None => {
                    self.capability_params_map.remove(&name);
                }
            }
        }
    }
}

/// Emit TIR function parameters.
///
/// - `readonly_params` — parameter names the body-analysis proved are
///   never mutated.  Suppress the `mut` prefix that `ref`/`iso`
///   capabilities would otherwise force (#1654).
/// - `unused_params` — parameter names never referenced in the body,
///   requires, or ensures.  Prefix the emitted name with `_` (Rust's
///   idiom for "documented but unused") so `rustc` doesn't warn (#1658).
fn emit_tir_params(
    params: &[TirParam],
    borrows: &[Option<bool>],
    mutated_params: &std::collections::HashSet<String>,
    readonly_params: &std::collections::HashSet<String>,
    unused_params: &std::collections::HashSet<String>,
) -> String {
    params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let ty_str = match borrows.get(i).copied().flatten() {
                Some(mutable) if !matches!(p.ty, Ty::Ref(..)) => {
                    if mutable {
                        format!("&mut {}", emit_fn_param_ty(&p.ty))
                    } else {
                        format!("&{}", emit_fn_param_ty(&p.ty))
                    }
                }
                _ => emit_fn_param_ty(&p.ty),
            };
            let cap_comment = match &p.capability {
                Some(Capability::Iso) => "/* iso */ ",
                Some(Capability::Val) => "/* val */ ",
                Some(Capability::Ref) => "/* ref */ ",
                Some(Capability::Tag) => "/* tag */ ",
                None => "",
            };
            let needs_mut_for_body =
                borrows.get(i).copied().flatten().is_none() && mutated_params.contains(&p.name);
            let has_ref_cap = matches!(p.capability, Some(Capability::Ref) | Some(Capability::Iso));
            let is_readonly = readonly_params.contains(&p.name);
            let mut_prefix = if (has_ref_cap && !is_readonly) || needs_mut_for_body {
                "mut "
            } else {
                ""
            };
            let base_name = if p.name == "self" {
                "self_"
            } else {
                p.name.as_str()
            };
            // Rust idiom: prefix unused parameters with `_` so `rustc`
            // treats them as "documented but unused".  `self` is
            // already remapped to `self_`; extend to `_self_` when it
            // happens to be unused.
            let owned_underscored: String;
            let param_name = if unused_params.contains(&p.name) && !base_name.starts_with('_') {
                owned_underscored = format!("_{base_name}");
                owned_underscored.as_str()
            } else {
                base_name
            };
            format!("{cap_comment}{mut_prefix}{param_name}: {ty_str}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Emit generics with TirParam-based bounds scanning.
///
/// MVL does not admit user-declared trait bounds on fn signatures (ADR-0053),
/// so this pass derives every Rust bound the emit will actually rely on from
/// the signature itself:
///
/// - `Map<K, V>` positions → `K: Hash + Eq + Clone`, `V: Clone`
/// - `Set<T>` positions   → `T: Hash + Eq + Clone`
/// - `List<T>` positions  → `T: Clone`
/// - every generic type parameter used anywhere in `params` or `ret_ty` → `Clone`
///
/// The blanket `Clone` for every generic reflects the emit's argument-position
/// pattern (`arg.clone().into()`) which requires it for value-semantic MVL
/// calls.  It over-approximates — some fns don't actually need `Clone` — but
/// it stays inside the Rust backend, is invisible to MVL source, and matches
/// exactly what the emit outputs.  The user never sees these bounds.
fn emit_generics_with_tir_params(
    type_params: &[GenericParam],
    constraints: &[Constraint],
    params: &[TirParam],
    ret_ty: &Ty,
) -> String {
    if type_params.is_empty() {
        return String::new();
    }
    let mut bounds: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for c in constraints {
        bounds
            .entry(c.name.clone())
            .or_default()
            .push(c.bound.clone());
    }
    collect_map_set_bounds_tir(params, &mut bounds);
    collect_ty_bounds(ret_ty, &mut bounds);
    // Blanket `Clone` for every generic that appears in the signature.
    // The emit's value-arg pattern `expr.clone().into()` requires this
    // uniformly; deriving it here means MVL source never needs (and now
    // cannot express) it — see ADR-0053.
    let referenced_generics = generics_used_in_sig(type_params, params, ret_ty);
    for name in referenced_generics {
        add_bound_by_name(&name, "Clone", &mut bounds);
    }

    let params_out: Vec<String> = type_params
        .iter()
        .map(|p| match p {
            GenericParam::Const(name, _ty) => format!("const {name}: usize"),
            GenericParam::Type(name) => {
                let bs = bounds.get(name.as_str()).cloned().unwrap_or_default();
                if bs.is_empty() {
                    name.clone()
                } else {
                    format!("{name}: {}", bs.join(" + "))
                }
            }
        })
        .collect();
    format!("<{}>", params_out.join(", "))
}

fn collect_map_set_bounds_tir(
    params: &[TirParam],
    bounds: &mut std::collections::HashMap<String, Vec<String>>,
) {
    for p in params {
        collect_ty_bounds(&p.ty, bounds);
    }
}

fn collect_ty_bounds(ty: &Ty, bounds: &mut std::collections::HashMap<String, Vec<String>>) {
    match ty {
        Ty::Map(k, v) => {
            add_ty_bound_if_named(k, "std::hash::Hash", bounds);
            add_ty_bound_if_named(k, "std::cmp::Eq", bounds);
            add_ty_bound_if_named(k, "Clone", bounds);
            add_ty_bound_if_named(v, "Clone", bounds);
            collect_ty_bounds(k, bounds);
            collect_ty_bounds(v, bounds);
        }
        Ty::Set(t) => {
            add_ty_bound_if_named(t, "std::hash::Hash", bounds);
            add_ty_bound_if_named(t, "std::cmp::Eq", bounds);
            add_ty_bound_if_named(t, "Clone", bounds);
        }
        // List<T>: the emit's higher-order-method inlining
        // (`xs.map(f)` → `xs.into_iter().map(|__x| f(__x.clone())).collect()`)
        // requires `T: Clone`.  MVL user code cannot declare this bound (ADR-0053
        // — no trait bounds in MVL grammar); the Rust backend derives it from
        // the signature so the leak stays inside the emit and out of MVL source.
        Ty::List(t) => {
            add_ty_bound_if_named(t, "Clone", bounds);
            collect_ty_bounds(t, bounds);
        }
        Ty::Named(_, args) => {
            for a in args {
                collect_ty_bounds(a, bounds);
            }
        }
        Ty::Ref(_, inner) => collect_ty_bounds(inner, bounds),
        _ => {}
    }
}

fn add_ty_bound_if_named(
    ty: &Ty,
    bound: &str,
    bounds: &mut std::collections::HashMap<String, Vec<String>>,
) {
    if let Ty::Named(name, args) = ty {
        if args.is_empty() && !is_concrete_ty(name) {
            add_bound_by_name(name, bound, bounds);
        }
    }
}

fn add_bound_by_name(
    name: &str,
    bound: &str,
    bounds: &mut std::collections::HashMap<String, Vec<String>>,
) {
    if is_concrete_ty(name) {
        return;
    }
    let entry = bounds.entry(name.to_string()).or_default();
    if !entry.iter().any(|b| b == bound) {
        entry.push(bound.to_string());
    }
}

/// Return the set of generic type-parameter names actually referenced by
/// this fn's params or return type.  A signature can declare `[T, U]` but
/// use only `T` — no bound should be emitted for the unused `U`.
fn generics_used_in_sig(
    type_params: &[GenericParam],
    params: &[TirParam],
    ret_ty: &Ty,
) -> std::collections::HashSet<String> {
    let declared: std::collections::HashSet<String> = type_params
        .iter()
        .filter_map(|p| match p {
            GenericParam::Type(n) => Some(n.clone()),
            GenericParam::Const(..) => None,
        })
        .collect();
    let mut used = std::collections::HashSet::new();
    for p in params {
        walk_ty_generics(&p.ty, &declared, &mut used);
    }
    walk_ty_generics(ret_ty, &declared, &mut used);
    used
}

fn walk_ty_generics(
    ty: &Ty,
    declared: &std::collections::HashSet<String>,
    used: &mut std::collections::HashSet<String>,
) {
    match ty {
        Ty::Named(name, args) => {
            if args.is_empty() && declared.contains(name) {
                used.insert(name.clone());
            }
            for a in args {
                walk_ty_generics(a, declared, used);
            }
        }
        Ty::List(t) | Ty::Set(t) | Ty::Option(t) | Ty::Ref(_, t) | Ty::Ptr(t) => {
            walk_ty_generics(t, declared, used);
        }
        Ty::Map(k, v) | Ty::Result(k, v) => {
            walk_ty_generics(k, declared, used);
            walk_ty_generics(v, declared, used);
        }
        Ty::Fn(fn_params, ret, _, _) => {
            for p in fn_params {
                walk_ty_generics(p, declared, used);
            }
            walk_ty_generics(ret, declared, used);
        }
        Ty::Array(t, _) => walk_ty_generics(t, declared, used),
        Ty::Labeled(_, inner) | Ty::Refined(inner, _) => walk_ty_generics(inner, declared, used),
        _ => {}
    }
}

fn is_concrete_ty(name: &str) -> bool {
    matches!(
        name,
        "Int"
            | "Float"
            | "Bool"
            | "String"
            | "Char"
            | "Byte"
            | "Unit"
            | "Never"
            | "List"
            | "Map"
            | "Set"
            | "Option"
            | "Result"
    )
}

/// Collect names of Map/Set parameters mutated in the TIR body.
fn collect_mutated_map_params_tir(
    body: &TirBlock,
    cap_map: &std::collections::HashMap<String, Vec<Option<bool>>>,
) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    scan_tir_stmts_for_mut_calls(&body.stmts, &mut out, cap_map);
    out
}

fn scan_tir_stmts_for_mut_calls(
    stmts: &[TirStmt],
    out: &mut std::collections::HashSet<String>,
    cap_map: &std::collections::HashMap<String, Vec<Option<bool>>>,
) {
    for stmt in stmts {
        match stmt {
            TirStmt::Expr { expr, .. }
            | TirStmt::Return {
                value: Some(expr), ..
            } => {
                scan_tir_expr_for_mut_calls(expr, out, cap_map);
            }
            TirStmt::Let { init, .. } => scan_tir_expr_for_mut_calls(init, out, cap_map),
            TirStmt::Assign { value, .. } => scan_tir_expr_for_mut_calls(value, out, cap_map),
            TirStmt::If {
                cond, then, else_, ..
            } => {
                scan_tir_expr_for_mut_calls(cond, out, cap_map);
                scan_tir_stmts_for_mut_calls(&then.stmts, out, cap_map);
                if let Some(crate::mvl::ir::TirElseBranch::Block(b)) = else_ {
                    scan_tir_stmts_for_mut_calls(&b.stmts, out, cap_map);
                }
            }
            TirStmt::For { body, iter, .. } => {
                scan_tir_expr_for_mut_calls(iter, out, cap_map);
                scan_tir_stmts_for_mut_calls(&body.stmts, out, cap_map);
            }
            TirStmt::While { cond, body, .. } => {
                scan_tir_expr_for_mut_calls(cond, out, cap_map);
                scan_tir_stmts_for_mut_calls(&body.stmts, out, cap_map);
            }
            TirStmt::Match {
                scrutinee, arms, ..
            } => {
                scan_tir_expr_for_mut_calls(scrutinee, out, cap_map);
                for arm in arms {
                    match &arm.body {
                        crate::mvl::ir::TirMatchBody::Expr(e) => {
                            scan_tir_expr_for_mut_calls(e, out, cap_map);
                        }
                        crate::mvl::ir::TirMatchBody::Block(b) => {
                            scan_tir_stmts_for_mut_calls(&b.stmts, out, cap_map);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn scan_tir_expr_for_mut_calls(
    expr: &crate::mvl::ir::TirExpr,
    out: &mut std::collections::HashSet<String>,
    cap_map: &std::collections::HashMap<String, Vec<Option<bool>>>,
) {
    match &expr.kind {
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } => {
            if matches!(method.as_str(), "insert" | "remove" | "retain") {
                if let TirExprKind::Var(name) = &receiver.kind {
                    out.insert(name.clone());
                }
            }
            scan_tir_expr_for_mut_calls(receiver, out, cap_map);
            for a in args {
                scan_tir_expr_for_mut_calls(a, out, cap_map);
            }
        }
        TirExprKind::FnCall { name, args, .. } => {
            if let Some(borrows) = cap_map.get(name.as_str()) {
                for (i, arg) in args.iter().enumerate() {
                    if let Some(Some(true)) = borrows.get(i) {
                        if let TirExprKind::Var(param_name) = &arg.kind {
                            out.insert(param_name.clone());
                        }
                    }
                }
            }
            for a in args {
                scan_tir_expr_for_mut_calls(a, out, cap_map);
            }
        }
        _ => {}
    }
}

// ── Tail expression emitter (TIR) ─────────────────────────────────────────

impl RustEmitter {
    /// Emit an expression as the tail (implicit return) of a TIR function body.
    fn emit_expr_tail_with_return_type_tir(
        &mut self,
        expr: &crate::mvl::ir::TirExpr,
        ret_ty: &Ty,
        params: &[TirParam],
    ) {
        if matches!(ret_ty, Ty::Unit) {
            self.emit_expr(expr);
            self.push(";");
            return;
        }
        match ret_ty {
            Ty::Labeled(label, ..) if is_raw_value_tir(expr, params) => {
                let label_name = emit_label(label.as_str());
                self.push(&format!("{label_name}("));
                self.emit_expr(expr);
                self.push(")");
                return;
            }
            Ty::Labeled(..)
                if matches!(
                    &expr.kind,
                    TirExprKind::FnCall { .. } | TirExprKind::MethodCall { .. }
                ) && expr.ty != *ret_ty =>
            {
                // Coerce the call's raw result into the labeled return type
                // via `.into()`.  Skip the coercion when the call's static
                // result type already equals `ret_ty` — the `.into()` is a
                // no-op there AND, for generic callees, blocks Rust from
                // inferring the type parameter (#1707 phase 11).
                self.emit_expr(expr);
                self.push(".into()");
                return;
            }
            Ty::Result(ok, _) => {
                if let Ty::Labeled(label, _) = ok.as_ref() {
                    if let TirExprKind::FnCall { name, args, .. } = &expr.kind {
                        if name == "Ok" && args.len() == 1 && is_raw_value_tir(&args[0], params) {
                            let label_name = emit_label(label.as_str());
                            self.push("Ok(");
                            self.push(&format!("{label_name}("));
                            self.emit_expr(&args[0]);
                            self.push("))");
                            return;
                        }
                    }
                }
            }
            _ => {}
        }
        // Refined alias wrapping: return base type where refined alias expected (#1326)
        if let Some(_base) = self.refined_alias_base(ret_ty) {
            if self.refined_alias_base(&expr.ty).is_none() {
                if let Ty::Named(name, _) = ret_ty {
                    self.push(&format!("{}::new(", name));
                    self.emit_expr(expr);
                    self.push(")");
                    return;
                }
            }
        }
        // Refined alias unwrapping: return refined alias where base type expected (#1326)
        if self.refined_alias_base(&expr.ty).is_some() && self.refined_alias_base(ret_ty).is_none()
        {
            self.emit_expr(expr);
            self.push(".0");
            return;
        }
        // Lambda return from function: wrap in move so closure owns captures (#1313)
        if matches!(ret_ty, Ty::Fn(..)) && matches!(&expr.kind, TirExprKind::Lambda { .. }) {
            self.push("move ");
        }
        self.emit_expr(expr);
    }
}

fn is_raw_value_tir(expr: &crate::mvl::ir::TirExpr, params: &[TirParam]) -> bool {
    match &expr.kind {
        TirExprKind::Literal(_) => true,
        TirExprKind::Var(name) => params
            .iter()
            .any(|p| &p.name == name && !is_labeled_ty(&p.ty)),
        _ => false,
    }
}

fn is_labeled_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::Labeled(..))
}
