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

use crate::mvl::backends::rust::emit_exprs::{emit_block_stmts, emit_expr};
use crate::mvl::backends::rust::emit_stmts::emit_mcdc_return_expr;
use crate::mvl::backends::rust::emit_types::{
    emit_label, emit_ref_expr_for_assert, emit_ty, emit_type_expr,
};
use crate::mvl::backends::rust::emitter::RustEmitter;
use crate::mvl::backends::rust::last_use::{compute_last_uses, compute_last_uses_ast};
use crate::mvl::ir::{
    Capability, Constraint, GenericParam, TirBlock, TirExprKind, TirFn, TirParam, TirStmt,
    Totality, Ty,
};
use crate::mvl::parser::ast::{
    expr_to_ref_expr_ext, Block, Capability as AstCapability, Constraint as AstConstraint, Expr,
    FnDecl, GenericParam as AstGenericParam, Param, Stmt, Totality as AstTotality, TypeExpr,
};
use crate::mvl::parser::lexer::Span;
use crate::mvl::passes::coverage::BranchKind;

// ── TIR version ───────────────────────────────────────────────────────────

/// Emit a function-parameter type from a resolved `Ty`.
///
/// MVL's `fn(T) -> U` is a bare Rust function pointer.  When used as a
/// function *parameter* the caller may pass a closure, which is not a bare
/// `fn` pointer.  Emitting `fn(T) -> U` accepts both in practice for the
/// transpiler's use cases.
fn emit_fn_param_ty(ty: &Ty) -> String {
    match ty {
        Ty::Fn(params, ret, _, _) => {
            let params_str: Vec<String> = params.iter().map(emit_ty).collect();
            format!("fn({}) -> {}", params_str.join(", "), emit_ty(ret))
        }
        _ => emit_ty(ty),
    }
}

/// Emit a TIR function declaration.
pub fn emit_fn_decl(cg: &mut RustEmitter, fd: &TirFn) {
    // Track current function name and test status for coverage metadata.
    cg.current_fn = fd.name.clone();
    cg.current_fn_is_test = fd.is_test;
    // #1048: inject mvl_join_actors() at the end of fn main() when actors are present.
    cg.inject_actor_join = cg.has_actors && fd.name == "main";

    let borrows: Vec<Option<bool>> = cg
        .capability_params_map
        .get(&fd.name)
        .cloned()
        .unwrap_or_default();

    let mutated_params = collect_mutated_map_params_tir(&fd.body, &cg.capability_params_map);

    if fd.is_test {
        cg.line("#[test]");
        let generics =
            emit_generics_with_tir_params(&fd.type_params, &fd.constraints, &fd.params, &fd.ret_ty);
        let params_str = emit_tir_params(&fd.params, &borrows, &mutated_params);
        let ret_str = emit_ty(&fd.ret_ty);
        cg.line(&format!(
            "fn {}{generics}({params_str}) -> {ret_str} {{",
            fd.name
        ));
        cg.push_indent();
        emit_fn_body_tir(cg, fd);
        cg.pop_indent();
        cg.line("}");
        return;
    }

    // Doc comments for MVL-specific annotations that Rust cannot express directly
    if let Some(Totality::Total) = &fd.totality {
        cg.line("/// # Totality");
        cg.line("/// This function is declared `total` in MVL: it must terminate for all inputs.");
    }
    if !fd.effects.is_empty() {
        cg.line(&format!(
            "/// # Effects: {}",
            fd.effects
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        cg.line("/// MVL effect annotations — informational in Phase 1.");
    }

    // Function signature
    let generics =
        emit_generics_with_tir_params(&fd.type_params, &fd.constraints, &fd.params, &fd.ret_ty);
    let ret_str = emit_ty(&fd.ret_ty);

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
            let (param_start, self_prefix) = if has_self { (1usize, "&self") } else { (0, "") };
            let rest_params = emit_tir_params(
                fd.params.get(param_start..).unwrap_or(&[]),
                borrows.get(param_start..).unwrap_or(&[]),
                &mutated_params,
            );
            let params_str = match (self_prefix.is_empty(), rest_params.is_empty()) {
                (true, true) => String::new(),
                (true, false) => rest_params,
                (false, true) => self_prefix.to_string(),
                (false, false) => format!("{self_prefix}, {rest_params}"),
            };
            cg.line(&format!("impl {recv_ty} {{"));
            cg.push_indent();
            cg.line(&format!(
                "pub fn {}{generics}({params_str}) -> {ret_str} {{",
                fd.name
            ));
            cg.push_indent();
            if let Some(id) = cg.alloc_branch(fd.span.line, BranchKind::FnEntry) {
                cg.emit_cov_hit(id);
            }
            for req_pred in &fd.requires {
                let pred_str = emit_ref_expr_for_assert(req_pred, "self");
                let msg = pred_str.replace('{', "{{").replace('}', "}}");
                cg.line(&format!("assert!({pred_str}, \"requires: {msg}\");"));
            }
            emit_fn_body_tir(cg, fd);
            cg.pop_indent();
            cg.line("}");
            cg.pop_indent();
            cg.line("}");
            return;
        }
    }

    let has_self_param = fd.receiver_type.is_some();
    if has_self_param {
        cg.self_as_free_param = true;
    }

    let params_str = emit_tir_params(&fd.params, &borrows, &mutated_params);
    cg.line(&format!(
        "pub fn {}{generics}({params_str}) -> {ret_str} {{",
        fd.name
    ));
    cg.push_indent();
    if let Some(id) = cg.alloc_branch(fd.span.line, BranchKind::FnEntry) {
        cg.emit_cov_hit(id);
    }
    for req_pred in &fd.requires {
        let pred_str = emit_ref_expr_for_assert(req_pred, "self");
        let msg = pred_str.replace('{', "{{").replace('}', "}}");
        cg.line(&format!("assert!({pred_str}, \"requires: {msg}\");"));
    }
    emit_fn_body_tir(cg, fd);
    cg.pop_indent();
    cg.line("}");

    if has_self_param {
        cg.self_as_free_param = false;
    }
}

