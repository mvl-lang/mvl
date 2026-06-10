// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Information Flow Control: security lattice operations and implicit flow analysis.
//!
//! Implements Requirement 11 of the MVL spec (003-information-flow).
//!
//! User-defined label system (#894):
//!   Labels are opaque types declared via `label Name`. No hierarchy, no lattice.
//!   `relabel` transitions are the only bridge between labeled and bare types.
//!
//! # Implicit flow analysis (#1007)
//!
//! Beyond direct-flow enforcement (Req 1, 3, 4, 6, 7 — done in the type checker),
//! this pass detects *implicit* flows: information leaked through control flow rather
//! than data flow.  The canonical example:
//!
//! ```mvl
//! if secret_flag { println("branch taken") }
//! ```
//!
//! Even though the `println` argument is a literal string (bare), whether the
//! print fires at all reveals whether `secret_flag` was truthy.  This is an
//! implicit flow from the labeled condition to an observable (effectful) function.
//!
//! The analysis tracks the **Program Counter (PC) label**: the label of any
//! condition controlling the current execution point.
//! Any effectful function call inside a branch whose PC is labeled
//! is flagged as `ImplicitFlowViolation`.
//!
//! Observable functions are determined by the **effect system**: any function
//! with declared effects (`! Console`, `! Log`, `! FileWrite`, etc.) is observable.
//! This replaces the previous `sink` keyword approach (#1007).

use std::collections::{HashMap, HashSet};

use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::types::Ty;
use crate::mvl::checker::walk::walk_block;
use crate::mvl::parser::ast::{
    Block, Decl, ElseBranch, Expr, MatchBody, Pattern, Program, Stmt, TypeExpr,
};

/// Extract the outermost security label from a type, if any.
/// Looks through Refined wrappers to find the label.
///
/// NOTE: Nested `Labeled` types (e.g., `Labeled("A", Labeled("B", T))`) are not
/// valid IR — the parser and checker must never produce them. This function
/// only reads the outermost label, which is sufficient for valid IR.
pub fn label_of(ty: &Ty) -> Option<&str> {
    match ty {
        Ty::Labeled(l, _) => Some(l.as_str()),
        Ty::Refined(inner, _) => label_of(inner),
        _ => None,
    }
}

/// Remove the outermost security label from a type, returning the inner type.
/// Used for argument type-checking in label-transparent functions (ADR-0024):
/// the function accepts any label on its arguments; the label is collected
/// separately and applied to the return type.
pub fn strip_label(ty: &Ty) -> &Ty {
    match ty {
        Ty::Labeled(_, inner) => inner,
        Ty::Refined(inner, _) => strip_label(inner),
        _ => ty,
    }
}

/// Wrap a type in a security label, or return it unchanged if label is None.
pub fn apply_label(label: Option<String>, ty: Ty) -> Ty {
    match label {
        Some(l) => Ty::Labeled(l, Box::new(ty)),
        None => ty,
    }
}

/// Compute the "join" of two optional label names.
/// In the user-defined label model there is no lattice order, so the join
/// is used only for PC tracking: any labeled condition raises the PC.
/// If both labels are the same, the result is that label; if they differ or
/// either is None, the result is the non-None one (if any).
pub fn join_opt(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (None, None) => None,
        (Some(l), None) | (None, Some(l)) => Some(l),
        // Two distinct labels: keep the first (conservative — any label is high enough)
        (Some(la), Some(_lb)) => Some(la),
    }
}

// ── Implicit flow analysis (#1007) ────────────────────────────────────────────

/// Walk every function in `prog` and emit [`CheckError::ImplicitFlowViolation`]
/// for any effectful (observable) function call inside a branch controlled by a
/// `Secret` or `Tainted` condition.
///
/// **Precondition:** `TypeChecker::check_program` MUST have run first so that
/// direct-flow violations (Req 11 Phase 1) are already captured.
///
/// Observable functions are those with declared effects (`! Console`, `! Log`, etc.).
/// This replaces the previous `sink` keyword approach — the effect system provides
/// the same information without a dedicated keyword.
/// Recursively bind every identifier in `pat` to `label` in `env`.
/// Handles nested patterns like `(Some(a), b)` by walking the full tree.
pub(crate) fn bind_pattern_labels(pat: &Pattern, label: &str, env: &mut HashMap<String, String>) {
    match pat {
        Pattern::Ident(name, _) => {
            env.insert(name.clone(), label.to_string());
        }
        Pattern::Tuple { elems, .. } => {
            for elem in elems {
                bind_pattern_labels(elem, label, env);
            }
        }
        Pattern::TupleStruct { fields, .. } => {
            for field in fields {
                bind_pattern_labels(field, label, env);
            }
        }
        Pattern::Struct { fields, .. } => {
            for (_, field) in fields {
                bind_pattern_labels(field, label, env);
            }
        }
        Pattern::Some { inner, .. } | Pattern::Ok { inner, .. } | Pattern::Err { inner, .. } => {
            bind_pattern_labels(inner, label, env);
        }
        Pattern::Wildcard(_) | Pattern::Literal(..) | Pattern::None(_) => {}
    }
}

