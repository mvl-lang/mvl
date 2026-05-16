// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Interprocedural IFC analysis: label propagation and violation detection.
//!
//! Implements three sub-tickets of the IFC epic (#825):
//!
//! - **#833 Return-label inference**: Infer security labels for unannotated
//!   function return types from body analysis and an external-source registry.
//!
//! - **#830 Forward label propagation**: Fixed-point algorithm — propagate
//!   inferred labels through function bodies until stable.
//!
//! - **#831 Interprocedural violation detection**: After propagation, detect
//!   call sites where an inferred arg label cannot flow to the required
//!   parameter label; emit [`CheckError::InterprocFlowViolation`] with a
//!   call-chain path.
//!
//! # Algorithm
//!
//! ```text
//! 1. Seed: TypeEnv explicit return labels (stdlib stdin_read_line → Tainted, etc.)
//! 2. Seed: external taint source registry (#833)
//! 3. Fixed-point (#830 + #833):
//!    for each unannotated fn F in user programs:
//!      infer return label from body using current table
//!      if label changed → mark changed
//!    repeat until stable
//! 4. Violation detection (#831):
//!    for each fn body, at each FnCall callee(args):
//!      if callee has labeled params in TypeEnv:
//!        infer arg label using enhanced infer_label (checks table)
//!        if inferred > required → emit InterprocFlowViolation
//! ```
//!
//! # References
//! - #830 — forward label propagation
//! - #831 — interprocedural violation detection
//! - #833 — return-label inference
//! - #825 — interprocedural IFC epic

use std::collections::HashMap;

use crate::mvl::checker::context::TypeEnv;
use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::ifc;
use crate::mvl::parser::ast::{
    Block, Decl, ElseBranch, Expr, MatchBody, Program, SecurityLabel, Stmt, TypeExpr,
};
use crate::mvl::parser::lexer::Span;

// ── External taint source registry (#833) ─────────────────────────────────────

/// Function names that always return `Tainted` data, independent of arguments.
///
/// This supplements TypeEnv's explicit labels (e.g., `stdin_read_line → Tainted[String]`
/// from std.io) for environments where the full stdlib is not loaded — for example,
/// standalone test programs or sandboxed compilation.
const TAINT_SOURCES: &[&str] = &["read_line", "args", "read_tainted"];

// ── Inferred label table ───────────────────────────────────────────────────────

/// Inferred security labels for function return types.
///
/// Built by [`propagate`]; stored in [`CheckResult`] for use by downstream
/// passes and tools.  Explicit TypeEnv annotations (seeded first) cannot be
/// downgraded by inference.
#[derive(Debug, Default, Clone)]
pub struct InferredLabels(HashMap<String, SecurityLabel>);

impl InferredLabels {
    /// Return the inferred return label for `fn_name`, if any.
    pub fn get(&self, fn_name: &str) -> Option<SecurityLabel> {
        self.0.get(fn_name).copied()
    }
}

// ── Propagation (#830 + #833) ─────────────────────────────────────────────────