/// Emit the statements and return-refinement check for a TIR function body.
fn emit_fn_body_tir(cg: &mut RustEmitter, fd: &TirFn) {
    cg.last_uses = compute_last_uses(&fd.body);

    cg.capability_param_names.clear();
    let borrows = cg
        .capability_params_map
        .get(&fd.name)
        .cloned()
        .unwrap_or_default();
    for (i, param) in fd.params.iter().enumerate() {
        if borrows.get(i).copied().flatten().is_some() {
            cg.capability_param_names.insert(param.name.clone());
        }
    }

    // #960: for HOF params (fn-typed parameters), temporarily insert their inner
    // parameter borrow flags into capability_params_map.
    let mut hof_param_entries: Vec<(String, Option<Vec<Option<bool>>>)> = Vec::new();
    for param in &fd.params {
        if let Ty::Fn(fn_params, _, _, _) = &param.ty {
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
                let previous = cg.capability_params_map.insert(param.name.clone(), flags);
                hof_param_entries.push((param.name.clone(), previous));
            }
        }
    }

    let needs_actor_scope = cg.inject_actor_join;
    if needs_actor_scope {
        cg.inject_actor_join = false;
        cg.indent();
        cg.push("{");
        cg.nl();
        cg.push_indent();
    }

    let stmts = &fd.body.stmts;
    if stmts.is_empty() {
        let is_unit = matches!(fd.ret_ty, Ty::Unit);
        if !is_unit {
            unreachable!("non-Unit function with empty body — blocked by checker (#990)");
        }
    } else {
        let (head, tail) = stmts.split_at(stmts.len() - 1);
        emit_block_stmts(cg, head);

        let last = &tail[0];
        let is_unit = matches!(fd.ret_ty, Ty::Unit);
        let has_ensures = !fd.ensures.is_empty() && !is_unit;
        match last {
            TirStmt::Expr { expr, .. } => {
                if !emit_mcdc_return_expr(cg, expr, &fd.ret_ty, expr.span.line) {
                    if has_ensures {
                        cg.indent();
                        cg.push("let _result = ");
                        emit_expr_tail_with_return_type_tir(cg, expr, &fd.ret_ty, &fd.params);
                        cg.push(";");
                        cg.nl();
                        for ens_pred in &fd.ensures {
                            let pred_str = emit_ref_expr_for_assert(ens_pred, "_result");
                            let msg = pred_str.replace('{', "{{").replace('}', "}}");
                            cg.line(&format!("assert!({pred_str}, \"ensures: {msg}\");"));
                        }
                        cg.line("_result");
                    } else {
                        cg.indent();
                        emit_expr_tail_with_return_type_tir(cg, expr, &fd.ret_ty, &fd.params);
                        cg.nl();
                    }
                }
            }
            other => {
                emit_block_stmts(cg, std::slice::from_ref(other));
            }
        }
    }

    if needs_actor_scope {
        cg.pop_indent();
        cg.indent();
        cg.push("}");
        cg.nl();
        cg.indent();
        cg.push("mvl_join_actors()");
        cg.nl();
    }

    if let Some(pred) = &fd.return_refinement {
        let pred_str = emit_ref_expr_for_assert(pred, "_return_val");
        cg.line(&format!(
            "// return refinement: assert!({pred_str}) — checked by MVL type checker"
        ));
    }

    for (name, previous) in hof_param_entries {
        match previous {
            Some(v) => {
                cg.capability_params_map.insert(name, v);
            }
            None => {
                cg.capability_params_map.remove(&name);
            }
        }
    }
}