/// Collect function names that have declared effects (observable functions).
///
/// Any function with `! Effect` in its signature is observable — calling it under
/// a high-PC branch leaks information through control flow (#1007).
///
/// Seeds from both AST program declarations and builtin functions registered in the
/// TypeEnv (e.g. `write` with `! Console`), so Rust-level builtins without `.mvl`
/// source are also detected.
fn collect_effectful_names(
    programs: &[&Program],
    builtin_fns: Option<&HashMap<String, super::context::FnInfo>>,
) -> HashSet<String> {
    let mut names = HashSet::new();
    // Seed from builtins registered in TypeEnv.
    if let Some(fns) = builtin_fns {
        for (name, info) in fns {
            if !info.effects.is_empty() {
                names.insert(name.clone());
            }
        }
    }
    for prog in programs {
        for decl in &prog.declarations {
            match decl {
                Decl::Fn(fd) if !fd.effects.is_empty() => {
                    if let Some(recv_ty) = &fd.receiver_type {
                        names.insert(format!("{}::{}", recv_ty, fd.name));
                    } else {
                        names.insert(fd.name.clone());
                    }
                }
                Decl::Impl(id) => {
                    for m in &id.methods {
                        if !m.effects.is_empty() {
                            names.insert(format!("{}::{}", id.type_name, m.name));
                        }
                    }
                }
                Decl::Actor(ad) => {
                    for m in &ad.methods {
                        if !m.effects.is_empty() {
                            names.insert(format!("{}::{}", ad.name, m.name));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    names
}

/// Build a map from user-defined function name → name of observable (effectful) fn reachable from it.
///
/// Seeds from functions that directly call an effectful function, then propagates
/// transitively via a fixed-point BFS so that `a→b→println` marks both `b` and `a`.
fn build_effect_reachability(
    programs: &[&Program],
    effectful_names: &HashSet<String>,
) -> HashMap<String, String> {
    // Step 1: collect per-function callee sets.
    let mut callee_map: HashMap<String, HashSet<String>> = HashMap::new();
    for prog in programs {
        for decl in &prog.declarations {
            let named_bodies: Vec<(String, &Block)> = match decl {
                Decl::Fn(fd) => vec![(fd.name.clone(), &fd.body)],
                Decl::Impl(id) => id
                    .methods
                    .iter()
                    .map(|m| (m.name.clone(), &m.body))
                    .collect(),
                Decl::Actor(ad) => ad
                    .methods
                    .iter()
                    .map(|m| (m.name.clone(), &m.body))
                    .collect(),
                _ => vec![],
            };
            for (name, body) in named_bodies {
                let callees = callee_map.entry(name).or_default();
                walk_block(body, &mut |expr| match expr {
                    Expr::FnCall { name: callee, .. } => {
                        callees.insert(callee.clone());
                    }
                    Expr::MethodCall {
                        receiver, method, ..
                    } => {
                        callees.insert(method.clone());
                        if let Expr::Ident(recv_name, _) = receiver.as_ref() {
                            callees.insert(format!("{recv_name}::{method}"));
                        }
                    }
                    _ => {}
                });
            }
        }
    }
    // Step 2: seed — functions that directly call an effectful function.
    // Sort callees for deterministic selection when multiple effectful callees exist.
    let mut reach: HashMap<String, String> = HashMap::new();
    for (fn_name, callees) in &callee_map {
        let mut sorted: Vec<&String> = callees
            .iter()
            .filter(|c| effectful_names.contains(c.as_str()))
            .collect();
        sorted.sort();
        if let Some(first) = sorted.first() {
            reach
                .entry(fn_name.clone())
                .or_insert_with(|| (*first).clone());
        }
    }
    // Step 3: fixed-point propagation.
    loop {
        let mut changed = false;
        for (fn_name, callees) in &callee_map {
            if reach.contains_key(fn_name.as_str()) {
                continue;
            }
            for callee in callees {
                if let Some(observable) = reach.get(callee.as_str()).cloned() {
                    reach.insert(fn_name.clone(), observable);
                    changed = true;
                    break;
                }
            }
        }
        if !changed {
            break;
        }
    }
    reach
}

pub fn check_implicit_flows(
    prog: &Program,
    all_programs: &[&Program],
    builtin_fns: Option<&HashMap<String, super::context::FnInfo>>,
    errors: &mut Vec<CheckError>,
) {
    let effectful_names = collect_effectful_names(all_programs, builtin_fns);
    let mut effect_reach = build_effect_reachability(all_programs, &effectful_names);
    // Merge direct effectful functions into the reachability map so check_expr_flows
    // can use a single lookup: effect_reach.get(name) covers both direct observable
    // functions and functions that transitively call observable ones.
    for name in &effectful_names {
        effect_reach
            .entry(name.clone())
            .or_insert_with(|| name.clone());
    }
    for decl in &prog.declarations {
        match decl {
            Decl::Fn(fd) => {
                let mut env: HashMap<String, String> = HashMap::new();
                for param in &fd.params {
                    if let Some(label) = label_of_type_expr(&param.ty) {
                        env.insert(param.name.clone(), label);
                    }
                }
                check_block_flows(&fd.body, None, &mut env, &fd.name, &effect_reach, errors);
            }
            Decl::Impl(id) => {
                for m in &id.methods {
                    let mut env: HashMap<String, String> = HashMap::new();
                    for param in &m.params {
                        if let Some(label) = label_of_type_expr(&param.ty) {
                            env.insert(param.name.clone(), label);
                        }
                    }
                    let fn_name = format!("{}::{}", id.type_name, m.name);
                    check_block_flows(&m.body, None, &mut env, &fn_name, &effect_reach, errors);
                }
            }
            Decl::Actor(ad) => {
                for m in &ad.methods {
                    let mut env: HashMap<String, String> = HashMap::new();
                    for param in &m.params {
                        if let Some(label) = label_of_type_expr(&param.ty) {
                            env.insert(param.name.clone(), label);
                        }
                    }
                    let fn_name = format!("{}::{}", ad.name, m.name);
                    check_block_flows(&m.body, None, &mut env, &fn_name, &effect_reach, errors);
                }
            }
            _ => {}
        }
    }
}

/// Returns `true` if any prelude function that is called from `prog` carries
/// IFC-labeled parameters or a labeled return type.
///
/// Used to populate [`crate::mvl::checker::CheckResult::has_prelude_ifc_boundary`]
/// so the IFC pass recognises cross-module security boundary exercise (e.g.
/// `main.mvl` calling `execute(db, sql: Tainted[String])`).
pub fn prelude_has_ifc_boundary(
    prelude_a: &[Program],
    prelude_b: &[&Program],
    prog: &Program,
) -> bool {
    let called = collect_called_fn_names(prog);
    let fn_has_label = |params: &[crate::mvl::parser::ast::Param], ret: &TypeExpr| -> bool {
        params.iter().any(|p| label_of_type_expr(&p.ty).is_some())
            || label_of_type_expr(ret).is_some()
    };
    prelude_a
        .iter()
        .chain(prelude_b.iter().copied())
        .flat_map(|p| p.declarations.iter())
        .any(|d| match d {
            Decl::Fn(fd) => called.contains(&fd.name) && fn_has_label(&fd.params, &fd.return_type),
            Decl::Extern(ed) => ed
                .fns
                .iter()
                .any(|ef| called.contains(&ef.name) && fn_has_label(&ef.params, &ef.return_type)),
            _ => false,
        })
}

/// Collect all function names called anywhere in `prog`'s function bodies.
fn collect_called_fn_names(prog: &Program) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for decl in &prog.declarations {
        let bodies: Vec<&Block> = match decl {
            Decl::Fn(fd) => vec![&fd.body],
            Decl::Impl(id) => id.methods.iter().map(|m| &m.body).collect(),
            Decl::Actor(ad) => ad.methods.iter().map(|m| &m.body).collect(),
            _ => vec![],
        };
        for body in bodies {
            walk_block(body, &mut |expr| match expr {
                Expr::FnCall { name, .. } => {
                    names.insert(name.clone());
                }
                Expr::MethodCall {
                    receiver, method, ..
                } => {
                    names.insert(method.clone());
                    if let Expr::Ident(recv_name, _) = receiver.as_ref() {
                        names.insert(format!("{recv_name}::{method}"));
                    }
                }
                _ => {}
            });
        }
    }
    names
}

/// Count all `Expr::Relabel` call sites in the program.
///
/// Used by the IFC pass to include the auditable relabel count in the `Proven` evidence.
pub fn count_relabels(prog: &Program) -> usize {
    let mut rc = 0usize;
    for decl in &prog.declarations {
        if let Decl::Fn(fd) = decl {
            walk_block(&fd.body, &mut |expr| {
                if matches!(expr, Expr::Relabel { .. }) {
                    rc += 1;
                }
            });
        }
    }
    rc
}

/// Count relabel declarations that carry the `audit` keyword (#896).
///
/// These are transitions where every call site emits a runtime audit event.
pub fn count_audit_relabels(prog: &Program) -> usize {
    prog.declarations
        .iter()
        .filter(|d| matches!(d, Decl::Relabel(rd) if rd.audit))
        .count()
}

/// Count function parameters that carry a security label (`Tainted[T]`, `Secret[T]`, etc.).
pub fn count_labeled_params(prog: &Program) -> usize {
    let mut count = 0;
    for decl in &prog.declarations {
        let params = match decl {
            Decl::Fn(fd) => &fd.params[..],
            _ => continue,
        };
        count += params
            .iter()
            .filter(|p| label_of_type_expr(&p.ty).is_some())
            .count();
    }
    // Also count actor method params
    for decl in &prog.declarations {
        if let Decl::Actor(ad) = decl {
            for method in &ad.methods {
                count += method
                    .params
                    .iter()
                    .filter(|p| label_of_type_expr(&p.ty).is_some())
                    .count();
            }
        }
    }
    count
}

/// Count the number of flow-check sites: branches controlled by labeled values
/// plus fn-call arguments that pass labeled data.
///
/// Walks all function bodies and counts `if`/`match` nodes where the
/// condition/scrutinee references a labeled parameter, plus all fn-call
/// arguments that use a labeled variable.
pub fn count_flow_check_sites(prog: &Program) -> usize {
    let mut count = 0;
    for decl in &prog.declarations {
        if let Decl::Fn(fd) = decl {
            let labeled_params: HashSet<String> = fd
                .params
                .iter()
                .filter(|p| label_of_type_expr(&p.ty).is_some())
                .map(|p| p.name.clone())
                .collect();
            if !labeled_params.is_empty() {
                count += count_flow_sites_in_block(&fd.body, &labeled_params);
            }
        }
    }
    count
}

fn count_flow_sites_in_block(block: &Block, labeled: &HashSet<String>) -> usize {
    let mut count = 0;
    for stmt in &block.stmts {
        count += count_flow_sites_in_stmt(stmt, labeled);
    }
    count
}

fn count_flow_sites_in_stmt(stmt: &Stmt, labeled: &HashSet<String>) -> usize {
    match stmt {
        Stmt::Expr { expr, .. } => count_flow_sites_in_expr(expr, labeled),
        Stmt::Return { value: Some(e), .. } => count_flow_sites_in_expr(e, labeled),
        Stmt::Return { value: None, .. } => 0,
        Stmt::Let { init, .. } => count_flow_sites_in_expr(init, labeled),
        Stmt::Assign { value, .. } => count_flow_sites_in_expr(value, labeled),
        Stmt::If {
            cond, then, else_, ..
        } => {
            let mut c = if expr_uses_labeled(cond, labeled) {
                1
            } else {
                0
            };
            c += count_flow_sites_in_block(then, labeled);
            if let Some(eb) = else_ {
                match eb {
                    ElseBranch::Block(b) => c += count_flow_sites_in_block(b, labeled),
                    ElseBranch::If(s) => c += count_flow_sites_in_stmt(s, labeled),
                }
            }
            c
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            let mut c = if expr_uses_labeled(scrutinee, labeled) {
                1
            } else {
                0
            };
            for arm in arms {
                match &arm.body {
                    MatchBody::Block(b) => c += count_flow_sites_in_block(b, labeled),
                    MatchBody::Expr(e) => c += count_flow_sites_in_expr(e, labeled),
                }
            }
            c
        }
        Stmt::For { body, .. } | Stmt::While { body, .. } => {
            count_flow_sites_in_block(body, labeled)
        }
    }
}

fn count_flow_sites_in_expr(expr: &Expr, labeled: &HashSet<String>) -> usize {
    match expr {
        Expr::If {
            cond, then, else_, ..
        } => {
            let mut c = if expr_uses_labeled(cond, labeled) {
                1
            } else {
                0
            };
            c += count_flow_sites_in_block(then, labeled);
            if let Some(eb) = else_ {
                c += count_flow_sites_in_expr(eb, labeled);
            }
            c
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            let mut c = if expr_uses_labeled(scrutinee, labeled) {
                1
            } else {
                0
            };
            for arm in arms {
                match &arm.body {
                    MatchBody::Block(b) => c += count_flow_sites_in_block(b, labeled),
                    MatchBody::Expr(e) => c += count_flow_sites_in_expr(e, labeled),
                }
            }
            c
        }
        Expr::Block(b) => count_flow_sites_in_block(b, labeled),
        Expr::FnCall { args, .. } => args
            .iter()
            .filter(|a| expr_uses_labeled(a, labeled))
            .count(),
        _ => 0,
    }
}

fn expr_uses_labeled(expr: &Expr, labeled: &HashSet<String>) -> bool {
    match expr {
        Expr::Ident(name, _) => labeled.contains(name.as_str()),
        Expr::FieldAccess { expr, .. } => expr_uses_labeled(expr, labeled),
        Expr::Binary { left, right, .. } => {
            expr_uses_labeled(left, labeled) || expr_uses_labeled(right, labeled)
        }
        Expr::Unary { expr, .. } => expr_uses_labeled(expr, labeled),
        _ => false,
    }
}

/// Extract the outermost security label name from a `TypeExpr`, if any.
pub(crate) fn label_of_type_expr(te: &TypeExpr) -> Option<String> {
    match te {
        TypeExpr::Labeled { label, .. } => Some(label.clone()),
        TypeExpr::Refined { inner, .. } => label_of_type_expr(inner),
        TypeExpr::Tuple { elems, .. } => elems.iter().find_map(label_of_type_expr),
        TypeExpr::Base { .. }
        | TypeExpr::Option { .. }
        | TypeExpr::Result { .. }
        | TypeExpr::Ref { .. }
        | TypeExpr::Fn { .. }
        | TypeExpr::IntConst { .. }
        | TypeExpr::Session { .. } => None,
    }
}

/// Infer the security label of an expression from the current label env.
/// Conservative: returns `None` if the label cannot be determined.
fn infer_label(expr: &Expr, env: &HashMap<String, String>) -> Option<String> {
    match expr {
        Expr::Ident(name, _) => env.get(name.as_str()).cloned(),
        Expr::Binary { left, right, .. } => {
            join_opt(infer_label(left, env), infer_label(right, env))
        }
        Expr::Unary { expr, .. } | Expr::Borrow { expr, .. } => infer_label(expr, env),
        Expr::FieldAccess { expr, .. } => infer_label(expr, env),
        // `relabel` produces the `to` side label (None = bare). Conservative: treat as None
        // (unlabeled after relabel), which avoids false positives in sink detection.
        Expr::Relabel { .. } => None,
        // Function calls: join labels of arguments (conservative over-approximation).
        Expr::FnCall { args, .. } => args
            .iter()
            .map(|a| infer_label(a, env))
            .fold(None, join_opt),
        // Method calls: join receiver label with argument labels.
        Expr::MethodCall { receiver, args, .. } => {
            let recv_label = infer_label(receiver, env);
            let arg_label = args
                .iter()
                .map(|a| infer_label(a, env))
                .fold(None, join_opt);
            join_opt(recv_label, arg_label)
        }
        _ => None,
    }
}

/// True if `label` is present (any label is "high" in the new model — all labels
/// are sensitive and should not control whether a public sink fires).
fn is_high_opt(label: &Option<String>) -> bool {
    label.is_some()
}

/// Walk a block, tracking the current PC label and the label env.
/// Let bindings extend the env sequentially within the block.
fn check_block_flows(
    block: &Block,
    pc: Option<String>,
    env: &mut HashMap<String, String>,
    caller_fn: &str,
    effect_reach: &HashMap<String, String>,
    errors: &mut Vec<CheckError>,
) {
    for stmt in &block.stmts {
        check_stmt_flows(stmt, pc.clone(), env, caller_fn, effect_reach, errors);
    }
}

fn check_stmt_flows(
    stmt: &Stmt,
    pc: Option<String>,
    env: &mut HashMap<String, String>,
    caller_fn: &str,
    effect_reach: &HashMap<String, String>,
    errors: &mut Vec<CheckError>,
) {
    match stmt {
        Stmt::Let {
            pattern, ty, init, ..
        } => {
            // Walk the RHS under the current PC label.
            check_expr_flows(init, pc, env, caller_fn, effect_reach, errors);
            // Extend the label env; use recursive helper so nested patterns
            // like `(Some(a), b)` are fully bound.
            let label = label_of_type_expr(ty).or_else(|| infer_label(init, env));
            if let Some(l) = label {
                bind_pattern_labels(pattern, &l, env);
            }
        }
        Stmt::Assign { value, .. } => {
            check_expr_flows(value, pc, env, caller_fn, effect_reach, errors);
        }
        Stmt::Return { value: Some(e), .. } => {
            check_expr_flows(e, pc, env, caller_fn, effect_reach, errors);
        }
        Stmt::Return { value: None, .. } => {}
        Stmt::Expr { expr, .. } => {
            check_expr_flows(expr, pc, env, caller_fn, effect_reach, errors);
        }
        Stmt::If {
            cond, then, else_, ..
        } => {
            let cond_label = infer_label(cond, env);
            let body_pc = join_opt(pc.clone(), cond_label);
            check_expr_flows(cond, pc.clone(), env, caller_fn, effect_reach, errors);
            let mut then_env = env.clone();
            check_block_flows(
                then,
                body_pc.clone(),
                &mut then_env,
                caller_fn,
                effect_reach,
                errors,
            );
            match else_ {
                Some(ElseBranch::Block(blk)) => {
                    let mut else_env = env.clone();
                    check_block_flows(blk, body_pc, &mut else_env, caller_fn, effect_reach, errors);
                }
                Some(ElseBranch::If(nested)) => {
                    let mut else_env = env.clone();
                    check_stmt_flows(
                        nested,
                        body_pc,
                        &mut else_env,
                        caller_fn,
                        effect_reach,
                        errors,
                    );
                }
                None => {}
            }
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            let scr_label = infer_label(scrutinee, env);
            let body_pc = join_opt(pc.clone(), scr_label);
            check_expr_flows(scrutinee, pc, env, caller_fn, effect_reach, errors);
            for arm in arms {
                let mut arm_env = env.clone();
                match &arm.body {
                    MatchBody::Expr(expr) => check_expr_flows(
                        expr,
                        body_pc.clone(),
                        &mut arm_env,
                        caller_fn,
                        effect_reach,
                        errors,
                    ),
                    MatchBody::Block(blk) => check_block_flows(
                        blk,
                        body_pc.clone(),
                        &mut arm_env,
                        caller_fn,
                        effect_reach,
                        errors,
                    ),
                }
            }
        }
        Stmt::While { cond, body, .. } => {
            let cond_label = infer_label(cond, env);
            let body_pc = join_opt(pc.clone(), cond_label);
            check_expr_flows(cond, pc, env, caller_fn, effect_reach, errors);
            let mut body_env = env.clone();
            check_block_flows(
                body,
                body_pc,
                &mut body_env,
                caller_fn,
                effect_reach,
                errors,
            );
        }
        // Fix #858: bind the for-loop pattern variable to the iterator label so
        // that uses inside the body see the correct security label.
        Stmt::For {
            pattern,
            iter,
            body,
            ..
        } => {
            let iter_label = infer_label(iter, env);
            let body_pc = join_opt(pc.clone(), iter_label.clone());
            check_expr_flows(iter, pc, env, caller_fn, effect_reach, errors);
            let mut body_env = env.clone();
            if let Some(l) = iter_label {
                bind_pattern_labels(pattern, &l, &mut body_env);
            }
            check_block_flows(
                body,
                body_pc,
                &mut body_env,
                caller_fn,
                effect_reach,
                errors,
            );
        }
    }
}

fn check_expr_flows(
    expr: &Expr,
    pc: Option<String>,
    env: &mut HashMap<String, String>,
    caller_fn: &str,
    effect_reach: &HashMap<String, String>,
    errors: &mut Vec<CheckError>,
) {
    match expr {
        Expr::FnCall {
            name, args, span, ..
        } => {
            if is_high_opt(&pc) {
                if let Some(observable) = effect_reach.get(name.as_str()) {
                    if observable == name {
                        // Direct observable (effectful) fn under high PC — implicit flow.
                        errors.push(CheckError::ImplicitFlowViolation {
                            pc_label: pc.as_deref().unwrap_or("labeled").to_string(),
                            observable_fn: name.clone(),
                            span: *span,
                        });
                    } else {
                        // Callee transitively reaches an observable fn — cross-function implicit flow.
                        errors.push(CheckError::CrossFunctionImplicitFlowViolation {
                            pc_label: pc.as_deref().unwrap_or("labeled").to_string(),
                            caller: caller_fn.to_string(),
                            callee: name.clone(),
                            observable_fn: observable.clone(),
                            span: *span,
                        });
                    }
                }
            }
            for arg in args {
                check_expr_flows(arg, pc.clone(), env, caller_fn, effect_reach, errors);
            }
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            let cond_label = infer_label(cond, env);
            let body_pc = join_opt(pc.clone(), cond_label);
            check_expr_flows(cond, pc, env, caller_fn, effect_reach, errors);
            let mut then_env = env.clone();
            check_block_flows(
                then,
                body_pc.clone(),
                &mut then_env,
                caller_fn,
                effect_reach,
                errors,
            );
            if let Some(e) = else_ {
                let mut else_env = env.clone();
                check_expr_flows(e, body_pc, &mut else_env, caller_fn, effect_reach, errors);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            let scr_label = infer_label(scrutinee, env);
            let body_pc = join_opt(pc.clone(), scr_label);
            check_expr_flows(scrutinee, pc, env, caller_fn, effect_reach, errors);
            for arm in arms {
                let mut arm_env = env.clone();
                match &arm.body {
                    MatchBody::Expr(e) => check_expr_flows(
                        e,
                        body_pc.clone(),
                        &mut arm_env,
                        caller_fn,
                        effect_reach,
                        errors,
                    ),
                    MatchBody::Block(blk) => check_block_flows(
                        blk,
                        body_pc.clone(),
                        &mut arm_env,
                        caller_fn,
                        effect_reach,
                        errors,
                    ),
                }
            }
        }
        Expr::Binary { left, right, .. } => {
            check_expr_flows(left, pc.clone(), env, caller_fn, effect_reach, errors);
            check_expr_flows(right, pc, env, caller_fn, effect_reach, errors);
        }
        Expr::Unary { expr, .. }
        | Expr::Relabel { expr, .. }
        | Expr::Consume { expr, .. }
        | Expr::Propagate { expr, .. }
        | Expr::FieldAccess { expr, .. }
        | Expr::Borrow { expr, .. }
        | Expr::As { expr, .. } => {
            check_expr_flows(expr, pc, env, caller_fn, effect_reach, errors);
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            span,
            ..
        } => {
            // Method calls: check for implicit flow using qualified name in effect_reach (#1007).
            if is_high_opt(&pc) {
                // Build candidate qualified names matching collect_calls_in_expr / collect_effectful_names.
                let mut qualified_names: Vec<String> = vec![method.clone()];
                if let Expr::Ident(recv_name, _) = receiver.as_ref() {
                    qualified_names.push(format!("{recv_name}::{method}"));
                }
                // Also match Type::method keys where the method name matches,
                // since the receiver is a variable name but observable fns are registered
                // by type name (e.g. "Logger::info" vs "logger.info()").
                let method_suffix = format!("::{method}");
                for key in effect_reach.keys() {
                    if key.ends_with(&method_suffix) && !qualified_names.contains(key) {
                        qualified_names.push(key.clone());
                    }
                }
                for qn in &qualified_names {
                    if let Some(observable) = effect_reach.get(qn.as_str()) {
                        if observable == qn {
                            errors.push(CheckError::ImplicitFlowViolation {
                                pc_label: pc.as_deref().unwrap_or("labeled").to_string(),
                                observable_fn: qn.clone(),
                                span: *span,
                            });
                        } else {
                            errors.push(CheckError::CrossFunctionImplicitFlowViolation {
                                pc_label: pc.as_deref().unwrap_or("labeled").to_string(),
                                caller: caller_fn.to_string(),
                                callee: qn.clone(),
                                observable_fn: observable.clone(),
                                span: *span,
                            });
                        }
                        break;
                    }
                }
            }
            check_expr_flows(receiver, pc.clone(), env, caller_fn, effect_reach, errors);
            for arg in args {
                check_expr_flows(arg, pc.clone(), env, caller_fn, effect_reach, errors);
            }
        }
        Expr::Block(blk) => {
            let mut blk_env = env.clone();
            check_block_flows(blk, pc, &mut blk_env, caller_fn, effect_reach, errors);
        }
        Expr::Lambda { body, .. } => {
            // Lambdas capture the outer env but reset pc (they are called later).
            check_expr_flows(body, None, env, caller_fn, effect_reach, errors);
        }
        Expr::Construct { fields, .. } => {
            for (_, v) in fields {
                check_expr_flows(v, pc.clone(), env, caller_fn, effect_reach, errors);
            }
        }
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            for e in elems {
                check_expr_flows(e, pc.clone(), env, caller_fn, effect_reach, errors);
            }
        }
        Expr::Map { pairs, .. } => {
            for (k, v) in pairs {
                check_expr_flows(k, pc.clone(), env, caller_fn, effect_reach, errors);
                check_expr_flows(v, pc.clone(), env, caller_fn, effect_reach, errors);
            }
        }
        Expr::Spawn { fields, .. } => {
            for (_, v) in fields {
                check_expr_flows(v, pc.clone(), env, caller_fn, effect_reach, errors);
            }
        }
        Expr::Select { arms, .. } => {
            for arm in arms {
                check_expr_flows(&arm.expr, pc.clone(), env, caller_fn, effect_reach, errors);
                for stmt in &arm.body.stmts {
                    check_stmt_flows(stmt, pc.clone(), env, caller_fn, effect_reach, errors);
                }
            }
        }
        // Leaves — no sub-expressions to walk.
        Expr::Literal(..) | Expr::Ident(..) | Expr::Quantifier(..) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::Parser;

    fn parse(src: &str) -> crate::mvl::parser::ast::Program {
        let (mut p, _) = Parser::new(src);
        p.parse_program()
    }

    fn cross_fn_violations(src: &str) -> Vec<CheckError> {
        let prog = parse(src);
        let prog_ref = &prog;
        let mut errors = Vec::new();
        check_implicit_flows(&prog, &[prog_ref], None, &mut errors);
        errors
            .into_iter()
            .filter(|e| matches!(e, CheckError::CrossFunctionImplicitFlowViolation { .. }))
            .collect()
    }

    // ── cross-function implicit flow tests (#832) ─────────────────────────

    #[test]
    fn direct_call_to_println_wrapper_under_high_pc() {
        // fn log_access(msg: String) { println(msg) }
        // fn check_auth(flag: Secret[Bool]) { if flag { log_access("x") } }
        let violations = cross_fn_violations(
            "label Secret \
             fn println(msg: String) -> Unit ! Console { } \
             fn log_access(msg: String) -> Unit ! Console { println(msg) } \
             fn check_auth(flag: Secret[Bool]) -> Unit ! Console { if flag { log_access(\"x\") } }",
        );
        assert!(
            !violations.is_empty(),
            "call to println-wrapper under high PC should be a cross-function implicit flow"
        );
        if let Some(CheckError::CrossFunctionImplicitFlowViolation {
            pc_label,
            callee,
            observable_fn,
            ..
        }) = violations.first()
        {
            assert_eq!(pc_label, "Secret");
            assert_eq!(callee, "log_access");
            assert_eq!(observable_fn, "println");
        }
    }

    #[test]
    fn transitive_chain_a_calls_b_calls_println() {
        // fn a() calls b(), b() calls println — all three have effects.
        // if secret { a() } → cross-function implicit flow.
        // a's nearest observable callee is b (which itself has `! Console`).
        let violations = cross_fn_violations(
            "label Secret \
             fn println(msg: String) -> Unit ! Console { } \
             fn b(msg: String) -> Unit ! Console { println(msg) } \
             fn a(msg: String) -> Unit ! Console { b(msg) } \
             fn entry(flag: Secret[Bool]) -> Unit ! Console { if flag { a(\"x\") } }",
        );
        assert!(
            !violations.is_empty(),
            "transitive a→b→println under high PC should produce cross-function implicit flow"
        );
        if let Some(CheckError::CrossFunctionImplicitFlowViolation {
            callee,
            observable_fn,
            ..
        }) = violations.first()
        {
            assert_eq!(callee, "a");
            // b is itself effectful (`! Console`), so it's the nearest observable callee of a.
            assert_eq!(observable_fn, "b");
        }
    }

    #[test]
    fn no_false_positive_for_unlabeled_branch() {
        // if flag { log_access("x") } where flag is bare Bool — no violation
        let violations = cross_fn_violations(
            "fn println(msg: String) -> Unit ! Console { } \
             fn log_access(msg: String) -> Unit ! Console { println(msg) } \
             fn entry(flag: Bool) -> Unit ! Console { if flag { log_access(\"x\") } }",
        );
        assert!(
            violations.is_empty(),
            "unlabeled branch condition should not produce cross-function implicit flow: {violations:?}"
        );
    }

    #[test]
    fn no_false_positive_for_fn_not_reaching_observable() {
        // fn helper() -> Unit { }  — no effects, not observable
        // if secret { helper() } → no cross-function implicit flow
        let violations = cross_fn_violations(
            "label Secret \
             fn println(msg: String) -> Unit ! Console { } \
             fn helper() -> Unit { } \
             fn entry(flag: Secret[Bool]) -> Unit { if flag { helper() } }",
        );
        assert!(
            violations.is_empty(),
            "fn not reaching an observable function should not produce cross-function implicit flow: {violations:?}"
        );
    }

    #[test]
    fn cross_fn_violation_has_req_11() {
        let violations = cross_fn_violations(
            "label Secret \
             fn println(msg: String) -> Unit ! Console { } \
             fn log(msg: String) -> Unit ! Console { println(msg) } \
             fn entry(flag: Secret[Bool]) -> Unit ! Console { if flag { log(\"x\") } }",
        );
        assert!(!violations.is_empty());
        assert_eq!(violations[0].requirement_number(), 11);
    }

    #[test]
    fn join_opt_both_none_is_none() {
        assert_eq!(join_opt(None, None), None);
    }

    #[test]
    fn join_opt_with_one_none_preserves_label() {
        assert_eq!(
            join_opt(Some("Secret".to_string()), None),
            Some("Secret".to_string())
        );
        assert_eq!(
            join_opt(None, Some("Tainted".to_string())),
            Some("Tainted".to_string())
        );
    }

    #[test]
    fn join_opt_takes_first_label_when_both_present() {
        // No lattice ordering; first label wins.
        assert_eq!(
            join_opt(Some("Tainted".to_string()), Some("Secret".to_string())),
            Some("Tainted".to_string())
        );
    }

    // ── build_effect_reachability unit tests ──────────────────────────────────

    /// Non-effectful intermediate in a chain: seed from direct caller of effectful fn.
    ///
    /// Chain: `a` calls `b` (non-effectful), `b` calls `println` (effectful).
    /// Seeding: `b` → "println".
    /// Propagation: `a` → "println" (inherited from `b`).
    ///
    /// When `entry` calls `a` under high PC, the violation should name
    /// observable_fn="println" because `b` is not in `effectful_names`.
    #[test]
    fn reachability_non_effectful_intermediate_propagates_terminal() {
        let src = "fn println(msg: String) -> Unit ! Console { } \
                   fn b(msg: String) -> Unit { println(msg) } \
                   fn a(msg: String) -> Unit { b(msg) } \
                   fn entry(flag: Secret[Bool]) -> Unit { if flag { a(\"x\") } }";
        let prog = parse(src);
        let effectful = collect_effectful_names(&[&prog], None);
        let reach = build_effect_reachability(&[&prog], &effectful);
        // b directly calls effectful println → seeded as "println"
        assert_eq!(
            reach.get("b").map(String::as_str),
            Some("println"),
            "b should reach println"
        );
        // a calls b (non-effectful) → propagated to "println" (not "b")
        assert_eq!(
            reach.get("a").map(String::as_str),
            Some("println"),
            "a should reach println via b"
        );
    }

    /// Effectful intermediate in a chain: seed from nearest effectful callee.
    ///
    /// Chain: `a` calls `b` (effectful), `b` calls `println` (effectful).
    /// Seeding: `b` → "println", `a` → "b" (first effectful callee of a).
    ///
    /// `a` is directly seeded from `b` (which is in `effectful_names`), so
    /// observable_fn for `a` is "b", not "println".
    #[test]
    fn reachability_effectful_intermediate_stored_as_nearest_observable() {
        let src = "fn println(msg: String) -> Unit ! Console { } \
                   fn b(msg: String) -> Unit ! Console { println(msg) } \
                   fn a(msg: String) -> Unit ! Console { b(msg) }";
        let prog = parse(src);
        let effectful = collect_effectful_names(&[&prog], None);
        let reach = build_effect_reachability(&[&prog], &effectful);
        // b directly calls effectful println → seeded as "println"
        assert_eq!(
            reach.get("b").map(String::as_str),
            Some("println"),
            "b should reach println"
        );
        // a directly calls effectful b → seeded as "b" (not "println")
        assert_eq!(
            reach.get("a").map(String::as_str),
            Some("b"),
            "a should reach b (nearest effectful callee)"
        );
    }

    /// Function with no path to any effectful fn must NOT appear in reach map.
    #[test]
    fn reachability_pure_fn_absent_from_map() {
        let src = "fn println(msg: String) -> Unit ! Console { } \
                   fn pure_helper() -> Unit { }";
        let prog = parse(src);
        let effectful = collect_effectful_names(&[&prog], None);
        let reach = build_effect_reachability(&[&prog], &effectful);
        assert!(
            !reach.contains_key("pure_helper"),
            "pure_helper has no path to an effectful fn — must not appear in reachability map"
        );
    }

    /// collect_effectful_names picks up functions from multiple programs (prelude scenario).
    #[test]
    fn collect_effectful_names_spans_multiple_programs() {
        let prelude_src = "fn println(msg: String) -> Unit ! Console { }";
        let user_src = "fn greet() -> Unit { println(\"hi\") }";
        let prelude = parse(prelude_src);
        let user = parse(user_src);
        let effectful = collect_effectful_names(&[&prelude, &user], None);
        assert!(
            effectful.contains("println"),
            "println from prelude must be in effectful_names"
        );
        assert!(
            !effectful.contains("greet"),
            "greet has no effects — must not be in effectful_names"
        );
    }

    /// Direct implicit flow (not cross-function) via `ImplicitFlowViolation`.
    ///
    /// When a function directly calls an effectful fn under a high PC, it should
    /// emit `ImplicitFlowViolation`, not `CrossFunctionImplicitFlowViolation`.
    #[test]
    fn direct_effectful_call_under_high_pc_emits_implicit_flow_violation() {
        let src = "label Secret \
                   fn println(msg: String) -> Unit ! Console { } \
                   fn f(flag: Secret[Bool]) -> Unit ! Console { if flag { println(\"x\") } }";
        let prog = parse(src);
        let mut errors = Vec::new();
        check_implicit_flows(&prog, &[&prog], None, &mut errors);
        assert!(
            errors.iter().any(
                |e| matches!(e, CheckError::ImplicitFlowViolation { observable_fn, .. }
                if observable_fn == "println")
            ),
            "direct println call under high PC should emit ImplicitFlowViolation, got: {errors:?}"
        );
        // Must NOT emit CrossFunctionImplicitFlowViolation for the direct call.
        assert!(
            !errors.iter().any(|e| matches!(e, CheckError::CrossFunctionImplicitFlowViolation { callee, .. }
                if callee == "println")),
            "direct effectful fn call should not emit CrossFunctionImplicitFlowViolation with callee=println"
        );
    }

    /// Implicit flow inside an `impl` method body is detected.
    ///
    /// Note: bare `self` in impl blocks requires `self: Type` syntax (parser limitation).
    #[test]
    fn impl_method_body_implicit_flow_detected() {
        let src = "label Secret
fn println(msg: String) -> Unit ! Console { }
type Ctx = struct { dummy: Int }
trait Foo { fn bar(self, flag: Secret[Bool]) -> Unit ! Console; }
impl Foo for Ctx {
    fn bar(self: Ctx, flag: Secret[Bool]) -> Unit ! Console {
        if flag { println(\"leak\"); }
    }
}";
        let prog = parse(src);
        let mut errors = Vec::new();
        check_implicit_flows(&prog, &[&prog], None, &mut errors);
        assert!(
            errors.iter().any(|e| matches!(e, CheckError::ImplicitFlowViolation { observable_fn, .. }
                if observable_fn == "println")),
            "impl method body: println under Secret PC should emit ImplicitFlowViolation, got: {errors:?}"
        );
    }
}