/// Build the inferred label table by fixed-point body-analysis propagation.
///
/// Seeds from TypeEnv explicit return labels (covers all loaded stdlib functions),
/// then augments with the hardcoded external-source registry (#833), and finally
/// runs a fixed-point loop over user program function bodies (#830).
///
/// Called from `check_with_two_preludes_mode` with the user-supplied programs
/// (excluding prelude slices — their annotations are already in TypeEnv).
pub fn propagate(programs: &[&Program], type_env: &TypeEnv) -> InferredLabels {
    let mut table: HashMap<String, SecurityLabel> = HashMap::new();

    // Seed 1: explicit return labels from TypeEnv (covers stdlib taint sources).
    for (name, fn_info) in &type_env.fns {
        if let Some(label) = ifc::label_of(&fn_info.ret) {
            table.insert(name.clone(), label);
        }
    }

    // Seed 2: external taint source registry (#833) — safety net for unloaded stdlib.
    for &src in TAINT_SOURCES {
        table
            .entry(src.to_string())
            .or_insert(SecurityLabel::Tainted);
    }

    // Fixed-point body-analysis loop (#830 + #833).
    loop {
        let mut changed = false;
        for prog in programs {
            for decl in &prog.declarations {
                if let Decl::Fn(fd) = decl {
                    // Skip functions with an explicit return label — annotation wins.
                    if label_of_type_expr(&fd.return_type).is_some() {
                        continue;
                    }
                    // Build param label env from declared annotations.
                    let mut param_env: HashMap<String, SecurityLabel> = HashMap::new();
                    for param in &fd.params {
                        if let Some(l) = label_of_type_expr(&param.ty) {
                            param_env.insert(param.name.clone(), l);
                        }
                    }
                    // Infer return label from body return expressions.
                    if let Some(label) = infer_return_label(&fd.body, &param_env, &table) {
                        let current = table.get(&fd.name).copied();
                        let new_label = match current {
                            Some(c) => ifc::join(c, label),
                            None => label,
                        };
                        if current != Some(new_label) {
                            table.insert(fd.name.clone(), new_label);
                            changed = true;
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    InferredLabels(table)
}

/// Infer the return label for a function body from its return points.
///
/// Returns the join of labels inferred from:
/// - Explicit `return expr` statements anywhere in the body.
/// - The tail expression of the block (implicit return value).
fn infer_return_label(
    block: &Block,
    param_env: &HashMap<String, SecurityLabel>,
    table: &HashMap<String, SecurityLabel>,
) -> Option<SecurityLabel> {
    let explicit = collect_explicit_returns(block, param_env, table);
    let tail = block.stmts.last().and_then(|s| {
        if let Stmt::Expr { expr, .. } = s {
            infer_label_extended(expr, param_env, table)
        } else {
            None
        }
    });
    ifc::join_opt(explicit, tail)
}

/// Walk a block collecting labels from explicit `return expr` statements.
fn collect_explicit_returns(
    block: &Block,
    env: &HashMap<String, SecurityLabel>,
    table: &HashMap<String, SecurityLabel>,
) -> Option<SecurityLabel> {
    block
        .stmts
        .iter()
        .map(|s| collect_returns_in_stmt(s, env, table))
        .fold(None, ifc::join_opt)
}

fn collect_returns_in_stmt(
    stmt: &Stmt,
    env: &HashMap<String, SecurityLabel>,
    table: &HashMap<String, SecurityLabel>,
) -> Option<SecurityLabel> {
    match stmt {
        Stmt::Return { value: Some(e), .. } => infer_label_extended(e, env, table),
        Stmt::Return { value: None, .. } => None,
        Stmt::If { then, else_, .. } => {
            let then_ret = collect_explicit_returns(then, env, table);
            let else_ret = match else_ {
                Some(ElseBranch::Block(b)) => collect_explicit_returns(b, env, table),
                Some(ElseBranch::If(s)) => collect_returns_in_stmt(s, env, table),
                None => None,
            };
            ifc::join_opt(then_ret, else_ret)
        }
        Stmt::Match { arms, .. } => arms
            .iter()
            .map(|arm| match &arm.body {
                MatchBody::Block(b) => collect_explicit_returns(b, env, table),
                MatchBody::Expr(_) => None,
            })
            .fold(None, ifc::join_opt),
        Stmt::While { body, .. } | Stmt::For { body, .. } => {
            collect_explicit_returns(body, env, table)
        }
        _ => None,
    }
}

// ── Extended label inference ──────────────────────────────────────────────────

/// Infer the security label of an expression using the inferred label table.
///
/// Extends `ifc::infer_label` by looking up FnCall callee return labels in
/// `table` (both TypeEnv explicit and propagation-inferred labels), giving
/// correct results for chains through unannotated wrapper functions.
pub fn infer_label_extended(
    expr: &Expr,
    env: &HashMap<String, SecurityLabel>,
    table: &HashMap<String, SecurityLabel>,
) -> Option<SecurityLabel> {
    match expr {
        Expr::Ident(name, _) => env.get(name.as_str()).copied(),
        Expr::FnCall { name, args, .. } => {
            // If the callee has a known return label (explicit TypeEnv annotation or
            // inferred via body analysis), use it exclusively.
            // Explicit annotations (e.g. clean_input → Clean, sanitize → Clean) guarantee
            // the output label regardless of input labels — joining with arg labels would
            // produce false positives (e.g. treating clean_input(tainted) as Tainted).
            if let Some(label) = table.get(name.as_str()) {
                return Some(*label);
            }
            // Callee has no known return label — conservatively propagate from args.
            args.iter()
                .map(|a| infer_label_extended(a, env, table))
                .fold(None, ifc::join_opt)
        }
        // declassify/sanitize always produce specific labels.
        Expr::Declassify { .. } => Some(SecurityLabel::Public),
        Expr::Sanitize { .. } => Some(SecurityLabel::Clean),
        Expr::Binary { left, right, .. } => ifc::join_opt(
            infer_label_extended(left, env, table),
            infer_label_extended(right, env, table),
        ),
        Expr::Unary { expr, .. }
        | Expr::Borrow { expr, .. }
        | Expr::FieldAccess { expr, .. }
        | Expr::Consume { expr, .. }
        | Expr::Propagate { expr, .. } => infer_label_extended(expr, env, table),
        Expr::If {
            cond, then, else_, ..
        } => {
            let cond_label = infer_label_extended(cond, env, table);
            let then_label = tail_label_of_block(then, env, table);
            let else_label = else_
                .as_ref()
                .and_then(|e| infer_label_extended(e, env, table));
            ifc::join_opt(cond_label, ifc::join_opt(then_label, else_label))
        }
        Expr::Block(b) => tail_label_of_block(b, env, table),
        Expr::Match {
            scrutinee, arms, ..
        } => {
            let scr = infer_label_extended(scrutinee, env, table);
            let arms_label = arms
                .iter()
                .map(|arm| match &arm.body {
                    MatchBody::Expr(e) => infer_label_extended(e, env, table),
                    MatchBody::Block(b) => tail_label_of_block(b, env, table),
                })
                .fold(None, ifc::join_opt);
            ifc::join_opt(scr, arms_label)
        }
        Expr::MethodCall { receiver, args, .. } => {
            let recv = infer_label_extended(receiver, env, table);
            let arg_label = args
                .iter()
                .map(|a| infer_label_extended(a, env, table))
                .fold(None, ifc::join_opt);
            ifc::join_opt(recv, arg_label)
        }
        Expr::Construct { fields, .. } | Expr::Spawn { fields, .. } => fields
            .iter()
            .map(|(_, e)| infer_label_extended(e, env, table))
            .fold(None, ifc::join_opt),
        Expr::List { elems, .. } | Expr::Set { elems, .. } => elems
            .iter()
            .map(|e| infer_label_extended(e, env, table))
            .fold(None, ifc::join_opt),
        Expr::Map { pairs, .. } => pairs
            .iter()
            .map(|(k, v)| {
                ifc::join_opt(
                    infer_label_extended(k, env, table),
                    infer_label_extended(v, env, table),
                )
            })
            .fold(None, ifc::join_opt),
        Expr::Lambda { body, .. } => infer_label_extended(body, env, table),
        Expr::Select { arms, .. } => arms
            .iter()
            .map(|a| infer_label_extended(&a.expr, env, table))
            .fold(None, ifc::join_opt),
        Expr::Concurrently { body, .. } => tail_label_of_block(body, env, table),
        Expr::Literal(..) => None,
    }
}

/// Return the label of the tail expression of a block, if it ends in an expression.
fn tail_label_of_block(
    block: &Block,
    env: &HashMap<String, SecurityLabel>,
    table: &HashMap<String, SecurityLabel>,
) -> Option<SecurityLabel> {
    block.stmts.last().and_then(|s| {
        if let Stmt::Expr { expr, .. } = s {
            infer_label_extended(expr, env, table)
        } else {
            None
        }
    })
}

// ── Violation detection (#831) ────────────────────────────────────────────────

/// Detect interprocedural IFC violations in `prog`.
///
/// For each `FnCall callee(args)` where `callee` has labeled parameters in
/// `type_env`, computes the inferred label of each arg via [`infer_label_extended`]
/// and checks whether it can flow to the required parameter label.
///
/// Reports violations that the direct type checker missed because the arg's
/// *declared* type is unlabeled (treated as Public) but its *inferred* label
/// is higher — e.g., an unannotated wrapper returning data from `stdin_read_line`.
///
/// Returns [`CheckError::InterprocFlowViolation`] values (Req 11) ready to
/// append to `CheckResult.errors`.
pub fn detect_violations(
    prog: &Program,
    type_env: &TypeEnv,
    inferred: &InferredLabels,
) -> Vec<CheckError> {
    let mut errors = Vec::new();
    for decl in &prog.declarations {
        if let Decl::Fn(fd) = decl {
            let mut param_env: HashMap<String, SecurityLabel> = HashMap::new();
            for param in &fd.params {
                if let Some(l) = label_of_type_expr(&param.ty) {
                    param_env.insert(param.name.clone(), l);
                }
            }
            collect_violations_in_block(
                &fd.body,
                &fd.name,
                &param_env,
                type_env,
                inferred,
                &mut errors,
            );
        }
    }
    errors
}

fn collect_violations_in_block(
    block: &Block,
    caller: &str,
    env: &HashMap<String, SecurityLabel>,
    type_env: &TypeEnv,
    inferred: &InferredLabels,
    errors: &mut Vec<CheckError>,
) {
    let mut local_env = env.clone();
    for stmt in &block.stmts {
        collect_violations_in_stmt(stmt, caller, &mut local_env, type_env, inferred, errors);
    }
}

fn collect_violations_in_stmt(
    stmt: &Stmt,
    caller: &str,
    env: &mut HashMap<String, SecurityLabel>,
    type_env: &TypeEnv,
    inferred: &InferredLabels,
    errors: &mut Vec<CheckError>,
) {
    match stmt {
        Stmt::Let { pattern, init, .. } => {
            collect_violations_in_expr(init, caller, env, type_env, inferred, errors);
            // Track let-bound variable labels for subsequent stmts in this block.
            if let crate::mvl::parser::ast::Pattern::Ident(name, _) = pattern {
                if let Some(l) = infer_label_extended(init, env, &inferred.0) {
                    env.insert(name.clone(), l);
                }
            }
        }
        Stmt::Assign { value, .. } => {
            collect_violations_in_expr(value, caller, env, type_env, inferred, errors);
        }
        Stmt::Return { value: Some(e), .. } => {
            collect_violations_in_expr(e, caller, env, type_env, inferred, errors);
        }
        Stmt::Return { value: None, .. } => {}
        Stmt::Expr { expr, .. } => {
            collect_violations_in_expr(expr, caller, env, type_env, inferred, errors);
        }
        Stmt::If {
            cond, then, else_, ..
        } => {
            collect_violations_in_expr(cond, caller, env, type_env, inferred, errors);
            collect_violations_in_block(then, caller, env, type_env, inferred, errors);
            match else_ {
                Some(ElseBranch::Block(b)) => {
                    collect_violations_in_block(b, caller, env, type_env, inferred, errors);
                }
                Some(ElseBranch::If(s)) => {
                    let mut else_env = env.clone();
                    collect_violations_in_stmt(
                        s,
                        caller,
                        &mut else_env,
                        type_env,
                        inferred,
                        errors,
                    );
                }
                None => {}
            }
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            collect_violations_in_expr(scrutinee, caller, env, type_env, inferred, errors);
            for arm in arms {
                let arm_env = env.clone();
                match &arm.body {
                    MatchBody::Expr(e) => {
                        collect_violations_in_expr(e, caller, &arm_env, type_env, inferred, errors)
                    }
                    MatchBody::Block(b) => {
                        collect_violations_in_block(b, caller, &arm_env, type_env, inferred, errors)
                    }
                }
            }
        }
        Stmt::While { cond, body, .. } => {
            collect_violations_in_expr(cond, caller, env, type_env, inferred, errors);
            collect_violations_in_block(body, caller, env, type_env, inferred, errors);
        }
        Stmt::For { iter, body, .. } => {
            collect_violations_in_expr(iter, caller, env, type_env, inferred, errors);
            collect_violations_in_block(body, caller, env, type_env, inferred, errors);
        }
    }
}

fn collect_violations_in_expr(
    expr: &Expr,
    caller: &str,
    env: &HashMap<String, SecurityLabel>,
    type_env: &TypeEnv,
    inferred: &InferredLabels,
    errors: &mut Vec<CheckError>,
) {
    match expr {
        Expr::FnCall {
            name, args, span, ..
        } => {
            check_call_violations(name, args, *span, caller, env, type_env, inferred, errors);
            for arg in args {
                collect_violations_in_expr(arg, caller, env, type_env, inferred, errors);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            collect_violations_in_expr(receiver, caller, env, type_env, inferred, errors);
            for arg in args {
                collect_violations_in_expr(arg, caller, env, type_env, inferred, errors);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_violations_in_expr(left, caller, env, type_env, inferred, errors);
            collect_violations_in_expr(right, caller, env, type_env, inferred, errors);
        }
        Expr::Unary { expr, .. }
        | Expr::Propagate { expr, .. }
        | Expr::Consume { expr, .. }
        | Expr::Declassify { expr, .. }
        | Expr::Sanitize { expr, .. }
        | Expr::Borrow { expr, .. }
        | Expr::FieldAccess { expr, .. } => {
            collect_violations_in_expr(expr, caller, env, type_env, inferred, errors);
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            collect_violations_in_expr(cond, caller, env, type_env, inferred, errors);
            collect_violations_in_block(then, caller, env, type_env, inferred, errors);
            if let Some(e) = else_ {
                collect_violations_in_expr(e, caller, env, type_env, inferred, errors);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_violations_in_expr(scrutinee, caller, env, type_env, inferred, errors);
            for arm in arms {
                let arm_env = env.clone();
                match &arm.body {
                    MatchBody::Expr(e) => {
                        collect_violations_in_expr(e, caller, &arm_env, type_env, inferred, errors)
                    }
                    MatchBody::Block(b) => {
                        collect_violations_in_block(b, caller, &arm_env, type_env, inferred, errors)
                    }
                }
            }
        }
        Expr::Block(b) => collect_violations_in_block(b, caller, env, type_env, inferred, errors),
        Expr::Lambda { body, .. } => {
            collect_violations_in_expr(body, caller, env, type_env, inferred, errors)
        }
        Expr::Construct { fields, .. } | Expr::Spawn { fields, .. } => {
            for (_, e) in fields {
                collect_violations_in_expr(e, caller, env, type_env, inferred, errors);
            }
        }
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            for e in elems {
                collect_violations_in_expr(e, caller, env, type_env, inferred, errors);
            }
        }
        Expr::Map { pairs, .. } => {
            for (k, v) in pairs {
                collect_violations_in_expr(k, caller, env, type_env, inferred, errors);
                collect_violations_in_expr(v, caller, env, type_env, inferred, errors);
            }
        }
        Expr::Select { arms, .. } => {
            for arm in arms {
                collect_violations_in_expr(&arm.expr, caller, env, type_env, inferred, errors);
                collect_violations_in_block(&arm.body, caller, env, type_env, inferred, errors);
            }
        }
        Expr::Concurrently { body, .. } => {
            collect_violations_in_block(body, caller, env, type_env, inferred, errors)
        }
        Expr::Literal(..) | Expr::Ident(..) => {}
    }
}

/// Check a single call site for interprocedural IFC violations.
#[allow(clippy::too_many_arguments)]
fn check_call_violations(
    callee_name: &str,
    args: &[Expr],
    call_span: Span,
    caller: &str,
    env: &HashMap<String, SecurityLabel>,
    type_env: &TypeEnv,
    inferred: &InferredLabels,
    errors: &mut Vec<CheckError>,
) {
    let Some(fn_info) = type_env.lookup_fn(callee_name) else {
        return;
    };
    // Skip variadic builtins (empty params is the sentinel).
    if fn_info.params.is_empty() {
        return;
    }
    // Skip generic functions (monomorphization deferred to #838).
    if !fn_info.type_params.is_empty() {
        return;
    }

    for (param_idx, (arg, param_ty)) in args.iter().zip(fn_info.params.iter()).enumerate() {
        let Some(required) = ifc::label_of(param_ty) else {
            continue; // Param has no label requirement.
        };
        let Some(arg_label) = infer_label_extended(arg, env, &inferred.0) else {
            continue; // Cannot determine arg label.
        };
        if ifc::can_flow(arg_label, required) {
            continue; // No violation.
        }
        // Build a simplified call chain for the error message.
        let chain = extract_chain(arg, &inferred.0);
        errors.push(CheckError::InterprocFlowViolation {
            callee: callee_name.to_string(),
            param_idx,
            required_label: ifc::label_name(required).to_string(),
            inferred_label: ifc::label_name(arg_label).to_string(),
            chain,
            caller: caller.to_string(),
            span: call_span,
        });
    }
}

/// Extract a simplified call chain from an arg expression for error messages.
///
/// Returns function/variable names from outermost to innermost, tracing the
/// path through which labeled data flows into the violation.
fn extract_chain(expr: &Expr, table: &HashMap<String, SecurityLabel>) -> Vec<String> {
    match expr {
        Expr::FnCall { name, args, .. } => {
            let mut chain = vec![name.clone()];
            // Descend into the first arg that contributes a label.
            for arg in args {
                if infer_label_extended(arg, &HashMap::new(), table).is_some() {
                    let sub = extract_chain(arg, table);
                    if !sub.is_empty() {
                        chain.extend(sub);
                    }
                    break;
                }
            }
            chain
        }
        Expr::Ident(name, _) => vec![name.clone()],
        _ => vec![],
    }
}

// ── Label extraction from TypeExpr ───────────────────────────────────────────

/// Extract the outermost security label from a `TypeExpr`, if any.
fn label_of_type_expr(te: &TypeExpr) -> Option<SecurityLabel> {
    match te {
        TypeExpr::Labeled { label, .. } => Some(*label),
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::checker::context::TypeEnv;
    use crate::mvl::parser::Parser;

    fn parse(src: &str) -> Program {
        let (mut p, _) = Parser::new(src);
        p.parse_program()
    }

    fn env_with_taint_source() -> TypeEnv {
        use crate::mvl::checker::context::FnInfo;
        use crate::mvl::checker::types::Ty;
        let mut env = TypeEnv::default();
        // Simulate: fn source() -> Tainted[String]
        env.fns.insert(
            "source".into(),
            FnInfo {
                ret: Ty::Labeled(SecurityLabel::Tainted, Box::new(Ty::String)),
                ..Default::default()
            },
        );
        // Simulate: fn sink(q: Clean[String]) -> Unit
        env.fns.insert(
            "sink".into(),
            FnInfo {
                params: vec![Ty::Labeled(SecurityLabel::Clean, Box::new(Ty::String))],
                ret: Ty::Unit,
                ..Default::default()
            },
        );
        env
    }

    // ── Propagation tests ─────────────────────────────────────────────────

    #[test]
    fn external_source_registry_seeds_tainted() {
        let prog = parse("");
        let env = TypeEnv::default();
        let labels = propagate(&[&prog], &env);
        assert_eq!(labels.get("read_line"), Some(SecurityLabel::Tainted));
        assert_eq!(labels.get("args"), Some(SecurityLabel::Tainted));
    }

    #[test]
    fn typeenv_explicit_label_seeded() {
        let prog = parse("");
        let env = env_with_taint_source();
        let labels = propagate(&[&prog], &env);
        assert_eq!(labels.get("source"), Some(SecurityLabel::Tainted));
    }

    #[test]
    fn unannotated_fn_calling_taint_source_inferred_tainted() {
        // fn wrapper() -> String { source() }
        // source() is in TypeEnv as Tainted[String]
        let prog = parse("fn wrapper() -> String { source() }");
        let env = env_with_taint_source();
        let labels = propagate(&[&prog], &env);
        assert_eq!(
            labels.get("wrapper"),
            Some(SecurityLabel::Tainted),
            "wrapper calling Tainted source should be inferred Tainted"
        );
    }

    #[test]
    fn annotated_fn_not_overridden() {
        // fn clean() -> Clean[String] { source() }  — annotation wins
        let prog = parse("fn clean() -> Clean[String] { source() }");
        let env = env_with_taint_source();
        let labels = propagate(&[&prog], &env);
        // clean() has explicit label Clean — propagation must not upgrade it
        // (it's skipped by the "annotation wins" guard)
        assert_eq!(
            labels.get("clean"),
            None,
            "explicitly annotated fn should not be inferred"
        );
    }

    #[test]
    fn chain_propagation_two_hops() {
        // fn step1() -> String { source() }
        // fn step2() -> String { step1() }
        let prog = parse("fn step1() -> String { source() } fn step2() -> String { step1() }");
        let env = env_with_taint_source();
        let labels = propagate(&[&prog], &env);
        assert_eq!(labels.get("step1"), Some(SecurityLabel::Tainted));
        assert_eq!(
            labels.get("step2"),
            Some(SecurityLabel::Tainted),
            "step2 calling step1 (Tainted) should be inferred Tainted"
        );
    }

    #[test]
    fn external_registry_read_line_propagates() {
        // read_line is in TAINT_SOURCES — even without TypeEnv registration
        let prog = parse("fn wrapper() -> String { read_line() }");
        let env = TypeEnv::default();
        let labels = propagate(&[&prog], &env);
        assert_eq!(labels.get("wrapper"), Some(SecurityLabel::Tainted));
    }

    // ── Violation detection tests ─────────────────────────────────────────

    #[test]
    fn no_violations_on_clean_flow() {
        // fn wrapper() -> String { source() }
        // fn caller() -> Unit { sink(wrapper()) }  — Tainted → Clean[String]: violation!
        // But here we test the CLEAN case: calling with a non-tainted value
        let prog = parse("fn caller() -> Unit { sink(\"safe\") }");
        let env = env_with_taint_source();
        let inferred = propagate(&[&prog], &env);
        let violations = detect_violations(&prog, &env, &inferred);
        assert!(
            violations.is_empty(),
            "literal arg to Clean sink should not violate"
        );
    }

    #[test]
    fn violation_detected_tainted_to_clean_sink() {
        // fn wrapper() -> String { source() }   ← inferred Tainted
        // fn caller() -> Unit { sink(wrapper()) }  ← Tainted → Clean param
        let prog =
            parse("fn wrapper() -> String { source() } fn caller() -> Unit { sink(wrapper()) }");
        let env = env_with_taint_source();
        let inferred = propagate(&[&prog], &env);
        let violations = detect_violations(&prog, &env, &inferred);
        assert!(
            !violations.is_empty(),
            "Tainted arg to Clean[String] param should produce a violation"
        );
        // Verify the error is tagged as Req 11
        for v in &violations {
            assert_eq!(v.requirement_number(), 11);
        }
    }

    #[test]
    fn violation_chain_extracted() {
        let prog =
            parse("fn wrapper() -> String { source() } fn caller() -> Unit { sink(wrapper()) }");
        let env = env_with_taint_source();
        let inferred = propagate(&[&prog], &env);
        let violations = detect_violations(&prog, &env, &inferred);
        if let Some(CheckError::InterprocFlowViolation { chain, .. }) = violations.first() {
            assert!(
                chain.contains(&"wrapper".to_string()),
                "chain should include wrapper, got {chain:?}"
            );
        }
    }
}