/// Emit TIR function parameters.
fn emit_tir_params(
    params: &[TirParam],
    borrows: &[Option<bool>],
    mutated_params: &std::collections::HashSet<String>,
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
            let mut_prefix = if has_ref_cap || needs_mut_for_body {
                "mut "
            } else {
                ""
            };
            let param_name = if p.name == "self" {
                "self_"
            } else {
                p.name.as_str()
            };
            format!("{cap_comment}{mut_prefix}{param_name}: {ty_str}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Emit generics with TirParam-based bounds scanning.
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
            let entry = bounds.entry(name.clone()).or_default();
            if !entry.iter().any(|b| b == bound) {
                entry.push(bound.to_string());
            }
        }
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

/// Emit an expression as the tail (implicit return) of a TIR function body.
fn emit_expr_tail_with_return_type_tir(
    cg: &mut RustEmitter,
    expr: &crate::mvl::ir::TirExpr,
    ret_ty: &Ty,
    params: &[TirParam],
) {
    if matches!(ret_ty, Ty::Unit) {
        emit_expr(cg, expr);
        cg.push(";");
        return;
    }
    match ret_ty {
        Ty::Labeled(label, ..) if is_raw_value_tir(expr, params) => {
            let label_name = emit_label(label.as_str());
            cg.push(&format!("{label_name}("));
            emit_expr(cg, expr);
            cg.push(")");
            return;
        }
        Ty::Labeled(..)
            if matches!(
                &expr.kind,
                TirExprKind::FnCall { .. } | TirExprKind::MethodCall { .. }
            ) =>
        {
            emit_expr(cg, expr);
            cg.push(".into()");
            return;
        }
        Ty::Result(ok, _) => {
            if let Ty::Labeled(label, _) = ok.as_ref() {
                if let TirExprKind::FnCall { name, args, .. } = &expr.kind {
                    if name == "Ok" && args.len() == 1 && is_raw_value_tir(&args[0], params) {
                        let label_name = emit_label(label.as_str());
                        cg.push("Ok(");
                        cg.push(&format!("{label_name}("));
                        emit_expr(cg, &args[0]);
                        cg.push("))");
                        return;
                    }
                }
            }
        }
        _ => {}
    }
    emit_expr(cg, expr);
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

// ── AST version (for prelude/stdlib) ─────────────────────────────────────

/// Emit a function-parameter type from an AST TypeExpr (used for prelude).
fn emit_fn_param_type(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Fn { params, ret, .. } => {
            let params_str: Vec<String> = params.iter().map(emit_type_expr).collect();
            format!("fn({}) -> {}", params_str.join(", "), emit_type_expr(ret))
        }
        _ => emit_type_expr(ty),
    }
}

/// Emit an AST function declaration (used for prelude and stdlib functions).
pub fn emit_fn_decl_ast(cg: &mut RustEmitter, fd: &FnDecl) {
    // Track current function name and test status for coverage metadata.
    cg.current_fn = fd.name.clone();
    cg.current_fn_is_test = fd.is_test;
    // #1048: inject mvl_join_actors() at the end of fn main() when actors are present.
    cg.inject_actor_join = cg.has_actors && fd.name == "main";

    // Test functions are emitted inside a #[cfg(test)] mod tests block.
    // The caller (codegen) is responsible for grouping them; here we just
    // emit the #[test] attribute and a non-pub signature.
    // Retrieve borrow flags for this function (Phase B, Spec 009 Req 2).
    let borrows: Vec<Option<bool>> = cg
        .capability_params_map
        .get(&fd.name)
        .cloned()
        .unwrap_or_default();

    let mutated_params = collect_mutated_map_params(&fd.body, &cg.capability_params_map);

    if fd.is_test {
        cg.line("#[test]");
        let generics = emit_generics_with_params(
            &fd.type_params,
            &fd.constraints,
            &fd.params,
            &fd.return_type,
        );
        let params_str = emit_params(&fd.params, &borrows, &mutated_params);
        let ret_str = emit_type_expr(&fd.return_type);
        cg.line(&format!(
            "fn {}{generics}({params_str}) -> {ret_str} {{",
            fd.name
        ));
        cg.push_indent();
        emit_fn_body(cg, fd);
        cg.pop_indent();
        cg.line("}");
        return;
    }

    // Doc comments for MVL-specific annotations that Rust cannot express directly
    if let Some(AstTotality::Total) = &fd.totality {
        cg.line("/// # Totality");
        cg.line("/// This function is declared `total` in MVL: it must terminate for all inputs.");
    }
    if !fd.effects.is_empty() {
        cg.line(&format!(
            "/// # Effects: {}",
            fd.effects
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        cg.line("/// MVL effect annotations — informational in Phase 1.");
    }

    // Function signature
    let generics = emit_generics_with_params(
        &fd.type_params,
        &fd.constraints,
        &fd.params,
        &fd.return_type,
    );
    let ret_str = emit_type_expr(&fd.return_type);

    if let Some(recv_ty) = &fd.receiver_type {
        // #928: Built-in types (String, List, Map, etc.) cannot have `impl` blocks
        // in Rust because they are defined outside the crate.  Emit their extension
        // methods as free functions so the UFCS dispatch table can call them.
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
            // Type-attached method (#868): emit inside `impl ReceiverType { … }`.
            let has_self = fd.params.first().is_some_and(|p| p.name == "self");
            let (param_start, self_prefix) = if has_self { (1usize, "&self") } else { (0, "") };
            let rest_params = emit_params(
                fd.params.get(param_start..).unwrap_or(&[]),
                borrows.get(param_start..).unwrap_or(&[]),
                &mutated_params,
            );
            let params_str = match (self_prefix.is_empty(), rest_params.is_empty()) {
                (true, true) => String::new(),
                (true, false) => rest_params,
                (false, true) => self_prefix.to_string(),
                (false, false) => format!("{self_prefix}, {rest_params}"),
            };
            cg.line(&format!("impl {recv_ty} {{"));
            cg.push_indent();
            cg.line(&format!(
                "pub fn {}{generics}({params_str}) -> {ret_str} {{",
                fd.name
            ));
            cg.push_indent();
            // Fn-entry probe
            if let Some(id) = cg.alloc_branch(fd.span.line, BranchKind::FnEntry) {
                cg.emit_cov_hit(id);
            }
            for req_expr in &fd.requires {
                if let Some(req_pred) = expr_to_ref_expr_ext(req_expr, Span::default()) {
                    let pred_str = emit_ref_expr_for_assert(&req_pred, "self");
                    let msg = pred_str.replace('{', "{{").replace('}', "}}");
                    cg.line(&format!("assert!({pred_str}, \"requires: {msg}\");"));
                }
            }
            emit_fn_body(cg, fd);
            cg.pop_indent();
            cg.line("}");
            cg.pop_indent();
            cg.line("}");
            return;
        }
        // Fall through: emit as a free function with `self` as a regular parameter.
    }

    // #928: extension methods on built-in types are emitted as free functions
    // where `self` is renamed to `self_` (Rust keyword).
    let has_self_param = fd.receiver_type.is_some();
    if has_self_param {
        cg.self_as_free_param = true;
    }

    let params_str = emit_params(&fd.params, &borrows, &mutated_params);
    cg.line(&format!(
        "pub fn {}{generics}({params_str}) -> {ret_str} {{",
        fd.name
    ));
    cg.push_indent();
    // Fn-entry probe: records whether the function was ever called.
    if let Some(id) = cg.alloc_branch(fd.span.line, BranchKind::FnEntry) {
        cg.emit_cov_hit(id);
    }
    // Emit runtime precondition guards for `requires` clauses (Phase 4, #627).
    for req_expr in &fd.requires {
        if let Some(req_pred) = expr_to_ref_expr_ext(req_expr, Span::default()) {
            let pred_str = emit_ref_expr_for_assert(&req_pred, "self");
            let msg = pred_str.replace('{', "{{").replace('}', "}}");
            cg.line(&format!("assert!({pred_str}, \"requires: {msg}\");"));
        }
    }
    emit_fn_body(cg, fd);
    cg.pop_indent();
    cg.line("}");

    if has_self_param {
        cg.self_as_free_param = false;
    }
}

/// Emit the statements and return-refinement check for an AST function body.
fn emit_fn_body(cg: &mut RustEmitter, fd: &FnDecl) {
    // Phase A: compute last uses so emit_expr_as_arg can elide .clone() at move points.
    cg.last_uses = compute_last_uses_ast(&fd.body);

    // Populate borrow_param_names so let-binding emission can add `.clone()`
    // when reading a field from a borrowed parameter (e.g. `acc.items`).
    cg.capability_param_names.clear();
    let borrows = cg
        .capability_params_map
        .get(&fd.name)
        .cloned()
        .unwrap_or_default();
    for (i, param) in fd.params.iter().enumerate() {
        if borrows.get(i).copied().flatten().is_some() {
            cg.capability_param_names.insert(param.name.clone());
        }
    }

    // #960: for HOF params (fn-typed parameters), temporarily insert their inner
    // parameter borrow flags into capability_params_map so that calls through
    // the fn pointer use `&x` instead of `x.clone().into()` for `val T` params.
    let mut hof_param_entries: Vec<(String, Option<Vec<Option<bool>>>)> = Vec::new();
    for param in &fd.params {
        if let TypeExpr::Fn {
            params: fn_params, ..
        } = &param.ty
        {
            let flags: Vec<Option<bool>> = fn_params
                .iter()
                .map(|p| {
                    if let TypeExpr::Ref { mutable, .. } = p {
                        Some(*mutable)
                    } else {
                        None
                    }
                })
                .collect();
            if flags.iter().any(|b| b.is_some()) {
                let previous = cg.capability_params_map.insert(param.name.clone(), flags);
                hof_param_entries.push((param.name.clone(), previous));
            }
        }
    }

    // #1048 + deadlock fix: when inject_actor_join is true, wrap the entire body
    // in an inner scope `{ ... }` so that all actor handles are dropped before
    // mvl_join_actors().
    let needs_actor_scope = cg.inject_actor_join;
    if needs_actor_scope {
        cg.inject_actor_join = false;
        cg.indent();
        cg.push("{");
        cg.nl();
        cg.push_indent();
    }

    // NOTE: emit_fn_body for AST FnDecl uses AST-based emit_block_stmts.
    // Since emit_block_stmts now takes TirStmt, we need a separate AST-based emission.
    // We use the internal helpers directly.
    use crate::mvl::backends::rust::emit_stmts_ast::emit_stmt_ast;
    let stmts = &fd.body.stmts;
    if stmts.is_empty() {
        let is_unit =
            matches!(fd.return_type.as_ref(), TypeExpr::Base { name, .. } if name == "Unit");
        if !is_unit {
            unreachable!("non-Unit function with empty body — blocked by checker (#990)");
        }
    } else {
        let (head, tail) = stmts.split_at(stmts.len() - 1);
        for stmt in head {
            emit_stmt_ast(cg, stmt);
        }

        let last = &tail[0];
        let is_unit =
            matches!(fd.return_type.as_ref(), TypeExpr::Base { name, .. } if name == "Unit");
        let has_ensures = !fd.ensures.is_empty() && !is_unit;
        match last {
            Stmt::Expr { expr, .. } => {
                use crate::mvl::backends::rust::emit_stmts_ast::emit_mcdc_return_expr_ast;
                if !emit_mcdc_return_expr_ast(cg, expr, &fd.return_type, expr.span().line) {
                    if has_ensures {
                        cg.indent();
                        cg.push("let _result = ");
                        emit_expr_tail_with_return_type_ast(cg, expr, &fd.return_type, &fd.params);
                        cg.push(";");
                        cg.nl();
                        for ens_expr in &fd.ensures {
                            if let Some(ens_pred) = expr_to_ref_expr_ext(ens_expr, Span::default())
                            {
                                let pred_str = emit_ref_expr_for_assert(&ens_pred, "_result");
                                let msg = pred_str.replace('{', "{{").replace('}', "}}");
                                cg.line(&format!("assert!({pred_str}, \"ensures: {msg}\");"));
                            }
                        }
                        cg.line("_result");
                    } else {
                        cg.indent();
                        emit_expr_tail_with_return_type_ast(cg, expr, &fd.return_type, &fd.params);
                        cg.nl();
                    }
                }
            }
            other => {
                emit_stmt_ast(cg, other);
            }
        }
    }

    if needs_actor_scope {
        cg.pop_indent();
        cg.indent();
        cg.push("}");
        cg.nl();
        cg.indent();
        cg.push("mvl_join_actors()");
        cg.nl();
    }

    if let Some(pred) = &fd.return_refinement {
        let pred_str = emit_ref_expr_for_assert(pred, "_return_val");
        cg.line(&format!(
            "// return refinement: assert!({pred_str}) — checked by MVL type checker"
        ));
    }

    // #960: restore capability_params_map entries displaced above.
    for (name, previous) in hof_param_entries {
        match previous {
            Some(v) => {
                cg.capability_params_map.insert(name, v);
            }
            None => {
                cg.capability_params_map.remove(&name);
            }
        }
    }
}

// ── Generics (AST) ─────────────────────────────────────────────────────────

/// Like `emit_generics` but also scans `params` to auto-add `Hash + Eq` for
/// type params used as Map/Set keys and `Clone` for Map value params.
fn emit_generics_with_params(
    type_params: &[AstGenericParam],
    constraints: &[AstConstraint],
    params: &[Param],
    return_ty: &TypeExpr,
) -> String {
    if type_params.is_empty() {
        return String::new();
    }
    // Build bounds map from explicit MVL where-clause constraints.
    let mut bounds: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for c in constraints {
        bounds
            .entry(c.name.clone())
            .or_default()
            .push(c.bound.clone());
    }

    // Auto-add Hash+Eq for Map/Set key type params and Clone for Map value params.
    collect_map_set_bounds(params, &mut bounds);
    collect_type_bounds(return_ty, &mut bounds);

    let params_out: Vec<String> = type_params
        .iter()
        .map(|p| match p {
            AstGenericParam::Const(name, _ty) => format!("const {name}: usize"),
            AstGenericParam::Type(name) => {
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

/// Scan function parameters for `Map[K, V]` and `Set[T]` types and add the
/// Rust trait bounds needed for HashMap/HashSet to function correctly.
fn collect_map_set_bounds(
    params: &[Param],
    bounds: &mut std::collections::HashMap<String, Vec<String>>,
) {
    for p in params {
        collect_type_bounds(&p.ty, bounds);
    }
}

fn collect_type_bounds(ty: &TypeExpr, bounds: &mut std::collections::HashMap<String, Vec<String>>) {
    match ty {
        TypeExpr::Base { name, args, .. } => match name.as_str() {
            "Map" if args.len() == 2 => {
                add_bound_if_type_param(&args[0], "std::hash::Hash", bounds);
                add_bound_if_type_param(&args[0], "std::cmp::Eq", bounds);
                add_bound_if_type_param(&args[0], "Clone", bounds);
                add_bound_if_type_param(&args[1], "Clone", bounds);
                for a in args {
                    collect_type_bounds(a, bounds);
                }
            }
            "Set" if args.len() == 1 => {
                add_bound_if_type_param(&args[0], "std::hash::Hash", bounds);
                add_bound_if_type_param(&args[0], "std::cmp::Eq", bounds);
                add_bound_if_type_param(&args[0], "Clone", bounds);
            }
            _ => {
                for a in args {
                    collect_type_bounds(a, bounds);
                }
            }
        },
        TypeExpr::Ref { inner, .. } => collect_type_bounds(inner, bounds),
        _ => {}
    }
}

/// If `ty` is a bare type-parameter name (single uppercase letter or camel-case
/// ident with no args), add the given bound for that name.
fn add_bound_if_type_param(
    ty: &TypeExpr,
    bound: &str,
    bounds: &mut std::collections::HashMap<String, Vec<String>>,
) {
    if let TypeExpr::Base { name, args, .. } = ty {
        if args.is_empty() && !is_concrete_type(name) {
            let entry = bounds.entry(name.clone()).or_default();
            if !entry.iter().any(|b| b == bound) {
                entry.push(bound.to_string());
            }
        }
    }
}

/// Concrete MVL built-in types that are never generic type parameters.
fn is_concrete_type(name: &str) -> bool {
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

// ── Parameters (AST) ─────────────────────────────────────────────────────

/// Collect names of Map/Set parameters that are mutated in the body (i.e. have
/// `.insert(…)` or `.remove(…)` called on them).
fn collect_mutated_map_params(
    body: &Block,
    cap_map: &std::collections::HashMap<String, Vec<Option<bool>>>,
) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    scan_stmts_for_mut_calls(&body.stmts, &mut out, cap_map);
    out
}

fn scan_stmts_for_mut_calls(
    stmts: &[Stmt],
    out: &mut std::collections::HashSet<String>,
    cap_map: &std::collections::HashMap<String, Vec<Option<bool>>>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Expr { expr, .. }
            | Stmt::Return {
                value: Some(expr), ..
            } => {
                scan_expr_for_mut_calls(expr, out, cap_map);
            }
            Stmt::Let { init, .. } => scan_expr_for_mut_calls(init, out, cap_map),
            Stmt::Assign { value, .. } => scan_expr_for_mut_calls(value, out, cap_map),
            Stmt::If {
                cond, then, else_, ..
            } => {
                scan_expr_for_mut_calls(cond, out, cap_map);
                scan_stmts_for_mut_calls(&then.stmts, out, cap_map);
                if let Some(crate::mvl::parser::ast::ElseBranch::Block(b)) = else_ {
                    scan_stmts_for_mut_calls(&b.stmts, out, cap_map);
                }
            }
            Stmt::For { body, iter, .. } => {
                scan_expr_for_mut_calls(iter, out, cap_map);
                scan_stmts_for_mut_calls(&body.stmts, out, cap_map);
            }
            Stmt::While { cond, body, .. } => {
                scan_expr_for_mut_calls(cond, out, cap_map);
                scan_stmts_for_mut_calls(&body.stmts, out, cap_map);
            }
            _ => {}
        }
    }
}

fn scan_expr_for_mut_calls(
    expr: &Expr,
    out: &mut std::collections::HashSet<String>,
    cap_map: &std::collections::HashMap<String, Vec<Option<bool>>>,
) {
    match expr {
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            if matches!(method.as_str(), "insert" | "remove" | "retain") {
                if let Expr::Ident(name, _) = receiver.as_ref() {
                    out.insert(name.clone());
                }
            }
            scan_expr_for_mut_calls(receiver, out, cap_map);
            for a in args {
                scan_expr_for_mut_calls(a, out, cap_map);
            }
        }
        Expr::FnCall { name, args, .. } => {
            if let Some(borrows) = cap_map.get(name.as_str()) {
                for (i, arg) in args.iter().enumerate() {
                    if let Some(Some(true)) = borrows.get(i) {
                        if let Expr::Ident(param_name, _) = arg {
                            out.insert(param_name.clone());
                        }
                    }
                }
            }
            for a in args {
                scan_expr_for_mut_calls(a, out, cap_map);
            }
        }
        _ => {}
    }
}

fn emit_params(
    params: &[Param],
    borrows: &[Option<bool>],
    mutated_params: &std::collections::HashSet<String>,
) -> String {
    params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let ty_str = match borrows.get(i).copied().flatten() {
                Some(mutable) if !matches!(p.ty, TypeExpr::Ref { .. }) => {
                    if mutable {
                        format!("&mut {}", emit_fn_param_type(&p.ty))
                    } else {
                        format!("&{}", emit_fn_param_type(&p.ty))
                    }
                }
                _ => emit_fn_param_type(&p.ty),
            };
            let cap_comment = match &p.capability {
                Some(AstCapability::Iso) => "/* iso */ ",
                Some(AstCapability::Val) => "/* val */ ",
                Some(AstCapability::Ref) => "/* ref */ ",
                Some(AstCapability::Tag) => "/* tag */ ",
                None => "",
            };
            let needs_mut_for_body =
                borrows.get(i).copied().flatten().is_none() && mutated_params.contains(&p.name);
            let has_ref_cap = matches!(
                p.capability,
                Some(AstCapability::Ref) | Some(AstCapability::Iso)
            );
            let mut_prefix = if has_ref_cap || needs_mut_for_body {
                "mut "
            } else {
                ""
            };
            let param_name = if p.name == "self" {
                "self_"
            } else {
                p.name.as_str()
            };
            format!("{cap_comment}{mut_prefix}{param_name}: {ty_str}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// ── AST tail expression emitter ─────────────────────────────────────────

fn emit_expr_tail_with_return_type_ast(
    cg: &mut RustEmitter,
    expr: &Expr,
    return_type: &TypeExpr,
    params: &[Param],
) {
    use crate::mvl::backends::rust::emit_exprs_ast::emit_expr_ast;
    if matches!(return_type, TypeExpr::Base { name, .. } if name == "Unit") {
        emit_expr_ast(cg, expr);
        cg.push(";");
        return;
    }
    match return_type {
        TypeExpr::Labeled { label, .. } if is_raw_value(expr, params) => {
            let label_name = emit_label(label.as_str());
            cg.push(&format!("{label_name}("));
            emit_expr_ast(cg, expr);
            cg.push(")");
            return;
        }
        TypeExpr::Labeled { .. }
            if matches!(expr, Expr::FnCall { .. } | Expr::MethodCall { .. }) =>
        {
            emit_expr_ast(cg, expr);
            cg.push(".into()");
            return;
        }
        TypeExpr::Result { ok, .. } => {
            if let TypeExpr::Labeled { label, .. } = ok.as_ref() {
                if let Expr::FnCall { name, args, .. } = expr {
                    if name == "Ok" && args.len() == 1 && is_raw_value(&args[0], params) {
                        let label_name = emit_label(label.as_str());
                        cg.push("Ok(");
                        cg.push(&format!("{label_name}("));
                        emit_expr_ast(cg, &args[0]);
                        cg.push("))");
                        return;
                    }
                }
            }
        }
        _ => {}
    }
    emit_expr_ast(cg, expr);
}

fn is_raw_value(expr: &Expr, params: &[Param]) -> bool {
    match expr {
        Expr::Literal(_, _) => true,
        Expr::Ident(name, _) => params
            .iter()
            .any(|p| &p.name == name && !is_labeled_type(&p.ty)),
        _ => false,
    }
}

fn is_labeled_type(ty: &TypeExpr) -> bool {
    matches!(ty, TypeExpr::Labeled { .. })
}
